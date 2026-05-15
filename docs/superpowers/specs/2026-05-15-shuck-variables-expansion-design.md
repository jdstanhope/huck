# shuck — variables and expansion

**Date:** 2026-05-15
**Status:** Approved
**Builds on:** `2026-05-15-shuck-sequencing-design.md` (shuck v3)

## Overview

This adds shell variables and parameter expansion to `shuck`. After this
version, users can set shell variables with `FOO=bar`, expand them in
commands and redirects (`$FOO`, `${FOO}`), expand the previous command's
exit status (`$?`) and the home directory (`~`), and manage which
variables are exported to child processes (`export`, `unset`). Expansion
is quote-aware (single quotes literal, double quotes interpolate) and
performs unquoted-word-splitting on whitespace, like bash.

## Goals

- Shell variables: `FOO=bar` (standalone assignment, no command after).
- Parameter expansion: `$VAR`, `${VAR}`. Unknown variables expand to `""`.
- Last-status expansion: `$?`.
- Tilde expansion: `~` at the very start of a word (before `/`,
  whitespace, an operator, or EOF) → `$HOME`.
- Builtins `export` and `unset`. `export` marks variables as visible to
  child processes; `unset` removes them entirely.
- Quote-aware expansion: single quotes preserve `$` and `~` literally;
  double quotes interpolate `$VAR`/`${VAR}`/`$?` (still honoring `\$`,
  `\\`, `\"` escapes); tilde only expands unquoted.
- Word splitting on **unquoted** variable expansion (whitespace), so
  `FOO="a b"; echo $FOO` passes two args to `echo`.
- Children spawn with `env_clear()` + only the exported variables.
- Every existing v1–v3 behavior remains correct (pipes, redirection,
  sequencing, history, SIGINT, Ctrl-D, builtins).

## Non-goals (this version)

