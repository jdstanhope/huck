# Background pipeline-stage redirect flip (P3b background half) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Flip `run_background_sequence` (bare trailing-`&` pipeline) EXTERNAL stages off the lossy `RedirectSlot` fast-path onto v292's ordered `build_child_redir_plan` replay — mirroring the v293 foreground flip — closing #50 and #69.

**Architecture:** `run_background_sequence` is a near-verbatim clone of the already-flipped `run_multi_stage`. Apply the identical change: external stages build a pipe/`stage0_default`-only `ChildStdio` base and replay their full ordered `ChildRedirPlan` in the child (via the `spawn_external_with_fds(…, Some(plan))` seam that v293 already added). Gate it with a new marker-poll differential audit for background pipelines (bare `pipeline &` cases observed via result files, since a trailing `wait` would skip this code path).

**Tech Stack:** Rust, `std::process::Command` + `pre_exec`, libc. Crate: `huck-engine`. Plus POSIX shell for the async audit harness.

**Spec:** `docs/superpowers/specs/2026-07-14-bg-pipeline-stage-redirect-flip-design.md`. **Issues:** #50, #69 (this iteration CLOSES both). **Precedent:** the v293 plan `docs/superpowers/plans/2026-07-14-pipeline-stage-redirect-flip.md` (same flip on `run_multi_stage`).

## Global Constraints

- **The ONLY intended behavior change is the background external-stage half of #50 + #69.** Single commands, foreground pipelines (`run_multi_stage`), InProcess stages (compound AND builtin), and every non-external background stage must stay byte-identical.
- **New gate:** `tools/bg_pipeline_redirect_audit.sh` (`HUCK=<bin> bash tools/bg_pipeline_redirect_audit.sh`) — bare `pipeline &` cases, result-file comparison vs bash 5.2.21, marker-poll (poll-until-stable) async observation, per-case timeout. Task 1 builds it RED; Task 2 turns it GREEN (0 DIVERGE).
- **These must not move:** `tools/pipeline_redirect_audit.sh` (foreground) stays **0 DIVERGE**; `tools/redirect_audit.sh` (single-command) stays **16 DIVERGE**.
- **Do NOT fix or regress:** #144 (builtin-stage stderr), #145 (pipeline-abort-on-stage-redirect-failure), #78, #79.
- **Do NOT touch:** `run_multi_stage`, the software-sink layer, job-control/pgroup wiring, and — this iteration — the now-dead slot machinery (`slots_for_simple_path`/`RedirectSlot`/`build_child_extra_ops`); its deletion is a deferred follow-up. Do NOT add `fd_torture` background cases (a bare `pipeline &` detaches with no synchronous output, so `fd_torture`'s output comparison can't test it; the bg audit is the gate).
- **Test/build discipline (this box OOMs on `cargo test --workspace`):** build `cargo build -p huck` / `--release`; lib `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` (~1806); integration `( ulimit -v 6000000; cargo test -p huck --test named_fd_integration --jobs 1 -- --test-threads 1 )` (7); sweep `( ulimit -v 1500000; timeout 1200 tests/scripts/run_diff_checks.sh )`.
- **Commit trailer, verbatim, last line of every commit:** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Formatting:** `cargo fmt --all` before every commit.

---

### Task 1: The background-pipeline differential audit (marker-poll gate)

