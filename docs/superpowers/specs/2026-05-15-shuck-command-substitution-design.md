# shuck Command Substitution Design

**Status:** Draft (2026-05-15)

**Goal:** Add command substitution to shuck — both `$(...)` and backtick `` `...` `` forms. The substituted command's stdout replaces the expression; trailing newlines are stripped; unquoted substitutions word-split like `$VAR`. Substitutions update the parent shell's `$?` and execute against a cloned `Shell` so assignments and `export`/`unset` inside `$(...)` don't leak to the parent.

**Scope (v5):**
- In: `$(...)` and `` `...` ``, anywhere a `Word` can appear; word-splitting in unquoted contexts; `$?` propagation; subshell-style isolation of `Shell` state (vars, exported flags, last_status).
- Out: `$(...)` arithmetic forms (`$((...))`), process substitution (`<(...)`), heredocs (`<<EOF`), `$$`, `$#`, `$@`, positional parameters. cwd isolation (the cwd is process-global; `cd` inside `$(...)` will leak — documented limitation).

---

## 1. Syntax

Two equivalent forms produce the same AST node:

| Form | Notes |
|------|-------|
| `$(cmd)` | Modern. Body is any valid shuck `Sequence` (pipelines, `&&`/`\|\|`/`;`, redirects, nested substitutions, assignments). Nests naturally. |
| `` `cmd` `` | Legacy. Body parsed with bash backtick escape rules: `\`` → literal backtick, `\\` → literal `\`, `\$` → literal `$`, every other backslash is preserved as `\x`. Newlines inside backticks are allowed (whitespace in the inner parse). Backticks do **not** nest naturally (the inner backtick must be `\``-escaped). |

**Quoting:**
- Unquoted `$(cmd)` / `` `cmd` `` → word-split (like unquoted `$VAR`).
- Quoted `"$(cmd)"` / ``"`cmd`"`` → one field, output appended verbatim.
- `'$(cmd)'` / `` '`cmd`' `` → literal text inside single quotes; no substitution.

**Inside double quotes**: both forms are still recognized as substitutions (just with `quoted: true`).

**Where allowed**: anywhere a `Word` can appear — args, program word, redirect target, assignment RHS, and inside the body of another substitution.

---

## 2. AST changes

One new `WordPart` variant:

```rust
pub enum WordPart {
    Literal(String),
    Var { name: String, quoted: bool },
    LastStatus { quoted: bool },
    Tilde,
    CommandSub { sequence: crate::command::Sequence, quoted: bool },  // NEW
}
```

The AST becomes recursive: `Sequence → Pipeline → SimpleCommand → ExecCommand → Word → Vec<WordPart> → CommandSub → Sequence`. Rust handles this via the `Vec` indirection — no `Box` required.

`lexer.rs` adds `use crate::command::Sequence;`. Rust permits mutual imports between modules in the same crate; no module restructure required.

`Shell` gains `#[derive(Clone)]` so `Shell::clone()` is available for the cloned-subshell mechanism. (`Variable` already derives `Clone` from v4.)

---

## 3. Lexer changes

The tokenizer recognizes substitution in three contexts. Each produces a `WordPart::CommandSub`:

1. **`$(` outside any quote**: in the existing `'$'` arm, when the next char is `(`, call `scan_paren_substitution`.
2. **`$(` inside double quotes**: in the existing `Some('$')` arm of the double-quote loop, when the next char is `(`, call the same scanner with `quoted: true`.
3. **`` ` ``**: a new arm (active both outside any quote and inside double quotes). Calls `scan_backtick_substitution`.

### `scan_paren_substitution(chars, quoted) -> Result<WordPart, LexError>`

Reads characters into a body string, tracking:
- `depth: usize` — starts at 0 (already past the opening `$(`). Increments on every nested `$(`, decrements on `)`. Stop when `depth == 0` and we see `)`.
- Quote state: `'...'` (literal, depth-unaware until matching `'`), `"..."` (depth-unaware until matching `"`, with `\"`/`\\`/`\$` escapes preserved character-for-character in the body), `\<ch>` (consume two chars literally).
- Newlines are permitted in the body (treated as whitespace by the inner parse).

