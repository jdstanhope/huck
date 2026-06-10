# v128 — Suppress Job Notices in a Subshell Environment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Suppress automatic `&` job-control notices (`[N] pid` start, `[N]- Done … &`) when running inside a subshell environment — matching bash — so `nvm ls` (whose alias loops are `( … & … wait ) | sort`) is byte-clean. Top-level interactive `&` still notifies.

**Architecture:** Three sites emit automatic background-job notices, each gated only on `shell.is_interactive`. Add `&& !shell.in_subshell && !shell.in_completion` to each (the v108/v121 flag pattern; both fields already exist). User-invoked `jobs`/`bg`/`fg` and Ctrl-Z stopped-job notices are untouched.

**Tech Stack:** Rust; `expectrl` (PTY tests, dev-dep). Notices are interactive-only → PTY verification.

Spec: `docs/superpowers/specs/2026-06-10-subshell-job-notice-suppression-design.md`.

**Conventions:**
- Build/test: `cargo build`/`cargo build --release`; `cargo test`; `cargo clippy --all-targets`.
- Commit trailer EXACTLY (keep "(1M context)"): `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
- **GIT SAFETY**: do NOT `git checkout <sha>` / detach HEAD (a prior iteration lost commits that way). Stay on the branch; inspect with `git show`/`git diff`.
- Branch: `v128-subshell-job-notice-suppression` (from `main` before Task 1).

---

## File Structure

| File | Responsibility | Tasks |
|------|----------------|-------|
| `src/executor.rs` | gate the two `&` start-notice sites (`:1501`, `:2026`) | 1 |
| `src/jobs.rs` | gate the `reap_and_notify` done-notice (`:320`) | 1 |
| `tests/subshell_job_notice_pty.rs` | NEW — PTY regression | 1 |
| `docs/bash-divergences.md` | resolve/delete L-28 | 2 |

---

### Task 1: Gate the three notice sites + PTY regression test

**Files:**
- Modify: `src/executor.rs` (`:1501`, `:2026`)
- Modify: `src/jobs.rs` (`:320`, in `reap_and_notify`)
- Create: `tests/subshell_job_notice_pty.rs`

- [ ] **Step 1: Write the failing PTY regression test**

Create `tests/subshell_job_notice_pty.rs` (mirror `tests/completion_jobcontrol_pty.rs` / `tests/subshell_tty_pty.rs` for the `expectrl::session::OsSession` idiom). Notices are interactive-only, so this drives huck under a real PTY. It strips ANSI, then counts lines that look like a job notice (`[<digit>…`):

```rust
//! v128: automatic `&` job notices (`[N] pid`, `[N]- Done … &`) must be
//! SUPPRESSED inside a subshell environment (matching bash), but STILL printed
//! for a top-level `&`. nvm's alias loops are `( … & … wait ) | sort`, so the
//! notices were polluting `nvm ls`. Skips (passes) if no PTY is available.

use std::process::Command;
use std::time::Duration;

use expectrl::session::OsSession;
use expectrl::Expect;

/// Spawn huck under a PTY, send `cmd` then a sentinel, return the captured
/// output (ANSI already present; caller greps). Returns None if no PTY.
fn run_in_pty(cmd: &str) -> Option<String> {
    let mut session = match OsSession::spawn(Command::new(env!("CARGO_BIN_EXE_huck"))) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("subshell_job_notice_pty: skipping — no PTY: {e}");
            return None;
        }
    };
    session.set_expect_timeout(Some(Duration::from_secs(8)));
    // settle the first prompt
    let _ = session.send("echo READY_$((6*7))");
    let _ = session.send("\r");
    let _ = session.expect("READY_42");
    // the command under test, then a sentinel
    let _ = session.send(cmd);
    let _ = session.send("; echo MK_$((7*8))\r");
    let got = session.expect("MK_56");
    let buf = match got {
        Ok(found) => {
            // everything captured up to and including the match
            String::from_utf8_lossy(found.before()).into_owned()
        }
        Err(_) => String::new(),
    };
    drop(session);
    Some(buf)
}

/// Count lines that look like an automatic job notice: `[<n>] pid` or `[<n>]<flag> Done …`.
fn job_notice_lines(out: &str) -> usize {
    let ansi = regex_lite_strip(out);
    ansi.lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with('[') && t[1..].chars().next().is_some_and(|c| c.is_ascii_digit())
        })
        .count()
}

