# fd one-model — Stage 1 (fork command substitution) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `$( … )` / backticks capture output by **forking a real subshell over a pipe** (parent drains, then `waitpid`s), replacing the in-process clone + `Capture` sink for command substitution. Handle `$(<file)` as a direct file read (fork-free). This fixes #353 and #195 **by construction** and is the first execution change of the #197 one-model arc.

**Architecture:** A new `executor::capture_via_fork(seq, shell) -> (String, i32)` reuses the existing fork machinery (`fork_and_run_in_subshell`, `ChildStdio`, `make_pipe`, `raw_status_to_exit_code`). `run_substitution` calls it instead of `execute_capturing`. The child runs the body as a `BraceGroup` (one fork), writing to real fd 1 (the pipe write-end) — so an inner `2>&1` is a real `dup2` onto the pipe and stderr is captured, and state changes / traps are isolated by the fork. `execute_capturing` is retained (dead-code-allowed) for the ~30 executor unit tests + the Stage 3 migration.

**Tech Stack:** Rust (huck-engine); the Stage-0 harnesses + `run_diff_checks.sh` + `tools/redirect_audit.sh` + the bash test-suite runner as the verification net.

**Issue:** [#197](https://github.com/jdstanhope/huck/issues/197) (umbrella); this stage closes [#353](https://github.com/jdstanhope/huck/issues/353) and [#195](https://github.com/jdstanhope/huck/issues/195). **Design:** `docs/superpowers/specs/2026-08-02-fd-one-model-design.md`.

## Validated by spike (2026-08-02)

A throwaway spike proved this exact approach: engine lib **1980/1980 green**, `$(…)` smoke + state-isolation + `$?`-from-exit all pass, a **>64 KB** capture does **not** deadlock, and **#353 flips** (huck captures the readonly-var error into `x` like bash). The full sweep's blast radius was **exactly 4 harnesses**: `comsub_capture_matrix` (the #353 pin now matches bash), `comsub_merge_stderr` (the #195 `2>&1 >file` pin now captures `er` like bash), `comsub_subshell_semantics` (the comsub `trap`-listing pin), and `dollar_lt` (the `$(<file)` path — must be handled pre-fork). This plan turns those four into: two closed divergences, one decided trap outcome, and one fixed file-read.

## Global Constraints

- Reuse existing fork machinery; do **not** hand-roll a second `fork`. The parent side is: `make_pipe` → `fork_and_run_in_subshell(BraceGroup(seq), shell, ChildStdio{Inherit, owned_raw(write_fd), Inherit}, NO_PGROUP, &[read_fd], None, None)` → close `write_fd` → drain `read_fd` to EOF → `waitpid` → `raw_status_to_exit_code` (which re-raises `sigint_flag` on SIGINT death, preserving interrupt propagation).
- The comsub child is a **transient foreground** child: `NO_PGROUP` (stays in the shell's process group), a direct `waitpid`, **never** entered in the `JobTable`, **never** sets `$!`.
- `$(<file)` (a comsub body that is exactly one redirect-only command with a single stdin `File{ReadOnly}` `<file`) is read directly into the result string **before** forking — model the read on executor.rs:4736-4760. No fork for this case.
- Preserve the three semantics `execute_capturing` handled: SIGINT propagation (re-raise `sigint_flag`), Timeout propagation, and DiscardCommand **containment** (a `$(( bad ))` inside `$()` returns 1 and the enclosing command **continues**).
- Do NOT run `cargo test --workspace` (OOM). Per-crate, single-threaded: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1`; relevant integration bins via `cargo test -p huck --test <name> --jobs 1 -- --test-threads 1` under `ulimit -v 8000000`.
- Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. `cargo fmt --all` before each commit.

---

### Task 1: The forking capture core + `$(<file)`

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (add `capture_via_fork`, near `fork_and_run_in_subshell` ~8686)
- Modify: `crates/huck-engine/src/expand.rs` (`run_substitution` ~2243; add the `$(<file)` pre-fork read)

**Interfaces:**
- Produces: `pub fn capture_via_fork(seq: &Sequence, shell: &mut Shell) -> (String, i32)`.
- Consumes: `fork_and_run_in_subshell`, `ChildStdio`, `ChildFd::{Inherit, owned_raw}`, `crate::child_fd::make_pipe`, `raw_status_to_exit_code`, `NO_PGROUP`, `Command::BraceGroup`.

- [ ] **Step 1: Add `capture_via_fork` to executor.rs** (validated spike body):

```rust
/// Stage 1 (#197): run a command-substitution body by FORKING a real subshell
/// whose stdout is a pipe the parent drains — replacing the in-process clone +
/// `Capture` sink. The child writes to real fd 1 (the pipe), so an inner `2>&1`
/// is a real dup2 onto the pipe (fixes #353/#195 by construction). Transient
/// foreground child in the shell's process group (not a job, no `$!`).
pub fn capture_via_fork(seq: &Sequence, shell: &mut Shell) -> (String, i32) {
    use crate::child_fd::{ChildFd, ChildStdio};
    use std::io::Read;
    use std::os::unix::io::FromRawFd;

    let (read_fd, write_fd) = match crate::child_fd::make_pipe(false) {
        Ok(p) => p,
        Err(e) => {
            crate::sh_error!(shell, None, "pipe: {}", crate::bash_io_error(&e));
            return (String::new(), 1);
        }
    };
    let body = Command::BraceGroup(Box::new(seq.clone()));
    let stdio = ChildStdio::new(
        ChildFd::Inherit,
        unsafe { ChildFd::owned_raw(write_fd) },
        ChildFd::Inherit,
    );
    let pid = match fork_and_run_in_subshell(&body, shell, stdio, NO_PGROUP, &[read_fd], None, None)
    {
        Ok(pid) => pid,
        Err(e) => {
            unsafe { libc::close(read_fd); libc::close(write_fd); }
            crate::sh_error!(shell, None, "fork: {}", crate::bash_io_error(&e));
            return (String::new(), 1);
        }
    };
    unsafe { libc::close(write_fd); }
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut f = unsafe { std::fs::File::from_raw_fd(read_fd) };
        let _ = f.read_to_end(&mut buf);
    }
    let mut raw: libc::c_int = 0;
    unsafe { libc::waitpid(pid, &mut raw, 0); }
    let status = raw_status_to_exit_code(raw, shell);
    (String::from_utf8_lossy(&buf).into_owned(), status)
}
```

- [ ] **Step 2: Add the `$(<file)` pre-fork read in `run_substitution`** (expand.rs). Before calling `capture_via_fork`, detect the redirect-only-input body and read the file directly:

```rust
pub fn run_substitution(seq: &Sequence, shell: &mut Shell) -> String {
    // `$(<file)` — a body that is exactly one redirect-only command with a single
    // stdin ReadOnly `<file` reads the file directly; no fork (design: file read).
    if let Some(contents) = try_read_file_substitution(seq, shell) {
        return strip_trailing_newlines(&contents);
    }
    shell.xtrace_depth += 1; // PS4 depth-repeat: $() / backticks add a level
    let (output, status) = executor::capture_via_fork(seq, shell);
    shell.xtrace_depth -= 1;
    shell.set_last_status(status);
    shell.set_last_cmd_sub_status(Some(status));
    strip_trailing_newlines(&output)
}
```

Implement `try_read_file_substitution(seq, shell) -> Option<String>` returning `Some` only when `seq.rest.is_empty()` and `seq.first` is an `ExecCommand` with empty program, no inline assignments, exactly one redirect that is `RedirOp::File { mode: FileMode::ReadOnly, target }` with `target_fd() == Some(0)`. Expand `target` to a path (via the existing single-word expansion), `std::fs::read` it, set `$?` to 0 (or 1 on read error, emitting `redir_open_error`), and return the bytes as a lossy `String`. Model the read + error path on executor.rs:4749-4764. On the error path set `last_status`/`last_cmd_sub_status` = 1 and return `Some(String::new())`. (Check the exact AST accessor names for the `ExecCommand` variant of `Command` and its `program`/`inline_assignments`/`redirects` fields.)

- [ ] **Step 3: Mark the now-unused helpers** — add `#[allow(dead_code)]` to `execute_capturing` and (if it becomes unused) `callbacks_thread_local::suspend`/`SuspendGuard`, with a comment: *retained for executor unit tests + the Stage 3 sink deletion (#197)*. Confirm `cargo build -p huck` is warning-clean.

- [ ] **Step 4: Build + targeted verification**

```bash
cargo build -p huck
HUCK=target/debug/huck
# $(<file) fixed:
$HUCK -c 'printf abc>/tmp/t1; x=$(</tmp/t1); printf "<%s>" "$x"'   # <abc>
# #353 flips (matches bash structure, prog-name aside):
$HUCK -c 'x=$(readonly r=1; (( r++ )) 2>&1); printf "<%s>" "$x"'
# DiscardCommand contained (enclosing continues):
$HUCK -c 'x=$( echo $((3.5)) ); echo after'                        # after
# >64 KB no deadlock:
$HUCK -c 'x=$(printf "%70000s" ""); echo ${#x}'                    # 70000
# procsub inside comsub:
$HUCK -c 'x=$(cat <(echo hi)); printf "<%s>" "$x"'                 # <hi>
bash tests/scripts/dollar_lt_diff_check.sh                         # green
```

- [ ] **Step 5: Engine lib + comsub integration bins**

```bash
ulimit -v 8000000
cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1        # expect all pass (spike: 1980/1980)
# run comsub / capture / procsub / xtrace integration bins that exist:
for t in $(ls tests/*.rs | xargs -n1 basename | sed 's/\.rs$//' | grep -iE 'comsub|capture|procsub|subshell|xtrace|dollar'); do
  cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 2>&1 | tail -1
done
```
Expected: green. If a comsub-behavior unit/integration test encodes the old in-process quirk, investigate against bash before changing it (it may be a real Stage-1 behavior improvement — or a genuine regression).

- [ ] **Step 6: Commit** — `cargo fmt --all`; commit executor.rs + expand.rs: `feat(#197): fork command substitution (Stage 1); $(<file) as a file read`.

---

### Task 2: Reconcile the flipped harness pins (close #353, #195)

**Files:**
- Modify: `tests/scripts/comsub_capture_matrix_diff_check.sh`
- Modify: `tests/scripts/comsub_merge_stderr_diff_check.sh`

- [ ] **Step 1:** In `comsub_capture_matrix`, the `readonly-arith-2>&1` case now matches bash — change it from `check_pin` to `check` (delete the pinned expected strings + the `# STAGE-1 TARGET (#353)` comment). Run the harness; it must be green as a `check` (assert vs bash).

- [ ] **Step 2:** In `comsub_merge_stderr`, the `oos-2>&1>file` case (pinned huck-to-itself for #195) now captures `er` like bash — convert its bespoke self-comparison into a real `check` vs bash (or the harness's standard `check` helper). Update the OUT-OF-SCOPE comment to record #195 as resolved by the Stage-1 fork. Run it green.

- [ ] **Step 3:** Re-run both harnesses (`bash tests/scripts/<name>` with the debug binary) — green. Commit: `test(#197): close #353/#195 pins now that the fork captures stderr`.

---

### Task 3: comsub `trap` listing under the fork

**Files:**
- Investigate: `crates/huck-engine/src/executor.rs` / `traps.rs` / the `trap` builtin
- Modify: `tests/scripts/comsub_subshell_semantics_diff_check.sh` (and engine code only if an in-scope fix is clean)

- [ ] **Step 1: Characterize.** Under Task 1's fork, `x=$(trap)` inside a shell with an EXIT trap now lists **nothing** (spike: `list=[]`), where bash lists the inherited EXIT trap plus the default job-control ignores (`SIGTSTP`/`SIGTTIN`/`SIGTTOU`). Determine why the forked child's `trap` builtin sees no traps (does `fork_and_run_in_subshell`'s child reset `shell.traps`, or does the `trap` builtin filter under a subshell?). Compare to how a plain `( trap )` subshell behaves in huck — this may be a pre-existing subshell-trap-display gap the fork merely exposes.

- [ ] **Step 2: Decide (ask the coordinator if unclear).** If a clean, localized fix makes the forked child's `trap` list the inherited settings for display (without changing pending-action reset semantics), do it and turn the harness case into a `check` vs bash. If it is a broader subshell-trap-display issue out of Stage-1 scope, **re-pin** the case to the new current behavior with a `# STAGE-1 TARGET (#NEW)` comment and file a follow-up issue. Either way the harness ends green.

- [ ] **Step 3:** Commit: `fix(#197): comsub trap listing under fork` (or `test(#197): re-pin comsub trap listing; file #NEW`).

---

### Task 4: Full verification, dead-code, docs

**Files:**
- Modify: `docs/superpowers/specs/2026-08-02-fd-one-model-design.md` (mark Stage 1 landed; record the trap outcome)

- [ ] **Step 1: Build both binaries** — `cargo build --locked -p huck && cargo build --release --locked -p huck`.
- [ ] **Step 2: Full sweep** — `ulimit -v 8000000; tests/scripts/run_diff_checks.sh` → `… 0 failed` (all comsub harnesses green; `dollar_lt` green).
- [ ] **Step 3: Differential audits** — `tools/redirect_audit.sh` must show **0 DIVERGE** (or only pre-existing ones — compare to main); note the result. Optionally run a short `tools/soak/run_soak.sh` sample to confirm no new fd/job leak from the per-comsub fork.
- [ ] **Step 4: Integration bins + bash-suite runner** — run each `tests/*.rs` integration binary that touches comsub/capture/redirect/procsub/xtrace/jobs single-threaded; then the bash test-suite runner (with `BASH_SOURCE_DIR` + `HUCK_BIN=release`) and confirm the PASS-set is unchanged vs main (comsub forking must not regress any category). Record the count.
- [ ] **Step 5: Perf note (informational, not a gate)** — time `for i in $(seq 1 1000); do x=$(echo hi); done` on huck before/after; record the fork-per-comsub cost in the report. No action unless it is catastrophic.
- [ ] **Step 6:** Update the design doc: under Stage 1, note it landed, #353/#195 closed, and the trap outcome. Commit: `docs(#197): Stage 1 landed — fork comsub; #353/#195 closed`.

## Self-Review

- **Spec coverage:** Task 1 = the design's "captured execution regions fork" + "`$(<file)` is a file read"; Task 2 = the pinned Stage-1 targets #353/#195; Task 3 = the trap semantics risk from the design's "real-subshell semantics may shift edges"; Task 4 = the design's verification net (audits + sweep + bash harness + perf note). Covered.
- **Placeholder scan:** the core code is the validated spike verbatim; `try_read_file_substitution` and the trap decision are the two genuinely open pieces and are specified with their model code / decision procedure, not left vague.
- **Behavior net:** every task ends green; Task 1's deliverable is verified by engine-lib + `$(<file)` + the #353/discard/large-output/procsub smoke checks even though the two win-harnesses are reconciled in Task 2 (ledger tracks the ordering).
- **Interfaces consistent:** `capture_via_fork` signature and the fork-helper argument list match executor.rs as read during the spike.
