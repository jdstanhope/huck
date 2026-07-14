# Pipeline stage redirect failure fails only that stage — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a pipeline stage's own redirect setup fails, report the error, fail just that stage (exit 1, wired into the pipe topology), and run the remaining stages — instead of aborting the whole pipeline — matching bash.

**Architecture:** In `spawn_pipeline`'s stage loop, replace the four "stage redirect setup failed → `return Err(bail_teardown_pipeline(...))`" branches with a per-iteration `redirect_failed` flag; at the spawn point, when set, fork a `spawn_failed_stage` child wired to the stage's inter-stage pipe ends that immediately `_exit(1)`. The failed stage flows through the existing pid → `wait_pipeline_raw` → `$PIPESTATUS` machinery as an ordinary rc-1 child, so downstream reads EOF, upstream SIGPIPEs (141), and the pipeline rc is the last stage's — with no new stage variant.

**Tech Stack:** Rust (`crates/huck-engine/src/executor.rs`), bash-diff harnesses (`tests/scripts/*_diff_check.sh`, `tools/bg_pipeline_redirect_audit*.sh`).

## Global Constraints

- **Reference spec:** `docs/superpowers/specs/2026-07-14-pipeline-stage-redirect-fail-design.md`. If the code contradicts the spec, STOP and report.
- **Scope:** only a stage's OWN redirect setup failing continues (builtin slot opens: stdin/stdout/stderr; external `build_child_redir_plan`). Infrastructure failures (`make_pipe`, `make_orphan_pipe_for_eof_reader`, and `fork()` inside `spawn_failed_stage`) STILL abort the whole pipeline. Spawn/exec failures are #78 (out of scope). Compound stages already work (untouched).
- **Failed-stage exit code is `1`**; the redirect error is printed ONCE by the existing setup code (do not re-print in the dummy).
- **Behavior-preserving on the success path:** no passing pipeline behavior changes. `git diff -w` on the success-path loop body should be unchanged.
- **Test discipline (this box OOMs on `cargo test --workspace` — NEVER run that):** build with `cargo build -p huck`; engine lib `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` (~1806); integration `( ulimit -v 6000000; cargo test -p huck --test <name> --jobs 1 -- --test-threads 1 )`; sweep `( ulimit -v 1500000; timeout 1200 tests/scripts/run_diff_checks.sh )`.
- **Gates:** the NEW `tests/scripts/pipeline_stage_redirect_fail_diff_check.sh` goes byte-identical to bash (RED before the fix, GREEN after); `tools/pipeline_redirect_audit.sh` stays `0 DIVERGE`; `tools/bg_pipeline_redirect_audit.sh` stays `0 DIVERGE` (with the new bg cases added, still 0); `tools/redirect_audit.sh` stays `16 DIVERGE`; `fd_torture` 44; engine lib green; `named_fd` 7/7; `bg_pipeline_line_number` 1/1; full sweep `0 failed` BOTH binaries (the new diff_check raises the passing count from 188 to 189).
- **Commit trailer, verbatim:** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. `cargo fmt --all` before each code commit.
- **Anti-hang:** every harness case runs under `timeout`; a mis-wired pipe end that never EOFs is a divergence, never a stall.

## Current-code landmarks (main; verify by reading)

