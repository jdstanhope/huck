# v298 batch B — builtin write-error (#137) + InProcess stage redirect order (#144) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix two builtin/pipeline output-fidelity divergences from bash: (#137) a builtin's swallowed stdout write error, and (#144) an in-process pipeline stage applying `2>&1 >file` out of source order.

**Architecture:** #137 — the builtins' own `write_all` checks miss line-buffered `io::stdout()`'s deferred error; capture the discarded flush `Result` at the `run_builtin_with_redirects` epilogue and emit bash's `write error:` diagnostic with a double-emit guard. #144 — InProcess pipeline stages pre-wire their own `>file` fd into the child's *base* stdout/stderr, so the forked child's source-order re-application of `2>&1` binds to the file instead of the pipe; give InProcess stages a neutral pipe/capture/inherit base (like external stages already have since v293) and let the child's `run_command` apply the full redirect list in source order.

**Tech Stack:** Rust, `crates/huck-engine/src/executor.rs` + `builtins.rs`, bash-diff harnesses under `tests/scripts/`.

**Design spec:** `docs/superpowers/specs/2026-07-15-builtin-write-error-and-inprocess-stage-order-design.md`. Closes [#137](https://github.com/jdstanhope/huck/issues/137), [#144](https://github.com/jdstanhope/huck/issues/144); advances but does NOT close [#147](https://github.com/jdstanhope/huck/issues/147).

## Global Constraints

- **Commit trailer** on every commit, verbatim: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **`cargo fmt --all`** before every commit (CI runs `cargo fmt --all --check`).
- **Build the binary** with `cargo build -p huck` (produces `target/debug/huck`). Release for the sweep: `cargo build --release --locked -p huck`.
- **Never** `cargo test --workspace` (OOMs this box). Engine lib tests: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. Integration binaries: `( ulimit -v 6000000; cargo test -p huck --test <name> --jobs 1 -- --test-threads 1 )`.
- **Diff harnesses** compare huck vs the system bash (5.2.21 on CI) byte-identically. New harnesses go in `tests/scripts/` and must be registered in `tests/scripts/run_diff_checks.sh`.
- **bash message parity:** builtin write error is `<name>: write error: <strerror>` and exit status 1 (verified: `bash -c 'exec >&-; echo end'` → `bash: line 1: echo: write error: Bad file descriptor`, rc 1). Use `crate::bash_io_error(&e)` for the `<strerror>` suffix (strips Rust's ` (os error N)`).
- **Regression gates that MUST stay green** (both binaries): `pipeline_stage_redirect_fail_diff_check.sh` (#145), `redirect_diag_diff_check.sh` (#140/#152), `builtin_stdout_dup_diff_check.sh`, `builtin_pipe_flush_diff_check.sh`, `sigpipe_diff_check.sh`, plus `tools/redirect_audit.sh`, `tools/pipeline_redirect_audit.sh`, and the `fd_torture` sweep case.

---

### Task 1: #137 — detect a builtin's swallowed stdout write error

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (the `run_builtin_with_redirects` epilogue, around line 1531)
- Create: `tests/scripts/builtin_write_error_diff_check.sh`
- Modify: `tests/scripts/run_diff_checks.sh` (register the new harness)

**Interfaces:**
- Consumes: `write_to_fd1: bool` (computed at executor.rs:1371 — true iff the builtin wrote to real fd 1), `outcome: ExecOutcome` (the builtin's result), `resolved.program: String` (the builtin name), `err_writer(err_sink, sink) -> Box<dyn Write>`, `crate::bash_io_error(&io::Error) -> String`, `crate::sh_error_to!`.
- Produces: nothing consumed by later tasks.

**Background (verified):** `builtin_echo`/`builtin_printf` check `out.write_all(...)` but line-buffered `io::stdout()` defers the real `write(2)` to flush, which every site discards (`let _ = io::stdout().flush()`). The authoritative flush is the `run_builtin_with_redirects` epilogue (executor.rs:1531), run while the builtin's redirect scope is still installed. In the `write_to_fd1` branch (executor.rs:1474) the builtin wrote to `io::stdout()`; in the capture branches it wrote to a buffer, so gating on `write_to_fd1` restricts the check to exactly the case where fd 1 was the builtin's target.

- [ ] **Step 1: Write the failing harness**

Create `tests/scripts/builtin_write_error_diff_check.sh`:

```bash
#!/usr/bin/env bash
# v298 (#137): a builtin whose stdout write fails (closed fd, full disk) must
# report `<name>: write error: <strerror>` and exit 1, matching bash. Rust's
# line-buffered io::stdout() defers the write(2) to flush, so the builtins'
# own write_all checks miss it; the run_builtin_with_redirects epilogue flush
# is the authoritative detection site. Compares stdout+stderr+rc byte-identically.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
# Normalise each shell's own prefix (`bash: line N:` / `<huckpath>: line N:` /
# `<huckpath>:`) to `SH:` so only the diagnostic text + rc are compared.
norm() { sed -e 's#^bash: line [0-9]*: #SH: #' -e 's#^bash: #SH: #' \
             -e "s#^$HUCK: line [0-9]*: #SH: #" -e "s#^$HUCK: #SH: #"; }
# The frag may close fd 1 (`exec >&-`), so read the rc via fd 2, never fd 1.
check() {
  local label=$1 frag=$2 b h
  b=$(timeout 10 bash -c "$frag"'; e=$?; echo "rc=$e" >&2' 2>&1 | norm)
  h=$(timeout 10 "$HUCK" -c "$frag"'; e=$?; echo "rc=$e" >&2' 2>&1 | norm)
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"; echo "  bash: $b"; echo "  huck: $h"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

check 'echo-closed'    'exec >&-; echo end'
check 'printf-closed'  'exec >&-; printf "end\n"'
check 'echo-n-closed'  'exec >&-; echo -n end'
check 'echo-redir-dup' 'echo hi >&-'
# fd 1 open -> no write error, plain success (guards against false positives)
check 'echo-ok'        'echo hi'

if [ $FAIL -ne 0 ]; then echo "builtin_write_error_diff_check FAILED" >&2; exit 1; fi
echo "builtin_write_error_diff_check OK"
```

- [ ] **Step 2: Run it to confirm it fails (RED)**

```bash
cargo build -p huck && chmod +x tests/scripts/builtin_write_error_diff_check.sh && tests/scripts/builtin_write_error_diff_check.sh
```
Expected: FAIL on `echo-closed`/`printf-closed`/`echo-n-closed`/`echo-redir-dup` (huck emits nothing, rc 0; bash emits `SH: <name>: write error: Bad file descriptor`, rc 1). `echo-ok` should already PASS.

- [ ] **Step 3: Implement the flush-error check**

In `crates/huck-engine/src/executor.rs`, replace the epilogue flush pair at line 1531-1532:

```rust
    let _ = io::stdout().flush();
    let _ = std::io::Write::flush(&mut std::io::stderr());
```

with:

```rust
    // #137: a builtin's stdout write failure (closed fd, full disk) only
    // surfaces at flush — line-buffered io::stdout() defers the write(2), so the
    // builtins' own write_all checks miss it. Detect it here and emit bash's
    // `<name>: write error: <strerror>` + exit 1. Guards:
    //  - `write_to_fd1`: only the branch where the builtin wrote to real fd 1
    //    (the capture branches write to a buffer, so an io::stdout() flush error
    //    there is unrelated to this builtin).
    //  - double-emit guard `Continue(0)`: don't override a builtin that already
    //    reported a different failure, and don't re-report over a nonzero status.
    let stdout_flush = io::stdout().flush();
    let _ = std::io::Write::flush(&mut std::io::stderr());
    let outcome = if write_to_fd1
        && let Err(e) = &stdout_flush
        && matches!(outcome, ExecOutcome::Continue(0))
    {
        {
            let mut ew = err_writer(err_sink, sink);
            crate::sh_error_to!(
                shell,
                &mut *ew,
                None,
                "{}: write error: {}",
                resolved.program,
                crate::bash_io_error(e)
            );
        }
        ExecOutcome::Continue(1)
    } else {
        outcome
    };
```

(The subsequent `scope.reap_heredoc_writers(); drop(scope); drain_procsubs(...); outcome` lines are unchanged — the shadowed `outcome` flows into the existing `outcome` return at the end of the function.)

- [ ] **Step 4: Format, build, confirm GREEN**

```bash
cargo fmt --all && cargo build -p huck && tests/scripts/builtin_write_error_diff_check.sh
```
Expected: `builtin_write_error_diff_check OK` (all PASS).

- [ ] **Step 5: Confirm no regression in the sibling builtin-output harnesses**

```bash
tests/scripts/builtin_stdout_dup_diff_check.sh && tests/scripts/builtin_pipe_flush_diff_check.sh && tests/scripts/sigpipe_diff_check.sh
```
Expected: all three print their `... OK` line. (These exercise `>&2`/`2>&1` routing, pipe flush, and SIGPIPE — the double-emit guard and `write_to_fd1` gate must not perturb them.)

- [ ] **Step 6: Register the harness in the sweep**

In `tests/scripts/run_diff_checks.sh`, add `builtin_write_error_diff_check.sh` to the list of harnesses it runs (follow the existing pattern for `builtin_pipe_flush_diff_check.sh`).

- [ ] **Step 7: Commit**

```bash
git add crates/huck-engine/src/executor.rs tests/scripts/builtin_write_error_diff_check.sh tests/scripts/run_diff_checks.sh
git commit -m "$(cat <<'EOF'
fix(#137): report a builtin's swallowed stdout write error

Line-buffered io::stdout() defers the write(2) to flush, so builtin_echo/
builtin_printf's own write_all checks never see a closed-fd/full-disk error.
Capture the run_builtin_with_redirects epilogue flush Result and emit bash's
`<name>: write error: <strerror>` with exit 1, gated on write_to_fd1 (the
builtin actually targeted fd 1) and a Continue(0) double-emit guard.

Refs #137.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: #144 — InProcess pipeline stages apply redirects in source order

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (`spawn_pipeline` stage loop: the `explicit_stdout` block ~6347-6415, the `explicit_stderr` block ~6419-6487, the stdout-base `if let Some(cf) = explicit_stdout` branch ~6532-6566, the stderr-base `explicit_stderr` checks ~6659/6661, and the InProcess dup-target resolution block ~6762-6842)
- Create: `tests/scripts/builtin_stage_stderr_diff_check.sh`
- Modify: `tests/scripts/run_diff_checks.sh` (register the new harness)

**Interfaces:**
- Consumes: the stage loop's existing `stdout: ChildFd` / `stderr: ChildFd` base construction, `mode`, `sink`, `err_sink`, `is_last`, `stage_is_external`, `redirect_failed`, `stage_cmd`, `fork_and_run_in_subshell(cmd, shell, child_stdio, pgid_target, fds, stdout_dup_target, stderr_dup_target)`.
- Produces: nothing consumed by later tasks.

**Background (verified empirically):** For an InProcess `Simple(Exec)` stage, `explicit_stdout`/`explicit_stderr` pre-open the stage's own `>file` and wire it into the child's *base* fd 1/2; `stdout_dup_target`/`stderr_dup_target` pre-resolve `2>&1`/`>&$v` and `dup2` them onto fd 1/2 after the base install. The forked child THEN re-applies the full `exec.redirects` in source order via `run_command` (the code comment at executor.rs:6772 already calls this re-application "authoritative"). Because the base fd 1 is already the file, `2>&1` in `printf abc 2>&1 >f | cat` binds stderr to the file instead of the pipe. External stages (v293) and compound stages (which are not `Simple(Exec)`, so never pre-wired) already use a neutral base + child/plan replay and are correct. Probe confirmed: `printf '%d\n' abc 2>&1 >/tmp/f | cat` → bash routes the error to the pipe (cat) with `0` in the file; huck routes the error to the file. `>f 2>&1` already matches. The fix: stop pre-wiring for InProcess stages; the child's existing source-order re-application (which also re-derives `{var}` fds in order, per #140d) becomes the single source of truth — exactly how stdin `<file` redirects and compound stages already behave.

**Note (#147):** removing this pre-wire retires the InProcess-stage slot machinery that #147 targets, but #147's remaining scope (`slots_for_simple_path`, the single-command builtin base, `RedirectSlot` retirement) stays open. Do NOT expand this task into #147.

- [ ] **Step 1: Write the failing harness**

Create `tests/scripts/builtin_stage_stderr_diff_check.sh`:

```bash
#!/usr/bin/env bash
# v298 (#144): an in-process pipeline stage must apply its OWN redirects in
# source order. `printf abc 2>&1 >f | cat` sends the error to the pipe (bash),
# not the file; huck pre-wired the file into the child's stdout base so 2>&1
# bound to the file. Verifies the fd DESTINATION (pipe vs file) for each stage
# type. Compares captured pipe output + file contents + rc + PIPESTATUS.
#
# printf's invalid-number message text diverges orthogonally (huck quotes the
# arg: `abc' vs bash's bare abc); norm() strips backticks/single-quotes so only
# WHERE the bytes land is compared, not the wording.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }
TMPF=/tmp/huck_st144.$$
trap 'rm -f "$TMPF"' EXIT

FAIL=0
norm() { sed -e 's#^bash: line [0-9]*: #SH: #' -e 's#^bash: #SH: #' \
             -e "s#^$HUCK: line [0-9]*: #SH: #" -e "s#^$HUCK: #SH: #" \
             -e 's#`##g' -e "s#'##g"; }
# Capture the pipe/terminal output (stdout+stderr, +rc+PIPESTATUS) AND the file
# contents separately, so a byte moving from pipe->file is visible.
run_one() {
  local sh=$1 frag=$2
  rm -f "$TMPF"
  local out; out=$(timeout 10 "$sh" -c "$frag"'; echo "rc=$? PS=(${PIPESTATUS[@]})"' 2>&1 | norm)
  local file; file=$(cat "$TMPF" 2>/dev/null | norm)
  printf 'OUT{%s}FILE{%s}' "$out" "$file"
}
check() {
  local label=$1 frag=$2 b h
  b=$(run_one bash "$frag"); h=$(run_one "$HUCK" "$frag")
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"; echo "  bash: $b"; echo "  huck: $h"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# --- core #144: interleaved 2>&1 >f (error must reach the pipe) ---
check 'builtin-interleave' "printf '%d\n' abc 2>&1 >$TMPF | cat"
# --- reverse order (already matched: error+0 both to file) ---
check 'builtin-fileorder'  "printf '%d\n' abc >$TMPF 2>&1 | cat"
# --- no error, plain data to file, empty pipe ---
check 'builtin-nodata'     "echo hi 2>&1 >$TMPF | cat"
check 'builtin-fileonly'   "echo hi >$TMPF 2>&1 | cat"
# --- per InProcess stage type: each must re-apply its own redirects in order ---
check 'function-stage'     "f(){ printf '%d\n' abc; }; f 2>&1 >$TMPF | cat"
check 'compound-stage'     "{ printf '%d\n' abc; } 2>&1 >$TMPF | cat"   # regression guard (already correct)
check 'assign-stage'       "x=1 2>&1 >$TMPF | cat"
# --- real open failure in a builtin stage: message parity (norm-compared) + PS ---
check 'open-fail'          "printf hi >/no/such/dir/f 2>&1 | cat"
# --- #140d {var} source-order visibility must survive the base change ---
check 'var-true'           "true {v}>$TMPF 2>&\$v | cat"
check 'var-echo'           "echo hi {v}>$TMPF 2>&\$v | cat"

if [ $FAIL -ne 0 ]; then echo "builtin_stage_stderr_diff_check FAILED" >&2; exit 1; fi
echo "builtin_stage_stderr_diff_check OK"
```

- [ ] **Step 2: Run it to confirm it fails (RED)**

```bash
cargo build -p huck && chmod +x tests/scripts/builtin_stage_stderr_diff_check.sh && tests/scripts/builtin_stage_stderr_diff_check.sh
```
Expected: FAIL on `builtin-interleave` and `function-stage` (huck routes the error to the file; bash to the pipe). `builtin-fileorder`, `builtin-nodata`, `builtin-fileonly`, `compound-stage`, `assign-stage`, `open-fail`, `var-true`, `var-echo` should already PASS.

- [ ] **Step 3: Neutralize the InProcess stdout base pre-wire**

In `crates/huck-engine/src/executor.rs`, the `explicit_stdout` binding (currently `let explicit_stdout: Option<ChildFd> = if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd && !stage_is_external && !redirect_failed { … } else { None };`, roughly lines 6347-6415) is now always `None` (external stages already got `None`; InProcess stages must too). Replace the entire binding with:

```rust
        // #144: InProcess stages get a NEUTRAL stdout base (pipe/capture/inherit);
        // the forked child re-applies the stage's `>file` in source order via
        // run_command, so `2>&1 >f` binds stderr to the pipe like bash. (External
        // stages replay their ChildRedirPlan; both now share the neutral-base model.)
        let explicit_stdout: Option<ChildFd> = None;
```

Then in the stdout-base construction (currently `let stdout: ChildFd = if let Some(cf) = explicit_stdout { … } else if !is_last { … } else { … };`, roughly lines 6532-6640), delete the now-dead `if let Some(cf) = explicit_stdout { … }` arm and its trailing non-last inter-stage-pipe bookkeeping, so the construction begins directly at the `if !is_last {` (create inter-stage pipe) arm. The `else if !is_last` becomes `if !is_last`.

- [ ] **Step 4: Neutralize the InProcess stderr base pre-wire**

Replace the `explicit_stderr` binding (roughly lines 6419-6487) with:

```rust
        // #144: neutral stderr base too; the child re-applies `2>file`/`2>&n` in
        // source order.
        let explicit_stderr: Option<ChildFd> = None;
```

In the stderr-base construction (roughly lines 6655-6733), delete the two `explicit_stderr` uses: the `SpawnMode::Background => explicit_stderr.unwrap_or(ChildFd::Inherit),` arm becomes `SpawnMode::Background => ChildFd::Inherit,`, and the `SpawnMode::Foreground => { if let Some(cf) = explicit_stderr { cf } else { <match err_sink …> } }` becomes `SpawnMode::Foreground => { <match err_sink …> }` (drop the `if let Some(cf) = explicit_stderr` wrapper, keep the `match err_sink { Terminal … Merged … Capture … }` body).

- [ ] **Step 5: Neutralize the InProcess dup-target pre-resolution**

Replace the whole `let (stdout_dup_target, stderr_dup_target) = if !stage_is_external && !redirect_failed && let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd { … } else { (None, None) };` block (roughly lines 6762-6842, including the `{var}` snapshot/restore and the `slot_stdout`/`slot_stderr` Dup resolution) with:

```rust
        // #144/#140d: no dup-target pre-resolution. The forked child re-applies
        // the FULL redirect list in source order — a `{v}>f` earlier in the stage
        // assigns `$v` before a later `2>&$v` reads it, so the child derives the
        // correct fd natively (no parent-side snapshot/restore, no spurious
        // `$v: ambiguous redirect`). Matches how stdin `<file` redirects and
        // compound stages already behave. (#147 will drop the now-unused
        // dup_target parameters from fork_and_run_in_subshell.)
        let (stdout_dup_target, stderr_dup_target): (Option<RawFd>, Option<RawFd>) = (None, None);
```

(The `fork_and_run_in_subshell(..., stdout_dup_target, stderr_dup_target)` call at ~6884-6892 is unchanged; both args are now `None`, so the child's post-base `dup2` blocks at ~6937-6942 become runtime no-ops. Leave the parameters in place — #147 removes them. If the compiler flags the explicit `(Option<RawFd>, Option<RawFd>)` annotation as needing a different concrete type than the params expect, match the parameter type of `fork_and_run_in_subshell`.)

- [ ] **Step 6: Format, build, confirm the #144 harness is GREEN**

```bash
cargo fmt --all && cargo build -p huck && tests/scripts/builtin_stage_stderr_diff_check.sh
```
Expected: `builtin_stage_stderr_diff_check OK` (all PASS, including `builtin-interleave` and `function-stage` now).

- [ ] **Step 7: Confirm the #145 and #140d gates stay GREEN**

```bash
tests/scripts/pipeline_stage_redirect_fail_diff_check.sh && tests/scripts/redirect_diag_diff_check.sh
```
Expected: both print `... OK`. These prove that moving InProcess redirect-failure detection into the child (open failures, ambiguous-redirect expansion failures) still yields per-stage failure with matching rc/PIPESTATUS/message, and that `{var}` source-order visibility is preserved. **If either regresses, STOP and investigate — do not paper over it.** The likely culprit is a `line N:` presence mismatch or an rc difference in a child-side redirect failure; report it in the task report with the exact failing case.

- [ ] **Step 8: Run the redirect audits (differential fd gate)**

```bash
( ulimit -v 6000000; tools/redirect_audit.sh ) ; ( ulimit -v 6000000; tools/pipeline_redirect_audit.sh )
```
Expected: `pipeline_redirect_audit.sh` stays at 0 DIVERGE. `redirect_audit.sh` should stay at its prior DIVERGE count or drop (the `2>&1 >f`-class InProcess cases move to agreeing). Record the before/after DIVERGE counts in the task report.

- [ ] **Step 9: Run the engine lib tests (fd/pipeline coverage)**

```bash
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -5
```
Expected: all pass (~1806).

- [ ] **Step 10: Register the harness in the sweep**

In `tests/scripts/run_diff_checks.sh`, add `builtin_stage_stderr_diff_check.sh` to the harness list (next to `pipeline_stage_redirect_fail_diff_check.sh`).

- [ ] **Step 11: Commit**

```bash
git add crates/huck-engine/src/executor.rs tests/scripts/builtin_stage_stderr_diff_check.sh tests/scripts/run_diff_checks.sh
git commit -m "$(cat <<'EOF'
fix(#144): apply InProcess pipeline-stage redirects in source order

An InProcess Simple(Exec) stage pre-wired its own `>file` into the child's
base fd 1/2 (explicit_stdout/explicit_stderr) and pre-resolved 2>&1/>&$v dup
targets, then the forked child re-applied the full redirect list on top. With
the base already the file, `printf abc 2>&1 >f | cat` bound stderr to the file
instead of the pipe. Give InProcess stages a neutral pipe/capture/inherit base
(as external stages have since v293) and let the child's run_command apply the
stage's redirects in source order — the single authoritative pass, which also
re-derives {var} fds in order (#140d). Retires the InProcess pre-wire; advances
but does not close #147.

Refs #144, #147.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: whole-branch verification sweep

**Files:** none (verification only).

- [ ] **Step 1: Build both binaries**

```bash
cargo build --locked --bin huck && cargo build --release --locked --bin huck
```

- [ ] **Step 2: Run the full diff sweep on both binaries**

```bash
( ulimit -v 1500000; timeout 1100 tests/scripts/run_diff_checks.sh )
```
Expected: 0 failed, and the two new harnesses (`builtin_write_error_diff_check.sh`, `builtin_stage_stderr_diff_check.sh`) appear in the run and pass. (`run_diff_checks.sh` re-execs with the SIGPIPE default; confirm it reports its total green.)

- [ ] **Step 3: Run the relevant `-p huck` integration binaries single-threaded**

```bash
for t in sigpipe_integration; do ( ulimit -v 6000000; cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 2>&1 | tail -3 ); done
```
Expected: pass. (These run locally too — CI is not special. If `git grep -l` finds other `tests/*.rs` binaries touching pipeline/redirect/builtin fd behavior, run each the same way.)

- [ ] **Step 4: Confirm the tree is formatted**

```bash
cargo fmt --all --check
```
Expected: no output (clean).

---

## Self-Review

- **Spec coverage:** §1 (#137 flush-error + double-emit guard) → Task 1. §2 (#144 minimal source-order fix: neutral InProcess base + child re-application, #147 noted-not-closed) → Task 2. Testing plan (two new harnesses + redirect_audit before/after + existing gates green) → Tasks 1-3. Error-handling section (double-emit guard; #144 fd order only, `{var}` preserved) → Task 1 Step 3 guard + Task 2 Steps 5,7. All covered.
- **Placeholder scan:** every code step contains the actual code or the exact anchored transformation; the two harnesses are complete; commands have expected output. No TBD/TODO.
- **Type consistency:** `write_to_fd1: bool`, `outcome: ExecOutcome`, `resolved.program: String`, `err_writer(err_sink, sink)`, `bash_io_error(&io::Error) -> String`, `sh_error_to!` — all used as defined in executor.rs. `explicit_stdout`/`explicit_stderr: Option<ChildFd>` set to `None`; `stdout_dup_target`/`stderr_dup_target` set to `None` and passed to `fork_and_run_in_subshell` unchanged.
- **Risk note carried into execution:** Task 2 moves InProcess redirect-failure detection from parent (`spawn_failed_stage`) to child (`apply_redirects`); Steps 7-9 are hard gates precisely because the observable failure semantics (#145) and `{var}` visibility (#140d) must not shift. A regression there is a blocker, not something to normalize away.
