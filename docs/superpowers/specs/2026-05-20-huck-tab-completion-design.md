# huck v14: Tab Completion

**Date:** 2026-05-20
**Status:** Design

## Goal

Add Tab completion to huck's interactive prompt: command names,
filenames/paths, and variable names, depending on where the cursor
sits. Completion is supplied to `rustyline` through a custom
`Helper`.

## Scope

**In scope:**
- Command-name completion in command position — builtins plus
  executables found in `$PATH`
- Filename/path completion in argument position — entries of the
  relevant directory, directories suffixed with `/`
- Variable-name completion after `$` or `${` — current shell
  variable names
- bash-like display: first Tab completes the longest common prefix,
  a second Tab lists all candidates (`CompletionType::List`)
- Filename escaping: candidates with shell-special characters are
  backslash-escaped in the inserted text
- Leading `~/` in a path being completed is expanded before the
  directory is scanned
- Hidden files appear only when the typed prefix begins with `.`

**Out of scope (deferred):**
- `~user` username completion
- Completion of options/flags, or per-command argument completion
  (bash's programmable `complete`)
- Completion inside command substitution or arithmetic
- Glob-aware or context-aware completion beyond the three targets
- Auto-inserting the closing `}` for `${VAR`
- An integration test suite (Tab is a TTY-only keypress and cannot
  be driven from piped stdin — see Testing)

## Architecture

A new `src/completion.rs` module owns all completion logic. huck
switches its line editor from `rustyline::DefaultEditor` to a
parameterized `Editor<HuckHelper, FileHistory>`, where `HuckHelper`
implements rustyline's `Helper` trait and provides real completion.

The flow on each Tab press: rustyline calls
`HuckHelper::complete(line, pos, ctx)`; the helper calls `analyze`
to classify the cursor context, dispatches to one of three
completion sources, and returns `(start, candidates)` to rustyline,
which performs the longest-common-prefix insertion and list display.

### `HuckHelper`

```rust
pub struct HuckHelper {
    var_names: Vec<String>,  // all shell variable names, synced before each readline
    path: String,            // $PATH value, synced before each readline
}
```

`HuckHelper` implements four rustyline traits. `Completer` is
implemented for real; `Hinter` (with `type Hint = String`),
`Highlighter`, and `Validator` get trivial impls (their methods are
all defaulted, or near enough). `Helper` itself is an empty marker
supertrait: `impl Helper for HuckHelper {}`.

```rust
impl HuckHelper {
    pub fn new() -> Self;
    /// Refreshes the cached snapshot from live shell state.
    pub fn refresh(&mut self, shell: &Shell);
}
```

`refresh` copies `shell.var_names()` into `var_names` and
`shell.get("PATH").unwrap_or("")` into `path`. The REPL loop calls it
through `editor.helper_mut()` before every `readline`, so completion
stays correct after in-session `export PATH=...` or new assignments.

### `Candidate` and `CompletionContext`

```rust
pub struct Candidate {
    pub display: String,      // shown in the Tab-Tab list
    pub replacement: String,  // text inserted into the line
}

pub enum CompletionContext {
    Command  { prefix: String },
    Variable { prefix: String },
    File     { dir: String, prefix: String },
}
```

### Cursor-context analysis

```rust
pub fn analyze(line: &str, pos: usize) -> (usize, CompletionContext);
```

The returned `usize` is the byte offset where rustyline begins
replacing; the `CompletionContext` says what to complete and carries
the typed prefix (with backslash-escapes resolved).

Algorithm — a forward scan of `line[..pos]` (text after the cursor
is ignored):

Walk character by character tracking `in_single`, `in_double`, and
backslash-escape state. Maintain:
- `word_start` — byte offset where the current word began; reset to
  the next character's offset whenever an unquoted separator or
  whitespace is consumed.
- `is_command_pos` — true while no "real" word has appeared since the
  last command separator. A `NAME=value` assignment word does **not**
  count as the command, so a word following assignment words is
  still command position.

Word separators are unquoted whitespace, `;`, `|`, `&`, `<`, `>`. An
escaped space (`\ `) or a quoted space is part of the word.

After the scan, the current word is `line[word_start..pos]`, and the
prefix is that text with backslash-escapes resolved (`\x` → `x`).
Classify:

1. **Variable.** Find the last `$` in the current word. If
   everything from that `$` to the cursor is a valid in-progress
   reference — `$`, then an optional `{`, then zero or more name
   characters (`[A-Za-z0-9_]`) — the context is `Variable`. The
   replacement `start` is the offset just after the `$` (or after
   `${`); `prefix` is the name fragment. Works mid-word
   (`echo foo$HO`). No closing `}` is auto-inserted.

2. **File.** Otherwise, if the current word is not in command
   position: split the word at its last `/`. `dir` is everything up
   to and including that `/` (empty if there is none); `prefix` is
   the trailing filename fragment. The replacement `start` is the
   offset just after the last `/` (or `word_start`).

3. **Command.** Otherwise: `Command { prefix }`, `start = word_start`.

Edge cases:
- Empty line, or cursor right after a separator → `Command { prefix:
  "" }`.
- Cursor right after whitespace following a command word →
  `File { dir: "", prefix: "" }`.
- A word containing `/` is always `File`, even in command position
  (`./scr` or `bin/`) — bash path-completes a command with a slash.

This is a dedicated lightweight scanner, deliberately separate from
the huck lexer. The lexer parses complete commands for execution;
completion needs cursor-relative classification of a possibly
incomplete line.

### Completion sources

All return `Vec<Candidate>` and never error (filesystem failures are
swallowed — completion degrades to fewer candidates).

**`complete_command(prefix: &str, helper: &HuckHelper) -> Vec<Candidate>`**
- Builtins: every name in `BUILTIN_NAMES` that starts with `prefix`.
- `$PATH` executables: split `helper.path` on `:`; for each
  directory, read its entries; keep regular files whose name starts
  with `prefix` and whose Unix mode has an executable bit set
  (`mode & 0o111 != 0`). Unreadable directories are skipped.
- Merge, deduplicate, sort. `display == replacement`.
- Empty `prefix` yields all candidates.

**`complete_variable(prefix: &str, helper: &HuckHelper) -> Vec<Candidate>`**
- Every name in `helper.var_names` starting with `prefix`, sorted.
- `display == replacement` (variable names are `[A-Za-z0-9_]`, no
  escaping needed). The leading `$`/`${` is left untouched because
  `analyze` positions `start` after it.

**`complete_file(dir: &str, prefix: &str) -> Vec<Candidate>`**
- Resolve the scan directory: `dir` empty → `"."`; `dir` beginning
  with `~/` → the `~/` replaced by `$HOME/` (if `HOME` is unset, the
  `~/` form yields no candidates); otherwise `dir` as given (relative
  to CWD or absolute).
- Read directory entries; keep names starting with `prefix`. Hidden
  names (leading `.`) are kept only when `prefix` itself starts with
  `.`.
- A directory entry gets a trailing `/` appended to both `display`
  and `replacement` (and no trailing space, so the user can keep
  Tab-ing deeper).
- `replacement` backslash-escapes characters that would otherwise
  re-parse incorrectly: ASCII whitespace and the metacharacters
  `' " \ $ ; & | < > ( ) * ? [ ] ~ #` and backtick. `display` keeps
  the plain unescaped name.
- Sort by name.

Prefix matching is always on unescaped text: `analyze` resolves
backslash-escapes when extracting the word, so a user who typed
`my\ no` matches a real file `my notes.txt`. The escaped candidate
then overwrites the escaped prefix span `line[start..pos]` cleanly.

### `Completer::complete`

```rust
fn complete(&self, line: &str, pos: usize, _ctx: &Context<'_>)
    -> rustyline::Result<(usize, Vec<Pair>)>
{
    let (start, context) = analyze(line, pos);
    let candidates = match context {
        CompletionContext::Command  { prefix }      => complete_command(&prefix, self),
        CompletionContext::Variable { prefix }      => complete_variable(&prefix, self),
        CompletionContext::File     { dir, prefix } => complete_file(&dir, &prefix),
    };
    let pairs = candidates
        .into_iter()
        .map(|c| Pair { display: c.display, replacement: c.replacement })
        .collect();
    Ok((start, pairs))
}
```

`Completer::Candidate` is set to `rustyline::completion::Pair`.

### `builtins.rs` change

Command completion needs the set of builtin names, currently
hardcoded inside `is_builtin`'s `matches!`. Replace it with a single
source of truth:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history",
];

