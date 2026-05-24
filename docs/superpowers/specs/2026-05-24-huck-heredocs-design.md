# v24: Here-Documents — Design Spec

## Goal

Support POSIX here-document redirection: `cmd <<DELIM`, `cmd <<'DELIM'`,
and `cmd <<-DELIM`, with the body collected from subsequent input lines
up to a line containing exactly the closing delimiter. Closes M-12 from
`docs/bash-divergences.md` — one of the most foundational POSIX
constructs and a frequent script pattern.

Pre-v24, `cat <<EOF` is a parse error (`<<` lexes as two `<` tokens).
After v24, the user can:

```sh
cat <<EOF
hello $USER
EOF
```

…and the child process sees the expanded body on stdin.

## Variants supported

All three POSIX variants, composable:

| Form | Body expansion | Body leading-tab strip | Notes |
| --- | --- | --- | --- |
| `<<DELIM` | yes (`$var`, `${var}`, `$(cmd)`, backticks, `\$ \\ \``) | no | The default "expanding" form |
| `<<'DELIM'` / `<<"DELIM"` / `<<\DELIM` | no — body taken verbatim | no | Any quoted/escaped char in the delim word triggers literal mode |
| `<<-DELIM` | yes | yes — leading tabs stripped from each body line AND from the close-delim line | The dash variant |
| `<<-'DELIM'` (etc.) | no | yes | Composition |

**Quoted-delimiter detection**: per POSIX 2.7.4, the body is literal
(no expansion) if ANY character of the delimiter word was quoted in the
source. So `<<E"O"F` is literal mode with delimiter `EOF`. The lexer
records "was any part of this delim quoted?" while parsing the delim
word.

**Multiple here-docs on one command**: `cmd <<A <<B<NL>body_a<NL>A<NL>body_b<NL>B`
— both bodies are consumed in order, but only the LAST `<<` redirect
actually becomes `cmd`'s stdin (POSIX last-wins for redirects to the
same fd). Earlier bodies are dropped at parse time.

## Lexer / tokenizer changes

`src/lexer.rs`:

**New tokens / operators**:
- `Operator::Heredoc` for `<<`
- `Operator::HeredocStrip` for `<<-`

These appear in the token stream during normal lexing. After emitting
the operator, the lexer next consumes the delimiter word (which it
classifies as "had any quoted part?" → literal vs expanding).

**Pending-heredoc queue**: while tokenizing a line, the lexer maintains
`pending_heredocs: VecDeque<PendingHeredoc>` where:
```rust
struct PendingHeredoc {
    delim: String,         // literal text (after quote-removal)
    expand: bool,          // false if any part of the delim word was quoted
    strip_tabs: bool,      // true for `<<-`
    body_token_idx: usize, // back-reference into tokens to fill in later
}
```

When the lexer encounters the line's terminating `\n` (or end-of-input
with pending heredocs still queued), it switches modes: consumes
subsequent lines as bodies for each pending heredoc, in queue order.

**Body collection per pending heredoc**:
- Read input lines until a line that matches the closing delimiter.
- For `<<-` (strip_tabs): strip leading `\t` characters from each line
  AND from the close-delimiter line BEFORE the match check.
- The close-delimiter line must be exactly `DELIM\n` (or `\t…\tDELIM\n`
  for strip_tabs). Trailing whitespace, leading whitespace (other than
  tabs in `<<-` mode), or any other variation invalidates the close.
- If end-of-input is hit before a matching close: return
  `LexError::UnterminatedHeredoc`.

