# v124 — Interactive Subshell tty-deadlock + Builtin `>&N` stdout redirect Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a real `~/.bashrc`'s `nvm ls` work — fix the interactive foreground-subshell tty deadlock (Bug A) and make builtins honor a `>&N` stdout redirect (Bug B).

**Architecture:** Two localized fixes in `src/executor.rs`. Bug A: the standalone `Command::Subshell` execution arm gains an interactive branch that hands the terminal to the subshell's process group and waits with `WUNTRACED` (mirroring the existing single-command/pipeline job-control dance), then reclaims the terminal. Bug B: a small helper resolves a stdout `Dup` (`>&N`) into a `File` so the two in-process builtin branches route the builtin's stdout to fd N (symmetric to the existing `2>&1`-on-builtin handling). No AST/lexer/parser changes.

**Tech Stack:** Rust; `libc` (`dup`, `setpgid`, `tcsetpgrp`, `waitpid` `WUNTRACED`, `WSTOPSIG`/`WEXITSTATUS`/`WTERMSIG`); `expectrl` (PTY tests, dev-dep).

Spec: `docs/superpowers/specs/2026-06-09-subshell-tty-builtin-dup-design.md`.

**Conventions:**
- Build/test with `cargo build` (debug); harness uses `target/debug/huck`.
- Commit trailer EXACTLY (keep the "(1M context)" parenthetical):
  `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
- Bash-diff harness fragments run as FILE-ARG scripts (L-27).
- Branch: `v124-subshell-tty-builtin-dup` (create from `main` before Task 1).

---

## File Structure

| File | Responsibility | Tasks |
|------|----------------|-------|
| `src/executor.rs` | Fix B helper `builtin_stdout_dup_file` + wire into both builtin branches (Task 1); Fix A interactive subshell branch (Task 2) | 1, 2 |
| `tests/builtin_stdout_dup_integration.rs` | NEW — `>&N`-on-builtin vs bash | 1 |
| `tests/scripts/builtin_stdout_dup_diff_check.sh` | NEW — 47th bash-diff harness | 1 |
| `tests/subshell_tty_pty.rs` | NEW — PTY regression: interactive subshell pipeline must not hang | 2 |
| `README.md` | harness count 46 → 47 | 3 |

---

### Task 1: Fix B — builtins honor `>&N` stdout redirect

**Files:**
- Modify: `src/executor.rs` (new helper near `resolve_fd_target` ~`:2056`; wire into control-builtin branch ~`:2801-2843` and regular-builtin branch ~`:2846-2913`)
- Create: `tests/builtin_stdout_dup_integration.rs`
- Create: `tests/scripts/builtin_stdout_dup_diff_check.sh`

Context: a stdout `>&N` parses to `Redirect::Dup { fd:1, source:Word(N) }` and goes in the `cmd.stdout` slot. `open_stage_files` resolves it to `files.stdout = None` (executor.rs:2188). External commands honor it via pre_exec `dup2`; builtins do not — they write to the `StdoutSink` (capture buffer or real stdout). This task adds the missing builtin handling. `resolve_fd_target(source, shell) -> Result<i32, io::Error>` already exists (executor.rs:2056) and expands+parses the source word to an fd number.

- [ ] **Step 1: Write the failing integration test**

Create `tests/builtin_stdout_dup_integration.rs`. Copy the binary-invocation helper idiom from an existing integration test (e.g. `tests/bash_rematch_integration.rs`): write the fragment to a temp file and run `env!("CARGO_BIN_EXE_huck")` with that file as the arg, capturing stdout/stderr/exit. Name the helper `run_huck_frag(frag: &str) -> (String, String, i32)`.

```rust
//! v124 Fix B: builtins must honor a `>&N` stdout redirect (M-? — undocumented).
//! A builtin's `>&2` must go to fd 2 (not stdout). File-arg execution (L-27).

use std::io::Write;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);

