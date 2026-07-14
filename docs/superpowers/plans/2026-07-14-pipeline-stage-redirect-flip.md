# Pipeline-stage redirect flip (P3b, foreground external stages) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route `run_multi_stage` (foreground pipeline) **external** stages' redirects through v292's ordered `build_child_redir_plan` instead of the last-wins `RedirectSlot` fast-path, fixing the foreground half of #50 (stage source-order + fd>2 heredoc, incl. a hang) and #69 (`line N:`).

**Architecture:** A pipeline stage's fds are two layers: a structural pipe/capture **base** (`ChildStdio`) and the stage's explicit **redirects**. Today the explicit redirects are split across last-wins 0/1/2 slots + `build_child_extra_ops` (fd>2), which loses source order and drops fd>2 heredocs. This plan makes external stages build a pipe/capture-only base and replay the stage's *full ordered* `ChildRedirPlan` on top of it in the child — the identical mechanism single external commands already use (`run_subprocess`). InProcess stages and the background pipeline function are untouched.

**Tech Stack:** Rust, `std::process::Command` + `pre_exec`, libc. Crate: `huck-engine`. Plus POSIX shell for the differential audit harness.

**Spec:** `docs/superpowers/specs/2026-07-14-pipeline-stage-redirect-flip-design.md`. **Issues:** #50 (foreground half), #69 (foreground half). **Design basis:** the fd-plumbing review `docs/superpowers/reviews/2026-07-13-engine-fd-plumbing-review.md` (Phase 3b).

## Global Constraints

- **The ONLY intended behavior change is fixing the foreground external-stage half of #50 + #69.** Everything else — single commands, InProcess stages (compound AND builtin), background pipelines — must stay byte-identical.
- **Acceptance gate — a NEW differential audit.** `tools/pipeline_redirect_audit.sh` (`HUCK=<debug bin> bash tools/pipeline_redirect_audit.sh`) runs pipeline-stage constructs through bash 5.2.21 and huck and asserts **bash == huck** on every case. Task 1 builds it (RED on current code); Task 3 makes it GREEN (0 divergences). Every case runs under `timeout` — a hang is a failure.
- **The single-command audit must not move:** `HUCK=<bin> bash tools/redirect_audit.sh` stays at **16 DIVERGE** through every task.
- **Do NOT fix, and do NOT regress:** #144 (builtin-stage stderr routing — H7 sink layer), #78 (spawn-failure message), #79 (`-c`-mode line-0). Those stay exactly as they are.
- **Do NOT touch:** `run_background_sequence`'s slot code (v294), `slots_for_simple_path`/`RedirectSlot`/`build_child_extra_ops` (retained for background), `fork_and_run_in_subshell` (InProcess path), the software-sink layer.
- **Test/build discipline (this box OOMs on `cargo test --workspace`):** build `cargo build -p huck` / `cargo build --release -p huck`; lib `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` (~1806); integration `( ulimit -v 6000000; cargo test -p huck --test named_fd_integration --jobs 1 -- --test-threads 1 )` (7); sweep `( ulimit -v 1500000; timeout 1200 tests/scripts/run_diff_checks.sh )`.
- **Commit trailer, verbatim, last line of every commit:** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Formatting:** `cargo fmt --all` before every commit.

---

### Task 1: The pipeline-stage differential audit (the gate)

