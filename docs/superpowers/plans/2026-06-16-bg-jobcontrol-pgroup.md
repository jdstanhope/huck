# v173: background job-control process group (fix L-53) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make non-interactive background jobs (`a | b &`, `( cmd ) &`) keep their stages in the shell's own process group like bash, reusing v172's `job_control_active()` + `NO_PGROUP`, while keeping `kill %n` / `wait` / `$!` and the partial-spawn cleanup correct without a dedicated group.

**Architecture:** Gate the two background functions' `setpgid` on `Shell::job_control_active()` (pass `NO_PGROUP` when off; the child-side `setpgid` in the fork primitives is already `>= 0`-gated from v172). Record per-job whether it owns a process group (`Job.own_pgroup`) so `kill %n` and `hangup_jobs` signal per-pid when there's no group, and harden `cleanup_partial_pipeline_raw` to kill children individually so its blocking `waitpid` can't hang.

**Tech Stack:** Rust, `libc::setpgid`/`killpg`/`kill`. PTY/process tests via `expectrl` + `ps`/`pgrep`; bash-diff harness (`tests/scripts/*_diff_check.sh`).

**Spec:** `docs/superpowers/specs/2026-06-16-bg-jobcontrol-pgroup-design.md`

**Branch:** `v173-bg-jobcontrol-pgroup`

**Risk note:** this is the v108/v121/v124 + v172 job-control area. The existing PTY suite (`subshell_pipeline_pty`, `subshell_tty_pty`, `subshell_job_notice_pty`, `completion_jobcontrol_pty`, `procsub_stop_pty`, `sigint_abort_pty`, `pty_interactive`, `jobcontrol_pgroup_pty`) is the deadlock-regression guard and MUST stay green after every behavior-changing task. Verify incrementally — do not batch.

---

## Confirmed sites (current line numbers)

- `src/jobs.rs`: `pub struct Job` (25), `marked_for_nohup` last field (36), `pub fn add` (57), `add_synthetic_done` Job initializer (80–95).
- `src/executor.rs`: `run_background_subshell` (1950) with `/*pgid_target=*/ 0,` (1965) and `shell.jobs.add(pid, vec![pid], display)` (1972); `run_background_sequence` (1986) with per-stage `let pgid_target = first_pid.unwrap_or(0);` at 2043 (assign-stage) and 2365 (general-stage), parent race-close `if first_pid.is_none() { … setpgid(pid,pid) … }` at 2075–2084 (assign-stage) and 2466–2477 (general-stage), and `shell.jobs.add(pgid, spawned_pids, display)` (2505); `cleanup_partial_pipeline_raw` (2521).
- `src/builtins.rs`: `builtin_kill` job-spec arm `killpg(pgid, sig)` (4574, the block 4566–4579).
- `src/shell_state.rs`: `hangup_jobs` `killpg(job.pgid, …)` (2356–2365).

---

### Task 1: Foundation — `Job.own_pgroup` + `JobTable::add_with_pgroup` (inert)

Adds the field (defaulting `true`) and a richer constructor. Behavior-neutral: every existing caller goes through `add` (→ `own_pgroup = true`), and nothing reads the field yet.

**Files:** Modify `src/jobs.rs`.

- [ ] **Step 1: Add the field to `Job`**

In `src/jobs.rs`, in `pub struct Job` (after `pub marked_for_nohup: bool,` at line 36), add:
```rust
    /// True when this job has its OWN process group (`setpgid`'d at spawn —
    /// interactive job control, or any stopped/own-group job). False when the
    /// job shares the shell's process group (a non-interactive background job,
    /// since v173): signal it per-pid, never `killpg`. Bash's `J_JOBCONTROL`.
    pub own_pgroup: bool,
```

- [ ] **Step 2: Default it `true` in `add_synthetic_done`'s initializer**

In `add_synthetic_done` (the `Job { … }` literal at 82–95) add `own_pgroup: true,` after `marked_for_nohup: false,` — a synthetic Done job has no real group, but it is never signalled, so `true` is harmless and keeps the literal valid. (The `add` method's initializer is rewritten in Step 3, which already includes the field — do NOT edit `add`'s literal here.)

- [ ] **Step 3: Split `add` into `add` + `add_with_pgroup`**