`spawn_pipeline` executor.rs:5668; stage loop phases — build stdin `~5975`; determine `explicit_stdout` `~6195`; determine `explicit_stderr` `~6281`; build external `ChildRedirPlan` `~6379` (its `Err(_) => return Err(bail_teardown_pipeline(...))` arm `~6394`); build stdout / inter-stage pipe `~6413`; build stderr `~6524`; classify + build `child_stdio` + spawn `~6618`; `let spawn_result = match kind {…}` `~6725`; `let pid = match spawn_result {…}` + post-spawn tracking `~6750`. `fork_and_run_in_subshell` `~7691` (model for the new helper's fork/setpgid/`into_raw` handling). `ChildStdio::new(stdin, stdout, stderr)` `child_fd.rs:125`; `ChildFd::into_raw() -> Option<RawFd>`. `PipelineStage::Forked(i32)` `~5426`.

---

### Task 1: RED gate — the failing-stage differential harness

**Files:**
- Create: `tests/scripts/pipeline_stage_redirect_fail_diff_check.sh`
- Modify: `tools/bg_pipeline_redirect_audit_cases.sh` (add 2 background failing-redirect cases)

**Interfaces:**
- Produces: a `_diff_check.sh` (auto-included in `run_diff_checks.sh`) that runs foreground fragments through bash and huck, comparing **stdout+stderr+rc+`$PIPESTATUS`** byte-identically (shell-name/line prefix normalized). RED on current code.

- [ ] **Step 1: Write the harness.** Create `tests/scripts/pipeline_stage_redirect_fail_diff_check.sh`:
```bash
#!/usr/bin/env bash
# v296 (#145): a pipeline STAGE's own redirect-setup failure must fail only that
# stage (report the error, exit 1, wired into the pipe topology) and let the rest
# of the pipeline run — matching bash. Compares stdout+stderr+rc+PIPESTATUS.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
# Run a fragment, appending an rc+PIPESTATUS probe; normalise the shell's own
# error prefix (`bash: line N:` / `<huckpath>: line N:` / `<huckpath>:`) to a
# uniform `SH:` so only the libc message text + rc + PIPESTATUS are compared.
norm() { sed -e 's#^bash: line [0-9]*: #SH: #' -e "s#^$HUCK: line [0-9]*: #SH: #" -e "s#^$HUCK: #SH: #"; }
check() {
  local label=$1 frag=$2 b h
  b=$(timeout 10 bash -c "$frag"'; echo "rc=$? PIPESTATUS=(${PIPESTATUS[@]})"' 2>&1 | norm)
  h=$(timeout 10 "$HUCK" -c "$frag"'; echo "rc=$? PIPESTATUS=(${PIPESTATUS[@]})"' 2>&1 | norm)
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"; echo "  bash: $b"; echo "  huck: $h"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# --- external stage fails at each position ---
check 'ext-middle'   'echo A | cat </no/such/file | cat'
check 'ext-first'    'cat </no/such/file | wc -c'
check 'ext-last'     'echo A | cat </no/such/file'
# --- builtin stage fails at each position ---
check 'blt-middle'   'echo A | read x </no/such/file | cat'
check 'blt-first'    'read x </no/such/file | wc -c'
check 'blt-last'     'echo A | read x </no/such/file'
# --- compound stage fails (regression guard: already correct) ---
check 'cmp-middle'   'echo A | { cat; } </no/such/file | cat'
# --- two stages fail ---
check 'two-fail'     'cat </no/a | cat </no/b | wc -c'
# --- upstream floods a dead reader -> SIGPIPE 141 (yes never exits so its 141
#     is deterministic; a `head -N` middle stage would race 0-vs-141 in bash) ---
check 'sigpipe-up'   'yes | cat </no/such/file | wc -l'
# --- failed stage redirects stdin AWAY from the pipe; upstream still SIGPIPEs ---
check 'stdin-away'   'yes | read x </no/such/file | cat'
# --- bad-fd source-order (message fixed in v293; only rc/continue diverged) ---
check 'badfd-simple' 'cat <&7 | cat'
check 'badfd-heredoc' "/bin/cat <&3 3<<<'HS' | cat"

if [ $FAIL -ne 0 ]; then echo "pipeline_stage_redirect_fail_diff_check FAILED" >&2; exit 1; fi
echo "pipeline_stage_redirect_fail_diff_check OK"
```

- [ ] **Step 2: Confirm the harness is RED and captures the real divergence.**
```bash
cargo build -p huck
bash tests/scripts/pipeline_stage_redirect_fail_diff_check.sh
```
Expected: multiple `FAIL [ext-*]`, `FAIL [blt-*]`, `FAIL [two-fail]`, `FAIL [sigpipe-up]`, `FAIL [stdin-away]`, `FAIL [badfd-*]` (huck aborts: `rc=1 PIPESTATUS=()`), and `PASS [cmp-middle]` (compound already works). Overall exit 1. Eyeball a couple of FAIL bodies to confirm huck shows `rc=1 PIPESTATUS=()` while bash shows the continuing `PIPESTATUS=(… 1 …)`.

- [ ] **Step 3: Add background failing-redirect cases to the bg audit.** In `tools/bg_pipeline_redirect_audit_cases.sh`, inside `emit_bg_cases()`, append two cases (real tabs between the three fields), mirroring the file's existing format (`label<TAB>resultfiles<TAB>fragment`, bare `pipeline &`):
```bash
  # A stage whose redirect fails must not abort the bg pipeline: the consumer
  # still runs and writes its marker, matching bash. Two constraints:
  #  - the failing redirect must be on a DIRECT stage (not wrapped in `sh -c`,
  #    which fails the open() inside a nested shell, bypassing huck's own path);
  #  - the consumer must write a MARKER (`echo DONE`), not just pass EOF through,
  #    because the audit compares file CONTENT and an empty `po` (bash continue)
  #    vs a missing `po` (huck abort) both read as the empty string — the marker
  #    makes the difference observable (`DONE` vs empty).
  printf '%s\t%s\t%s\n' "fail-continue"  "po" "cat </no/such/file | { cat; echo DONE; } >po &"
  printf '%s\t%s\t%s\n' "fail-mid"       "po" "echo A | cat </no/such/file | { cat; echo DONE; } >po &"
```

- [ ] **Step 4: Confirm the bg audit now shows the new cases RED (huck aborts → downstream file differs from bash).**
```bash
HUCK="$(pwd)/target/debug/huck" bash tools/bg_pipeline_redirect_audit.sh
```
Expected: `AUDIT: 12 cases, 10 agree, 2 DIVERGE` (the two new `fail-*` cases DIVERGE on current code; the original 10 still agree). If the new cases unexpectedly AGREE, investigate (bg may already continue for some shapes) and report — do not proceed assuming RED.

- [ ] **Step 5: Confirm existing harnesses are otherwise unperturbed.**
```bash
HUCK="$(pwd)/target/debug/huck" bash tools/pipeline_redirect_audit.sh | grep '^AUDIT'   # 15/15 0 DIVERGE
HUCK="$(pwd)/target/debug/huck" bash tools/redirect_audit.sh | grep '^AUDIT'            # 157 … 16 DIVERGE
```

- [ ] **Step 6: Commit (RED gate).**
```bash
chmod +x tests/scripts/pipeline_stage_redirect_fail_diff_check.sh
git add tests/scripts/pipeline_stage_redirect_fail_diff_check.sh tools/bg_pipeline_redirect_audit_cases.sh
git commit -m "$(cat <<'EOF'
v296 T1: RED gate for per-stage pipeline redirect failure (#145)

New pipeline_stage_redirect_fail_diff_check.sh (fg matrix: external/builtin/
compound x position, two-fail, SIGPIPE-141 upstream, stdin-redirected-away,
bad-fd) comparing stdout+stderr+rc+PIPESTATUS byte-identically to bash — RED
(huck aborts the pipeline). Plus two bg failing-redirect cases in the bg audit.
The fix (T2) turns both green.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: The fix — `spawn_failed_stage` + the `redirect_failed` restructure

**Files:**
- Modify: `crates/huck-engine/src/executor.rs`

**Interfaces:**
- Produces: `fn spawn_failed_stage(shell: &mut Shell, stdio: ChildStdio, pgid_target: i32, parent_fds_to_close: &[RawFd]) -> Result<i32, io::Error>`.
- Consumes: `ChildStdio`/`ChildFd::into_raw`, `flush_stdout`, `bail_teardown_pipeline`, `PipelineStage::Forked`, the existing stage-loop locals (`redirect_failed` is new; `child_stdio`, `pgid_target`, `fds_to_close_in_child`, `stdin`, `explicit_stdout`, `explicit_stderr`, `snap`, `stages`, `parent_held`, `procsub_base`, `first_pid`).

- [ ] **Step 1: Add `spawn_failed_stage`.** Place it next to `fork_and_run_in_subshell` (`~7691`). Exact code:
```rust
/// Fork a "failed pipeline stage": a child that inherits this stage's stdio
/// (its inter-stage pipe ends), joins the job's process group, closes every
/// OTHER parent-held pipe fd, and immediately `_exit(1)`. It runs no command and
/// prints nothing — the redirect error was already reported by the parent. Its
/// exit closes the pipe ends it holds, giving downstream EOF and upstream
/// SIGPIPE, reproducing bash's per-stage redirect failure (#145). Unlike
/// `fork_and_run_in_subshell` it does NOT dup2 stdio to 0/1/2: the child never
/// reads or writes, it only has to hold its pipe ends open until exit.
fn spawn_failed_stage(
    shell: &mut Shell,
    stdio: ChildStdio,
    pgid_target: i32,
    parent_fds_to_close: &[RawFd],
) -> Result<i32, io::Error> {
    let _ = shell; // reserved for symmetry with the other spawners
    flush_stdout();
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        unsafe {
            if pgid_target >= 0 {
                libc::setpgid(0, pgid_target);
            }
            let ChildStdio {
                stdin,
                stdout,
                stderr,
            } = stdio;
            // Convert to raw so no OwnedFd Drop runs in the forked child; keep
            // the fds OPEN (they close on _exit -> the pipe topology reacts).
            let own: [RawFd; 3] = [
                stdin.into_raw().unwrap_or(-1),
                stdout.into_raw().unwrap_or(-1),
                stderr.into_raw().unwrap_or(-1),
            ];
            for &fd in parent_fds_to_close {
                if fd != own[0] && fd != own[1] && fd != own[2] {
                    libc::close(fd);
                }
            }
            libc::_exit(1);
        }
    }
    if pgid_target >= 0 {
        unsafe {
            libc::setpgid(pid, pgid_target);
        }
    }
    Ok(pid)
}
```
(If `ChildFd::into_raw` is named differently, use the same method `fork_and_run_in_subshell` uses to turn a `ChildFd` into an `Option<RawFd>` without closing — read `~7730` to confirm.)

- [ ] **Step 2: Introduce the `redirect_failed` flag.** At the top of the per-stage iteration (right after `stage_is_external` is computed, `~5973`), add:
```rust
        // #145: a stage's OWN redirect setup failing must fail only this stage,
        // not the whole pipeline. On such a failure we print (already done), set
        // this flag, drop the partial fd, and fall through to spawn a wired
        // exit-1 child at the spawn point (spawn_failed_stage) instead of the
        // real command. Infrastructure failures (make_pipe) still bail.
        let mut redirect_failed = false;