**Body lexing**:
- For expanding heredocs (`expand=true`): scan each body line for `$`,
  `` ` ``, and `\` per POSIX 2.7.4. `$var`, `${var}`, `$(cmd)`,
  `` `cmd` `` emit the corresponding `WordPart::Var` / `ParamExpansion`
  / `CommandSub` parts. `\$`, `\\`, `` \` ``, and `\<NL>` are escape
  sequences (deleted backslash, the next char becomes a quoted Literal);
  all other backslashes are literal. Each body line is terminated by a
  literal `\n` part (also `Literal { text: "\n", quoted: true }`).
- For literal heredocs (`expand=false`): the entire body — including all
  newlines and any `$`/`` ` ``/`\` characters — is a single
  `WordPart::Literal { text: ..., quoted: true }`. No scanning.
- After collecting the body, the lexer writes the assembled `Word` back
  into the previously-stored `HeredocStart` token (via the
  `body_token_idx` back-reference).

**New `LexError` variant**:
```rust
pub enum LexError {
    ...
    UnterminatedHeredoc,
}
```

## AST representation

`src/command.rs`:

```rust
pub enum Redirect {
    Truncate(Word),
    Append(Word),
    Heredoc { body: Word, expand: bool, strip_tabs: bool },
}

pub struct ExecCommand {
    pub inline_assignments: Vec<(String, Word)>,
    pub program: Word,
    pub args: Vec<Word>,
    // BREAKING CHANGE: was Option<Word>; now Option<Redirect> so that
    // `<file` and `<<EOF` and (future) `<<<word` can be represented
    // uniformly. Last-wins: a later redirect to stdin overwrites earlier.
    pub stdin: Option<Redirect>,
    pub stdout: Option<Redirect>,
    pub stderr: Option<Redirect>,
}
```

**Why `Word` for the body** (not `Vec<u8>` or `String`):
- The expanding case needs `$var`/`$(cmd)`/etc. parts that are evaluated
  at runtime via the existing `expand_assignment` machinery. Storing as
  a `Word` reuses every expansion code path for free.
- The literal case is a `Word` with one `Literal { quoted: true }` part.
  `expand_assignment` on a single-quoted-Literal Word produces the text
  verbatim, no expansion.
- `strip_tabs` is applied at lex time (the body's Literal parts already
  have leading tabs removed). Runtime doesn't need to revisit it. The
  `strip_tabs` flag in the AST is for round-trip / debugging clarity.

**Multiple here-docs on one command**: only the LAST `<<` writes into
`ExecCommand.stdin`. Parser drops earlier heredocs' bodies (they are
still consumed by the lexer, so the queue is processed correctly). Bash
behaves the same way.

**AST shape change is breaking** for the `stdin: Option<Word>` field.
Every consumer of `cmd.stdin` in the executor needs to widen to handle
`Redirect::Truncate(Word)` (the `<file` case — Word is the filename) AND
`Redirect::Heredoc{...}` (the body). `Redirect::Append` is invalid for
stdin (`>>file` is stdout-only); the parser will not produce it.

## Continuation classifier integration

`src/continuation.rs`:

```rust
pub enum ContinuationReason {
    Backslash,
    Operator,
    OpenQuote,
    Compound,
    Heredoc,   // NEW
}
```

**`classify` change**: when `tokenize(buffer)` returns
`Err(LexError::UnterminatedHeredoc)`, classify returns
`Incomplete(Heredoc)`. This is checked alongside the existing
`is_unterminated_lex` set.

**`joiner_for(Heredoc, _)`**: returns `"\n"` — heredoc bodies are
newline-sensitive, so concatenation of REPL lines must use a literal
newline as the separator. Each subsequent line typed at the `> ` prompt
is appended with `\n` between it and the prior buffer.

**Continuation prompt**: unchanged — keep `CONT_PROMPT = "> "`. Bash
uses the same prompt for heredoc body collection.

**Ctrl-C at the heredoc prompt**: existing `ReadResult::Interrupted`
path discards the buffer and returns to the main prompt. No new code.

## Executor

`src/executor.rs`:

**`open_stage_files` widens stdin handling** to accept both
`Redirect::Truncate(path_word)` (existing file-open) and
`Redirect::Heredoc { body, expand, strip_tabs: _ }` (new). For Heredoc:
1. Expand the body via `expand_assignment(body, shell)` — same no-split
   semantics as a variable RHS. For literal heredocs, the single
   `Literal { quoted: true }` part contributes the text verbatim. For
   expanding heredocs, `$var`/`$(cmd)`/etc. parts evaluate.
2. Return the resulting bytes to be piped into the child's stdin.

**Plumbing**: route heredoc bytes through the existing
`pending_input: Option<Vec<u8>>` mechanism that's already used in
`run_multi_stage` (line ~1002) for buffering upstream output between
pipeline stages. Add the same path to `run_exec_single`. The child is
spawned with `process.stdin(Stdio::piped())`, then
`child.stdin.take().write_all(&body_bytes)` runs before the wait.

**Order of operations** (per stage):
1. `apply_inline_assignments` (v23 path).
2. Expand stdin redirect (open file OR compute heredoc body).
3. Spawn child with piped stdin if heredoc-driven.
4. Write heredoc bytes into the child's stdin pipe.
5. Wait (or for pipelines, let the wait loop reap).
6. `restore_inline_assignments`.

**Builtins**: huck builtins don't currently read stdin. A
`builtin <<EOF<NL>body<NL>EOF` invocation collects the body but silently
discards it (the builtin's `run_builtin` call doesn't accept a stdin
parameter). Document as a v24 limitation; consistent with bash for
shell-builtin no-readers like `cd`/`pwd`/`echo`/`test`.

**Functions**: similar — function-call path has no stdin attach point.
`func <<EOF<NL>body<NL>EOF` consumes the body but doesn't pipe it in.
This is the same documented gap as M-10 (function-call redirects
silently ignored).

**Last-wins for stdin**: if both `<file` and `<<EOF` appear on the same
command, the parser overwrites `cmd.stdin` with the second-parsed
redirect. Matches bash. No special handling needed at execution time —
the executor sees only the final `cmd.stdin` value.

## History persistence (option B)

The user chose option B for multi-line history: preserve heredoc bodies
verbatim across save/load by escaping `\n` and `\\` in the on-disk
format.

`src/history.rs`:

**On `save`**: for each history entry, escape `\` → `\\` and `\n` → `\n`
(the two-char sequence). Write one escaped line per entry.

**On `load`**: for each line, unescape `\n` → newline and `\\` → `\`.

**Backward-compat with pre-v24 history files**: pre-v24 files have no
escape encoding. The simple escape/unescape scheme is mostly transparent
for entries without literal `\` or newlines:
- A pre-v24 entry like `echo hi` round-trips unchanged.
- A pre-v24 entry containing a literal `\` (e.g., `echo a\\b` saved as
  `echo a\b` in the file) would, on first load with v24, unescape `\b`
  to whatever `b` is. Currently `\b` isn't a recognised escape — only
  `\n` and `\\` are. So loading is forgiving: any `\X` where X isn't
  `n` or `\` is left as `\X` literal. **This means pre-v24 history with
  literal backslashes loads unchanged**.

The escape set is INTENTIONALLY tiny — just `\n` and `\\`. This minimises
collision risk with pre-v24 contents and keeps the on-disk format
human-readable. New escapes can be added later without breaking
backward-compat (just need a versioned header eventually).

**History expansion** (`!!`, `!$`, etc.) operates on in-memory strings
that may now contain embedded newlines. Most operations don't care.
Confirm `history` builtin's output gracefully renders entries with
embedded newlines (probably print as-is, which is correct).

## Edge cases

- **Empty body** (`<<EOF<NL>EOF`): body is empty string. Child gets
  immediately-closed stdin. Valid.
- **`<<EOF` with no following lines at end-of-input**: `LexError::UnterminatedHeredoc`
  → `Incomplete(Heredoc)` → REPL prompts for more lines.
- **`<<EOF` followed by close on same line**: not valid POSIX —
  `<<EOF EOF` puts `EOF` as an arg, and the heredoc still demands a body
  on the next line.
- **Close-delimiter with trailing whitespace**: doesn't match.
  Continuation keeps prompting.
- **Close-delimiter inside a body line**: doesn't match (must be the
  entire line).
- **Multiple here-docs to different fds** — currently out of scope
  (huck doesn't support `<<EOF` on fd N other than stdin).
- **Heredoc body containing the delimiter as a substring**: not a close
  (must be the entire line).
- **Heredoc on a backgrounded command** (`cmd <<EOF & body EOF`):
  bash supports this; huck should too. The body is collected at parse
  time, then `cmd` runs in background with that stdin.
- **Escape inside expanding body**: `\$`, `\\`, `` \` ``, `\<NL>` are
  recognised. Other backslashes (e.g. `\n` literal text) are kept as-is
  (POSIX 2.7.4 — backslash retains specialness only before `$`, `` ` ``,
  `\`, and newline inside expanding heredocs).
- **Inline assignments + heredoc**: `FOO=hi cat <<EOF<NL>$FOO<NL>EOF` —
  v23's inline-assignment path applies FOO=hi, then heredoc body is
  expanded with FOO visible, then external cat sees the body. The
  combination works because each layer is independent.

## Out of scope

- Here-strings (`<<<word`) — M-13, separate iteration. Smaller scope;
  candidate for v25.
- Here-doc on fds other than stdin (`N<<EOF`) — fd-duplication is M-18,
  separate iteration.
- Function/builtin reads of heredoc body — documented limitation;
  unblocking would need M-10 (functions in pipelines) and a per-builtin
  stdin parameter.
- Embedded-arithmetic in body (`$((expr))`) — already covered by the
  existing `WordPart::Arith` path if the lexer's body scanner emits it.
  v24 will include it if it falls out naturally; not required.

## Tests

### Lexer (`src/lexer.rs::tests`)

| Test | Covers |
| --- | --- |
| `tokenize_heredoc_simple_expand` | `cat <<EOF\nhello\nEOF` → token with Heredoc{body=Word[Literal{"hello\n"}], expand:true, strip_tabs:false} |
| `tokenize_heredoc_literal_no_expand` | `cat <<'EOF'\n$HOME\nEOF` → body one `Literal{quoted:true, text:"$HOME\n"}` |
| `tokenize_heredoc_strip_tabs_dash` | `<<-EOF\n\t\thello\n\tEOF` → body `"hello\n"` (tabs stripped from body AND close line) |
| `tokenize_heredoc_strip_tabs_with_literal_delim` | `<<-'EOF'` composes strip + no-expansion |
| `tokenize_heredoc_unclosed_errors` | `cat <<EOF\nhello` → `LexError::UnterminatedHeredoc` |
| `tokenize_heredoc_close_must_match_exactly` | `cat <<EOF\nhello\nEOF ` (trailing space) → unterminated |
| `tokenize_heredoc_close_must_not_have_leading_spaces` | `cat <<EOF\nhello\n  EOF` → unterminated (non-strip-tabs doesn't strip) |
| `tokenize_heredoc_multiple_in_order` | `cmd <<A <<B\nbody_a\nA\nbody_b\nB` → both bodies consumed |
| `tokenize_heredoc_body_var_part` | `cat <<EOF\n$USER\nEOF` → body has Var{name:"USER"} part |
| `tokenize_heredoc_body_command_sub` | `cat <<EOF\n$(date)\nEOF` → body has CommandSub part |
| `tokenize_heredoc_body_escape_dollar` | `cat <<EOF\n\\$LITERAL\nEOF` → body has Literal "$LITERAL" (escape kept the $ literal) |
| `tokenize_heredoc_body_backslash_passthrough` | `cat <<EOF\n\\d\nEOF` → body has Literal "\\d" (POSIX: `\X` other than `\$\``\\` is literal) |
| `tokenize_heredoc_empty_body` | `cat <<EOF\nEOF` → body Word is empty |
| `tokenize_heredoc_delim_partially_quoted_is_literal_mode` | `cat <<E"O"F\n$X\nEOF` → expand:false, delim:"EOF" |
| `tokenize_heredoc_delim_backslash_escaped_is_literal_mode` | `cat <<\EOF\n$X\nEOF` → expand:false |

### Classifier (`src/continuation.rs::tests`)

| Test | Covers |
| --- | --- |
| `classify_heredoc_unclosed_is_incomplete` | `cat <<EOF\nhello` → Incomplete(Heredoc) |
| `classify_heredoc_closed_is_complete` | `cat <<EOF\nhello\nEOF` → Complete |
| `joiner_for_heredoc_is_newline` | `joiner_for(Heredoc, _)` → `"\n"` |

### Parser (`src/command.rs::tests`)

| Test | Covers |
| --- | --- |
| `parse_heredoc_redirect_attaches_to_command` | `cmd <<EOF\nbody\nEOF` → ExecCommand.stdin = Some(Redirect::Heredoc{...}) |
| `parse_heredoc_last_wins_over_file_redirect` | `cmd <file <<EOF\nbody\nEOF` → stdin is the heredoc |
| `parse_multiple_heredocs_keep_last` | `cmd <<A <<B\nbody_a\nA\nbody_b\nB` → stdin is body_b |
| `parse_heredoc_in_pipeline_stage` | `cat <<EOF \| grep foo\nbody\nEOF` → stage 0 has heredoc, stage 1 doesn't |

### Integration (new `tests/heredoc_integration.rs`)

| Test | Script | Expected |
| --- | --- | --- |
| `heredoc_simple_expand_no_vars` | `cat <<EOF\nhello\nEOF` | `hello\n` |
| `heredoc_literal_no_expand` | `FOO=secret cat <<'EOF'\n$FOO\nEOF` | `$FOO\n` |
| `heredoc_expand_var` | `FOO=hi cat <<EOF\n$FOO\nEOF` | `hi\n` |
| `heredoc_expand_cmd_sub` | `cat <<EOF\n$(echo via-sub)\nEOF` | `via-sub\n` |
| `heredoc_strip_tabs` | `cat <<-EOF\n\t\thello\n\tEOF` | `hello\n` |
| `heredoc_in_pipeline` | `cat <<EOF \| grep marker\nmarker\nother\nEOF` | `marker\n` |
| `heredoc_multiple_per_command_last_wins` | `cat <<A <<B\nfirst\nA\nsecond\nB` | `second\n` |
| `heredoc_empty_body` | `cat <<EOF\nEOF` | (empty output) |
| `heredoc_with_inline_assignment_expand` | `FOO=hi cat <<EOF\nval=$FOO\nEOF` | `val=hi\n` |
| `heredoc_escape_dollar` | `cat <<EOF\n\\$NOT_EXPANDED\nEOF` | `$NOT_EXPANDED\n` |
| `heredoc_multi_line_body` | `cat <<EOF\nline1\nline2\nline3\nEOF` | `line1\nline2\nline3\n` |
| `heredoc_backgrounded_command_sees_body` | `cat <<EOF > /tmp/huck_v24_$$ &\nbackground-test\nEOF\nwait\ncat /tmp/huck_v24_$$\nrm -f /tmp/huck_v24_$$` | `background-test\n` |

### History (`src/history.rs::tests`)

| Test | Covers |
| --- | --- |
| `history_round_trips_embedded_newline` | save+load preserves entry with embedded `\n` |
| `history_round_trips_literal_backslash` | save+load preserves entry with literal `\` |
| `history_load_pre_v24_unescaped_format_works` | a file without escape sequences loads as-is |
| `history_load_pre_v24_with_backslash_not_followed_by_n_loads_literally` | `\b` in a pre-v24 file stays `\b` after load |

### PTY interactive (`tests/pty_interactive.rs`)

| Test | Covers |
| --- | --- |
| `pty_heredoc_simple` | Type `cat <<EOF<ENTER>hello<ENTER>EOF<ENTER>` → see `hello`; prompt returns |
| `pty_heredoc_continuation_prompt` | After `cat <<EOF<ENTER>`, prompt is `> ` |
| `pty_heredoc_ctrl_c_aborts` | Mid-heredoc Ctrl-C → main prompt back, body discarded |

**Total**: ~15 lexer + 3 classifier + 4 parser + 12 integration + 4 history + 3 PTY = ~41 new tests.

## Change log

- **2026-05-24**: Spec drafted; user-chosen scope = full POSIX (all 3
  variants, multiple heredocs, full expansion set, option B for history
  persistence with `\\` + `\n` escape encoding).