If EOF is hit before the matching `)`, return `LexError::UnterminatedSubstitution`.

After capturing the body string:
```rust
let inner_tokens = tokenize(&body).map_err(|e| LexError::SubstitutionLexError(Box::new(e)))?;
let inner_seq = crate::command::parse(inner_tokens)
    .map_err(LexError::SubstitutionParseError)?
    .unwrap_or_else(empty_sequence);  // $() is allowed: a no-op sequence
Ok(WordPart::CommandSub { sequence: inner_seq, quoted })
```

`empty_sequence()` returns a `Sequence` whose `first` pipeline has zero commands. The executor's `execute_capturing` treats this as a no-op that returns `("", 0)`.

### `scan_backtick_substitution(chars, quoted) -> Result<WordPart, LexError>`

Reads characters into a body string, applying backtick escape rules:
- `\` followed by `` ` `` → push literal `` ` `` to body
- `\` followed by `\` → push literal `\` to body
- `\` followed by `$` → push literal `$` to body
- `\` followed by any other char `c` → push both `\` and `c` to body verbatim (preserves the backslash for the inner lex pass to handle normally)
- `` ` `` (unescaped) → end of substitution
- EOF before closing backtick → `LexError::UnterminatedSubstitution`

After capturing the body, same parse-and-wrap as `scan_paren_substitution`.

### New `LexError` variants

```rust
pub enum LexError {
    UnterminatedQuote,
    BareAmpersand,
    InvalidVarName,
    UnterminatedBrace,
    UnterminatedSubstitution,                                 // NEW
    SubstitutionLexError(Box<LexError>),                      // NEW
    SubstitutionParseError(crate::command::ParseError),       // NEW
}
```

`shell.rs::lex_error_message` is changed to return `String` (since the new variants carry dynamic content). The existing arms get `.to_string()` appended; the new arms format with the inner error rendered recursively:

```rust
fn lex_error_message(error: LexError) -> String {
    match error {
        LexError::UnterminatedQuote => "unterminated quote".to_string(),
        LexError::BareAmpersand => "unexpected '&'".to_string(),
        LexError::InvalidVarName => "invalid variable name in '${...}'".to_string(),
        LexError::UnterminatedBrace => "unterminated '${...}'".to_string(),
        LexError::UnterminatedSubstitution => "unterminated command substitution".to_string(),
        LexError::SubstitutionLexError(inner) =>
            format!("syntax error in command substitution: {}", lex_error_message(*inner)),
        LexError::SubstitutionParseError(inner) =>
            format!("syntax error in command substitution: {}", parse_error_message(&inner)),
    }
}
```

The single call site in `shell.rs::process_line` (`eprintln!("shuck: syntax error: {}", lex_error_message(e))`) is unchanged in shape — only the type of the rendered value differs.

---

## 4. Execution and capture

Two new arms in `expand.rs::expand`:

```rust
WordPart::CommandSub { sequence, quoted: true } => {
    let output = run_substitution(sequence, shell);
    current.push_str(&output);
    has_emitted = true;
}
WordPart::CommandSub { sequence, quoted: false } => {
    let output = run_substitution(sequence, shell);
    emit_split(&output, &mut current, &mut result, &mut has_emitted);
}
```

`expand` becomes `expand(word: &Word, shell: &mut Shell)` (currently `&Shell`) because `run_substitution` may update the parent shell's `last_status`. All call sites of `expand` in `executor.rs::resolve` already have `&mut Shell` available; just thread it through.

### `run_substitution(seq: &Sequence, shell: &mut Shell) -> String`

```rust
fn run_substitution(seq: &Sequence, shell: &mut Shell) -> String {
    let mut cloned = shell.clone();
    let (output, status) = executor::execute_capturing(seq, &mut cloned);
    shell.set_last_status(status);
    strip_trailing_newlines(&output)
}

fn strip_trailing_newlines(s: &str) -> String {
    let trimmed = s.trim_end_matches('\n');
    trimmed.to_string()
}
```

