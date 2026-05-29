# huck v44 — `disown` accepts bare PID Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close bash divergence M-44 by making `disown` accept a bare
PID (e.g. `disown 12345`) in addition to the existing `%spec` form.
The PID matches any pid in any tracked job's `pids` list; the
operation acts on the whole job.

**Architecture:** Single-file change in `src/builtins.rs`. Replace
the v43 positional-loop arm that rejected non-`%` args with one that
parses each non-`%` arg as a positive `i32` and looks up the
matching job via `shell.jobs.iter().find(|j| j.pids.contains(&pid))`.
Everything else in `builtin_disown` (flag parser, job-set selection,
`-r` filter, mark/remove action) is unchanged.

**Tech Stack:** Rust. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-29-huck-disown-bare-pid-design.md`

**Branch:** `v44-disown-bare-pid` (created in preamble step P.1).

**Commit trailer convention**:

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble: Create the working branch

- [ ] **Step P.1: Create branch from main and check it out**

```bash
git checkout main
git pull --ff-only
git checkout -b v44-disown-bare-pid
```

Expected: `Switched to a new branch 'v44-disown-bare-pid'`.

The spec + this plan are committed as the first commit on this
branch (handled by the controller before Task 1 begins).

---

## Task 1: Builtin change + unit tests

**Files:**
- Modify: `src/builtins.rs` — replace the positional-loop arm in
  `builtin_disown` (currently `src/builtins.rs:993-1005`); append 4
  new unit tests in `mod disown_tests`.

### Step 1.1: Replace the positional-loop arm

In `src/builtins.rs::builtin_disown`, find the `else if
!positional.is_empty() {` arm (currently at line 993). Inside that
arm is a `for arg in positional { ... }` loop. The CURRENT loop body
is:

```rust
        for arg in positional {
            if !arg.starts_with('%') {
                eprintln!("huck: disown: {arg}: not a valid job spec");
                return ExecOutcome::Continue(1);
            }
            match resolve_spec_or_error(arg, "disown", shell) {
                Ok(id) => ids.push(id),
                Err(outcome) => return outcome,
            }
        }
```

Replace it with:

```rust
        for arg in positional {
            if arg.starts_with('%') {
                match resolve_spec_or_error(arg, "disown", shell) {
                    Ok(id) => ids.push(id),
                    Err(outcome) => return outcome,
                }
            } else {
                match arg.parse::<i32>() {
                    Ok(pid) if pid > 0 => {
                        match shell.jobs.iter().find(|j| j.pids.contains(&pid)) {
                            Some(job) => ids.push(job.id),
                            None => {
                                eprintln!("huck: disown: {arg}: no such job");
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    _ => {
                        eprintln!("huck: disown: {arg}: not a valid job spec");
                        return ExecOutcome::Continue(1);
                    }
                }
            }
        }
```

- [ ] **Step 1.1: Apply the replacement**

### Step 1.2: Build

Run: `cargo build`
Expected: clean.

- [ ] **Step 1.2: Build clean**

### Step 1.3: Run existing disown tests for regression

Run: `cargo test disown_ -- --nocapture`
Expected: all prior `disown_*` tests still pass. The only path that
changed is the non-`%` positional branch; `%spec`, `-a`, `-r`, `-h`,
combined forms, and current-job semantics all flow through unchanged
code.

If `disown_with_non_percent_arg_returns_status_1` still asserts
status 1 for `disown 1` (a bare integer): it should still pass
because `1` doesn't match any job's `pids` list in an empty job
table (the test was constructed with `let mut shell = Shell::new();`
with no jobs), so the new code returns "no such job" status 1 —
same status, different message. Verify by reading the test.

If the existing test instead used a non-numeric value like `foo`,
the new code falls through to "not a valid job spec" status 1 —
also same status. Either way, the test should still pass.

- [ ] **Step 1.3: Existing tests still green**

### Step 1.4: Add the 4 new unit tests

Find `mod disown_tests` in `src/builtins.rs` (search for `mod
disown_tests`). Append these tests inside the mod block, before its
closing `}`. The mod already has `use super::*;` and `use
crate::shell_state::Shell;` so `run_builtin`, `ExecOutcome`, `Shell`
are in scope:

```rust
    #[test]
    fn disown_bare_pid_matches_job_leader() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "sleep".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["1234".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.jobs.iter().count(), 0);
    }

    #[test]
    fn disown_bare_pid_matches_pipeline_stage() {
        let mut shell = Shell::new();
        // Pipeline with three stages; pgid is the leader (1234), but
        // we'll disown by referencing the middle stage's pid (1235).
        shell.jobs.add(1234, vec![1234, 1235, 1236], "a | b | c".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["1235".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        // The whole job is removed, not just the matching stage.
        assert_eq!(shell.jobs.iter().count(), 0);
    }

    #[test]
    fn disown_unknown_pid_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["99999".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn disown_h_with_bare_pid_marks_job() {
        let mut shell = Shell::new();
        let id = shell.jobs.add(1234, vec![1234], "sleep".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "disown",
            &["-h".to_string(), "1234".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let job = shell.jobs.iter().find(|j| j.id == id).expect("job removed!");
        assert!(job.marked_for_nohup);
    }
```

- [ ] **Step 1.4: Append the 4 tests**

### Step 1.5: Run the new tests

Run: `cargo test disown_bare_pid_ disown_unknown_pid_ disown_h_with_bare_pid_ -- --nocapture`
Expected: all 4 new tests pass.

- [ ] **Step 1.5: New tests pass**

### Step 1.6: Run ALL disown tests for regression

Run: `cargo test disown_ -- --nocapture`
Expected: all tests pass (v43's tests + the 4 new ones).

- [ ] **Step 1.6: All disown tests green**

### Step 1.7: Full unit suite

Run: `cargo test --bin huck`
Expected: all unit tests pass.

- [ ] **Step 1.7: Full unit suite passes**

### Step 1.8: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 1.8: Clippy clean**

### Step 1.9: Commit

```bash
git add src/builtins.rs
git commit -m "$(cat <<'EOF'
builtin: disown accepts bare PID (v44 task 1)

Replace the v43 positional-loop arm in builtin_disown that rejected
any non-% arg with one that parses each non-% positional as a
positive i32 and looks up the matching job via
shell.jobs.iter().find(|j| j.pids.contains(&pid)). Operates on the
whole job once a match is found — matches against any pid in the
job's pids list, including non-leader pipeline stages
(bash-faithful).

Error paths:
- Unknown PID → "huck: disown: <arg>: no such job" + status 1
- Unparseable / non-positive → "huck: disown: <arg>: not a valid
  job spec" + status 1

The %spec path, flag parser, -a/-r/-h dispatch, and -r retain
filter are unchanged.

4 new unit tests cover bare-PID match against the job leader,
match against a middle pipeline stage (whole job removed), unknown
PID error, and -h + bare PID interaction (job stays, marked).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 1.9: Commit Task 1**

---

## Task 2: Integration test

**Files:**
- Create: `tests/disown_pid_integration.rs`

One binary-driven test verifying real SIGHUP-survival behavior via
bare PID, mirroring v43's `disown_h_lets_bg_job_survive` but using
`$!` (last bg PID) instead of `%1`.

### Step 2.1: Create the integration test file

Create `tests/disown_pid_integration.rs` with this exact content:

```rust
use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn pid_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

fn cleanup_kill(pid: i32) {
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
}

fn first_pid(s: &str) -> Option<i32> {
    for word in s.split_whitespace() {
        if let Ok(n) = word.parse::<i32>() {
            if n > 0 {
                return Some(n);
            }
        }
    }
    None
}

#[test]
fn disown_h_with_bare_pid_lets_bg_survive() {
    // sleep redirects stdio so wait_with_output() returns promptly
    // when huck exits. `echo $!` captures the bg PID (huck's `jobs
    // -p` is M-45 deferred). `disown -h $!` uses the bare-PID path
    // we just added.
    let script = "sleep 30 >/dev/null 2>&1 &\necho $!\ndisown -h $!\nexit\n";
    let (out, _) = run_capture(script);
    let pid = first_pid(&out).unwrap_or_else(|| panic!("no pid found in: {:?}", out));
    thread::sleep(Duration::from_millis(200));
    let alive = pid_alive(pid);
    cleanup_kill(pid);
    assert!(alive, "bg job (pid {pid}) was killed despite disown -h <pid>");
}
```

`libc` is already in `[dev-dependencies]` from v41 — no Cargo.toml
change needed.

- [ ] **Step 2.1: Create the integration test file**

### Step 2.2: Run the new integration test

Run: `cargo test --test disown_pid_integration -- --nocapture`
Expected: 1 test passes.

If it fails because the bg PID is dead at the 200ms probe: that's a
real bug — either Task 1's bare-PID path isn't reaching the job, or
the script-level `disown -h $!` isn't resolving correctly. Do NOT
relax the assertion; investigate the dispatcher.

- [ ] **Step 2.2: Test passes**

### Step 2.3: Full integration suite

Run: `cargo test --tests`
Expected: all integration tests pass. Known PTY flake
`pty_compound_stage_pipeline_stops_and_resumes` may flake under
load; re-run in isolation if hit.

- [ ] **Step 2.3: Full integration suite green**

### Step 2.4: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 2.4: Clippy clean**

### Step 2.5: Commit

```bash
git add tests/disown_pid_integration.rs
git commit -m "$(cat <<'EOF'
test: disown -h with bare PID integration coverage (v44 task 2)

One binary-driven test verifying the new bare-PID path end-to-end.
Script spawns `sleep 30 >/dev/null 2>&1 &`, captures the bg PID via
`echo $!`, calls `disown -h $!` (the bare PID form added in
v44 task 1), then exits. Asserts the sleep process is still alive
200ms after huck exits, then SIGTERM cleans it up.

The stdio redirect on the bg sleep is required so that
wait_with_output() returns promptly when huck exits (without it,
the test would block until sleep finishes naturally).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2.5: Commit Task 2**

---

## Task 3: Docs

**Files:**
- Modify: `docs/bash-divergences.md` — flip M-44, change-log entry.
- Modify: `README.md` — v44 row.

### Step 3.1: Flip M-44 in `docs/bash-divergences.md`

Find the M-44 entry in the Job-control section. Currently:

```markdown
- **M-44: `disown` accepts bare PID** — `[deferred]` low. huck: requires `%spec`. bash: accepts PIDs.
```

Replace with:

```markdown
- **M-44: `disown` accepts bare PID** — `[fixed v44]` low. `disown 12345` now resolves the PID against every tracked job's `pids` list (including non-leader pipeline stages) and operates on the matching job as a whole. Existing `%spec` path unchanged. Unknown PIDs error with "no such job" + status 1.
```

- [ ] **Step 3.1: Flip M-44**

### Step 3.2: Add v44 change-log entry

In `docs/bash-divergences.md`, find `## Change log` and the most
recent `**2026-05-2X**` entry (v43, M-43 — dated 2026-05-28). Add
IMMEDIATELY after it:

```markdown
- **2026-05-29**: M-44 (`disown` accepts bare PID) shipped as v44. One-arm change in `builtin_disown`'s positional loop: non-`%` args now parse as positive `i32` and look up the matching job via `shell.jobs.iter().find(|j| j.pids.contains(&pid))`. Match scope includes non-leader pipeline stages; the whole job is operated on. No new L-* divergences.
```

- [ ] **Step 3.2: Add change-log entry**

### Step 3.3: Add v44 row to README

In `README.md`, find the version table. After the v43 row (search
for `| v43       |`), add IMMEDIATELY after it:

```markdown
| v44       | `disown` accepts bare PID (M-44)                               |
```

Final block:

```markdown
| v42       | `kill -s SIGNAME` + `kill -n SIGNUM` (M-40)                    |
| v43       | `disown -a`/`-r`/`-h` + SIGHUP-on-exit (M-43)                  |
| v44       | `disown` accepts bare PID (M-44)                               |
```

Match column padding (count trailing spaces in the actual file
before the closing `|` so the right pipe aligns with v42/v43).

- [ ] **Step 3.3: Add README v44 row**

### Step 3.4: Run the full suite

Run: `cargo test --all-targets`
Expected: all tests pass (modulo known PTY flake).

- [ ] **Step 3.4: Full suite green**

### Step 3.5: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 3.5: Clippy clean**

### Step 3.6: Commit

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: mark M-44 fixed; v44 in README

Job-control section: M-44 (`disown` accepts bare PID) flipped from
[deferred] to [fixed v44] with descriptive text covering the
pids-list match scope (non-leader stages included), the
whole-job-operated-on rule, and the "no such job" error path.

Change log: 2026-05-29 v44 entry summarizing the one-arm
positional-loop change.

README: v44 row added to the version table.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3.6: Commit Task 3**

---

## Final verification (controller, not a task)

After the three task commits land:

1. Run `cargo test --all-targets` once more.
2. Run `cargo clippy --all-targets -- -D warnings`.
3. Confirm the branch has exactly four commits ahead of `main`:
   docs preamble (spec + plan), task 1, task 2, task 3.
4. Dispatch a final cross-task code-reviewer subagent over the full
   diff (`main..v44-disown-bare-pid`).
5. Merge to `main` with `--no-ff`, push, delete the branch, update
   the `huck iterations` memory with v44.
