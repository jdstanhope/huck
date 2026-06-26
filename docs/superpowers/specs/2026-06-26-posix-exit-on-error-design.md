# v226 — POSIX-mode non-interactive-exit-on-error (Cluster A)

## Status

Design approved 2026-06-26. Second iteration of "properly support POSIX mode"
(after v225's flag foundation). Implements bash's family of "a non-interactive
shell exits when this error occurs in POSIX mode" behaviors by reusing huck's
existing pending-fatal-status mechanism.

## Background

bash's "Bash POSIX Mode" reference lists a cluster of error classes that cause a
**non-interactive** shell to **exit** when `posix` mode is on, where a default
shell (or an interactive one) reports the error and continues. huck currently
*continues* on all of them. Verified against bash 5.2.21 (`bash --posix -c …`):

| trigger | bash --posix (non-interactive) | huck today |
| --- | --- | --- |
| `set -o nosuchopt` (special builtin error) | exit 2 | continues (rc 0) |
| `readonly x=1; x=2` (assign, no command) | exit 127 | continues |
| `readonly x=1; x=2 export y` (assign before special) | exit 127 | continues |
| `readonly i=1; for i in a b; do …; done` (readonly for var) | exit 127 | continues |
| `. /no/such/file` (source not found) | exit 1 | continues |
| `eval(){ :; }` (fn name == special builtin) | exit 2 | continues |
| `echo $(( 1 + ))` (arith syntax error) | exit 127 | continues (v215 made it non-fatal) |

This iteration is deliberate posix-correctness buildout. **It flips no bash-test
category** — the `errors` category FAIL is dominated by the error-message
prologue (`huck:` vs `script: line N:`), and `posix2`'s residual is OPTIND /
variable-quoting / `case esac` parsing — neither is gated by exit-on-error. The
value is broad correctness across ~7 error paths and the continued posix
foundation, accepted by the user with the no-flip known.

### Existing mechanism this reuses

huck already has `pending_fatal_pe_error: Option<i32>` on `Shell`, set by
parameter-expansion errors (`${x?word}`, nounset failures). When set, the
sequence runner short-circuits the rest of the current sequence
(`executor.rs:387,427`) and two top-level consumers drain it and exit with the
status: `run_program_in_sinks` (`shell.rs:265`, `-c`/script) and the line loop
(`repl.rs:207`, interactive/stdin). Verified behaviors of this path (bash-exact):
it unwinds through functions, `&&` lists, and `for` bodies; it runs the `EXIT`
trap on the way out; and a fatal inside `( … )` exits only the subshell (the
forked child has its own flag), the parent continuing.

## Goals

1. In `posix && !interactive` mode, each of the seven triggers above causes the
   shell to exit with bash 5.2.21's status; output is byte-identical to bash.
2. Default mode and interactive sessions are byte-for-byte unchanged.
3. The existing `${x?}` pending-fatal behavior is unchanged (same status, both
   modes).
4. No regression: `cargo test --workspace` green; func/cprint/herestr stay PASS.

## Non-goals / Out of scope

- **Assignment error before a regular/external command** (`x=2 true` with `x`
  readonly): bash aborts that command (and the rest of the input line) and
  *continues* — this is NOT a shell exit and is present-ish in both modes. A
  distinct "abort-rest-of-line" behavior; deferred.
- **Changing the existing `${x?}` pe-fatal exit status** (huck `1` vs bash
  `127`): pre-existing, both-modes; separate divergence, left as-is.
- **Cluster B** (format/validation toggles) and **gating huck's currently-
  unconditional posix behaviors** (trap names, bare `set`, inherit_errexit) —
  later iterations.
- Parameter-expansion errors beyond what huck already treats as fatal.

## Design

### Section 1 — mechanism

Rename the field to reflect its now-general role:

```rust
// Shell — was `pending_fatal_pe_error`
pub pending_fatal_status: Option<i32>,
```

Mechanical rename across its ~10 references (`expand.rs`, `executor.rs:387,427`,
`shell.rs:265`, `repl.rs:207`, `completion_spec.rs`, `take_pending_fatal_pe_error`
→ `take_pending_fatal_status`). No behavior change to the existing writers.

Add one gated setter on `Shell`:

```rust
/// Mark a POSIX-mode fatal error: a non-interactive posix shell exits with
/// `status`. No-op in default mode or interactively (matches bash).
pub fn posix_fatal(&mut self, status: i32) {
    if self.shell_options.posix && !self.is_interactive {
        self.pending_fatal_status = Some(status);
    }
}
```

Each detection site calls `shell.posix_fatal(code)` and returns
`ExecOutcome::Continue(code)`. The existing short-circuit drains the flag and the
top-level consumers exit with `code`. The existing `${x?}` path keeps writing
`pending_fatal_status = Some(1)` directly — unchanged.

### Section 2 — the seven exit cases

Each fires only in posix+non-interactive (via `posix_fatal`); default mode keeps
its current behavior. Statuses are bash 5.2.21's observed codes, pinned by the
diff harness.

1. **Special builtin hits a usage / bad-option / assignment error** (`set -o bad`
   → 2, `unset -z` → 2, `readonly -z` → 2, `return` outside a function → 2,
   `export AA[4]=1` → 1). This is NOT "the builtin returned non-zero": bash gates
   on the *kind* of error, not the status. Verified against bash 5.2.21:

   - **Fatal (exit):** bad option, bad usage, invalid-identifier assignment —
     bash's `EX_USAGE`/`EX_BADUSAGE` and assignment-error returns.
   - **Not fatal (continue):** runtime failures — `shift 99` (count out of range,
     status 1), `eval false` (propagated child status), and a *legitimate*
     `return 2` from a function. A status-value rule misfires on `return 2`.
   - **`command`/`builtin` strip it:** `command set -o bad` / `builtin set -o bad`
     / `command export AA[4]=1` all print the error and CONTINUE (exit 0).

   **Mechanism (per-error-kind, like bash's EX_USAGE):** add a signal
   `Shell.builtin_usage_error: Option<i32>`. Each special builtin sets it (to the
   status it is about to return) at its **usage / bad-option / bad-assignment**
   error sites — and only those, never its runtime-failure sites. The executor,
   in `run_exec_single`'s **bare special-builtin dispatch branch** (the path that
   excludes `command`/`builtin` and function shadowing — the same branch
   `is_special_builtin`/v225 `persistent` already live in), consumes the signal
   after dispatch: `if let Some(st) = shell.builtin_usage_error.take() {
   shell.posix_fatal(st); }`. Because `command`/`builtin` route through a
   different dispatch path that does not consume the signal (it is cleared/taken
   at the next command), their wrapped errors never exit — matching bash. The
   signal is cleared at the top of each simple-command dispatch so it cannot leak
   across commands. The per-builtin marking covers the POSIX special set (`:` `.`
   `break` `continue` `eval` `exec` `exit` `export` `readonly` `return` `set`
   `shift` `source` `trap` `unset`); each builtin's fatal-vs-runtime error
   classification is verified against `bash --posix` in the diff harness.
2. **Assignment error, no command follows** (`readonly x=1; x=2`). In the
   simple-command path when the program word is empty and an inline/standalone
   assignment failed (e.g. readonly target), call `posix_fatal(127)`.
3. **Assignment error before a special builtin** (`x=2 export y`). In
   `apply_inline_assignments`' error path: when the command's program
   `is_special_builtin`, `posix_fatal(127)`. When the program is a regular/
   external command, do NOT — that's the deferred abort-continue case.
4. **`for`/`select` iteration var is readonly.** In `run_for` (and `select` if
   present) when binding the loop variable fails because it is readonly,
   `posix_fatal(127)` and stop the loop.
5. **`.`/`source` filename not found.** In the source builtin's not-found branch,
   `posix_fatal(1)`.
6. **Function name == POSIX special builtin** (`eval(){ :; }`). At function-
   definition execution (`Command::FunctionDef` handling), if the name
   `is_special_builtin`, emit bash's message (`<name>: is a special builtin`),
   do not define the function, and `posix_fatal(2)`. A non-special clash (e.g.
   `cd`) is allowed (no error) — matches bash.
7. **Arith syntax error** (the v215 posix counterpart). At the arith-evaluation
   error path that v215 made non-fatal, additionally `posix_fatal(127)` so the
   non-interactive posix shell exits (default mode keeps v215's continue).

### Section 3 — semantics & edges

- **Gating:** `posix_fatal` is a no-op unless `posix && !interactive`; default and
  interactive behavior is unchanged. This is the entire regression surface.
- **Unwinding / EXIT trap / subshell isolation:** inherited from the reused
  mechanism (verified bash-exact) — no new code.
- **Status precedence:** each site sets the flag AND returns `Continue(code)` with
  that same `code`, and the short-circuit prevents later commands from
  overwriting `$?`, so the drained exit status equals the trigger's status.
- **#2/#3 share the assignment-application path:** one decision point —
  no-command or program-is-special → `posix_fatal`; program-is-regular → leave
  the current abort-continue behavior untouched (deferred case).
- **#1 `command`/`builtin` exclusion:** the `builtin_usage_error` signal is
  consumed (and thus turned into a `posix_fatal`) ONLY in the bare special-builtin
  dispatch branch. `command export …` / `builtin set …` run the builtin through a
  different path that does not consume the signal, so they print the error and
  continue — matching bash. The signal is cleared at the top of each simple-
  command dispatch so a wrapped error cannot leak into the next command's check.
- **#1 vs runtime errors:** only a special builtin's usage/bad-option/assignment
  error sites set the signal; runtime-failure sites (e.g. `shift` out-of-range)
  and status-propagating builtins (`eval`/`exec`/`exit`) leave it unset, so a
  legitimate `return 2` or `eval false` does not exit.

### Section 4 — testing & verification

- **Diff harness** `tests/scripts/posix_exit_on_error_diff_check.sh` (runner-
  style). **It compares STDOUT + exit code only, NOT stderr** — huck's error
  messages carry the `huck:` prologue (and some differ in wording) vs bash's
  `bash: line N: …`; that prologue/wording gap is a separate, deferred broad fix,
  and the Cluster A behavior under test (exit vs continue) is fully observable as
  "did `echo AFTER` reach stdout" + the exit status. So each fragment ends in
  `echo AFTER`, and the harness asserts huck's `(stdout, exit-code)` equals bash's
  for that fragment (stderr discarded). Each of the seven triggers runs twice —
  under `set -o posix` (no `AFTER`, bash's exit status) and in default mode
  (`AFTER` printed, exit status matches bash). Plus an EXIT-trap-fires fragment
  (the trap's `echo` reaches stdout before exit), a subshell-isolation fragment
  (`( trigger ); echo AFTER` → `AFTER` prints), and — for case #1 — the boundary
  cases that MUST still continue: `shift 99` (runtime error), `eval false`, a
  legitimate `f(){ return 2; }; f`, and the `command`/`builtin`-wrapped forms
  (`command set -o bad`, `builtin set -o bad`, `command export AA[4]=1`).
- **Unit tests** (`shell_state.rs` / `executor.rs`): `posix_fatal` sets the flag
  only when `posix && !interactive` (four-quadrant); a couple of triggers drain
  to the right status via the `exec_script` harness with `set -o posix`.
- **Regression guards:** existing `${x?}` pe-fatal status unchanged; default-mode
  behavior of all seven triggers unchanged; `cargo test --workspace` green;
  func/cprint/herestr stay PASS.
- **No category flip** — acceptance is the harness + no regression, documented.

## Risks

- **Status-code fidelity.** bash's codes are quirky (`127` for several, `2`/`1`
  for others). The harness pins each; if a code can't be matched cleanly,
  capture the discrepancy rather than guessing.
- **Rename churn.** The `pending_fatal_pe_error` → `pending_fatal_status` rename
  touches ~10 sites; a missed site is a compile error (caught immediately). The
  rename must not change any existing writer's status or gating.
- **#1 per-builtin marking is the bulk of the work and the main misfire risk.**
  Each special builtin's error sites must be classified fatal (usage/bad-option/
  assignment) vs non-fatal (runtime) and only the former set `builtin_usage_error`.
  Misclassifying makes a posix shell exit where bash continues (or vice-versa).
  Mitigations: (a) the diff harness pins the boundary cases (`shift 99`, `eval
  false`, legit `return 2`, the `command`/`builtin` forms) so a misclassification
  fails CI; (b) verify each special builtin's fatal set against `bash --posix`
  during implementation; (c) prefer marking at shared usage-error helpers (if the
  builtins funnel "invalid option"/"usage" through common code) to cover many
  sites at once, then spot-check the per-builtin runtime paths stay unmarked.
- **Signal leakage.** `builtin_usage_error` must be cleared at the top of each
  simple-command dispatch and only consumed in the bare special-builtin branch,
  or a wrapped/earlier error could wrongly exit a later command. The harness's
  `command set -o bad; echo AFTER` (must print AFTER) guards this.

## Divergence-doc / bookkeeping (on merge)

- `bash-divergences.md`: refresh the posix-mode roadmap `[deferred]` entry —
  Cluster A done; remaining = Cluster B + unconditional-behavior gating; note the
  two explicitly-excluded items (assignment-before-regular-command abort-continue;
  `${x?}` status `1` vs `127`).
- Memory files (`project_huck_iterations.md`, `bash-test-suite-value-map.md`,
  `MEMORY.md`): record v226 (Cluster A shipped, no flip, posix buildout).
