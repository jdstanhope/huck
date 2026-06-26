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

## The emission site

In `crates/huck-engine/src/executor.rs`:

**Site 5327** (in `run_subprocess`, `cmd: &ResolvedCommand`, `shell: &mut Shell`
in scope) — the spawn-`NotFound` branch, the path EVERY missing command takes
once it resolves to an external program and the spawn fails. This is the ONLY
site this iteration converts.
```rust
{ let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: command not found: {}", cmd.program); }
ExecOutcome::Continue(127)
```
This fires for a normal missing command (`nosuch`) AND for a quoted-empty `''`
command — `''` expands to one empty FIELD, so `cmd.program` is `""` and the spawn
of `""` fails `NotFound`, landing here with an empty program name.

**Site 3443 is NOT converted** (measurement correction — see below). It is the
`resolve` branch reached when the command word expands to ZERO fields
(`prog_fields.is_empty()`), e.g. unquoted `$empty`. bash never emits
`: command not found` from a zero-field word — it either no-ops (rc 0) or
promotes a surviving field — so converting its message would be wrong. It is left
as-is and folded into the deferred empty-command-word divergence (non-goals).

## Fix design

Route the site-5327 message through `error_prefix(None)` and put the name before
the phrase. `error_prefix(None)` yields `<BASH_SOURCE[0] or $0>: line N: `
non-interactively and `huck: ` interactively (it handles the mode split).

Site 5327 → `{error_prefix(None)}{program}: command not found`. Compute the
prefix from `shell` BEFORE acquiring the `err` writer (the `err_writer` borrow
shape; mirrors v227's `assign()` fix) so the `&shell` borrow ends first:
```rust
{
    let prefix = shell.error_prefix(None);
    let mut err = err_writer(err_sink, sink);
    e!(&mut *err, "{prefix}{}: command not found", cmd.program);
}
ExecOutcome::Continue(127)
```

Resulting parity (site 5327):
- normal missing command, file mode: `./s.sh: line 1: foo: command not found`
  — byte-identical to bash (the bash-test categories run in file mode).
- quoted-empty `''`, file mode: `./s.sh: line 1: : command not found` (empty
  program) — byte-identical to bash.
- `-c` / interactive: word order matches; the prologue NAME differs (`huck`/argv0
  vs `bash`) — the universal argv0 divergence, unchanged and out of scope.

Exit code (127) is already correct and unchanged.

## Non-goals

- **Empty-command-word divergence** (deferred, recorded as a new divergence —
  generalizes the field-promotion bug): when a command word expands to ZERO
  fields (site 3443), huck always errors `huck: command not found:` rc 127, but
  bash's behavior depends on what remains: `$empty` alone → no-op rc 0 (no
  command); `$empty >redir` → redirection-only, rc 0; `$empty arg` → promotes
  `arg` as the command (`./s.sh: line N: arg: command not found`). None of these
  is a `: command not found` from a zero-field word, so site 3443's message is
  never bash-correct and is left untouched here. This is an expansion/field +
  empty-simple-command-semantics bug, distinct from message word order, and the
  bash-test categories exercise the normal external-not-found path (site 5327),
  not this. (The quoted-empty `''` REAL-field case is handled — it goes through
  site 5327, not 3443.)
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
- quoted-empty `''` command (a real empty field → site 5327, empty program) →
  `<path>: line N: : command not found`, rc 127.

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
- Borrow shape at site 5327: compute the `error_prefix(None)` prefix into a
  local before taking the `err` writer (same pattern v227 used in `assign()`).