// Minimal ANSI/CSI stripper (avoid adding a dep; handle ESC[…<letter>).
fn regex_lite_strip(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // consume CSI: ESC [ ... <final byte 0x40-0x7e>
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&n) = chars.peek() {
                    chars.next();
                    if ('@'..='~').contains(&n) { break; }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[test]
fn subshell_background_job_emits_no_notice() {
    let Some(out) = run_in_pty("( sleep 0.05 & wait )") else { return };
    assert_eq!(job_notice_lines(&out), 0, "subshell `&` must not notify; got:\n{out}");
}

#[test]
fn subshell_pipeline_background_job_emits_no_notice() {
    let Some(out) = run_in_pty("( sleep 0.05 & wait ) | cat") else { return };
    assert_eq!(job_notice_lines(&out), 0, "subshell|pipe `&` must not notify; got:\n{out}");
}

#[test]
fn top_level_background_job_still_notifies() {
    // Must-not-regress: a top-level `&` STILL prints a `[N] pid` start notice.
    let Some(out) = run_in_pty("sleep 0.05 & wait") else { return };
    assert!(job_notice_lines(&out) >= 1, "top-level `&` must still notify; got:\n{out}");
}
```
(If the project already has a shared ANSI-strip/PTY helper used by the other `*_pty.rs` tests, reuse it instead of `regex_lite_strip`. Match the existing `expectrl` API — check `completion_jobcontrol_pty.rs` for `.before()`/`expect` usage and adapt if the version differs.)

- [ ] **Step 2: Run the test to verify it fails (or skips)**

Run: `cargo test --test subshell_job_notice_pty 2>&1 | tail -20`
Expected: `subshell_background_job_emits_no_notice` + `subshell_pipeline_background_job_emits_no_notice` FAIL (huck currently prints notices in subshells) — OR all skip if no PTY in the build env. `top_level_background_job_still_notifies` should pass already. If they SKIP (no PTY), rely on the manual PTY check in Step 6.

- [ ] **Step 3: Gate the two start-notice sites (`src/executor.rs`)**

At `:1501` — single `cmd &` start notice. Current:
```rust
            if shell.is_interactive {
                eprintln!("[{id}] {pid}");
            }
```
Change the condition to:
```rust
            if shell.is_interactive && !shell.in_subshell && !shell.in_completion {
                eprintln!("[{id}] {pid}");
            }
```

At `:2026` — group/pipeline `&` start notice. Current:
```rust
    if shell.is_interactive {
        eprintln!("[{id}] {last_pid}");
    }
```
Change to:
```rust
    if shell.is_interactive && !shell.in_subshell && !shell.in_completion {
        eprintln!("[{id}] {last_pid}");
    }
```

- [ ] **Step 4: Gate the done-notice site (`src/jobs.rs:320`, in `reap_and_notify`)**

Current:
```rust
        if shell.is_interactive {
            eprintln!("{}", notification_line(&job, flag));
        }
```
Change to:
```rust
        if shell.is_interactive && !shell.in_subshell && !shell.in_completion {
            eprintln!("{}", notification_line(&job, flag));
        }
```
Leave the surrounding `reap_completed` / `drain_notifications` / `remove_notified` UNCHANGED — only the `eprintln!` is gated, so a subshell still reaps + drops its jobs silently (no stale-job accumulation).

- [ ] **Step 5: Build + run the PTY test + full regression**

Run: `cargo build 2>&1 | tail -3` (clean).
Run: `cargo test --test subshell_job_notice_pty 2>&1 | tail -12` (the two subshell tests now pass; top-level still passes; or all skip without a PTY).
Run the existing job-control PTY suites — they MUST stay green (top-level job control, Ctrl-Z, fg/bg unaffected):
`cargo test --test pty_interactive --test subshell_pipeline_pty --test completion_jobcontrol_pty --test subshell_tty_pty 2>&1 | tail -20`
Run: `cargo test 2>&1 | grep -E "test result: FAILED|error\[" | head` (none).

- [ ] **Step 6: Check the `$()`-backgrounded edge (note only)**

Determine whether `run_substitution`/`execute_capturing` (`src/expand.rs`, `src/executor.rs`) sets `shell.in_subshell` for command substitution. Quick check: grep `in_subshell` in `expand.rs`/`run_substitution`. If a `$( cmd & )` would still leak a notice (because `$()` doesn't set `in_subshell`), that's an out-of-scope residual — note it in the commit message; do NOT expand scope (nvm uses the `( ) | sort` subshell, which DOES set `in_subshell`, so the payoff holds). No code change needed here.

- [ ] **Step 7: Manual PTY confirmation (subshell silent; top-level notifies)**

```bash
cargo build --release 2>&1 | tail -1
python3 - <<'PY'
import os, pty, select, time, re
def run(cmd):
    pid, fd = pty.fork()
    if pid == 0:
        os.environ["PS1"]="> "; os.execv("/home/john/projects/shuck/target/release/huck",["huck"]); os._exit(127)
    def drain(t):
        b=b""; e=time.time()+t
        while time.time()<e:
            r,_,_=select.select([fd],[],[],0.2)
            if r:
                try: d=os.read(fd,4096)
                except OSError: break
                if not d: break
                b+=d
        return b.decode(errors="replace")
    time.sleep(0.5); drain(0.8)
    os.write(fd,(cmd+"; echo MK\n").encode()); out=drain(2.0)
    os.write(fd,b"\x03"); time.sleep(0.1)
    try: os.close(fd); os.kill(pid,9); os.waitpid(pid,0)
    except OSError: pass
    out=re.sub(r'\x1b\[[0-9;?]*[a-zA-Z]','',out)
    return sum(1 for l in out.splitlines() if l.strip().startswith("[") and l.strip()[1:2].isdigit())
print("( sleep 0.05 & wait )      :", run("( sleep 0.05 & wait )"), "notices (expect 0)")
print("( sleep 0.05 & wait ) | cat:", run("( sleep 0.05 & wait ) | cat"), "notices (expect 0)")
print("sleep 0.05 & wait (top)    :", run("sleep 0.05 & wait"), "notices (expect >=1)")
PY
```
Expected: 0, 0, >=1. Investigate any mismatch.

- [ ] **Step 8: clippy + commit**

`cargo clippy --all-targets 2>&1 | tail -5` (clean).
```bash
git add src/executor.rs src/jobs.rs tests/subshell_job_notice_pty.rs
git commit -m "fix(v128): suppress job notices in a subshell environment (L-28 noise)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Docs (resolve L-28) + nvm payoff

**Files:**
- Modify: `docs/bash-divergences.md`

- [ ] **Step 1: nvm ls payoff (PTY — the headline: 36 notices → 0)**

```bash
cargo build --release 2>&1 | tail -1
python3 - <<'PY'
import os, pty, select, time, re
pid, fd = pty.fork()
if pid == 0:
    os.environ["PS1"]="> "; os.execv("/home/john/projects/shuck/target/release/huck",["huck"]); os._exit(127)
def drain(t):
    b=b""; e=time.time()+t
    while time.time()<e:
        r,_,_=select.select([fd],[],[],0.2)
        if r:
            try: d=os.read(fd,4096)
            except OSError: break
            if not d: break
            b+=d
    return b.decode(errors="replace")
time.sleep(0.5); drain(0.8)
os.write(fd, b'. "$HOME/.nvm/nvm.sh"; echo SRC\n'); drain(6)
os.write(fd, b'nvm ls; echo LS_DONE\n'); out=drain(20)
os.write(fd,b"\x03"); time.sleep(0.1)
try: os.close(fd); os.kill(pid,9); os.waitpid(pid,0)
except OSError: pass
out=re.sub(r'\x1b\[[0-9;?]*[a-zA-Z]','',out)
notes=sum(1 for l in out.splitlines() if l.strip().startswith("[") and l.strip()[1:2].isdigit())
infs=sum(1 for l in out.splitlines() if "∞" in l)
print("nvm ls job-notice lines:", notes, "(expect 0; was 36)")
print("nvm ls ∞-dup lines:", infs, "(expect 0)")
PY
```
Do NOT source `~/.bashrc` (creds). Expected: 0 notices, 0 dups. If notices remain, report BLOCKED with output.

- [ ] **Step 2: Resolve L-28 in the divergences doc**

Read the L-28 entry in `docs/bash-divergences.md` (Tier 4). It described two symptoms: (a) the `[N]`/Done job-notice noise (fixed by this v128), and (b) the alias-line duplication (fixed by v127). Both are now resolved → **DELETE the entire L-28 entry**. In the Summary table, decrement the Low-impact (Tier 4) count by 1 (read the current value — likely 25 → 24). Verify: `grep -n "L-28" docs/bash-divergences.md` returns nothing.

- [ ] **Step 3: Full regression sanity**

```bash
cargo test 2>&1 | grep -E "test result: FAILED|error\[" | head    # none
cargo clippy --all-targets 2>&1 | tail -3                          # clean
for h in tests/scripts/*_diff_check.sh; do bash "$h" >/dev/null 2>&1 || echo "FAIL $h"; done  # silent
```

- [ ] **Step 4: Commit**

```bash
git add docs/bash-divergences.md
git commit -m "docs(v128): resolve L-28 (job-notice noise fixed; dup fixed in v127)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Final verification (after all tasks)

- [ ] `cargo build` + `cargo clippy --all-targets` clean.
- [ ] `cargo test 2>&1 | grep -E "test result: FAILED|error\["` → none.
- [ ] all bash-diff harnesses pass; the 4 job-control PTY suites green.
- [ ] PTY: subshell `&` → 0 notices; top-level `&` → notice present; `nvm ls` → 0 notices, 0 dups.
- [ ] L-28 removed from the divergences doc (Tier-4 count decremented).

## Self-review notes (plan author)
- **Spec coverage:** the 3 gate changes → Task 1 Steps 3-4; PTY regression (subshell silent + top-level notifies) → Task 1 Steps 1/5/7; the `$()`-edge note → Step 6; nvm payoff → Task 2 Step 1; L-28 resolution → Task 2 Step 2; regression incl. job-control PTY suites → Task 1 Step 5 + Final.
- **Type/symbol consistency:** the gate `shell.is_interactive && !shell.in_subshell && !shell.in_completion` is identical at all 3 sites; `in_subshell`/`in_completion` are existing `pub bool` fields (shell_state.rs:295/302).
- **Risk hinge:** must-not-regress the top-level `&` notice (in_subshell=false → still prints) — explicitly tested (`top_level_background_job_still_notifies` + manual Step 7) — and the user-invoked `jobs`/`bg`/`fg` + Ctrl-Z notices are NOT among the 3 gated sites.
