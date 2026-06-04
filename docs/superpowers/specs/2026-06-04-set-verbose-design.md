# huck v89 — `set -v` verbose mode Design

**Status:** approved design, ready for implementation plan.
**Implements:** `set -v` / `set +v` / `set -o verbose` — the shell echoes each
input line to stderr as it is read, before executing it. huck currently rejects
`set -v`/`set +v`/`set -o verbose` with "not yet supported in this version".
**Discovered:** loading a stock Debian `~/.bashrc` (the last two
`huck: set: -v/+v: not yet supported` errors after v88).
**Divergence tracker:** update **M-08** (`set` option flags — drop `verbose`
from its "Still deferred" list) + new sub-entry **M-08e** `[fixed v89]`.
**Branch (impl):** `v89-set-verbose` (created from `main` at plan time).

## Scope

Decided during brainstorming: implement the **full verbose echo behavior** (not a
recognize-only toggle), so `$-` honestly reflects what the option does
(consistent with v69/v86's no-lie policy for `set -o` flags).

**Out of scope** (documented as a sub-divergence): bash also echoes the argument
that `eval` re-parses (and trap-action re-parsing), because bash echoes whatever
its parser reads. huck echoes only at its two input readers, so `eval 'echo x'`
under verbose echoes the input `eval` line but not the re-parsed `echo x`. The
primary readers (interactive, script, `source`/`.`, `-c`, `--rcfile`) are fully
faithful.

## Verified bash 5.2 semantics (the implementation targets these)

- Verbose echoes each input line to **stderr** (stdout is unaffected).
- The echo is the **raw input line** (no expansion), plus a trailing newline.
- Ordering is **read → echo → execute**: the `set -v` line that *enables* verbose
  is NOT echoed (verbose was off when it was read); the `set +v` line that
  *disables* it IS echoed (verbose was still on when read), then verbose turns off.
- Every physical line is echoed, including **continuation lines** of a multi-line
  command (`if true` / `then echo x` / `fi` each echoed before the `if` runs),
  **comment** lines, and **blank** lines.
- `v` is a short flag, so it appears in `$-` when verbose is on.

## Part 1 — The `verbose` flag

- `src/shell_state.rs`: add `pub verbose: bool` to `ShellOptions`.
- `src/builtins.rs`:
  - `option_get`: `"verbose" => Some(shell.shell_options.verbose)`.
  - `option_set`: `"verbose" => { shell.shell_options.verbose = value; Ok(()) }`
    (moved out of the `Err(OptSetErr::Unimplemented)` path). This makes
    `set -o verbose` / `set +o verbose` and `shopt -so verbose` work.
  - The `set` short-flag cluster loop (the `match c { b'e' => …, b'u' => …, b'o'
    => …, other => "not yet supported" }` blocks for both `-`-cluster and
    `+`-cluster): add `b'v' => shell.shell_options.verbose = true` (in the `-`
    block) and `b'v' => shell.shell_options.verbose = false` (in the `+` block).
- `src/shell_state.rs` `dollar_dash_value`: append `'v'` after the `'u'` push when
  `self.shell_options.verbose` (so `$-` reads `…u v…` in huck's existing order).

## Part 2 — The echo behavior

When `shell.shell_options.verbose` is true, print each physical input line to
stderr as it is read, before it is parsed/executed. Two wiring points (the only
places huck reads input lines):

1. **`read_logical_command` (`src/shell.rs`)** — the interactive/REPL reader. In
   the `loop`, after the physical `line` is obtained (post history-expansion,
   which is what bash would have read) and before `classify`/return, if the shell
   (via `cell.borrow().shell_options.verbose`) is verbose, `eprintln!("{line}")`.
   Echo every physical line, including continuation lines.

2. **`run_sourced_contents` (`src/builtins.rs`)** — the script / `-c` / `source` /
   `--rcfile` reader. At the **top** of the `for line in contents.lines()` loop,
   before pushing into `buf`, if `shell.shell_options.verbose`, `eprintln!("{line}")`.

`eprintln!` adds the trailing newline; `contents.lines()` / `readline` strip the
input newline, so the echoed form matches bash (line + `\n`).

**Why this reproduces bash's ordering:** the echo uses the verbose state at the
moment the line is read, and any change to verbose happens only when an
already-read command executes. So the `set -v` line (read while off) is not
echoed; the next line (read after `set -v` executed) is; the `set +v` line (read
while still on) is echoed, then off. Confirmed against bash.

**Documented minor divergence (sub-point of M-08e):** `eval`'d strings and
trap-action bodies are re-parsed through `process_line`, not the two input
readers, so their re-parsed lines are NOT echoed under verbose. bash echoes them.
Edge case (verbose + eval/trap); not exercised by the bashrc/script use cases.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/shell_state.rs` | `ShellOptions.verbose`; `dollar_dash_value` appends `v` |
| `src/builtins.rs` | `option_get`/`option_set` handle `verbose`; `set` `-v`/`+v` cluster arms; echo in `run_sourced_contents` loop |
| `src/shell.rs` | echo each physical line in `read_logical_command` when verbose |
| `tests/set_verbose_integration.rs` | NEW — binary-driven integration tests |
| `tests/scripts/verbose_diff_check.sh` | NEW — huck's 16th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-08 "Still deferred" drops verbose; M-08e `[fixed v89]`; changelog; README v89 row |

## Testing

1. **Unit tests** (`src/builtins.rs` / `src/shell_state.rs`):
   - `set -v` then `option_get("verbose")` is `Some(true)`; `set +v` → `Some(false)`.
   - the `-v` short flag sets `shell_options.verbose`; `+v` clears it; `set -o
     verbose` / `set +o verbose` likewise (no "not yet supported").
   - `dollar_dash_value` contains `v` when verbose on, absent when off; ordering
     keeps `v` after `u` (`set -uv` → `$-` includes both, `u` before `v`).
2. **Integration tests** (`tests/set_verbose_integration.rs`) — capture stdout and
   stderr separately:
   - `set -v\necho hi\n` → stdout `hi\n`, stderr contains `echo hi\n`; the `set -v`
     line is NOT in stderr.
   - `echo a\nset -v\necho b\nset +v\necho c\n` → stderr is exactly `echo b\nset +v\n`
     (b's line echoed; `set -v` not; `set +v` echoed; `echo c` not), stdout `a\nb\nc\n`.
   - `set -v\nif true\nthen echo x\nfi\n` → stderr contains all three lines
     `if true`, `then echo x`, `fi`.
   - `set -v; echo $-\n` style: confirm `$-` includes `v` while verbose.
3. **bash-diff harness** `tests/scripts/verbose_diff_check.sh` (huck's 16th),
   byte-identical to bash 5.2 via `2>&1` (verbose output is the raw line, so it
   byte-matches — unlike `huck:`-prefixed error messages): the enabling/disabling
   fragment, a multi-line fragment, and a comment/blank-line fragment. **Excluded
   with a NOTE** (tested for rc only, not byte-diffed): the `eval` case (huck does
   not echo the re-parsed argument — the documented divergence).

## Edge cases & notes

- Verbose echoes BEFORE execution, so a command that fails/exits still had its
  line echoed.
- A multi-line command echoes all its physical lines (each at read time) before
  the command runs — matches bash.
- `set -v` is purely additive: with verbose off (the default) there is zero
  behavior change and zero output, so all existing tests are unaffected.
- The REPL echo goes to stderr regardless of tty; interactive verbose is rare but
  handled for completeness and consistency with the script path.
- `--rcfile` loading runs through `run_sourced_contents`, so a `set -v` inside the
  rcfile correctly echoes subsequent rc lines (matches bash sourcing an rc with
  verbose set).