Build `tools/bg_pipeline_redirect_audit.sh` + `tools/bg_pipeline_redirect_audit_cases.sh`. Each case is a BARE `pipeline &` (so it truly hits `run_background_sequence` — a trailing `wait` would route elsewhere) whose stages write result files; the runner polls each result file until its size is stable (or a per-case timeout) and compares bash-vs-huck. On CURRENT code it is RED (background #50 divergences); Task 2 turns it GREEN.

**Files:** Create `tools/bg_pipeline_redirect_audit.sh`, `tools/bg_pipeline_redirect_audit_cases.sh`.

**Interfaces:**
- Produces: executable `tools/bg_pipeline_redirect_audit.sh` reading `HUCK` (default `./target/debug/huck`); prints `AUDIT: <n> cases, <k> agree, <d> DIVERGE` + one `DIVERGE: <label>` per divergence; exit 0 iff 0 diverge.

- [ ] **Step 1: Study the foreground harness** to mirror structure + normalization:

Run: `cat tools/pipeline_redirect_audit.sh tools/pipeline_redirect_audit_cases.sh`
Note: real TAB bytes between `label` and fragment (NOT literal `\t` — that produced a false-green in v293), absolute `$HERE` source path, fresh temp dir per case, program-name-prefix normalization.

- [ ] **Step 2: Write `tools/bg_pipeline_redirect_audit_cases.sh`.** Each case declares its result files and a fragment. The fragment runs a bare external-stage `pipeline &` writing to those files. Format per line: `label<TAB>resultfiles(space-sep)<TAB>fragment`. Use real TABs.

```bash
# bg_pipeline_redirect_audit_cases.sh — sourced by bg_pipeline_redirect_audit.sh.
# Each line: <label>\t<space-separated result files>\t<fragment>.
# Fragment is a BARE `pipeline &` (rest empty => run_background_sequence). External
# stage = /bin/sh -c so a real fork happens. The consumer stage captures the piped
# stream to a file so it is observable after the detached job finishes.
emit_bg_cases() {
  local W="/bin/sh -c 'echo O; echo E >&2'"
  # NOTE: real tab characters separate the three fields below.
  printf '%s\t%s\t%s\n' "ord 2>&1 >f"   "pf po" "$W 2>&1 >pf | cat >po &"
  printf '%s\t%s\t%s\n' "ord >f 2>&1"   "pf po" "$W >pf 2>&1 | cat >po &"
  printf '%s\t%s\t%s\n' "ord >f 2>f2"   "pf pf2 po" "$W >pf 2>pf2 | cat >po &"
  printf '%s\t%s\t%s\n' "fd4 open"       "pf po" "/bin/sh -c 'echo FOUR >&4' 4>pf | cat >po &"
  printf '%s\t%s\t%s\n' "fd3 dup"        "po" "/bin/sh -c 'echo THREE >&3' 3>&1 | cat >po &"
  printf '%s\t%s\t%s\n' "in <f"          "po" "echo FILE > infile; /bin/cat <infile | cat >po &"
  printf '%s\t%s\t%s\n' "stage0 nodir"   "po" "/bin/cat | cat >po &"
  printf '%s\t%s\t%s\n' "close 2>&-"     "po" "$W 2>&- | cat >po &"
  printf '%s\t%s\t%s\n' "last redir"     "pf po" "/bin/echo A | /bin/sh -c 'cat; echo E >&2' 2>&1 >pf | cat >po &"
}
```
(Every fragment MUST end with a bare `&` — no trailing `wait`/`;` command, or `seq.rest` becomes non-empty and the pipeline skips `run_background_sequence`. The `stage0 nodir` case pins that an external FIRST stage with no stdin redirect still gets the `/dev/null` stage-0 default — `/bin/cat` reads EOF immediately, so `po` is empty in both shells.)

- [ ] **Step 3: Write `tools/bg_pipeline_redirect_audit.sh`** — the marker-poll runner:

```bash
#!/usr/bin/env bash
# Differential audit: BACKGROUND pipeline-stage redirects, bash 5.2.21 vs huck.
# A bare `pipeline &` detaches; we observe via result files, polling each until
# its size is stable (job finished) or a per-case timeout elapses. Usage:
#   HUCK=./target/debug/huck bash tools/bg_pipeline_redirect_audit.sh
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
HUCK="${HUCK:-$HERE/../target/debug/huck}"
source "$HERE/bg_pipeline_redirect_audit_cases.sh"

# Poll the given files until their combined byte-size is unchanged for 3
# consecutive 50ms reads (job settled) or ~5s elapses. A minimum 200ms floor
# gives a slow-starting detached job time to write before we read "empty".
poll_settle() {
  local files="$1" elapsed=0 stable=0 prev="" sig f sz
  while [ "$elapsed" -lt 5000 ]; do
    sig=""
    for f in $files; do
      sz=$(wc -c < "$f" 2>/dev/null || echo -1); sig="$sig,$sz"
    done
    if [ "$sig" = "$prev" ]; then stable=$((stable+1)); else stable=0; fi
    prev="$sig"
    if [ "$stable" -ge 3 ] && [ "$elapsed" -ge 200 ]; then return; fi
    sleep 0.05; elapsed=$((elapsed+50))
  done
}

# Run one bare-bg fragment through $1 in a fresh temp dir; echo each result
# file's contents labeled, after polling for completion. Normalize the shell's
# own path prefix so only message text is compared.
run_one_bg() {
  local shell="$1" files="$2" frag="$3" d out f
  d="$(mktemp -d)"
  ( cd "$d" && timeout 10 "$shell" -c "$frag" ) >/dev/null 2>&1
  ( cd "$d" && poll_settle "$files" )
  out=""
  for f in $files; do
    out="$out==$f==
$(cd "$d" && cat "$f" 2>/dev/null)
"
  done
  rm -rf "$d"
  printf '%s' "$out" | sed -e "s#^$shell:#sh:#" -e "s#^${HUCK}:#sh:#" -e 's#^bash:#sh:#'
}

n=0; agree=0; diverge=0
while IFS=$'\t' read -r label files frag; do
  [ -z "${label:-}" ] && continue
  n=$((n+1))
  b="$(run_one_bg bash "$files" "$frag")"
  h="$(run_one_bg "$HUCK" "$files" "$frag")"
  if [ "$b" = "$h" ]; then agree=$((agree+1)); else diverge=$((diverge+1)); printf 'DIVERGE: %s\n' "$label"; fi
done < <(emit_bg_cases)

echo "=================================================================="
echo "AUDIT: $n cases, $agree agree, $diverge DIVERGE"
echo "=================================================================="
[ "$diverge" -eq 0 ]
```

- [ ] **Step 4: Make executable + run on CURRENT code — expect RED**

```bash
chmod +x tools/bg_pipeline_redirect_audit.sh
cargo build -p huck 2>&1 | tail -1
HUCK="$(pwd)/target/debug/huck" bash tools/bg_pipeline_redirect_audit.sh
```
Expected: a NON-ZERO `DIVERGE` count including at least `ord 2>&1 >f`, `ord >f 2>f2`, `fd4 open`, `last redir` (the background #50 cases). Record the full divergence list + summary in the report — Task 2 must drive it to **0**. (The non-zero exit is expected/correct for a RED gate.) Confirm the run completes in well under a minute (each case's poll caps at ~5s; most settle in ~200ms).

- [ ] **Step 5: Sanity-check the other audits are untouched**
```bash
HUCK="$(pwd)/target/debug/huck" bash tools/pipeline_redirect_audit.sh | grep '^AUDIT'
HUCK="$(pwd)/target/debug/huck" bash tools/redirect_audit.sh | grep '^AUDIT'
```
Expected: foreground `15 cases … 0 DIVERGE`; single-command `157 cases … 16 DIVERGE`.

- [ ] **Step 6: Commit**
```bash
git add tools/bg_pipeline_redirect_audit.sh tools/bg_pipeline_redirect_audit_cases.sh
git commit -m "$(cat <<'EOF'
v294 T1: add background pipeline-stage differential audit (marker-poll gate) (#50)

New tools/bg_pipeline_redirect_audit.sh (+cases): bare `pipeline &` cases that
truly hit run_background_sequence, observed via result files with a poll-until-
stable async wait (a trailing `wait` would skip the code path). RED on current
code (background #50 ordering + fd>2 divergences); Task 2 turns it green.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Flip `run_background_sequence` external stages onto the ordered plan (the fix)

Mirror the v293 `run_multi_stage` flip on `run_background_sequence`: hoist `classify_stage`, guard the three slot-read blocks `&& !stage_is_external`, build `external_plan` before the inter-stage pipe, pass `Some(plan)` to the External spawn arm. Closes #50/#69.

**Files:** Modify `crates/huck-engine/src/executor.rs` (`run_background_sequence`, ~lines 2845–3600).

**Interfaces:**
- Consumes: `spawn_external_with_fds(cmd, shell, sink, err_sink, stdio, pgid, parent_fds_to_close, plan: Option<ChildRedirPlan>)` (v293); `build_child_redir_plan(&[Redirection], shell, sink, err_sink) -> Result<ChildRedirPlan, i32>`; `classify_stage(&Command, &Shell) -> StageKind`; `ExecCommand.line: u32`; the loop's `first_pid`, `spawned_pids`, `parent_held`, `snap`, `procsub_base`, `stage0_default`, `bail_teardown_bg`, `restore_inline_assignments`.

- [ ] **Step 1: Hoist classification.** Read `run_background_sequence` (~2845–3600). Immediately before the `// ---- Stdin fd` comment at line ~3049 (after `stage_cmd` is bound and inline assignments are applied for the iteration), add:
```rust
// Classify up front: external stages get a pipe/stage0_default-only base + a
// full ChildRedirPlan replayed in the child; InProcess stages (compound AND
// builtin) keep the slot base. A Simple(Exec) stage can be either kind, so key
// the base on the classification, not on Simple-vs-Compound.
let stage_is_external = matches!(classify_stage(stage_cmd, shell), StageKind::External(_));
```

- [ ] **Step 2: Guard the three slot-read blocks.** Add `&& !stage_is_external` to each guard so external stages fall to the pipe-only/`None` `else` branch (which already preserves `stage0_default` / `prev_pipe_read` / `Inherit`):
  - `slot_stdin` (line 3053): `let stdin: ChildFd = if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {` → `… = stage_cmd && !stage_is_external {`.
  - `explicit_stdout` (line 3231): `if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {` → `… = stage_cmd && !stage_is_external {`.
  - `explicit_stderr` (line 3311): `if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {` → `… = stage_cmd && !stage_is_external {`.

- [ ] **Step 3: Build `external_plan` before the stdout build.** Immediately BEFORE `let stdout: ChildFd = if let Some(cf) = explicit_stdout {` (line 3390) — so a forked heredoc writer cannot inherit this stage's own inter-stage pipe write end (created inside the stdout build; the v293 deadlock lesson) — insert:
```rust
// External stage: lower the FULL ordered redirect list once (v292 machinery)
// and replay it in the child over the pipe/stage0_default base. Built BEFORE
// the inter-stage pipe so a forked heredoc writer can't inherit this stage's
// own write end (would deadlock EOF). #69: stamp current_lineno so a stage
// redirect-open error carries `line N:`. NOTE: unlike the foreground path,
// run_background_sequence does not track heredoc-writer pids — bg children are
// reaped by the SIGCHLD reaper — so the plan's heredoc_writers are left in the
// plan and dropped by the spawner (a no-op), matching the existing bg discard.
let external_plan: Option<ChildRedirPlan> = if stage_is_external {
    if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {
        if exec.line != 0 {
            shell.current_lineno = exec.line;
        }
        match build_child_redir_plan(&exec.redirects, shell, sink, err_sink) {
            Ok(p) => Some(p),
            Err(_) => {
                restore_inline_assignments(snap, shell);
                return bail_teardown_bg(
                    shell,
                    procsub_base,
                    first_pid,
                    &spawned_pids,
                    &mut parent_held,
                );
            }
        }
    } else {
        None
    }
} else {
    None
};
```

- [ ] **Step 4: Pass `external_plan` to the External spawn arm.** At the spawn `match` (line ~3556), change the External arm's final argument from `None` to `external_plan`; leave the InProcess arm unchanged:
```rust
        let spawn_result = match classify_stage(stage_cmd, shell) {
            StageKind::External(simple) => spawn_external_with_fds(
                simple,
                shell,
                sink,
                err_sink,
                child_stdio,
                pgid_target,
                &fds_to_close_in_child,
                external_plan,
            ),
            StageKind::InProcess(cmd) => fork_and_run_in_subshell(
                cmd,
                shell,
                child_stdio,
                pgid_target,
                &fds_to_close_in_child,
                stdout_dup_target,
                stderr_dup_target,
            ),
        };
```
(The `stdout_dup_target`/`stderr_dup_target` resolution block at ~3486 stays — InProcess stages still use it.)

- [ ] **Step 5: Build — warning-clean**
```bash
cargo build -p huck 2>&1 | tail -20
```
Expected: warning-clean. If a borrow error appears at the spawn-site `classify_stage(stage_cmd, shell)` (already borrowed for `stage_is_external`), that call takes `&Shell` and returns a fresh value — calling it twice is fine; if the borrow checker still complains, match on `stage_is_external` for the External/InProcess split reusing the earlier bool.

- [ ] **Step 6: Drive the bg audit GREEN + confirm the others unchanged**
```bash
HUCK="$(pwd)/target/debug/huck" bash tools/bg_pipeline_redirect_audit.sh
HUCK="$(pwd)/target/debug/huck" bash tools/pipeline_redirect_audit.sh | grep '^AUDIT'
HUCK="$(pwd)/target/debug/huck" bash tools/redirect_audit.sh | grep '^AUDIT'
```
Expected: bg audit `AUDIT: <n> cases, <n> agree, 0 DIVERGE` (exit 0); foreground `0 DIVERGE`; single-command `16 DIVERGE`. If any bg case still diverges, STOP and report the bash-vs-huck result-file diff for it — do NOT edit the audit to force green.

- [ ] **Step 7: Full verification — sweep green both binaries**
```bash
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
tests/scripts/fd_torture_diff_check.sh | tail -1
( ulimit -v 6000000; cargo test -p huck --test named_fd_integration --jobs 1 -- --test-threads 1 2>&1 | tail -3 )
cargo build --release -p huck 2>&1 | tail -1
( ulimit -v 1500000; timeout 1200 tests/scripts/run_diff_checks.sh 2>&1 | tail -3 )
```
Expected: lib ok (~1806); fd_torture unchanged (44); named_fd 7/7; sweep `0 failed` on debug AND release.

- [ ] **Step 8: Commit**
```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v294 T2: flip run_background_sequence external stages onto the ordered plan (#50, #69)

Background external pipeline stages now build a pipe/stage0_default-only base and
replay their full ordered ChildRedirPlan (v292 build_child_redir_plan) in the
child, mirroring the v293 foreground flip and retiring the last-wins RedirectSlot
path for background pipelines. Closes the background half of #50 (stage
source-order + fd>2) and #69 (line N:). InProcess stages + run_multi_stage
untouched. bg audit 0 DIVERGE; foreground + single-command audits unchanged.

Closes #50
Closes #69

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Notes for the whole-branch review

- **Acceptance is the three audits:** bg `bg_pipeline_redirect_audit.sh` = 0 DIVERGE (the fix); foreground `pipeline_redirect_audit.sh` = 0 DIVERGE and single-command `redirect_audit.sh` = 16 (both unperturbed).
- **Scope containment:** the diff must touch only `run_background_sequence` + the two new audit tool files. `run_multi_stage`, the slot machinery (retained), `fork_and_run_in_subshell`, and the sink layer unchanged.
- **The heredoc-writer difference from v293:** `run_background_sequence` has no heredoc-writer accumulator (bg children are SIGCHLD-reaped). Confirm `external_plan`'s `heredoc_writers` are left in the plan and dropped by the spawner (a `Vec<pid_t>` drop is a no-op) — NOT appended anywhere, and NOT reaped inline. This matches the existing bg stdin-heredoc path that discards `_pid`.
- **Ordering:** `external_plan` must be built before the stdout build's `make_pipe` (Step 3 places it before line 3390) — a forked heredoc writer inheriting the stage's own pipe write end would deadlock EOF.
- **stage-0 default:** confirm an external first stage with no stdin redirect still gets `/dev/null` (the `stage0 nodir` audit case); it comes from the compound `else` branch's `stage0_default.try_clone()`.
- **Async-audit robustness:** the bg audit polls result-file size until stable (3× consecutive unchanged reads after a 200ms floor, 5s cap) rather than a fixed sleep — note if any case shows flakiness across repeated runs.