Replace the whole `add` method (57–76) with:
```rust
    /// Inserts a new Running job that owns its process group (the common case:
    /// interactive job control). Allocates the lowest unused job id. Returns it.
    pub fn add(&mut self, pgid: i32, pids: Vec<i32>, command: String) -> u32 {
        self.add_with_pgroup(pgid, pids, command, true)
    }

    /// Like `add`, but records whether the job owns its process group. A
    /// non-interactive background job shares the shell's group (`own_pgroup =
    /// false`) and must be signalled per-pid.
    pub fn add_with_pgroup(
        &mut self,
        pgid: i32,
        pids: Vec<i32>,
        command: String,
        own_pgroup: bool,
    ) -> u32 {
        let id = self.next_id();
        let n = pids.len();
        let job = Job {
            id,
            pgid,
            pids,
            reaped: vec![false; n],
            last_status: None,
            command,
            state: JobState::Running,
            notified: false,
            created_at: self.next_created_at,
            marked_for_nohup: false,
            own_pgroup,
        };
        self.next_created_at += 1;
        self.jobs.push(job);
        self.jobs.sort_by_key(|j| j.id);
        id
    }
```

- [ ] **Step 4: Build + jobs tests**

Run: `cargo build 2>&1 | tail -2`
Expected: `Finished` (the `#[derive(Clone)]` on `Job` and all `add` call sites still compile — `add` keeps its signature).
Run: `cargo test --lib jobs 2>&1 | grep -E 'test result'`
Expected: `ok` (existing job-table unit tests unchanged).

- [ ] **Step 5: Commit**

```bash
git add src/jobs.rs
git commit -m "v173 task 1: add Job.own_pgroup + JobTable::add_with_pgroup (inert)

Foundation for L-53: record whether a job has its own process group (true for
all existing callers via add()). add_with_pgroup() lets the background paths
record a group-less job. Nothing reads the field yet.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Background paths inherit the shell's group when job control is off

**Files:** Modify `src/executor.rs`.

- [ ] **Step 1: `run_background_subshell` — gate the fork pgid + record own_pgroup**

In `run_background_subshell` (1950), immediately after `let display = display_command(source);` (line 1956) add:
```rust
    let job_control = shell.job_control_active();
```
Replace the `fork_and_run_in_subshell` 6th argument `/*pgid_target=*/ 0,` (1965) with:
```rust
        /*pgid_target=*/ if job_control { 0 } else { NO_PGROUP },
```
Replace `let id = shell.jobs.add(pid, vec![pid], display);` (1972) with:
```rust
            let id = shell.jobs.add_with_pgroup(pid, vec![pid], display, job_control);
```

- [ ] **Step 2: `run_background_sequence` — compute job_control once**

In `run_background_sequence` (1986), immediately after `let display = display_command(source);` (1992) add:
```rust
    let job_control = shell.job_control_active();
```

- [ ] **Step 3: Switch both per-stage `pgid_target` to NO_PGROUP when off**

At line 2043 (assign-stage) and line 2365 (general-stage), each reads
`let pgid_target = first_pid.unwrap_or(0);`. Replace EACH with:
```rust
        let pgid_target = if job_control { first_pid.unwrap_or(0) } else { NO_PGROUP };
```
(Match each site's indentation — 2043 is more deeply indented than 2365.)

- [ ] **Step 4: Gate the assign-stage parent race-close (2075–2084)**

Replace:
```rust
                    if first_pid.is_none() {
                        first_pid = Some(pid);
                        unsafe {
                            if libc::setpgid(pid, pid) != 0 {
                                let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                                debug_assert!(errno == libc::ESRCH || errno == libc::EACCES,
                                    "setpgid({pid},{pid}) failed errno {errno}");
                            }
                        }
                    }
```
with:
```rust
                    if first_pid.is_none() {
                        first_pid = Some(pid);
                        if job_control {
                            unsafe {
                                if libc::setpgid(pid, pid) != 0 {
                                    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                                    debug_assert!(errno == libc::ESRCH || errno == libc::EACCES,
                                        "setpgid({pid},{pid}) failed errno {errno}");
                                }
                            }
                        }
                    }
```

- [ ] **Step 5: Gate the general-stage parent race-close (2466–2477)**

Replace:
```rust
        if first_pid.is_none() {
            first_pid = Some(pid);
            unsafe {
                if libc::setpgid(pid, pid) != 0 {
                    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                    debug_assert!(
                        errno == libc::ESRCH || errno == libc::EACCES,
                        "setpgid({pid},{pid}) failed errno {errno}"
                    );
                }
            }
        }