- Command substitution `$(...)` and backticks.
- Pathname globbing (`*`, `?`, character classes).
- Inline assignments preceding a command (`FOO=bar cmd`).
- `${VAR:-default}` and other parameter-expansion variants.
- Arithmetic `$((...))`.
- Positional / special parameters other than `$?` (`$$`, `$1`, `$#`, …).
- Brace expansion `{a,b,c}`.
- `~user` (other users' home directories).
- Quoted `\$` is the only multi-character escape we model in double
  quotes; we are not adding new escape sequences.

## Architecture

The v3 data flow was `&str -> Vec<Token> -> Sequence -> ExecOutcome`. The
shape doesn't change, but most internal types do, and `Shell` state
threads through:

```
&str
  -> lexer::tokenize          -> Vec<Token>    (Word carries Vec<WordPart>)
  -> command::parse           -> Sequence      (of Pipeline -> Vec<SimpleCommand>)
  -> executor::execute(&mut Shell)
       -> expand::expand(word, &Shell) at command time
       -> ExecOutcome
```

The lexer becomes quote- and expansion-aware (it tracks `$VAR`, `${VAR}`,
`$?`, `~`). The command types grow to hold `Word`s instead of `String`s,
and gain a `SimpleCommand::Assign` variant. A new `expand` module turns
`(Word, &Shell)` into `Vec<String>` (0 or more args, with bash-style
word splitting on unquoted parts). The executor takes `&mut Shell` so it
can mutate state (assignments, builtins) and supply child envs.

## Components

### lexer.rs

`Token::Word(String)` becomes `Token::Word(Word)` where:

```rust
#[derive(Debug, PartialEq, Eq)]
pub struct Word(pub Vec<WordPart>);

#[derive(Debug, PartialEq, Eq)]
pub enum WordPart {
    Literal(String),
    Var { name: String, quoted: bool },
    LastStatus { quoted: bool },
    Tilde,
}
```

The state machine carries a `quoted: bool` set true inside `"..."`. It
flushes any pending `Literal` accumulator before pushing a non-Literal
part, and resets the accumulator after. Adjacent runs still concatenate
into one `Word` (`foo"$BAR"baz` → one `Word` with three parts).

**`$` recognition** (outside single quotes):

- `$NAME` greedy-matches `[A-Za-z_][A-Za-z0-9_]*` → `Var { name, quoted }`.
- `${NAME}` requires `NAME` to match the same identifier rule. Invalid
  name (empty, leading digit, illegal chars) → `LexError::InvalidVarName`.
  Missing closing `}` before EOF → `LexError::UnterminatedBrace`.
- `$?` → `LastStatus { quoted }`.
- `$` followed by anything else (digit, `$`, `.`, space, EOF, …) → the
  `$` is a literal character. (Bash's `$$`, `$1`, etc. are out of scope.)

**`~` recognition** (outside quotes only, double or single):

- `~` at word start (no current Literal accumulator) **and** followed by
  `/`, ASCII whitespace, an operator metacharacter (`|`, `<`, `>`, `&`,
  `;`), or EOF → `Tilde` part.
- Otherwise (`~xyz`, mid-word `~`) → literal `~` character.

Inside `'...'` everything is literal (no `$`/`~` recognition); inside
`"..."`, `$` triggers expansion and `\$` / `\\` / `\"` continue to
escape (existing behavior preserved).

New `LexError` variants: `InvalidVarName`, `UnterminatedBrace`. Existing
variants (`UnterminatedQuote`, `BareAmpersand`) are unchanged.

### command.rs

`String`-valued fields become `Word`-valued, and a new `SimpleCommand`
enum captures the assignment-vs-execute distinction:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum Redirect {
    Truncate(Word),
    Append(Word),
}

#[derive(Debug, PartialEq, Eq)]
pub struct ExecCommand {
    pub program: Word,
    pub args: Vec<Word>,
    pub stdin: Option<Word>,
    pub stdout: Option<Redirect>,
    pub stderr: Option<Redirect>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SimpleCommand {
    Assign { name: String, value: Word },
    Exec(ExecCommand),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Pipeline {
    pub commands: Vec<SimpleCommand>, // invariant: never empty
}

// Connector, Sequence, ParseError unchanged
```

### Parser behavior

The pipeline-stage builder accumulates program/args/redirects into a
working `ExecCommand`, exactly as v3. The only new logic happens when a
stage is being finalized (on `|`, on a sequencing op, or at end-of-input):

If the stage's working `ExecCommand` is **structurally pure** — exactly
one program word, zero args, no redirects (`stdin`/`stdout`/`stderr` all
`None`) — **and** the program word's first part is a `Literal` whose
text matches `^[A-Za-z_][A-Za-z0-9_]*=`, the parser produces
`SimpleCommand::Assign { name, value }`. The name is the identifier
prefix; the value is a new `Word` whose first part is a `Literal`
holding the rest of the original first Literal (`bar` from `FOO=bar`,
possibly empty), followed by any further parts of the original word
verbatim (so `FOO=$BAR` → `value = Word([Literal("") , Var{BAR,unquoted}])`,
`FOO="hello world"` → `value = Word([Literal("hello world")])`).

Otherwise the stage becomes `SimpleCommand::Exec(...)`. This deliberately
means:

| Input | Result |
|-------|--------|
| `FOO=bar` | `Assign { name: "FOO", value: Word("bar") }` |
| `FOO=` (empty value) | `Assign { name: "FOO", value: Word("") }` |
| `FOO=$BAR` | `Assign { name: "FOO", value: Word([..., Var{BAR}]) }` |
| `FOO="a b"` | `Assign { name: "FOO", value: Word("a b") }` |
| `1FOO=bar` (invalid name) | `Exec(program="1FOO=bar")` (will fail "command not found") |
| `FOO=bar baz` (assignment + arg) | `Exec(program="FOO=bar", args=["baz"])` |
| `FOO=bar > f` (assignment + redirect) | `Exec(program="FOO=bar", stdout=...)` |

`parse_pipeline` still stops without consuming on `;`/`&&`/`||`; the
outer `parse` still builds `Sequence`; `ParseError` variants
(`MissingCommand`, `MissingRedirectTarget`, `RedirectTargetIsOperator`)
are unchanged.

### expand.rs (new module)

```rust
pub fn expand(word: &Word, shell: &Shell) -> Vec<String>;
```

Returns 0 or more strings. Walks `word.0`, maintaining `current: String`,
`has_emitted: bool`, and `result: Vec<String>`:

| Part | Action |
|------|--------|
| `Literal(s)` | append `s` to `current`; `has_emitted = true` |
| `Tilde` | append `shell.get("HOME").unwrap_or("")` to `current`; `has_emitted = true` |
| `Var { name, quoted: true }` | append `shell.get(name).unwrap_or("")` to `current`; `has_emitted = true` |
| `LastStatus { quoted: true }` | append `shell.last_status().to_string()` to `current`; `has_emitted = true` |
| `Var { quoted: false }` / `LastStatus { quoted: false }` | resolve to value, split on ASCII whitespace into fields. Zero fields: nothing. One field `f`: append to `current`, `has_emitted = true`. ≥2 fields `f1..fn`: append `f1` to `current`, push `current` to `result`, push `f2..f_{n-1}` to `result`, set `current = fn`, `has_emitted = true`. |

After the loop: push `current` to `result` iff `has_emitted`.

Examples (with `FOO="a b"`, `UNSET` not set):

- `echo "$FOO"` second word → `["a b"]`
- `echo $FOO` second word → `["a", "b"]`
- `echo $UNSET` second word → `[]` (zero args)
- `echo "$UNSET"` second word → `[""]`
- `echo a$FOO b` first stage args: word `a$FOO` → `["aa", "b"]`, word `b` → `["b"]` → flattened `["aa", "b", "b"]`
- `echo ""` second word → `[""]` (the lexer emitted a Word, has_emitted is true via the empty Literal)

### Shell state (shell.rs)

A `Shell` struct owned by `shell::run` for the session's lifetime:

```rust
struct Variable { value: String, exported: bool }

pub struct Shell {
    vars: HashMap<String, Variable>,
    last_status: i32,
}

impl Shell {
    pub fn new() -> Self;                                    // seeds from std::env::vars(), all exported
    pub fn get(&self, name: &str) -> Option<&str>;
    pub fn set(&mut self, name: &str, value: String);        // preserves existing `exported` flag
    pub fn export(&mut self, name: &str);                    // mark existing exported; create empty exported if absent
    pub fn export_set(&mut self, name: &str, value: String); // set value AND mark exported
    pub fn unset(&mut self, name: &str);
    pub fn last_status(&self) -> i32;
    pub fn set_last_status(&mut self, status: i32);
    pub fn exported_env(&self) -> impl Iterator<Item = (&str, &str)>;
}
```

`Shell::new` calls `std::env::vars()` and inserts every entry as
exported. Subprocess spawning becomes
`process.env_clear().envs(shell.exported_env())`: the child sees
exactly the exported set, no more and no less. Without `env_clear`,
`unset FOO` would still leak `FOO` to children because shuck's own
process env still has it.

### executor.rs

Signature: `pub fn execute(seq: &Sequence, shell: &mut Shell) -> ExecOutcome`.
`&mut Shell` threads through `run_pipeline`, `run_single`, and
`run_multi_stage`. The pipeline-level Exit short-circuit and the
sequence-level `&&`/`||`/`;` logic are unchanged.

**Per-stage logic** (applies in both single and multi-stage execution):

- **`SimpleCommand::Assign { name, value }`** as a standalone command
  (`run_single` only, i.e. the pipeline has exactly one stage):
  `expand(value, shell)` joined with single spaces produces one
  `String`. Assignment values are not subject to word splitting —
  joining with spaces matches the bash rule that `FOO=$BAR` sets
  `FOO` to `$BAR`'s value with internal whitespace intact.
  `shell.set(name, joined)`. Return `Continue(0)`.
- **`SimpleCommand::Assign` inside a multi-stage pipeline**: no-op,
  returning `Continue(0)`. Consistent with v2/v3 treatment of `cd` and
  `exit` in pipelines (shell-state changes from pipeline stages are
  conceptually subshell-scoped and dropped).
- **`SimpleCommand::Exec(cmd)`**:
  1. Expand `cmd.program` → `Vec<String>`. If empty, print
     `shuck: command not found:` and return `Continue(127)`.
     Otherwise the first becomes the program name; any further fields
     are prepended to the args.
  2. Expand each `cmd.args` Word, flatten into `Vec<String>`.
  3. For each redirect (`cmd.stdin`, `cmd.stdout`, `cmd.stderr`),
     expand its Word. The result must be **exactly one string** — zero
     or multi-field results print `shuck: ambiguous redirect` and
     return `Continue(1)` without running anything.
  4. Dispatch to the builtin or subprocess path, exactly as today.
  5. For subprocesses, build child env via
     `process.env_clear().envs(shell.exported_env())`.

`shell.set_last_status(code)` happens once per command line at the REPL
loop's `Continue(status)` arm (see §shell.rs); executor functions don't
touch `last_status` themselves.

### builtins.rs

```rust
pub fn run_builtin(
    name: &str, args: &[String], out: &mut dyn Write, shell: &mut Shell,
) -> ExecOutcome;
```

`is_builtin` returns true for `cd`, `exit`, `pwd`, `echo`, **`export`,
`unset`** (six total).

- **`export`**:
  - No args: print every exported variable as `export NAME=value`, one
    per line (no quoting on the value — this matches bash for shells
    without `set -o posix`). Returns `Continue(0)`.
  - Each remaining arg is one of:
    - `NAME` — mark existing variable as exported; if it doesn't exist,
      create it with value `""` and `exported: true`.
    - `NAME=value` — set value and mark exported.
  - Invalid name (`NAME` doesn't match `[A-Za-z_][A-Za-z0-9_]*`) →
    `shuck: export: '<arg>': not a valid identifier` to stderr, skip
    that arg, continue with the rest. If any arg was invalid, return
    `Continue(1)`; otherwise `Continue(0)`.

- **`unset`**:
  - Each arg is a `NAME`. Removes it from the shell entirely (the
    `Variable` entry is dropped — both shell-only and exported variants
    disappear). Unknown names are silently ignored (bash behavior).
  - Invalid name → `shuck: unset: '<arg>': not a valid identifier`,
    skip, continue. Return `Continue(1)` if any was invalid; else `Continue(0)`.

- **`cd` adjustment**: `cd` with no arg now reads `HOME` via
  `shell.get("HOME")` rather than `env::var("HOME")`. The session's
  initial `Shell` was seeded from `std::env::vars()`, so the default
  behavior is identical; the change is that a user-set
  `export HOME=...` (or even unset HOME) now takes effect.

- **`pwd`, `echo`, `exit`**: logic unchanged; their signatures gain
  `_shell: &mut Shell` (named `_shell`, unused).

`export NAME=value`'s `value` is the already-expanded string the
executor passed in. So `export PATH=$PATH:/x` works: the executor
expands `$PATH:/x` first, then hands `PATH=<expanded>` to `export`.

### shell.rs

`shell::run` constructs and owns the `Shell`. The v3 local
`let mut last_status: i32 = 0;` is removed; `shell.last_status()` and
`shell.set_last_status(...)` replace it.

```rust
pub fn run() -> i32 {
    install_sigint_handler();
    let mut editor = ...;
    let mut shell = Shell::new();
    loop {
        match editor.readline(PROMPT) {
            Ok(line) => {
                if !line.trim().is_empty() {
                    let _ = editor.add_history_entry(line.as_str());
                }
                match process_line(&line, &mut shell) {
                    ExecOutcome::Exit(code) => return code,
                    ExecOutcome::Continue(status) => shell.set_last_status(status),
                }
            }
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => return shell.last_status(),
            Err(e) => { eprintln!("shuck: input error: {e}"); return 1; }
        }
    }
}
```

`process_line` takes `&mut Shell` and forwards it to `executor::execute`.
The two error arms (`Err(LexError::...)`, `Err(ParseError)`) gain the new
`InvalidVarName` and `UnterminatedBrace` lex-error mappings via the
existing `lex_error_message` helper.

## Error handling

| Situation | Behavior |
|-----------|----------|
| `${UNCLOSED` | `shuck: syntax error: unterminated '${...}'`, `Continue(2)` |
| `${1foo}` / `${}` / `${-}` | `shuck: syntax error: invalid variable name in '${...}'`, `Continue(2)` |
| Unset `$UNSET` | Expands to empty string (0 args unquoted, 1 empty arg quoted) — not an error |
| `> $EMPTY` (redirect target expands to zero fields) | `shuck: ambiguous redirect`, `Continue(1)` |
| `> $MULTI` where `MULTI="a b"` unquoted (multi-field) | `shuck: ambiguous redirect`, `Continue(1)` |
| Program word expands to nothing | `shuck: command not found:`, `Continue(127)` |
| `export 1FOO=bar` | `shuck: export: '1FOO=bar': not a valid identifier`, that arg skipped, `Continue(1)` |
| `unset 1FOO` | `shuck: unset: '1FOO': not a valid identifier`, that arg skipped, `Continue(1)` |
| Assignment with redirect (`FOO=bar > file`) | Parser produces `Exec(program="FOO=bar")` → command not found at execute time |

Quote-aware expansion still respects the existing `LexError` rules:
single quotes preserve `$` and `~` literally; double quotes allow `$`
expansion and continue to honor `\$`, `\\`, `\"` escapes; backslash
outside quotes still escapes the next character. Every failure path
returns to the prompt; no input crashes the shell.

## Testing

- **Lexer unit tests:** `$VAR`, `${VAR}`, `$?`, `$NAME` inside `"..."`
  (still expansion, `quoted: true`), `$NAME` inside `'...'` (literal),
  `$$` / `$5` / `$.` (literal `$` + rest), `~` at word start vs. mid-
  word vs. `~xyz`, malformed `${...}` cases (`InvalidVarName`,
  `UnterminatedBrace`), and the existing test suite (sequencing /
  redirection / quoting) updated to expect structured `Word`s.

- **Parser unit tests:** assignment recognition (`FOO=bar`, `FOO=`,
  `FOO=$BAR`, `FOO="hello world"`), non-assignment edges (`1FOO=bar`,
  `FOO=bar baz`, `FOO=bar > f`, `FOO=bar | cat`), and that all existing
  parser tests still pass with the renamed `SimpleCommand::Exec`
  variant.

- **Expander unit tests** (new): each example in the §expand.rs table,
  plus empty literal, mixed quoted/unquoted, `$?`, tilde, and
  consecutive expansions inside one Word.

- **Shell unit tests** (new): `Shell::new()` captures env;
  `set`/`get`/`export`/`unset`/`export_set` round-trip; `exported_env`
  yields only exported entries.

- **Builtins unit tests** (new): `export NAME=value` sets and exports,
  `export NAME` marks existing, `unset` removes (including a previously
  inherited env var), invalid-identifier rejection. Testable purely
  against a `Shell`, no process needed.

- **Executor:** manual smoke testing (real processes), per the v1/v2/v3
  pattern. The smoke checklist covers:
  - `FOO=bar; echo $FOO`
  - `FOO=bar; echo "$FOO"` vs `echo $FOO` after `FOO="a b"` (word splitting)
  - `$?` after a known-exit command (`false; echo $?`)
  - `cd /tmp; echo ~` and `echo ~/projects`
  - `export FOO=bar; env | grep ^FOO=`
  - `unset PATH` (then run a builtin to confirm shell still works) and
    confirm a subprocess can no longer find unset-only-on-shuck binaries
  - `> $UNSET` and `> $MULTI_FIELD` → ambiguous redirect
  - Mixed with `&&`/`||`/`;` and `|`/`>`/`<`

## Future extensions (still not in scope)

- Command substitution `$(...)`, backticks.
- Pathname globbing.
- Inline assignments (`FOO=bar cmd`).
- `${VAR:-default}`, `${#VAR}`, etc.
- Arithmetic `$((...))`.
- Other positional / special parameters.
- `~user`, brace expansion.