fn run_huck_frag(frag: &str) -> (String, String, i32) {
    let dir = std::env::temp_dir();
    let path = dir.join(format!(
        "huck_v124_{}_{}.sh",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let mut f = std::fs::File::create(&path).expect("create temp script");
    f.write_all(frag.as_bytes()).expect("write temp script");
    drop(f);
    let out = Command::new(env!("CARGO_BIN_EXE_huck"))
        .arg(&path)
        .output()
        .expect("run huck");
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn echo_to_stderr_not_captured_on_stdout() {
    // `>&2` on a builtin must NOT land on stdout.
    let (out, _err, _c) = run_huck_frag(r#"a=$(echo Z >&2); echo "[$a]""#);
    assert_eq!(out.trim_end(), "[]", "stdout capture must be empty, got {out:?}");
}

#[test]
fn printf_to_stderr_not_captured() {
    let (out, _e, _c) = run_huck_frag(r#"a=$(printf '%s\n' Z >&2); echo "[$a]""#);
    assert_eq!(out.trim_end(), "[]", "{out:?}");
}

#[test]
fn echo_redirect_to_2_reaches_stderr() {
    // `>&2` text must appear on stderr.
    let (_o, err, _c) = run_huck_frag(r#"echo HELLO >&2"#);
    assert!(err.contains("HELLO"), "stderr must contain HELLO, got {err:?}");
}

#[test]
fn echo_ampersand1_still_stdout() {
    // `>&1` is a no-op: builtin output stays on stdout.
    let (out, _e, _c) = run_huck_frag(r#"echo KEEP >&1"#);
    assert!(out.contains("KEEP"), "{out:?}");
}

#[test]
fn func_err_to_stderr_suppressed_by_caller_redirect() {
    // The nvm pattern: a function whose body does `>&2 builtin`, called with
    // 2>/dev/null and captured, must capture empty.
    let (out, _e, _c) = run_huck_frag(
        r#"f() { >&2 printf '%s\n' MSG; }; a=$( (f 2>/dev/null) ); echo "[$a]""#,
    );
    assert_eq!(out.trim_end(), "[]", "{out:?}");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --test builtin_stdout_dup_integration 2>&1 | tail -20`
Expected: `echo_to_stderr_not_captured_on_stdout`, `printf_to_stderr_not_captured`, `func_err_to_stderr_suppressed_by_caller_redirect` FAIL (capture is `[Z]`/`[MSG]`, not `[]`). The `>&1` and reaches-stderr tests may already pass.

- [ ] **Step 3: Add the `builtin_stdout_dup_file` helper**

In `src/executor.rs`, add a free function next to `resolve_fd_target` (~`:2056`):

```rust
/// For an in-process builtin whose stdout has a `>&N` (`Redirect::Dup`)
/// redirect, returns the `File` the builtin should write to:
///   - not a Dup, or `>&1` (source == 1)  -> `Ok(None)` (use the normal sink)
///   - `>&-` (source expands to "-")        -> `Ok(Some(/dev/null))` (discard)
///   - `>&N`, N != 1                         -> `Ok(Some(File))` dup'd from fd N
/// On a bad fd target, prints `huck: <err>` and returns `Err(())` (status 1),
/// mirroring the external path's `resolve_fd_target` error handling.
fn builtin_stdout_dup_file(cmd: &ExecCommand, shell: &mut Shell) -> Result<Option<File>, ()> {
    let source = match &cmd.stdout {
        Some(Redirect::Dup { source, .. }) => source,
        _ => return Ok(None),
    };
    // `>&-` close form: the expanded source is "-".
    let expanded = expand_assignment(source, shell);
    if expanded == "-" {
        return match OpenOptions::new().write(true).open("/dev/null") {
            Ok(f) => Ok(Some(f)),
            Err(e) => { eprintln!("huck: /dev/null: {e}"); Err(()) }
        };
    }
    let target = match resolve_fd_target(source, shell) {
        Ok(fd) => fd,
        Err(e) => { eprintln!("huck: {e}"); return Err(()); }
    };
    if target == 1 {
        // `>&1`: builtin's stdout already is fd 1 / the sink — no-op.
        return Ok(None);
    }
    // Dup the target fd into an owned File (closes its own copy on drop;
    // does NOT close the real fd `target`).
    let dup_fd = unsafe { libc::dup(target) };
    if dup_fd < 0 {
        let e = io::Error::last_os_error();
        eprintln!("huck: {target}: {e}");
        return Err(());
    }
    use std::os::unix::io::FromRawFd;
    Ok(Some(unsafe { File::from_raw_fd(dup_fd) }))
}
```

(`expand_assignment`, `resolve_fd_target`, `OpenOptions`, `File`, `io`, `Redirect`, `ExecCommand` are all already in scope in this module.)

- [ ] **Step 4: Wire into the regular-builtin branch (~`:2846`)**

In `run_exec_single`, the regular-builtin branch begins:
```rust
    } else if builtins::is_builtin(&resolved.program) {
        let mut files = match open_stage_files(&resolved, shell) {
            Ok(f) => f,
            Err(()) => { … return ExecOutcome::Continue(1); }
        };
```
Immediately AFTER that `let mut files = …;` block (before `prepare_builtin_stdin`/`dup_target`), insert:
```rust
        // `>&N` on a builtin's stdout: open_stage_files leaves files.stdout
        // None for a Dup; resolve it to the target fd so the builtin writes
        // there (symmetric to the 2>&1-on-builtin handling below).
        if files.stdout.is_none() {
            match builtin_stdout_dup_file(cmd, shell) {
                Ok(f) => files.stdout = f,
                Err(()) => {
                    if !persistent { restore_inline_assignments(snap, shell); }
                    return ExecOutcome::Continue(1);
                }
            }
        }
```

- [ ] **Step 5: Wire into the control-builtin branch (~`:2801`)**

The control-builtin branch begins:
```rust
    let outcome = if is_control_builtin(&resolved.program) {
        let mut files = match open_stage_files(&resolved, shell) {
            Ok(f) => f,
            Err(()) => { return ExecOutcome::Continue(1); }
        };
```
Immediately AFTER that `let mut files = …;` block (before the `dup_target` computation), insert the same fill — but control builtins always persist inline assignments (no `restore_inline_assignments` on the error path):
```rust
        if files.stdout.is_none() {
            match builtin_stdout_dup_file(cmd, shell) {
                Ok(f) => files.stdout = f,
                Err(()) => return ExecOutcome::Continue(1),
            }
        }
```

- [ ] **Step 6: Build + run the integration tests**

Run: `cargo build 2>&1 | tail -3 && cargo test --test builtin_stdout_dup_integration 2>&1 | tail -12`
Expected: all 5 pass. If a borrow-checker error arises because `cmd` is borrowed while `shell` is mutably borrowed, note `builtin_stdout_dup_file(cmd, shell)` takes `&ExecCommand` + `&mut Shell`; `cmd` is the function's `&ExecCommand` param and is not aliased by `shell`, so this composes — match the call style of the neighboring `stderr_dups_to_stdout(cmd, shell)` call.

- [ ] **Step 7: Add a unit test for the helper**

Add to the `#[cfg(test)] mod tests` in `src/executor.rs`:
```rust
#[test]
fn builtin_stdout_dup_file_none_when_no_dup() {
    // A command with no stdout redirect -> Ok(None).
    let mut shell = Shell::new();
    let cmd = ExecCommand { // construct a minimal ExecCommand with stdout: None
        ..ExecCommand::default()
    };
    assert!(matches!(builtin_stdout_dup_file(&cmd, &mut shell), Ok(None)));
}
```
If `ExecCommand` has no `Default`/is verbose to construct, SKIP this unit test and rely on the integration tests (note the skip in the commit message). Do not fabricate a constructor.

- [ ] **Step 8: Add the 47th bash-diff harness**

Create `tests/scripts/builtin_stdout_dup_diff_check.sh` (model on `tests/scripts/bash_rematch_diff_check.sh`):
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v124 Fix B: builtins honor a `>&N`
# stdout redirect. File-arg execution (L-27; avoids huck's history-expansion
# on piped stdin).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h tf
    tf=$(mktemp)
    printf '%s\n' "$frag" > "$tf"
    b=$(bash --norc --noprofile "$tf" 2>/dev/null; echo "EXIT:$?")   # stdout only
    h=$("$HUCK_BIN" "$tf" 2>/dev/null; echo "EXIT:$?")
    rm -f "$tf"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "echo>&2 captured empty"  'a=$(echo Z >&2); echo "[$a]"'
check "printf>&2 captured empty" 'a=$(printf "%s\n" Z >&2); echo "[$a]"'
check "echo>&1 stays stdout"     'echo KEEP >&1'
check "func >&2 under 2>/dev/null" 'f() { >&2 printf "%s\n" MSG; }; a=$( (f 2>/dev/null) ); echo "[$a]"'
check "echo>&- discards"         'a=$(echo GONE >&-); echo "[$a]"'
check "two builtins one >&2"     'a=$( { echo A; echo B >&2; } ); echo "[$a]"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```
Note: the harness compares **stdout only** (`2>/dev/null` on both) so the `huck:`/`bash:` error-prefix divergence is irrelevant; the "reaches stderr" direction is covered by the Rust integration test.

- [ ] **Step 9: Run the harness**

Run: `chmod +x tests/scripts/builtin_stdout_dup_diff_check.sh && cargo build 2>&1 | tail -1 && ./tests/scripts/builtin_stdout_dup_diff_check.sh`
Expected: `Total: 6, Pass: 6, Fail: 0`. If `echo>&- discards` differs (bash may emit a "write error: Bad file descriptor" to stderr and exit non-zero — already filtered by `2>/dev/null`, but the EXIT code may differ), and you cannot make it byte-identical, replace that fragment's assertion approach or drop just that one case and note it; do NOT weaken the others.

- [ ] **Step 10: Commit**

```bash
git add src/executor.rs tests/builtin_stdout_dup_integration.rs tests/scripts/builtin_stdout_dup_diff_check.sh
git commit -m "feat(v124): builtins honor >&N stdout redirect (fixes nvm alias ->∞)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Fix A — interactive foreground subshell is a proper job

**Files:**
- Modify: `src/executor.rs` (the `Command::Subshell` arm, ~`:305-374`)
- Create: `tests/subshell_tty_pty.rs`

Context: the subshell arm forks the body with `pgid_target = 0` (child does `setpgid(0,0)` → its own pgroup) but never hands it the terminal and waits with plain `waitpid(pid, 0)`. Interactively, the background subshell pgroup is stopped and the parent never wakes → deadlock. Template to mirror: the single-command interactive dance at `executor.rs:3060-3113`. Helpers: `wait_with_untraced(pid) -> Result<(c_int, bool), ()>` (`:4308`), `give_terminal_to(pgid)` (`:4302`, no-op without a tty), `shell.jobs.add(pgid, pids, command) -> u32` (`jobs.rs:57`), `crate::jobs::notification_line(job, flag) -> String` (`jobs.rs:346`), `JobState::Stopped(i32)` (`jobs.rs:13`), `shell.set_pipestatus(&[i32])` (`shell_state.rs:837`).

- [ ] **Step 1: Write the failing PTY regression test**

Create `tests/subshell_tty_pty.rs` (model on `tests/completion_jobcontrol_pty.rs`):
```rust
//! v124 Fix A: an interactive foreground subshell whose body is a pipeline with
//! large output must NOT deadlock. Pre-fix, `( command ls -1qA /usr/bin |
//! grep -q . )` hung: the subshell ran in a background process group that was
//! never handed the terminal, and the parent waited without WUNTRACED. This is
//! nvm's `nvm.sh:1485` form. Skips (passes) if no PTY can be allocated.

use std::process::Command;
use std::time::Duration;

use expectrl::session::OsSession;
use expectrl::Expect;

#[test]
fn interactive_subshell_pipeline_does_not_hang() {
    let cmd = Command::new(env!("CARGO_BIN_EXE_huck"));
    let mut session = match OsSession::spawn(cmd) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("subshell_tty_pty: skipping — no PTY: {e}");
            return;
        }
    };
    session.set_expect_timeout(Some(Duration::from_secs(8)));

    // Warm up: confirm the shell is alive and reads commands.
    let _ = session.send("echo WARM_$((6*7))");
    let _ = session.send("\r");
    assert!(
        session.expect("WARM_42").is_ok(),
        "shell did not start / read a command"
    );

    // The repro: a subshell-wrapped pipeline whose first stage fills the pipe.
    // Pre-fix this wedges the shell; post-fix it returns and the sentinel comes.
    let _ = session.send("( command ls -1qA /usr/bin | grep -q . ); echo SUB_$((7*8))");
    let _ = session.send("\r");
    let ok = session.expect("SUB_56").is_ok();

    drop(session);
    assert!(
        ok,
        "interactive subshell pipeline hung (Fix A): shell unresponsive after `( ls | grep -q . )`"
    );
}
```

- [ ] **Step 2: Run to verify it fails (hangs → timeout → failed expect)**

Run: `cargo test --test subshell_tty_pty 2>&1 | tail -20`
Expected: FAIL (the `SUB_56` expect times out after 8s because the subshell deadlocks) — OR skips if no PTY in the build environment. If it SKIPS (no PTY), note that and rely on manual PTY verification in Step 6; the fix is still required.

- [ ] **Step 3: Add the `interactive` gate at the top of the Subshell arm**

In `src/executor.rs`, the arm is `Command::Subshell { .. } => { … }` (~`:305`). Right after the opening `{`, before the `(stdout_fd, capture_read_fd)` match, add:
```rust
            let interactive = matches!(sink, StdoutSink::Terminal)
                && !shell.in_subshell
                && !shell.in_completion;
```

- [ ] **Step 4: Replace the wait block with an interactive/plain branch**

The current tail of the arm (after the capture-drain `io::copy`, ~`:359-374`) is:
```rust
            // Wait for the child.
            let mut raw_status: libc::c_int = 0;
            let r = unsafe { libc::waitpid(pid, &mut raw_status, 0) };
            if r < 0 {
                return ExecOutcome::Continue(1);
            }
            let code = if libc::WIFEXITED(raw_status) {
                libc::WEXITSTATUS(raw_status)
            } else if libc::WIFSIGNALED(raw_status) {
                128 + libc::WTERMSIG(raw_status)
            } else {
                1
            };
            // A subshell is one forked unit → 1-element PIPESTATUS.
            shell.set_pipestatus(&[code]);
            ExecOutcome::Continue(code)
```
Replace that block with:
```rust
            if interactive {
                // Foreground subshell: make it a job that owns the terminal,
                // mirroring the single-command/pipeline dance. Without this the
                // subshell runs in a background pgroup and deadlocks on tty I/O.
                unsafe {
                    if libc::setpgid(pid, pid) != 0 {
                        let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
                        debug_assert!(
                            errno == libc::ESRCH || errno == libc::EACCES,
                            "setpgid({pid}, {pid}) failed with unexpected errno {errno}"
                        );
                    }
                }
                give_terminal_to(pid);
                let outcome = match wait_with_untraced(pid) {
                    Ok((raw_status, true)) => {
                        // Subshell stopped (e.g. Ctrl-Z).
                        let sig = libc::WSTOPSIG(raw_status);
                        let job_id = shell.jobs.add(pid, vec![pid], "( subshell )".to_string());
                        for job in shell.jobs.jobs_mut() {
                            if job.id == job_id {
                                job.state = crate::jobs::JobState::Stopped(sig);
                                job.notified = true;
                                break;
                            }
                        }
                        let line = shell.jobs.iter()
                            .find(|j| j.id == job_id)
                            .map(|j| crate::jobs::notification_line(j, '+'))
                            .unwrap_or_default();
                        eprintln!("\n{line}");
                        128 + sig
                    }
                    Ok((raw_status, false)) => {
                        if libc::WIFEXITED(raw_status) {
                            libc::WEXITSTATUS(raw_status)
                        } else if libc::WIFSIGNALED(raw_status) {
                            128 + libc::WTERMSIG(raw_status)
                        } else {
                            1
                        }
                    }
                    Err(()) => 1,
                };
                give_terminal_to(shell.shell_pgid);
                shell.set_pipestatus(&[outcome]);
                ExecOutcome::Continue(outcome)
            } else {
                // Non-interactive (script), capture (`$( ( … ) )`), nested
                // subshell, or completion: plain reap, no terminal handoff.
                let mut raw_status: libc::c_int = 0;
                let r = unsafe { libc::waitpid(pid, &mut raw_status, 0) };
                if r < 0 {
                    return ExecOutcome::Continue(1);
                }
                let code = if libc::WIFEXITED(raw_status) {
                    libc::WEXITSTATUS(raw_status)
                } else if libc::WIFSIGNALED(raw_status) {
                    128 + libc::WTERMSIG(raw_status)
                } else {
                    1
                };
                shell.set_pipestatus(&[code]);
                ExecOutcome::Continue(code)
            }
```
(Confirm `shell.jobs.jobs_mut()` and `shell.jobs.iter()` exist by matching the single-command path's usage at `executor.rs:3079-3088`. Use the same method names verbatim.)

- [ ] **Step 5: Build + run the PTY test + full regression**

Run: `cargo build 2>&1 | tail -3`
Run: `cargo test --test subshell_tty_pty 2>&1 | tail -12`
Expected: PASS (or skip if no PTY).
Run the existing job-control suites — they MUST stay green:
`cargo test --test pty_interactive --test subshell_pipeline_pty --test completion_jobcontrol_pty 2>&1 | tail -20`

- [ ] **Step 6: Manual PTY verification (no regression on scripts; hang gone)**

Non-interactive must be unchanged:
```bash
printf '( command ls -1qA /usr/bin | cat ) >/dev/null; echo "DONE rc=$?"\n' > /tmp/v124_ni.sh
timeout 6 ./target/debug/huck /tmp/v124_ni.sh   # expect: DONE rc=0
```
Interactive (reuse the project's python PTY approach if `expectrl` skipped): drive `( command ls -1qA /usr/bin | grep -q . )` and confirm a prompt returns. Report before/after.

- [ ] **Step 7: Commit**

```bash
git add src/executor.rs tests/subshell_tty_pty.rs
git commit -m "fix(v124): interactive foreground subshell owns the terminal (fixes hang)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Docs + nvm ls payoff verification

**Files:**
- Modify: `README.md` (harness count)
- Verify: `~/.nvm/nvm.sh` payoff (no source of `~/.bashrc` — creds)

- [ ] **Step 1: Bump the harness count in the README**

In `README.md`, find "**46 bash-diff harnesses**" and change to "**47 bash-diff harnesses**". Leave the approximate test count (`~2,900 tests`) as-is.

- [ ] **Step 2: Verify the nvm ls payoff (non-interactive correctness for Fix B)**

Run:
```bash
printf '. "$HOME/.nvm/nvm.sh"\nnvm alias 2>/dev/null | sed -n "1,3p"\n' > /tmp/v124_nvm.sh
timeout 30 ./target/debug/huck /tmp/v124_nvm.sh
```
Expected: alias lines now show real resolved versions (e.g. `default -> lts/* (-> v24.16.0)`), NO `-> ∞`. (Fix B.) Capture the output for the commit/PR notes.

- [ ] **Step 3: Verify the nvm ls payoff (interactive, no hang — Fix A)**

Using the project's PTY approach (expectrl test from Task 2, or a one-off python pty harness), source `~/.nvm/nvm.sh` and run `nvm ls`; confirm it returns to a prompt (does not hang) and the alias section shows real versions. Honest report: this closes the user's `nvm ls` hang; note any residual slowness (nvm forks heavily) is expected, not a hang.

- [ ] **Step 4: Decide on a divergence-doc entry**

Both bugs were undocumented in `docs/bash-divergences.md`. They are now fixed, so no new entry is needed (the doc tracks CURRENT divergences only). IF the nvm payoff reveals a NEW residual gap (e.g. some other nvm construct still misbehaves), add a `[deferred]` entry for that specific gap with severity. Otherwise leave the doc unchanged. Do NOT add `[fixed]` entries.

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs(v124): bump harness count to 47

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Final verification (after all tasks)

- [ ] `cargo build 2>&1 | tail -3` — clean.
- [ ] `cargo test 2>&1 | grep -E "test result: FAILED|error\[" | head` — none.
- [ ] `cargo clippy --all-targets 2>&1 | tail -5` — no warnings.
- [ ] `for h in tests/scripts/*_diff_check.sh; do bash "$h" >/dev/null 2>&1 || echo "FAIL $h"; done` — all 47 pass (silent = all pass).
- [ ] The job-control PTY suites (`pty_interactive`, `subshell_pipeline_pty`, `completion_jobcontrol_pty`, `subshell_tty_pty`) green.
- [ ] `nvm ls` interactive: no hang, no `→ ∞`.

## Self-review notes (plan author)
- **Spec coverage:** Fix A → Task 2 (interactive branch mirroring the 3060-3113 dance, gated `Terminal && !in_subshell && !in_completion`, reclaim terminal, stopped-job registration, capture/script branch unchanged). Fix B → Task 1 (`builtin_stdout_dup_file` + both builtin-branch wire-ins, `>&1`/`>&-`/`>&N`/bad-fd). Tests: PTY (Task 2), integration + 47th harness + unit (Task 1). Docs + payoff → Task 3.
- **Type consistency:** `builtin_stdout_dup_file(&ExecCommand, &mut Shell) -> Result<Option<File>, ()>`; `wait_with_untraced(pid) -> Result<(libc::c_int, bool), ()>`; `shell.jobs.add(pid, vec![pid], String) -> u32`; `JobState::Stopped(i32)`; `set_pipestatus(&[i32])` — all match the existing signatures cited.
- **Order:** Fix B first (no PTY, fully unit/integration testable), then Fix A (PTY), then docs/payoff — so a PTY-less CI still validates B fully and A skips gracefully.
- **Zero-regression hinges:** Fix A `else` branch is the verbatim old wait block (scripts/captures unchanged); `give_terminal_to` is a no-op without a tty. Fix B only changes the builtin path when `cmd.stdout` is a `Dup`; external commands and non-Dup builtins are untouched.
