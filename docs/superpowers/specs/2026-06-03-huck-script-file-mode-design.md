# huck v82 — script-file mode + `-c` Design

**Status:** approved design, ready for implementation plan.
**Closes:** the deferred "bare-positional script-execution mode" and `-c COMMAND` noted in M-77's deferred list (`docs/bash-divergences.md`).
**Branch (impl):** `v82-script-mode` (created from `main` at plan time).

## Goal

Let huck run a script non-interactively, two ways:

```sh
huck SCRIPT [ARG...]        # file mode: run the file, $0=SCRIPT, $1..=ARGs
huck -c COMMAND [NAME [ARG...]]   # command mode: run the string, $0=NAME, $1..=ARGs
```

Plus `--` to end huck's own option scanning. Both modes are non-interactive
(no REPL, no rc file, no history) and read the *program* from a file / the
command string rather than from stdin — which leaves **stdin (fd 0) free** for
interactive builtins (`read`, `select`). This directly removes the M-72/L-12
testing constraint that blocked piped testing of `select`'s read path in v81.

**Scope (approved at brainstorm):** file mode, `-c`, and `--`. Out of scope:
`-s` (read commands from stdin while taking positionals), a `-` operand
(explicit stdin script), and login-shell / `-i` / `-l` (separate concern).

## Background — verified bash 5.2 behavior (the semantics to match)

- **`-c COMMAND [NAME [ARG...]]`**: run COMMAND. The FIRST operand after COMMAND
  becomes `$0` (NAME), the rest become `$1`, `$2`, …
  - `bash -c 'echo "0=$0 1=$1 #=$#"'` → `0=bash 1= #=0` (no operands → `$0` = shell name, `$#` = 0).
  - `bash -c 'echo "0=$0 1=$1 2=$2 #=$#"' name a b` → `0=name 1=a 2=b #=2`.
- **File mode**: `bash SCRIPT x y` → `$0` = SCRIPT (path as given), `$1`=x, `$2`=y, `$#`=2.
- **Missing file**: `bash /nope` → `bash: /nope: No such file or directory`, exit **127**.
- **`--`**: ends option scanning. Once `-c COMMAND` is consumed, the remaining
  args are operands verbatim — a literal `--` among them is just `$1` (NOT
  special). So `--` is only meaningful while scanning leading options (e.g.
  `huck -- script` treats `script` as the script even if it began with `-`).

## Design

### Mode resolution (`src/shell.rs`, `parse_cli` → a `RunMode`)

Replace `CliOptions`'s "reject any positional" with a resolver producing one of:

```rust
enum RunMode {
    Interactive,                         // current behavior (REPL or piped stdin)
    Command { command: String, argv0: String, args: Vec<String> },
    File    { path: PathBuf, args: Vec<String> },   // argv0 = path
}
```