```
with:
```rust
        if first_pid.is_none() {
            first_pid = Some(pid);
            if job_control {
                unsafe {
                    if libc::setpgid(pid, pid) != 0 {
                        let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                        debug_assert!(
                            errno == libc::ESRCH || errno == libc::EACCES,
                            "setpgid({pid},{pid}) failed errno {errno}"
                        );
                    }
                }
            }
        }
```

- [ ] **Step 6: Record own_pgroup on the sequence job**

Replace `let id = shell.jobs.add(pgid, spawned_pids, display);` (2505) with:
```rust
    let id = shell.jobs.add_with_pgroup(pgid, spawned_pids, display, job_control);
```

- [ ] **Step 7: Harden `cleanup_partial_pipeline_raw` against the no-group hang**

Replace the body of `cleanup_partial_pipeline_raw` (2521–2531) with:
```rust
fn cleanup_partial_pipeline_raw(pgid: Option<i32>, pids: &[i32]) {
    if let Some(pg) = pgid {
        unsafe {
            libc::killpg(pg, libc::SIGKILL);
        }
    }
    // Also SIGKILL each pid directly. When job control is off the stages share
    // the shell's group (no dedicated group to killpg), so the killpg above is a
    // no-op (ESRCH) and the blocking waitpid below would otherwise hang on a
    // still-running stage. The pids are our direct children, so this is safe and
    // (when a group DOES exist) merely redundant.
    for &pid in pids {
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
    for &pid in pids {
        let mut raw: libc::c_int = 0;
        unsafe { libc::waitpid(pid, &mut raw, 0); }
    }
}
```

- [ ] **Step 8: Build + verify L-53 fixed + interactive unchanged**

Run: `cargo build 2>&1 | tail -2` → `Finished`.
Non-interactive background pipeline + subshell now share the shell's group:
```bash
H=$(pwd)/target/debug/huck
cat > /tmp/bg.sh <<'EOF'
fifo=$(mktemp -u); mkfifo "$fifo"
cat "$fifo" | wc -c &
( exec sleep 5 ) &
sleep 1
echo "shell pgid=$(ps -o pgid= -p $$ | tr -d ' ')"
ps -eo pid,ppid,pgid,comm | awk -v s=$$ '$2==s{print "  "$0}'
kill -9 $(pgrep -P $$) 2>/dev/null; rm -f "$fifo"
EOF
"$H" /tmp/bg.sh; rm -f /tmp/bg.sh
```
Expected: every listed child's `pgid` equals the shell's `pgid` (same group), not a sibling group.
Run: `cargo test --test subshell_job_notice_pty --test pty_interactive 2>&1 | grep 'test result'`
Expected: `ok` (interactive background job notices + job control unchanged).

- [ ] **Step 9: Commit**

```bash
git add src/executor.rs
git commit -m "v173 task 2: background jobs inherit the shell's group when job control is off

run_background_subshell / run_background_sequence now pass NO_PGROUP and skip the
parent race-close setpgid when job_control_active() is false, and record
own_pgroup on the job. Non-interactive a|b& and (cmd)& stages stay in the shell's
process group like bash. cleanup_partial_pipeline_raw now SIGKILLs each pid
directly so its blocking waitpid can't hang when there's no group to killpg.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Signal group-less jobs per-pid (`kill %n`, `hangup_jobs`)

**Files:** Modify `src/builtins.rs`, `src/shell_state.rs`.

- [ ] **Step 1: `kill %n` — per-pid when the job has no group**

In `src/builtins.rs`, the `builtin_kill` job-spec arm (block 4566–4579) currently is:
```rust
            let pgid = match shell.jobs.iter().find(|j| j.id == id) {
                Some(j) => j.pgid,
                None => {
                    eprintln!("huck: kill: {target}: no such job");
                    any_failed = true;
                    continue;
                }
            };
            let rc = unsafe { libc::killpg(pgid, sig) };
            if rc != 0 {
                let errno = std::io::Error::last_os_error();
                eprintln!("huck: kill: ({target}) - {errno}");
                any_failed = true;
            }
```
Replace it with:
```rust
            let (own_pgroup, pgid, pids) = match shell.jobs.iter().find(|j| j.id == id) {
                Some(j) => (j.own_pgroup, j.pgid, j.pids.clone()),
                None => {
                    eprintln!("huck: kill: {target}: no such job");
                    any_failed = true;
                    continue;
                }
            };
            // A job that owns its group is signalled via the group (catches
            // grandchildren); a group-less job (non-interactive background, v173)
            // is signalled per-pid, matching bash's J_JOBCONTROL-unset path.
            let rc = if own_pgroup {
                unsafe { libc::killpg(pgid, sig) }
            } else {
                let mut r = 0;
                for pid in &pids {
                    if unsafe { libc::kill(*pid, sig) } != 0 {
                        r = -1;
                    }
                }
                r
            };
            if rc != 0 {
                let errno = std::io::Error::last_os_error();
                eprintln!("huck: kill: ({target}) - {errno}");
                any_failed = true;
            }
```

- [ ] **Step 2: `hangup_jobs` — per-pid when the job has no group**

In `src/shell_state.rs`, `hangup_jobs` (2356–2365) currently is:
```rust
    pub fn hangup_jobs(&mut self) {
        for job in self.jobs.iter() {
            if !should_hangup(job) {
                continue;
            }
            unsafe {
                libc::killpg(job.pgid, libc::SIGCONT);
                libc::killpg(job.pgid, libc::SIGHUP);
            }
        }
    }
```
Replace the `unsafe { … }` block with a branch on `own_pgroup`:
```rust
    pub fn hangup_jobs(&mut self) {
        for job in self.jobs.iter() {
            if !should_hangup(job) {
                continue;
            }
            if job.own_pgroup {
                unsafe {
                    libc::killpg(job.pgid, libc::SIGCONT);
                    libc::killpg(job.pgid, libc::SIGHUP);
                }
            } else {
                for &pid in &job.pids {
                    unsafe {
                        libc::kill(pid, libc::SIGCONT);
                        libc::kill(pid, libc::SIGHUP);
                    }
                }
            }
        }
    }
```

- [ ] **Step 3: Build + verify `kill %n` works non-interactively**

Run: `cargo build 2>&1 | tail -2` → `Finished`.
Run (compare huck to bash — `kill %1` on a non-interactive running job):
```bash
H=$(pwd)/target/debug/huck
frag='sleep 1 & kill %1; echo "rc=$?"; wait 2>/dev/null; echo done'
echo "--- bash ---"; printf '%s\n' "$frag" | bash --norc --noprofile 2>&1
echo "--- huck ---"; printf '%s\n' "$frag" | "$H" 2>&1
```
Expected: both print `rc=0` then `done` (byte-identical). Before this task huck would print a `kill: … No such process` error because `killpg(first_pid)` hit a non-existent group.

- [ ] **Step 4: Commit**

```bash
git add src/builtins.rs src/shell_state.rs
git commit -m "v173 task 3: signal group-less background jobs per-pid (kill %n, hangup)

kill %n and hangup_jobs now branch on Job.own_pgroup: killpg for jobs that own
their group, per-pid kill otherwise. Keeps kill %n working on non-interactive
background jobs (which no longer have their own group) — matching bash.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Regression tests (PTY/process pgroup + bash-diff harness)

**Files:** Modify `tests/jobcontrol_pgroup_pty.rs`. Create `tests/scripts/bg_jobcontrol_diff_check.sh`.

- [ ] **Step 1: Add non-interactive background pgroup tests**

Append to `tests/jobcontrol_pgroup_pty.rs` (it already has `pgid_of` / `child_pids` helpers and the `use` imports). Add:
```rust
/// L-53: a non-interactive huck running a BACKGROUND pipeline (`a | b &`) keeps
/// its stages in huck's own process group (not a sibling group). huck stays
/// alive via a trailing `sleep` while we inspect.
#[test]
fn noninteractive_background_pipeline_shares_shell_pgroup() {
    let huck = env!("CARGO_BIN_EXE_huck");
    let fifo = std::env::temp_dir().join(format!("v173_bgp_{}", std::process::id()));
    let _ = std::fs::remove_file(&fifo);
    if Command::new("mkfifo").arg(&fifo).status().map(|s| !s.success()).unwrap_or(true) {
        eprintln!("jobcontrol_pgroup_pty: skipping — mkfifo unavailable");
        return;
    }
    let mut child = Command::new(huck)
        .arg("-c")
        .arg(format!("cat {} | wc -c & sleep 3", fifo.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn huck");
    std::thread::sleep(Duration::from_millis(600));
    let huck_pid = child.id();
    let huck_pgid = pgid_of(huck_pid);
    let stages = child_pids(huck_pid);
    let stage_pgids: Vec<(u32, i32)> = stages.iter().map(|&p| (p, pgid_of(p))).collect();

    let _ = std::fs::remove_file(&fifo);
    let _ = child.kill();
    for &p in &stages {
        let _ = Command::new("kill").args(["-9", &p.to_string()]).status();
    }
    let _ = child.wait();

    if stages.is_empty() || huck_pgid < 0 {
        eprintln!("jobcontrol_pgroup_pty: skipping — could not observe stages/pgid");
        return;
    }
    for (p, pg) in stage_pgids {
        assert_eq!(
            pg, huck_pgid,
            "background stage {p} pgid {pg} != huck pgid {huck_pgid} \
             (L-53: backgrounded pipeline stage landed in a sibling group)"
        );
    }
}

/// L-53: a non-interactive huck running `( cmd ) &` keeps the subshell child in
/// huck's own process group.
#[test]
fn noninteractive_background_subshell_shares_shell_pgroup() {
    let huck = env!("CARGO_BIN_EXE_huck");
    let mut child = Command::new(huck)
        .arg("-c")
        .arg("( exec sleep 3 ) & sleep 3")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn huck");
    std::thread::sleep(Duration::from_millis(600));
    let huck_pid = child.id();
    let huck_pgid = pgid_of(huck_pid);
    let kids = child_pids(huck_pid);
    let kid_pgids: Vec<(u32, i32)> = kids.iter().map(|&p| (p, pgid_of(p))).collect();

    let _ = child.kill();
    for &p in &kids {
        let _ = Command::new("kill").args(["-9", &p.to_string()]).status();
    }
    let _ = child.wait();

    if kids.is_empty() || huck_pgid < 0 {
        eprintln!("jobcontrol_pgroup_pty: skipping — could not observe child/pgid");
        return;
    }
    for (p, pg) in kid_pgids {
        assert_eq!(
            pg, huck_pgid,
            "background subshell child {p} pgid {pg} != huck pgid {huck_pgid} \
             (L-53: (cmd)& child landed in a sibling group)"
        );
    }
}
```

- [ ] **Step 2: Run the extended PTY/process test**

Run: `cargo test --test jobcontrol_pgroup_pty 2>&1 | grep -E 'test result|FAILED'`
Expected: `ok` (now 4 tests — the two v172 ones plus the two new background ones; or fewer if any cleanly skips on no `ps`/`pgrep`).

- [ ] **Step 3: Create the bash-diff harness**

Create `tests/scripts/bg_jobcontrol_diff_check.sh` (mode `+x`):
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v173 (L-53): non-interactive background
# job control. Stages of a backgrounded job share the shell's process group, but
# kill %n / wait / $! must still work. We assert OBSERVABLE behavior (exit codes
# + stdout) since process-group ids are not byte-stable.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "bgpid nonempty"     'sleep 0.2 & [ -n "$!" ] && echo "have-pid"; wait; echo done'
check "kill %1 running"    'sleep 1 & kill %1; echo "rc=$?"; wait 2>/dev/null; echo done'
check "wait %1 reaps"      'sleep 0.2 & wait %1; echo "rc=$?"'
check "wait \$! exit code" 'sh -c "exit 7" & wait $!; echo "rc=$?"'
check "two bg wait all"    'sleep 0.2 & sleep 0.2 & wait; echo "all done rc=$?"'
check "no job notice"      'sleep 0.2 & wait; echo only-this-line'
check "kill bad spec"      'kill %9 2>&1; echo "rc=$?"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 4: Make it executable and run it**

Run: `chmod +x tests/scripts/bg_jobcontrol_diff_check.sh && bash tests/scripts/bg_jobcontrol_diff_check.sh`
Expected: `Total: 7, Pass: 7, Fail: 0`.
If any case FAILs because of a genuine pre-existing huck/bash divergence unrelated to L-53 (e.g. a `kill` diagnostic-wording difference), adjust THAT fragment to target the L-53 behavior precisely (per-pid signalling success) rather than the unrelated text — do not weaken the pgroup assertions.

- [ ] **Step 5: Commit**

```bash
git add tests/jobcontrol_pgroup_pty.rs tests/scripts/bg_jobcontrol_diff_check.sh
git commit -m "test: v173 background pgroup (L-53) — process tests + bash-diff harness

Two non-interactive tests assert a|b& and (cmd)& stages share huck's own process
group; bg_jobcontrol_diff_check.sh asserts kill %n / wait / \$! observable
behavior matches bash on non-interactive background jobs.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Full regression + resolve L-53 doc

**Files:** Modify `docs/bash-divergences.md`.

- [ ] **Step 1: Full suite + clippy + all harnesses**

Run: `cargo clippy --lib --bins --quiet 2>&1 | grep -E 'warning|error' || echo CLEAN` → `CLEAN`.
Run: `cargo test >/tmp/v173.log 2>&1; echo "exit: $?"; grep -cE 'test result: FAILED' /tmp/v173.log` → `exit: 0`, `0`.
Run: `p=0; f=0; for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 && p=$((p+1)) || { f=$((f+1)); echo "FAIL $s"; }; done; echo "$p passed, $f failed"` → `0 failed` (count is now 94 with the new harness).

- [ ] **Step 2: Delete the L-53 entry + fix the Tier-4 count**

In `docs/bash-divergences.md`, remove the entire `- **L-53: background (\`&\`) pipelines/subshells get their own process group when job control is OFF** …` bullet (line ~190). Decrement the Tier-4 summary count in the table (line 33) from `40` to `39` (L-53 removed, nothing added).

- [ ] **Step 3: Commit**

```bash
git add docs/bash-divergences.md
git commit -m "docs: resolve L-53 (background process group now matches bash)

v173 makes non-interactive background pipelines/subshells inherit the shell's
process group (NO_PGROUP + job_control_active()), with per-pid signalling for
group-less jobs. Tier-4 count 40 -> 39.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final review (orchestrator, after all tasks)

- Whole-branch diff: `jobs.rs` (field + add_with_pgroup), `executor.rs` (job_control + NO_PGROUP at the 4 background sites + 2 gated race-closes + add_with_pgroup + cleanup per-pid kill), `builtins.rs` (kill %n per-pid), `shell_state.rs` (hangup per-pid), the two new tests + harness, `bash-divergences.md`. Confirm interactive behavior is unchanged by inspection (the `if job_control` blocks still fire when interactive).
- Re-run the FULL PTY suite (`cargo test --test '*_pty'` or each) — the deadlock-regression guard — and the L-53 reproduction by hand (background pipeline + subshell share the shell's pgid).
- Manually drive an interactive huck over a real terminal: `sleep 30 &` → `jobs` shows it, `kill %1` reaps it, `fg`/`bg` on a Ctrl-Z'd job still work; confirm no wedge.
- Merge `v173-bg-jobcontrol-pgroup` to main `--no-ff` after user confirmation (AskUserQuestion); push; delete the branch.
- Record in `project_huck_iterations.md` + `MEMORY.md`. L-53 was the last open process-group follow-on from v172.

---

## Self-review (plan vs spec)

- **Spec coverage:** background `setpgid` gating + NO_PGROUP for both functions (Task 2 steps 1–6) ✓; `Job.own_pgroup` + `add_with_pgroup` (Task 1) ✓; `kill %n` per-pid (Task 3 step 1) ✓; `cleanup_partial_pipeline_raw` per-pid kill (Task 2 step 7) ✓; `hangup_jobs` per-pid (Task 3 step 2) ✓; PTY/process pgroup tests + bash-diff harness (Task 4) ✓; full regression + L-53 removal, count 40→39 (Task 5) ✓. `wait`/`fg`/`bg` correctly untouched (spec: per-pid / interactive-only — verified, no task needed).
- **Placeholder scan:** none — exact line numbers + full before/after for every edit; exact verification commands with expected output.
- **Type consistency:** `own_pgroup: bool` used identically in the struct, both initializers, `add_with_pgroup`, the `kill`/`hangup` reads; `add_with_pgroup(pgid, pids, command, own_pgroup)` signature matches all three call sites (subshell, sequence, and `add`'s delegation); `NO_PGROUP` and `job_control_active()` reused verbatim from v172.
