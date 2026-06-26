# v228 — command-not-found word order — Design

**Status:** approved (brainstorm 2026-06-26)
**Iteration goal:** Match bash 5.2.21's `<prologue> <name>: command not found`
format for the bare-command not-found error, replacing huck's
`huck: command not found: <name>`. A broad-shrink iteration — it flips no
bash-test category, but removes a recurring divergence across alias / builtins
/ execscript / errors (and any script that runs a missing command) and is a
prerequisite that brings those categories closer to future flips.

## Background & measurement

v227's measure-first survey of the 11 prologue-touched categories identified two
genuinely broad shared blockers (each spanning 4–5 categories) bigger in reach
than the error-prologue prefix itself: (1) command-not-found word order, and (2)
Rust `io::Error` text leakage. v228 takes the first.

huck currently prints `huck: command not found: <cmd>` in all modes. bash prints
`<name>: [line N: ]<cmd>: command not found` — the command name comes BEFORE the
phrase, and the line carries the non-interactive prologue:

| mode | bash 5.2.21 | huck (now) |
|---|---|---|
| file (`./s.sh`) | `./s.sh: line 1: foo: command not found` | `huck: command not found: foo` |
| `-c` | `bash: line 1: foo: command not found` | `huck: command not found: foo` |
| interactive/stdin | `bash: foo: command not found` | `huck: command not found: foo` |

Two wrongs: word order (`command not found: foo` → `foo: command not found`) and
the missing prologue (`huck:` → `<name>: line N:`). Both are fixed by routing
through the existing `Shell::error_prefix(None)` (the v216/v227 mechanism).

Category breadth (command-not-found lines per category diff, measured against
bash 5.2.21): alias 20/69, builtins 24/220, execscript 9/135, errors 11/290.
Fixing the format removes those lines, but every one of these categories retains
≥49 other diff lines (alias functionality, Rust `io::Error` text, unimplemented
umask/ulimit/enable, `set -o` gaps), so **no category flips**. The value is the
breadth (a recurring, correct-to-fix divergence) and the unblocking, consistent
with the "valuable across multiple issues" half of the project's bar.

## The two emission sites

Both in `crates/huck-engine/src/executor.rs`:

1. **Site 5327** — the spawn-`NotFound` branch (the normal path a bare missing
   command takes):
   ```rust
   { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: command not found: {}", cmd.program); }
   ExecOutcome::Continue(127)
   ```
2. **Site 3443** — `resolve`, reached when the command word expands to nothing
   (`prog_fields.is_empty()`), e.g. a quoted-empty `''` command:
   ```rust
   e!(err, "huck: command not found:");
   return Err(127);
   ```

## Fix design

Route both messages through `error_prefix(None)` and put the name before the
phrase. `error_prefix(None)` yields `<BASH_SOURCE[0] or $0>: line N: `
non-interactively and `huck: ` interactively (it handles the mode split).

- **Site 5327** → `{error_prefix(None)}{program}: command not found`, e.g.
  `./s.sh: line 1: foo: command not found` (file mode). Keep `ExecOutcome::Continue(127)`.
  Note the `err_writer`/`with_err` borrow shape: compute the prefix from `shell`
  BEFORE acquiring the `err` writer (mirroring v227's `assign()` fix), so the
  `&shell` borrow ends first.
- **Site 3443** → `{error_prefix(None)}: command not found` (empty name), e.g.
  `./s.sh: line 1: : command not found`. Keep `return Err(127)`. `error_prefix`
  borrows `&self` (`shell`) and `err` is a separate `&mut dyn Write` parameter,
  so compute the prefix into a local first.

Resulting parity:
- file mode: byte-identical to bash (the bash-test categories run in file mode).
- `-c` / interactive: word order matches; the prologue NAME differs (`huck`/argv0
  vs `bash`) — the universal argv0 divergence, unchanged and out of scope.

Exit code (127) is already correct and unchanged.

## Non-goals

- **Field-promotion bug** (deferred, recorded as a new divergence): when the
  command word expands to nothing but later fields survive (`$empty arg`), bash
  promotes the first surviving field as the command name
  (`./s.sh: line N: arg: command not found`); huck instead reaches site 3443 and
  reports an empty name. This is an expansion/field-handling bug, not a
  message-format issue, and the bash-test categories exercise the normal
  not-found path (site 5327), not this. Site 3443's format fix still makes the
  truly-empty `''` case bash-correct.
- **Builtin-specific `not found` messages** (`declare`/`type`/`hash`/`alias`/
  `unalias`/`command`/`.`): these are `huck: <builtin>: <name>: not found` with
  the builtin name and a different body; they are separate per-builtin prologue
  work for later iterations, not "command not found" word order.
- The interactive/`-c` prologue NAME (`huck`/argv0 vs `bash`) — the universal
  program-name divergence.

## Testing & verification

**New harness** `tests/scripts/command_not_found_diff_check.sh` — **file mode**
(write each fragment to a temp script, run `bash` and `huck` on the SAME path so
the prologue path matches, assert byte-identical stdout+stderr+rc):
- normal missing command on line 1 → `<path>: line 1: nosuch: command not found`,
  rc 127.
- missing command after earlier lines → asserts the line number is not 1 (the
  prologue's `line N:` tracks position).
- a missing command whose error is the second statement on a line / mid-script,
  confirming the word order and 127.
- truly-empty command (`''`) → `<path>: line N: : command not found`, rc 127.

**Regression:** the existing `command_bare_form_diff_check.sh` and
`assign_redirect_diff_check.sh` mention the not-found message but deliberately do
not assert it byte-identically (they were green before this change despite the
pre-existing format mismatch); re-run both to confirm they stay green after the
text change. Run `cargo test --workspace` and all 142 `tests/scripts/*_diff_check.sh`
harnesses (`funcnest_diff_check.sh` release-only). Re-measure the alias /
builtins / execscript / errors categories: the command-not-found lines must drop
out of each diff and no category may regress (FAIL→TIMEOUT/ERROR).

**Risks (assessed low):**
- The two harnesses that mention the message do not assert it (verified green
  pre-change); the full harness sweep is the backstop.
- Interactive output keeps `huck:` (via `error_prefix`), so no interactive test
  that substring-checks `command not found` regresses on word order alone —
  but the word order DOES change, so any test asserting the exact old string
  `command not found: NAME` would need updating; the workspace sweep catches it.
- Borrow shape at both sites: compute the `error_prefix(None)` prefix into a
  local before taking the `err` writer (same pattern v227 used in `assign()`).