Build `tools/pipeline_redirect_audit.sh` + `tools/pipeline_redirect_audit_cases.sh`, modeled on the existing `tools/redirect_audit.sh` family. On the CURRENT code it must FAIL (show the #50 divergences) — that is the RED state Task 3 turns GREEN.

**Files:** Create `tools/pipeline_redirect_audit.sh`, `tools/pipeline_redirect_audit_cases.sh`.

**Interfaces:**
- Produces: an executable `tools/pipeline_redirect_audit.sh` that reads `HUCK` (path to the huck binary, default `./target/debug/huck`), runs each case in a fresh temp dir through both `bash` and `$HUCK`, and prints `AUDIT: <n> cases, <k> agree, <d> DIVERGE` + one `DIVERGE: <label>` line per divergence. Exit 0 if 0 diverge, else 1.

- [ ] **Step 1: Study the existing harness** to mirror its structure exactly (runner, normalization, temp-dir setup, `tri`-style helpers):

Run: `cat tools/redirect_audit.sh tools/redirect_audit_cases.sh`
Note: the prog-name normalization (strip `$HUCK`/`bash` path prefixes so only the message text is compared), the fresh-temp-dir-per-case pattern, the combined `2>&1` + rc capture, and that the case-runner file path must be **absolute** (a relative path breaks when the runner `cd`s into the temp dir — the v292 lesson).

- [ ] **Step 2: Write `tools/pipeline_redirect_audit_cases.sh`** — the case list. Each case is a shell fragment exercising an **external** stage (use `/bin/sh -c '…'` or `/bin/cat`/`/bin/echo` so a real fork happens, NOT a builtin — builtins are #144, out of scope) in a `… | cat` pipeline. Emit `label<TAB>fragment` lines:

```bash
# pipeline_redirect_audit_cases.sh — sourced by pipeline_redirect_audit.sh.
# Each line: <label>\t<fragment>. External stage = /bin/sh -c so a real fork
# happens (builtin-stage stderr routing is #144, deliberately excluded).
emit_cases() {
  # writer emits OUT to stdout and ERR to stderr:
  local W="/bin/sh -c 'echo OUT; echo ERR >&2'"
  cat <<EOF
ord 2>&1 >f\t$W 2>&1 >pf | cat; echo --f--; cat pf
ord >f 2>&1\t$W >pf 2>&1 | cat; echo --f--; cat pf
ord 2>&1\t$W 2>&1 | cat
ord >f\t$W >pf | cat; echo --f--; cat pf
ord 1>&2\t$W 1>&2 | cat
ord >f 2>f2\t$W >pf 2>pf2 | cat; echo --f--; cat pf; echo --f2--; cat pf2
dup 3>&1\t/bin/sh -c 'echo THREE >&3' 3>&1 | cat
close 2>&-\t$W 2>&- | cat
in <f\techo FILE > infile; /bin/cat <infile | cat
readwrite <>f\techo RW > rwfile; /bin/cat <>rwfile | cat
fd3 heredoc\t/bin/cat <&3 3<<HD | cat
BODY3
HD
fd3 herestring\t/bin/cat <&3 3<<<'HS' | cat
fd4 open\t/bin/sh -c 'echo FOUR >&4' 4>pf | cat; echo --f--; cat pf
stage1 redir\t$W 2>&1 >pf | cat; echo --f--; cat pf
last redir\t/bin/echo A | /bin/sh -c 'cat; echo ERR >&2' 2>&1 >pf; echo --f--; cat pf
capture ctx\tout=\$($W 2>&1 >pf | cat); echo "cap=[\$out]"; echo --f--; cat pf
EOF
}
```
(The `fd3 heredoc` case is the one that HANGS on current huck — Step 4's runner wraps every case in `timeout`, so the hang registers as a divergence, not a stall.)

- [ ] **Step 3: Write `tools/pipeline_redirect_audit.sh`** — the runner. Mirror `redirect_audit.sh`'s `run_one`/normalization, add a `timeout`, compare bash-vs-huck output:

```bash
#!/usr/bin/env bash
# Differential audit: pipeline-stage redirects, bash 5.2.21 vs huck.
# Usage: HUCK=./target/debug/huck bash tools/pipeline_redirect_audit.sh
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
HUCK="${HUCK:-$HERE/../target/debug/huck}"
source "$HERE/pipeline_redirect_audit_cases.sh"

# Run one fragment through $1 (a shell) in a fresh temp dir; echo combined
# stdout+stderr with the shell's own path prefix normalized to "sh:".
run_one() {
  local shell="$1" frag="$2" d out
  d="$(mktemp -d)"
  out="$( cd "$d" && timeout 10 "$shell" -c "$frag" 2>&1 )"
  local rc=$?
  rm -rf "$d"
  # Normalize the leading program path (bash prints "bash:", huck prints its
  # binary path) so only the message text is compared; mark a timeout.
  if [ $rc -eq 124 ]; then printf 'TIMEOUT\n'; return; fi
  printf '%s\n' "$out" | sed -e "s#^$shell:#sh:#" -e "s#^${HUCK}:#sh:#" -e 's#^bash:#sh:#'
}

n=0; agree=0; diverge=0
while IFS=$'\t' read -r label frag; do
  [ -z "${label:-}" ] && continue
  n=$((n+1))
  b="$(run_one bash "$frag")"
  h="$(run_one "$HUCK" "$frag")"
  if [ "$b" = "$h" ]; then
    agree=$((agree+1))
  else
    diverge=$((diverge+1))
    printf 'DIVERGE: %s\n' "$label"
  fi
done < <(emit_cases)

echo "=================================================================="
echo "AUDIT: $n cases, $agree agree, $diverge DIVERGE"
echo "=================================================================="
[ "$diverge" -eq 0 ]
```

- [ ] **Step 4: Make executable + run on CURRENT code — expect RED**

```bash
chmod +x tools/pipeline_redirect_audit.sh
cargo build -p huck 2>&1 | tail -1
HUCK="$(pwd)/target/debug/huck" bash tools/pipeline_redirect_audit.sh
```
Expected: several `DIVERGE:` lines — at minimum `ord 2>&1 >f`, `ord 2>&1`, `fd3 heredoc`, `fd4 open`, `stage1 redir`, `capture ctx` (the #50 cases). A non-zero DIVERGE count confirms the gate detects the bug. Record the exact divergence list in the report — Task 3 must drive it to **0**.

- [ ] **Step 5: Sanity-check the single-command audit is untouched**
```bash
HUCK="$(pwd)/target/debug/huck" bash tools/redirect_audit.sh | sed -n '2p'
```
Expected: `AUDIT: 157 cases, 141 agree, 16 DIVERGE` (unchanged — this task adds no code).

- [ ] **Step 6: Commit**
```bash
git add tools/pipeline_redirect_audit.sh tools/pipeline_redirect_audit_cases.sh
git commit -m "$(cat <<'EOF'
v293 T1: add pipeline-stage differential audit (the P3b gate) (#50)

New tools/pipeline_redirect_audit.sh (+cases) runs external pipeline-stage
redirect constructs through bash 5.2.21 vs huck under timeout. On current code
it is RED (shows the #50 external-stage ordering + fd>2-heredoc divergences,
incl. the /bin/cat <&3 3<<EOF hang); Task 3 turns it green.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Give `spawn_external_with_fds` an optional full-plan path (behavior-preserving)

Add a `plan: Option<ChildRedirPlan>` parameter. When `Some`, replay the full ordered plan (mirroring `run_subprocess`) instead of the slot-derived dup-targets + `build_child_extra_ops`. Both current callers (`run_multi_stage` @~7235, `run_background_sequence` @~3557) pass `None` this task, so behavior is unchanged. Task 3 flips the foreground caller to `Some`.

**Files:** Modify `crates/huck-engine/src/executor.rs`.

**Interfaces:**
- Consumes (v292, unchanged): `struct ChildRedirPlan { ops: Vec<ChildRedirOp>, held: Vec<OwnedFd>, heredoc_writers: Vec<pid_t> }`, `unsafe fn replay_redir_ops(&[ChildRedirOp])`, `enum ChildRedirOp { Dup{target,source}, Close{target} }`.
- Produces: `fn spawn_external_with_fds(cmd, shell, sink, err_sink, stdio: ChildStdio, pgid_target, parent_fds_to_close, plan: Option<ChildRedirPlan>) -> Result<i32, io::Error>`. When `Some(plan)`, the caller has already taken `plan.heredoc_writers` (the spawner ignores that field) and built a pipe/capture-only `stdio`.

- [ ] **Step 1: Add the parameter.** Change the signature (currently ends `parent_fds_to_close: &[RawFd],`) to add a final param:
```rust
fn spawn_external_with_fds(
    cmd: &SimpleCommand,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
    stdio: ChildStdio,
    pgid_target: i32,
    parent_fds_to_close: &[RawFd],
    plan: Option<ChildRedirPlan>,
) -> Result<i32, io::Error> {
```

- [ ] **Step 2: Branch the redirect setup on `plan`.** The existing body (read via `cat`/Read around lines 8388–8456 and 8500–8527) does, for the LEGACY path: resolve `stdout_dup_target`/`stderr_dup_target` from `slot_stdout()/slot_stderr()`, chain a dup `pre_exec`, call `build_child_extra_ops` → `extra_ops`/`extra_held`, compute `extra_targets`, replay `extra_ops` in a `pre_exec`, and later `drop(extra_held)` + filter `fds_to_close` by `extra_targets`. Wrap ALL of that legacy redirect logic in `if plan.is_none()` / restructure so that when `plan.is_some()` the following holds instead:
  - Do NOT resolve slot dup-targets and do NOT chain the dup `pre_exec` (the plan's `ChildRedirOp::Dup{target:1/2,…}` already carries them).
  - Do NOT call `build_child_extra_ops`.
  - Take the plan's ops/held: `let (replay_ops, held): (Vec<ChildRedirOp>, Vec<OwnedFd>) = match &plan { Some(_) => { let p = plan_ref…} }` — simplest is to destructure up front:
    ```rust
    // Unify: `replay_ops` are replayed in the child pre_exec (source order),
    // `held` keeps parent-opened files alive until after spawn, `op_targets`
    // are fds a replay op installs (must be excluded from the close list).
    let stdout_dup_target: Option<i32>;
    let stderr_dup_target: Option<i32>;
    let replay_ops: Vec<ChildRedirOp>;
    let held: Vec<std::os::fd::OwnedFd>;
    if let Some(p) = plan {
        // NEW full-plan path (foreground external stages): the whole ordered
        // redirect list is in the plan; no slot dup-targets, no extra_ops.
        stdout_dup_target = None;
        stderr_dup_target = None;
        replay_ops = p.ops;
        held = p.held;
        // p.heredoc_writers was taken by the caller.
    } else {
        // LEGACY slot path (background stages, until v294): slot dup-targets +
        // build_child_extra_ops for fd>2/dup-in/close/<>.
        stdout_dup_target = match &exec.slot_stdout() {
            Some(RedirectSlot::Dup { source, .. }) => Some(resolve_fd_target(source, shell)?),
            _ => None,
        };
        stderr_dup_target = match &exec.slot_stderr() {
            Some(RedirectSlot::Dup { source, .. }) => Some(resolve_fd_target(source, shell)?),
            _ => None,
        };
        let (extra_ops, extra_held) =
            build_child_extra_ops(&exec.redirects, shell, sink, err_sink)
                .map_err(|code| io::Error::other(format!("redirect failed with code {code}")))?;
        replay_ops = extra_ops;
        held = extra_held;
    }
    ```
  Then, in the code that follows, replace the LEGACY-named locals with the unified ones:
  - The dup `pre_exec` block stays but is now guarded by `if stdout_dup_target.is_some() || stderr_dup_target.is_some()` (already the case — with the plan path both are `None`, so it is skipped).
  - `let extra_targets: Vec<RawFd> = replay_ops.iter().map(|op| match *op { ChildRedirOp::Dup { target, .. } | ChildRedirOp::Close { target } => target }).collect();`
  - The `if !extra_ops.is_empty()` replay block becomes `if !replay_ops.is_empty() { let ops = replay_ops; unsafe { process.pre_exec(move || replay_redir_ops(&ops)); } }`.
  - The stdout/stderr `Stdio` selection that checks `stdout_dup_target.is_some()` is unchanged (both None on the plan path → uses the `ChildStdio` base directly, which is the pipe/capture base).
  - `drop(extra_held)` becomes `drop(held)`.
  - `fds_to_close` filter `!extra_targets.contains(fd)` is unchanged (now sourced from the unified `replay_ops`).

- [ ] **Step 3: Update BOTH current call sites to pass `None`.** At `run_multi_stage` (~7235) and `run_background_sequence` (~3557), add `None` as the final argument to `spawn_external_with_fds(...)`:
```rust
StageKind::External(simple) => spawn_external_with_fds(
    simple, shell, sink, err_sink, child_stdio, pgid_target, &fds_to_close_in_child,
    None,
),
```

- [ ] **Step 4: Build — warning-clean**
```bash
cargo build -p huck 2>&1 | tail -20
```
Expected: warning-clean. (`ChildRedirPlan`'s fields are all read on the `Some` path; no dead-code warning even though no caller passes `Some` yet — the match arm is compiled, just not taken at runtime.)

- [ ] **Step 5: Verify NO behavior change**
```bash
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
tests/scripts/fd_torture_diff_check.sh | tail -1
HUCK="$(pwd)/target/debug/huck" bash tools/redirect_audit.sh | sed -n '2p'
HUCK="$(pwd)/target/debug/huck" bash tools/pipeline_redirect_audit.sh | sed -n '2p'
( ulimit -v 6000000; cargo test -p huck --test named_fd_integration --jobs 1 -- --test-threads 1 2>&1 | tail -3 )
```
Expected: lib ok (~1806); fd_torture unchanged (`41, Pass: 41`); single-command audit `16 DIVERGE`; **pipeline audit still shows the SAME divergence count as Task 1** (this task changed nothing observable — both callers pass `None`); named_fd 7/7.

- [ ] **Step 6: Commit**
```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v293 T2: spawn_external_with_fds gains an optional full-plan path (#50)

Add `plan: Option<ChildRedirPlan>`. When Some, replay the whole ordered
ChildRedirPlan (mirroring run_subprocess) instead of slot dup-targets +
build_child_extra_ops. Both callers pass None this task -> behavior-preserving
seam; Task 3 flips the foreground caller. Audits + sweep unchanged.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Flip `run_multi_stage` external stages onto the full plan (the fix)

Hoist `classify_stage` before the stage-stdio base construction; for **External** stages, build a pipe/capture-only base (skip the slot file/heredoc/dup reads), lower the stage's full `Redirection` list via `build_child_redir_plan`, and pass `Some(plan)`. This fixes #50 (foreground) + #69 (foreground, which falls out because the plan's opens route through `redir_open_error` from the caller with `current_lineno` set, and the spurious `"redirect failed with code"` wrapper is gone).

**Files:** Modify `crates/huck-engine/src/executor.rs` (`run_multi_stage`, ~lines 6528–7260); `tests/scripts/fd_torture_diff_check.sh`.

**Interfaces:**
- Consumes: `spawn_external_with_fds(..., plan: Option<ChildRedirPlan>)` (Task 2); `build_child_redir_plan(&[Redirection], shell, sink, err_sink) -> Result<ChildRedirPlan, i32>` (v292); `classify_stage(&Command, &Shell) -> StageKind`.

- [ ] **Step 1: Hoist classification.** Read `run_multi_stage`'s stage loop (~6528–7260). Immediately after `stage_cmd` is bound for the iteration and BEFORE the `let stdin: ChildFd = …` base construction (~6749), add:
```rust
// Classify the stage up front: the stdio-base construction differs for
// External (pipe/capture-only base + a full ChildRedirPlan replayed in the
// child) vs InProcess (existing slot-base; its body applies redirects). A
// Simple(Exec) stage can be EITHER kind (external program vs builtin), so we
// must key the base on the classification, not on Simple-vs-Compound.
let stage_is_external = matches!(classify_stage(stage_cmd, shell), StageKind::External(_));
```
(`classify_stage` borrows `shell` immutably and returns a `StageKind` borrowing `stage_cmd`; capturing just the `bool` avoids holding either borrow across the base construction.)

- [ ] **Step 2: Make the three slot-read blocks InProcess-only.** Each of the `slot_stdin` (~6749), `explicit_stdout` (~6877), and `explicit_stderr` (~6933) blocks is currently guarded `if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd { … } else { <pipe-only/None> }`. Change each guard so an **external** stage takes the pipe-only/`None` branch (i.e., behaves like the compound `else`):
  - `slot_stdin` (~6749): change `let stdin: ChildFd = if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {` to `let stdin: ChildFd = if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd && !stage_is_external {` (keeps the slot reads for InProcess builtin stages; external stages fall to the `else` → `prev_pipe_read`/`Inherit`).
  - `explicit_stdout` (~6877): change `if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {` to `if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd && !stage_is_external {` (external → `None` → the pipe/capture path in the `let stdout` block handles it; the orphan-pipe-for-file case is not needed because the real inter-stage pipe's write end is simply overridden by the plan's `>file` dup2 in the child, so the downstream stage still reads EOF).
  - `explicit_stderr` (~6933): same guard change → external → `None`.

- [ ] **Step 3: Build the plan for external stages and pass it.** At the spawn site (~7226–7253), replace the External arm. First, immediately before `let child_stdio = ChildStdio::new(stdin, stdout, stderr);`, build the plan for external stages (so its heredoc writers are reaped and its `held` survives into the spawner). Locate the `heredoc_writers` accumulator this loop already uses (the vec that slot heredocs push into via `heredoc_writers.push(pid)`) and extend it with the plan's writers:
```rust
// For an EXTERNAL stage, lower the FULL ordered redirect list once (v292
// machinery) and replay it in the child over the pipe/capture base. #69: set
// current_lineno so a stage redirect-open error carries `line N:` like a
// single command; the plan's opens route through redir_open_error.
let external_plan: Option<ChildRedirPlan> = if stage_is_external {
    if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
        // #69: stamp the stage's line so a redirect-open error carries
        // `line N:` (mirrors the single-command path at executor.rs:4550).
        if exec.line != 0 {
            shell.current_lineno = exec.line;
        }
        match build_child_redir_plan(&exec.redirects, shell, sink, err_sink) {
            Ok(mut p) => {
                heredoc_writers.append(&mut p.heredoc_writers);
                Some(p)
            }
            Err(_) => {
                restore_inline_assignments(snap, shell);
                return bail_teardown_stage(shell, procsub_base, &mut parent_held);
            }
        }
    } else {
        None
    }
} else {
    None
};
```
(`ExecCommand` has `pub line: u32` — confirmed; `heredoc_writers: Vec<libc::pid_t>` is the loop's existing writer accumulator. The `-c`-mode line-0 residue is #79, out of scope.)

Then change the External spawn arm to pass the plan (InProcess arm unchanged):
```rust
let spawn_result = match classify_stage(stage_cmd, shell) {
    StageKind::External(simple) => spawn_external_with_fds(
        simple, shell, sink, err_sink, child_stdio, pgid_target,
        &fds_to_close_in_child, external_plan,
    ),
    StageKind::InProcess(cmd) => fork_and_run_in_subshell(
        cmd, shell, child_stdio, pgid_target, &fds_to_close_in_child,
        stdout_dup_target, stderr_dup_target,
    ),
};
```
The InProcess dup-target resolution block (~7172–7225) stays (InProcess builtin stages still use it). Note: for external stages `external_plan` is moved into the spawner; the `stage_is_external` bool already gated the base, so the base `stdin/stdout/stderr` here are the pipe/capture-only ChildFds.

- [ ] **Step 4: Build — warning-clean**
```bash
cargo build -p huck 2>&1 | tail -20
```
Expected: warning-clean. If a borrow-checker error appears on `classify_stage(stage_cmd, shell)` at the spawn site (already borrowed via `stage_is_external`), reuse the earlier bool and match on a fresh `classify_stage` call only where needed — `classify_stage` is cheap and takes `&Shell`, so calling it twice is fine.

- [ ] **Step 5: Drive the pipeline audit to GREEN**
```bash
HUCK="$(pwd)/target/debug/huck" bash tools/pipeline_redirect_audit.sh
```
Expected: `AUDIT: <n> cases, <n> agree, 0 DIVERGE` (exit 0). If any case still diverges, STOP and report the bash-vs-huck diff for that case — do NOT edit the audit to force green.

- [ ] **Step 6: Add fd_torture regression cases.** Append to `tests/scripts/fd_torture_diff_check.sh` after the existing v292b block:
```bash
# --- v293: foreground external pipeline-stage redirect ordering (#50) ---
check "p ext 2>&1 >f"   '/bin/sh -c "echo O; echo E >&2" 2>&1 >pf | cat; echo --; cat pf; rm -f pf'
check "p ext >f 2>&1"   '/bin/sh -c "echo O; echo E >&2" >pf 2>&1 | cat; echo --; cat pf; rm -f pf'
check "p ext fd3 heredoc" '/bin/cat <&3 3<<HD | cat
BODY
HD'
```

- [ ] **Step 7: Full verification — single-command path unperturbed, sweep green both binaries**
```bash
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
tests/scripts/fd_torture_diff_check.sh | tail -1
HUCK="$(pwd)/target/debug/huck" bash tools/redirect_audit.sh | sed -n '2p'
( ulimit -v 6000000; cargo test -p huck --test named_fd_integration --jobs 1 -- --test-threads 1 2>&1 | tail -3 )
cargo build --release -p huck 2>&1 | tail -1
( ulimit -v 1500000; timeout 1200 tests/scripts/run_diff_checks.sh 2>&1 | tail -3 )
```
Expected: lib ok; fd_torture all pass (44); single-command audit **still 16 DIVERGE** (unchanged); named_fd 7/7; sweep `0 failed` on debug AND release.

- [ ] **Step 8: Commit**
```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs tests/scripts/fd_torture_diff_check.sh
git commit -m "$(cat <<'EOF'
v293 T3: flip run_multi_stage external stages onto the ordered plan (#50, #69)

External foreground pipeline stages now build a pipe/capture-only base and
replay their full ordered ChildRedirPlan (v292 build_child_redir_plan) in the
child, replacing the last-wins RedirectSlot stdio + build_child_extra_ops. Fixes
the foreground half of #50 (stage source-order + fd>2 heredoc, incl. the
/bin/cat <&3 hang) and #69 (line N: falls out; the spurious "redirect failed"
message is gone). InProcess stages + run_background_sequence untouched.
pipeline_redirect_audit 0 DIVERGE; single-command audit unchanged at 16.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Notes for the whole-branch review

- **Acceptance is the two audits together:** `pipeline_redirect_audit.sh` = 0 DIVERGE (the fix) AND `redirect_audit.sh` = 16 DIVERGE (single-command path unperturbed). Confirm both on the built binary.
- **Scope containment:** the diff must touch only `spawn_external_with_fds`, the `run_multi_stage` stage loop, and the two test/tool files. `run_background_sequence` must be unchanged except the one `None` argument added in Task 2; `slots_for_simple_path`/`RedirectSlot`/`build_child_extra_ops`/`fork_and_run_in_subshell` unchanged.
- **#144/#78/#79 must remain** (builtin-stage stderr routing, spawn-failure message, `-c`-mode line-0). The pipeline audit's corpus deliberately uses external `/bin/sh -c` stages so it does not assert #144 behavior.
- **Composition correctness (reason through, don't just trust the audit):** for `X 2>&1 >f | cat` the child gets pipe-write on fd 1 (base), then replays `dup2(1,2)` (2→pipe) then `dup2(f,1)` (1→f) ⇒ stdout→f, stderr→pipe. For `X >f | cat` the plan's `dup2(f,1)` overrides the inter-stage pipe write end ⇒ downstream reads EOF.
- **Heredoc-writer reaping:** the plan's `heredoc_writers` are appended to the loop's existing accumulator BEFORE the spawner is called (the spawner ignores `plan.heredoc_writers`); confirm they are reaped on every stage exit path exactly as the slot heredoc writers were.