Scanning algorithm (single left-to-right pass over `args`):
1. While the current arg starts with `-` and is a recognized option, consume it:
   - `--norc`, `--rcfile PATH`, `--rcfile=PATH` (existing).
   - `-c` → the NEXT arg is the command string (error "`-c`: option requires an
     argument" / exit 2 if absent). After consuming `-c COMMAND`, **stop option
     scanning**; everything after is operands.
   - `--` → stop option scanning; do NOT consume further as options.
   - An unrecognized `-x` → error "unrecognized option" / exit 2 (unchanged).
2. After scanning, with the remaining operands `rest`:
   - If `-c` was seen → `Command { command, argv0: rest.first() or shell name,
     args: rest[1..] }`.
   - Else if `rest` is non-empty → `File { path: rest[0], args: rest[1..] }`.
   - Else → `Interactive`.

Precedence: `-c` wins over a script path (matches bash — with `-c`, leading
operands are NAME/args, never a file). `--rcfile`/`--norc` are irrelevant in
non-interactive modes (rc is skipped) but parse without error.

Edge cases:
- `-c` with an empty COMMAND string (`huck -c ''`) → runs nothing, exit 0
  (bash: exit 0).
- `huck -- -weird-name` → File mode, path `-weird-name`.
- `huck -c cmd -- -x` → Command mode: `$0`=`--`, `$1`=`-x` (operands verbatim,
  matching bash).

### Execution (`src/shell.rs::run()`)

For `Command` and `File`, branch BEFORE building the rustyline editor / entering
the REPL loop. A single helper, e.g. `run_program(contents, argv0, args, label,
shell) -> i32`:
1. `shell.is_interactive = false` (so `maybe_source_rc_file` skips — already
   gated on `!is_interactive`).
2. `shell.shell_argv0 = argv0`; `shell.positional_args = args`.
3. Install the same job-control / SIGINT / SIGCHLD handlers `run()` installs
   (scripts may run pipelines/jobs).
4. Execute `contents` via the existing **`crate::builtins::run_sourced_contents(
   contents, label, shell)`** — the established non-interactive engine (rc-files
   + `source`); it assembles multi-line/compound commands via
   `continuation::classify`, so functions, loops, and heredocs in the script
   work. `label` is the script path (File) or the shell name (Command), used for
   syntax-error messages.
5. Translate the outcome to an exit code: `Exit(n)` → n; `Continue(s)` → s
   (also surface any pending fatal PE error as the exit status, mirroring the
   REPL's non-interactive behavior). Fire the EXIT trap, hang up jobs, then
   return. No history load/save.

- **File mode** reads the file first: `fs::read_to_string(path)`; on error print
  `huck: {path}: No such file or directory` (or the OS error) to stderr and
  return **127** (do NOT execute). If `path` is a directory, bash prints
  "… is a directory" and exits 126; huck may map any read error to 127 for v82
  and note the 126 distinction as a low divergence if not matched.
- **Command mode** passes the `-c` string as `contents` directly.

**Verification point for implementation (not a design change):** confirm
`run_sourced_contents` propagates `set -e` (errexit) and fatal parameter-
expansion errors as a non-zero exit in main-program use, matching a piped
non-interactive REPL. The rc path already runs through it non-interactively;
add a test that `huck script` with `set -e; false; echo nope` exits non-zero
and does not print `nope`.

### What stays unchanged
- Interactive REPL and piped-stdin REPL (no operands, no `-c`) — byte-identical
  to today.
- rc-file logic, history, completion — untouched (just skipped in script modes).

## The M-72/L-12 payoff

With the program coming from a file or the `-c` string, fd 0 is no longer
consumed by rustyline's script-ahead BufReader. So `huck script.sh < input`
feeds `input` to `read`/`select` normally. This lets v81's `select`
interactive-pick behavior be tested via a plain script file + redirected stdin
(no pty needed). v82 adds one such test as proof; retro-converting v81's pty
tests is optional follow-up, not in scope.

## Testing

1. **Integration tests** (`tests/script_mode_integration.rs`, drive the binary):
   - File mode: write a temp script, run `huck <file> a b c`; assert `$0`=path,
     `$1`=a, `$#`=3, `$@`; multi-line (function def + call, a `for` loop);
     `exit 7` → process exit 7; a leading `#!/usr/bin/env huck` shebang line is
     ignored (it's a comment); missing file → stderr message + exit 127.
   - `set -e` propagation: `set -e\nfalse\necho nope` in a file → exit non-zero,
     no `nope`.
   - Command mode: `huck -c 'echo "0=$0 1=$1 #=$#"' name a b` → `0=name 1=a #=2`;
     `huck -c 'echo hi'` → `hi`, `$0` is the shell name; multi-statement
     `-c 'x=1; echo $x'`; multi-line `-c $'for i in 1 2\ndo echo $i\ndone'`;
     `-c 'exit 5'` → exit 5; empty `-c ''` → exit 0.
   - `--`: `huck -- <file>` runs the file; `huck -c cmd -- -x` → `$1`=`-x`.
   - **M-72/L-12 payoff**: a temp script using `read x; echo "got=$x"` (and/or a
     `select` pick) run as `huck <file> < input` → reads `input` correctly.
2. **bash-diff harness** `tests/scripts/script_mode_diff_check.sh` (huck's 9th):
   compare `bash -c 'FRAG'` vs `huck -c 'FRAG'` byte-for-byte for several `-c`
   fragments (positional quirk, multi-statement, exit code, arithmetic/loops),
   and a temp-script-file fragment (`bash FILE a b` vs `huck FILE a b`). Use the
   same HUCK_BIN check + PASS/FAIL counting as the other harnesses.
3. **Unit tests** for `parse_cli`/mode resolution: each mode + precedence + `--`
   + `-c` missing-argument error + unknown-flag error.

## Out of scope / possible divergences

- `-s`, `-` operand, `--login`/`-l`/`-i`: deferred (noted in M-77).
- Directory-as-script exit code: bash 126 vs huck 127 (note as low divergence if
  not matched).
- `$0` in error messages for `-c` mode: bash uses the shell name; huck uses
  `shell_argv0` — match it.

## File-change map

| File | Change |
|------|--------|
| `src/shell.rs` | `CliOptions`→`RunMode` resolver in `parse_cli`; `-c`/`--` scanning; `run()` branches to a `run_program` helper for Command/File modes (set is_interactive/argv0/positionals, read file w/ 127 on error, run via `run_sourced_contents`, exit-code translation, EXIT trap, no REPL); unit tests for mode resolution |
| `src/main.rs` | unchanged (already passes argv) |
| `tests/script_mode_integration.rs` | NEW — binary-driven integration tests incl. the read-via-file payoff test |
| `tests/scripts/script_mode_diff_check.sh` | NEW — huck's 9th bash-diff harness |
| `docs/bash-divergences.md` | flip M-77's deferred script-mode/`-c` items to `[fixed v82]` (or new entry); describe the three modes + the M-72/L-12 resolution; changelog; summary stamp |
| `README.md` | v82 iteration row |