pub fn is_builtin(name: &str) -> bool {
    BUILTIN_NAMES.contains(&name)
}
```

`run_builtin`'s dispatch `match` is unchanged.

### `shell_state.rs` change

Add an accessor for all variable names (exported or not):

```rust
pub fn var_names(&self) -> impl Iterator<Item = &str> {
    self.vars.keys().map(|s| s.as_str())
}
```

### `shell.rs` integration

- Replace `use rustyline::DefaultEditor;` with the pieces needed for
  a custom editor: `rustyline::{Config, CompletionType, Editor}` and
  `rustyline::history::FileHistory`.
- Build the editor with completion configured:
  ```rust
  let config = Config::builder()
      .completion_type(CompletionType::List)
      .build();
  let mut editor: Editor<HuckHelper, FileHistory> =
      match Editor::with_config(config) {
          Ok(e) => e,
          Err(e) => { eprintln!("huck: failed to initialize line editor: {e}"); return 1; }
      };
  editor.set_helper(Some(HuckHelper::new()));
  ```
- Before each `editor.readline(PROMPT)`, refresh the helper:
  ```rust
  if let Some(h) = editor.helper_mut() {
      h.refresh(&shell);
  }
  ```
- The v13 history calls (`add_history_entry`, the startup seeding
  loop, `history().` use if any) work unchanged on the generic
  `Editor`.

### `main.rs` change

Register `mod completion;`.

## Data flow examples

`ec<Tab>` at an empty-ish prompt:

1. `analyze("ec", 2)` → `(0, Command { prefix: "ec" })`.
2. `complete_command("ec", helper)` → builtins starting with `ec`
   → `["echo"]`, plus any PATH executable starting with `ec`.
3. rustyline replaces `line[0..2]` — Tab completes to `echo`.

`cat src/le<Tab>`:

1. `analyze("cat src/le", 10)` → current word `src/le`, not command
   position → split at `/` → `(8, File { dir: "src/", prefix: "le" })`.
2. `complete_file("src/", "le")` scans `src/`, keeps `lexer.rs`
   → `[Candidate { display: "lexer.rs", replacement: "lexer.rs" }]`.
3. rustyline replaces `line[8..10]` (`le`) with `lexer.rs`.

`echo $HO<Tab>`:

1. `analyze("echo $HO", 8)` → current word `$HO`, last `$` at
   offset 5, valid reference → `(6, Variable { prefix: "HO" })`.
2. `complete_variable("HO", helper)` → `["HOME"]` (if `HOME` is set).
3. rustyline replaces `line[6..8]` (`HO`) with `HOME` → `echo $HOME`.

`cat my<Tab>` where the directory holds `my notes.txt`:

1. `analyze` → `File { dir: "", prefix: "my" }`, start at the `m`.
2. `complete_file(".", "my")` → `Candidate { display: "my notes.txt",
   replacement: "my\\ notes.txt" }`.
3. rustyline inserts `my\ notes.txt`; the list shows `my notes.txt`.

## Error handling summary

| Condition | Behavior |
|---|---|
| Unreadable directory (PATH entry or scan target) | skipped silently; fewer candidates |
| `HOME` unset while completing a `~/` path | that `dir` form yields no candidates |
| Empty command prefix | all builtins + all PATH executables returned |
| Empty variable prefix (`$<Tab>`, `${<Tab>`) | all variable names returned |
| No candidates match | empty list; rustyline does nothing |
| `complete` would error | never — the impl always returns `Ok` |

Completion is strictly read-only: it inspects the filesystem and the
cached env snapshot and never executes commands or expands globs.

## Testing

Tab is a TTY-only keypress; rustyline does not invoke completion on
piped stdin. There is therefore **no integration-test file** for
v14. The logic is covered by unit tests in `src/completion.rs`, all
callable directly:

**`analyze` tests:**
- Command position: empty line; after `;`, `|`, `&`; after one or
  more `NAME=value` assignment words
- File position: after a command word
- Variable: `$` at word end, `${`, mid-word `echo foo$BA`
- A word with `/` → `File { dir, prefix }` with the correct split
- Escaped space in the word → prefix has the escape resolved
- Cursor in the middle of the line (text after the cursor ignored)

**`complete_command` tests:**
- Builtin prefix match (`ec` → `echo`)
- PATH scan via a `TempDir`: an executable file matches, a
  non-executable file does not, a subdirectory does not
- Empty prefix returns builtins

**`complete_variable` tests:**
- Prefix match against a constructed `var_names` list, sorted

**`complete_file` tests** (using `tempfile::TempDir`):
- Plain prefix match
- A directory entry gets a trailing `/`
- A hidden file is excluded when the prefix lacks a leading `.`,
  included when it has one
- `~/` expansion with `HOME` pointed at a `TempDir`
- A space-containing filename: `replacement` is escaped, `display`
  is plain

**`Completer::complete` glue test:**
- Construct a `HuckHelper`, an empty `FileHistory`, a
  `rustyline::Context` from that history, and call
  `complete(line, pos, &ctx)` directly; assert the returned
  `(start, pairs)` for a command line, a file line, and a variable
  line. This exercises the full path without a TTY.

A manual smoke test covers actual interactive Tab behavior:
command, file, and variable completion; the Tab-Tab list; a
directory `/` suffix; a filename with a space.

## File layout impact

- **New:** `src/completion.rs` (~400 lines including tests)
- **Modify:** `src/builtins.rs` — `BUILTIN_NAMES` const; `is_builtin`
  rewritten to use it
- **Modify:** `src/shell_state.rs` — `var_names` accessor
- **Modify:** `src/shell.rs` — custom `Editor<HuckHelper,
  FileHistory>` with `CompletionType::List`; helper refresh before
  each `readline`
- **Modify:** `src/main.rs` — register `mod completion`
- **Modify:** `README.md` — v14 row, features section, test count
- **Cargo.toml:** no changes

## Open questions

None at design time.

## References

- rustyline 18 docs — `Completer`, `Helper`, `Config`,
  `CompletionType`, `completion::Pair`
- bash(1) — Programmable Completion / default completion behavior