```

- [ ] **Step 3: Rewire the external `build_child_redir_plan` failure (phase 4, `~6394`).** Change its `Err` arm from bailing to setting the flag:
```rust
                    Err(_) => {
                        // Error already reported by build_child_redir_plan.
                        redirect_failed = true;
                        None
                    }
```
(The whole `external_plan` expression already yields `Option<ChildRedirPlan>`; `None` here means the real spawn won't get a plan — and it won't be reached, because the spawn branches on `redirect_failed`.)

- [ ] **Step 4: Rewire the three builtin slot-open failures (phases 1–3).** Read the stdin build (`~5975`), `explicit_stdout` (`~6195`), and `explicit_stderr` (`~6281`) blocks. Each has an error arm that currently does `restore_inline_assignments(snap, shell); return Err(bail_teardown_pipeline(...));` after emitting the open error. In EACH:
  - Replace the `return Err(bail_teardown_pipeline(...))` with `redirect_failed = true;` and yield the appropriate fall-through value for that block's `let`:
    - **stdin block** (`let stdin: ChildFd = …`): on the slot-open failure, after the error is emitted, the `Read` path has already `take()`n+closed `prev_pipe_read` (so upstream will SIGPIPE on its own); set `redirect_failed = true` and evaluate the block to `ChildFd::Inherit` (a valid initialiser).
    - **`explicit_stdout` block** (`let explicit_stdout: Option<ChildFd> = …`): set `redirect_failed = true` and evaluate to `None` (so phase 5 builds the inter-stage pipe for the dummy's stdout).
    - **`explicit_stderr` block** (`let explicit_stderr: Option<ChildFd> = …`): set `redirect_failed = true` and evaluate to `None` (dummy stderr → `Inherit`).
  - Do NOT change `restore_inline_assignments` handling here — leave `snap` restoration to the existing post-spawn path (`~6747`), which runs for every stage including the failed one.
  - Guard the `explicit_stdout`, `explicit_stderr`, and `external_plan` builds so a stage that already failed an earlier redirect does not open a later one: wrap each with `if !redirect_failed { … } else { None }` (for `explicit_stdout`/`explicit_stderr`) / keep `external_plan` as `if stage_is_external && !redirect_failed { … } else { None }`. (In practice builtin fails only in phases 1–3 and external only in phase 4, so at most one fires — the guards make it uniform.)

- [ ] **Step 5: Branch the spawn on `redirect_failed` (phase 7).** The `child_stdio` value (`ChildStdio::new(stdin, stdout, stderr)`) is built in the classify+spawn section (`~6618`) before `let spawn_result = match kind {…}` (`~6725`). Restructure so the real spawn is skipped when `redirect_failed`:
```rust
        let pid = if redirect_failed {
            // The stage's own redirect failed; fork a wired exit-1 child so the
            // pipeline continues (downstream EOF, upstream SIGPIPE, PIPESTATUS=1).
            match spawn_failed_stage(shell, child_stdio, pgid_target, &fds_to_close_in_child) {
                Ok(pid) => pid,
                Err(_) => {
                    // fork() failed => genuine infrastructure failure: abort.
                    restore_inline_assignments(snap, shell);
                    return Err(bail_teardown_pipeline(
                        mode, shell, procsub_base, first_pid, &stages, &mut parent_held,
                    ));
                }
            }
        } else {
            let spawn_result = match kind {
                StageKind::External(simple) => spawn_external_with_fds(/* existing args */),
                StageKind::InProcess(cmd) => fork_and_run_in_subshell(/* existing args */),
            };
            match spawn_result {
                Ok(pid) => pid,
                Err(e) => { /* the EXISTING spawn-error handling, verbatim */ }
            }
        };
        // ... existing post-spawn tracking (first_pid / live-pid registry /
        //     stages.push(Forked(pid)) / parent close bookkeeping / prev_pipe_read
        //     update / restore_inline_assignments) runs UNCHANGED for `pid`.