Lives in `expand.rs` (since it's the bridge between expansion and execution).

### `executor::execute_capturing(seq, shell) -> (String, i32)`

A new public function. Runs the sequence the same way `execute` does, but with stdout redirected to a buffer.

Implementation: refactor `executor.rs` so that the "where does stdout go" decision flows through a single parameter. Concretely, introduce a private:

```rust
fn execute_inner(seq: &Sequence, shell: &mut Shell, sink: Option<&mut Vec<u8>>) -> ExecOutcome
```

- When `sink` is `None`: stdout flows to `io::stdout()` for terminal-stage builtins, and subprocesses inherit shuck's stdout. (Current behavior.)
- When `sink` is `Some(buf)`: terminal-stage builtin writes go to `buf`; terminal-stage subprocesses are spawned with `Stdio::piped()` and their stdout is read into `buf` after wait.

`execute(seq, shell)` becomes a thin wrapper: `execute_inner(seq, shell, None)`.

`execute_capturing(seq, shell) -> (String, i32)`:

```rust
pub fn execute_capturing(seq: &Sequence, shell: &mut Shell) -> (String, i32) {
    let mut buf: Vec<u8> = Vec::new();
    let outcome = execute_inner(seq, shell, Some(&mut buf));
    let status = match outcome {
        ExecOutcome::Continue(c) | ExecOutcome::Exit(c) => c,
    };
    (String::from_utf8_lossy(&buf).into_owned(), status)
}
```

Note: `ExecOutcome::Exit` inside a substitution is treated as a normal status. `exit 2` inside `$(...)` exits the cloned subshell, returning status 2 to the parent; the parent shuck continues running.

### Plumbing the sink through pipelines

The existing pipeline machinery already routes builtin output through buffers for non-terminal stages (`Carry::Buffer`). For terminal stages:

- **Single command, terminal**: currently `run_single` writes builtin output to `io::stdout()` directly or hands stdout to a subprocess via `Stdio::inherit()`. With a sink: write to the buffer for builtins; for subprocesses, use `Stdio::piped()` and read the child's stdout into the buffer after `wait()`.
- **Multi-stage pipeline, terminal stage**: same as above — the terminal stage's stdout goes to the sink instead of inheriting.

Non-terminal stages and explicit redirects (e.g., `> file` inside the substitution) are unchanged. An explicit redirect inside `$(echo hi > /tmp/f)` writes to the file, and the captured output is empty.

---

## 5. Subshell semantics

- **Cloned `Shell`** — `Shell::clone()` snapshots `vars` and `last_status`. Mutations inside the substitution (`FOO=bar`, `export X`, `unset Y`) affect the clone only.
- **Parent's `$?` is updated** to the substitution's exit status: `out=$(false); echo $?` → `1`.
- **Inherited environment for subprocess children**: the cloned shell's `exported_env()` drives subprocess env. Children of `$(...)` see the same exports as the parent — bash also does this.
- **cwd leakage (known limitation):** `env::set_current_dir` mutates the process cwd. `Shell::clone()` does not isolate that. `$(cd /tmp; pwd)` will leave the parent shell in `/tmp`. Future v6 work could track cwd inside `Shell` and pass it to subprocesses via `Command::current_dir`. Document this in the smoke-test plan output.
- **`exit` inside `$(...)`**: the substituted command's `exit N` exits the cloned subshell, returning status `N` to the parent. The parent does not terminate.
- **Nesting**: depth-first. `$(echo $(echo hi))` first runs the inner `echo hi`, captures `"hi\n"`, strips to `"hi"`, then runs `echo hi`, captures `"hi\n"`, strips to `"hi"`. Top-level value: `hi`.

### Edge cases

| Case | Behavior |
|------|----------|
| Empty body `$()` or `` `` `` | Zero-output substitution. Unquoted → 0 fields. Quoted → one empty segment. Parent `$? = 0`. |
| Substitution stdout has no trailing newline | Captured verbatim. (`$(printf hi)` → `hi`.) |
| Substitution stdout has multiple trailing newlines | All trailing `\n` bytes are stripped (`$(printf 'hi\n\n\n')` → `hi`). Non-`\n` trailing whitespace is preserved. |
| Substitution writes binary or invalid UTF-8 | `String::from_utf8_lossy` replaces invalid bytes with U+FFFD. (Bash treats this as bytes; we choose lossy decode for simplicity.) |
| Substitution body has only whitespace/comments | Empty Sequence; same as empty body. |
| Inner command not found | Inner sequence exits 127; captured output empty; parent `$? = 127`. |
| Inner command modifies cwd | **Leaks** to parent (cwd is process-global). Documented limitation. |
| Inner command does `export FOO=x` | Modifies the cloned shell only; does NOT leak. |

---

## 6. Error handling

| Source | Surfaces as |
|--------|-------------|
| Unterminated `$(...)` or `` `...` `` | `shuck: syntax error: unterminated command substitution` |
| Lex error inside `$(...)` body | `shuck: syntax error in command substitution: <inner lex message>` |
| Parse error inside `$(...)` body | `shuck: syntax error in command substitution: <inner parse message>` |
| Runtime error inside `$(...)` | Captured by the inner executor as it normally would (the error is printed to stderr at the time it occurs, captured stdout reflects what was emitted before the failure, `$?` records the failure code). |

Lex and parse errors inside a substitution prevent the outer command from running at all — same gate as any other syntax error.

---

## 7. Testing strategy

Following the v1–v4 pattern. Tests live in the modules they exercise; smoke tests are the final implementation-plan task.

### Lexer unit tests (`src/lexer.rs`)

- `tokenize_command_sub_basic` — `$(echo hi)` produces a single-word token with one `CommandSub` part whose `Sequence` contains `Exec(echo, [hi])`.
- `tokenize_command_sub_quoted_in_double_quotes` — `"$(echo hi)"` produces `quoted: true`.
- `tokenize_command_sub_in_single_quotes_is_literal` — `'$(echo hi)'` produces a single Literal `"$(echo hi)"`.
- `tokenize_command_sub_nested` — `$(echo $(echo hi))`.
- `tokenize_command_sub_with_paren_inside_quotes` — `$(echo "(")`. Inner `(` does not increment depth because it's inside double quotes.
- `tokenize_command_sub_empty` — `$()` produces an empty Sequence.
- `tokenize_command_sub_unterminated` — `$(echo` → `LexError::UnterminatedSubstitution`.
- `tokenize_command_sub_inner_syntax_error` — `$(echo |)` → `LexError::SubstitutionParseError(ParseError::MissingCommand)`.
- `tokenize_backtick_basic` — `` `echo hi` ``.
- `tokenize_backtick_escape_dollar` — `` `echo \$FOO` `` → inner body string is `echo $FOO` (the `\$` is unescaped to `$` for the inner lex).
- `tokenize_backtick_escape_backslash` — `` `echo \\` `` → inner body `echo \`.
- `tokenize_backtick_escape_backtick` — `` `echo \`hi\`` `` → inner body `` echo `hi` ``.
- `tokenize_backtick_unterminated` — `` `echo `` → `LexError::UnterminatedSubstitution`.
- `tokenize_command_sub_in_args` — `cat $(echo /etc/passwd)` → second token is a Word containing a CommandSub.
- `tokenize_command_sub_in_redirect_target` — `cat > $(echo /tmp/f)` → redirect target Word contains a CommandSub.

### Parser unit tests (`src/command.rs`)

The parser doesn't need new logic for CommandSub (the lexer constructs them). But add a parser test that confirms a CommandSub-containing Word can appear in each placement: program word, args, redirect target, assignment RHS.

### Expand unit tests (`src/expand.rs`)

Use real `echo` builtin invocations inside synthetic `Sequence`s — no test seam needed since `echo` runs entirely in-process via `builtins::run_builtin`.

- `expand_command_sub_invokes_inner` — `Sequence` is `echo hello`; assert `expand` of an unquoted `CommandSub` containing that Sequence returns `vec!["hello".to_string()]`.
- `expand_unquoted_sub_splits` — `Sequence` is `echo a b`; assert unquoted CommandSub produces `vec!["a", "b"]`.
- `expand_quoted_sub_preserves_whitespace` — `Sequence` is `echo a b`; assert quoted CommandSub produces `vec!["a b"]`.
- `expand_sub_with_literal_prefix_merges_first_field` — `pre` + CommandSub(echo x y) → `["prex", "y"]`.
- `expand_sub_strips_trailing_newlines` — `Sequence` is `echo hi`; the `\n` echo emits is stripped, so the result is exactly `"hi"`, not `"hi\n"`.
- `expand_sub_updates_parent_last_status` — `Sequence` is `exit 7`; after `expand`, parent `shell.last_status() == 7`. (`ExecOutcome::Exit` inside the captured execution is treated as a normal status code by `execute_capturing`.)

### Executor tests (`src/executor.rs`)

- `execute_capturing_simple_echo` — builds a Sequence for `echo hi`, runs `execute_capturing`, asserts `("hi\n", 0)`. (`execute_capturing` returns raw output; trailing-newline stripping happens in `run_substitution`, not here.)
- `execute_capturing_pipeline_uses_subprocess` — skip in unit tests since piping requires a real subprocess; cover via smoke test.
- `execute_capturing_exit_returns_status` — Sequence is `exit 7`; result is `("", 7)`.

### Smoke tests (final plan task)

```
echo $(echo hello)              → hello
echo "$(echo a b)"              → a b
echo $(echo a b)                → a b
FOO=$(echo bar); echo $FOO      → bar
PAD=$(echo "a  b"); echo "$PAD" → a  b
echo `echo via-backtick`        → via-backtick
echo $(echo $(echo nested))     → nested
echo `echo \`nested-backtick\`` → nested-backtick
false; X=$(true); echo $?       → 0
echo $(false); echo $?          → (blank line); 1
FOO=outer; X=$(FOO=inner; echo $FOO); echo $FOO/$X
                                → outer/inner   (subshell isolation)
$(echo undefined_cmd); echo $?  → shuck: command not found: undefined_cmd; 127
echo $(                         → shuck: syntax error: unterminated command substitution
echo $(echo |)                  → shuck: syntax error in command substitution: missing command
```

---

## 8. File summary

| File | Change |
|------|--------|
| `src/lexer.rs` | Add `WordPart::CommandSub` variant; add `scan_paren_substitution`, `scan_backtick_substitution`; extend `tokenize`'s `$`, double-quote, and outer-loop arms; add 3 new `LexError` variants; add ~15 unit tests. Imports `crate::command::Sequence`. No new `Clone` derives needed (the AST is borrowed during execution; only `Shell` clones, and `Shell` holds only `String` values). |
| `src/command.rs` | No new parser logic (the lexer constructs `CommandSub`s); add ~3 unit tests for CommandSub placement (program word, args, redirect target, assignment RHS). |
| `src/expand.rs` | Add 2 new `WordPart::CommandSub` arms; add `run_substitution`; add `strip_trailing_newlines`. Change `expand` signature from `(&Word, &Shell)` to `(&Word, &mut Shell)` and update callers. Add ~3 unit tests with a stubbed substitution function (test seam). |
| `src/executor.rs` | Add `execute_capturing` public function; refactor `execute` to delegate to `execute_inner(seq, shell, None)`; thread `sink: Option<&mut Vec<u8>>` through `run_pipeline`, `run_single`, `run_multi_stage`, and the subprocess and builtin code paths so terminal-stage output can go to a buffer. Add ~3 unit tests for `execute_capturing`. |
| `src/shell_state.rs` | Add `#[derive(Clone)]` to `Shell`. (`Variable` already derives Clone.) |
| `src/shell.rs` | Extend `lex_error_message` to handle the 3 new variants (likely changing its return type to `String`). |

Total estimate: ~600 LoC implementation + ~250 LoC tests across 5 files.

---

## 9. Out of scope / future work

- **`$$` (PID), `$#`, `$@`, `$1`–`$9`** — positional and special parameters. None of these exist yet.
- **`$((expression))`** — arithmetic expansion. Different syntax, different semantics.
- **`<(cmd)` / `>(cmd)`** — process substitution. Requires `/dev/fd` plumbing.
- **`<<EOF`** — heredocs. Multi-line input collection.
- **cwd isolation in `$(...)`** — would require tracking cwd inside `Shell` and using `Command::current_dir(...)` for all subprocesses. Worth doing but bigger than v5 scope.
- **Stderr capture for `$(...)`** — possible with `2>&1` inside the substitution; no shuck-side change needed for that idiom.