```
Move the existing `let spawn_result = match kind {…}` and its `let pid = match spawn_result {…}` INTO the `else` arm; everything after (post-spawn tracking) stays and now consumes the unified `pid`. Ensure `child_stdio` is consumed by exactly one branch (it is — `spawn_failed_stage` takes it by value in the `if`, the spawners take it in the `else`).

- [ ] **Step 6: Build — warning-clean.**
```bash
cargo build -p huck 2>&1 | tail -20
```
Expected: warning-clean. (Stale LSP diagnostics mid-edit are false — trust only a real `cargo build`.)

- [ ] **Step 7: Drive the new harness + bg audit GREEN.**
```bash
bash tests/scripts/pipeline_stage_redirect_fail_diff_check.sh
HUCK="$(pwd)/target/debug/huck" bash tools/bg_pipeline_redirect_audit.sh | grep '^AUDIT'
```
Expected: `pipeline_stage_redirect_fail_diff_check OK` (all PASS); bg audit `AUDIT: 12 cases, 12 agree, 0 DIVERGE`. If any case still diverges, STOP and report the bash-vs-huck diff for it — do NOT edit the harness to force green.

- [ ] **Step 8: Confirm no regression + full verification.**
```bash
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
HUCK="$(pwd)/target/debug/huck" bash tools/pipeline_redirect_audit.sh | grep '^AUDIT'   # 0 DIVERGE
HUCK="$(pwd)/target/debug/huck" bash tools/redirect_audit.sh | grep '^AUDIT'            # 16 DIVERGE
tests/scripts/fd_torture_diff_check.sh | tail -1                                        # 44
( ulimit -v 6000000; cargo test -p huck --test named_fd_integration --jobs 1 -- --test-threads 1 2>&1 | tail -3 )
( ulimit -v 6000000; cargo test -p huck --test bg_pipeline_line_number_integration --jobs 1 -- --test-threads 1 2>&1 | tail -3 )
cargo build --release --locked -p huck 2>&1 | tail -1
( ulimit -v 1500000; timeout 1200 tests/scripts/run_diff_checks.sh 2>&1 | tail -3 )
git diff -w HEAD -- crates/huck-engine/src/executor.rs >/dev/null   # (informational: success-path loop body unchanged)
```
Expected: lib ~1806; pipeline audit 0 DIVERGE; single-cmd 16; fd_torture 44; named_fd 7/7; bg_line_number 1/1; sweep `189 passed, 0 failed` (the new diff_check joins the 188).

- [ ] **Step 9: Commit.**
```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v296 T2: a pipeline stage redirect failure fails only that stage (#145)

Replace the four stage-redirect-setup bail sites in spawn_pipeline (builtin
slot stdin/stdout/stderr opens + external build_child_redir_plan) with a
redirect_failed flag; at the spawn point, fork a wired exit-1 child
(spawn_failed_stage) instead of aborting, so downstream reads EOF, upstream
SIGPIPEs (141), PIPESTATUS records 1, and the pipeline rc is the last stage's.
Infrastructure (make_pipe) failures still bail; spawn/exec failures = #78;
compound stages unchanged. New diff_check + bg audit cases green; all existing
audits + sweep unchanged.

Closes #145

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Notes for the whole-branch review

- **Acceptance:** `pipeline_stage_redirect_fail_diff_check.sh` GREEN (byte-identical to bash on the whole matrix, incl. the SIGPIPE-141 and stdin-redirected-away cases); bg audit 12/12; `pipeline_redirect_audit.sh` 15/15 and `redirect_audit.sh` 16 unchanged; sweep 189/0 both binaries.
- **Correctness crux — the failed stage's pipe wiring:** confirm the dummy holds exactly its two inter-stage pipe ends and closes all others (`fds_to_close_in_child`), so downstream EOFs and upstream SIGPIPEs; a mis-wire shows as a `timeout` hang or a wrong `$PIPESTATUS`.
- **Scope containment:** only stage-OWN-redirect failures continue; `make_pipe`/`make_orphan_pipe`/`fork()` failures still `bail_teardown_pipeline`; compound + single-command + success paths unchanged (`git diff -w`); no new `PipelineStage` variant or `wait_pipeline_raw` change.
- **Ordering:** the external `build_child_redir_plan` is still built before the inter-stage pipe (v293 deadlock); on failure phase 5 builds the pipe for the dummy.
