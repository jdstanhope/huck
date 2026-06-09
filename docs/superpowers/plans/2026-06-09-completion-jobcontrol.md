# v121 — suppress job control during completion-function invocation (M-116) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop interactive TAB completion from hanging when a completion function runs an external command/pipeline (e.g. bash-completion's `_longopt`: `ls --help | while read …`), by running completion functions without job control — as bash does.

**Architecture:** A transient `Shell.in_completion` flag, set for the dynamic extent of a completion-function call (`call_completion_function`), gates the two job-control decisions (`run_multi_stage` and `run_subprocess`) so a completer's subprocesses run foreground in huck's own process group (no `setpgid`/`give_terminal_to` mid-line-edit). Mirrors v108's `in_subshell` fix, applied to the completion path.

**Tech Stack:** Rust. `src/shell_state.rs`, `src/completion_spec.rs`, `src/executor.rs`. Tests: a pty regression test via `expectrl` (modeled on `tests/subshell_pipeline_pty.rs`).

**Spec:** `docs/superpowers/specs/2026-06-09-completion-jobcontrol-design.md`. Read it first.

**Commit trailer (MANDATORY, canonical — every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

Anchors (verify exact lines — code shifts):
- `Shell` struct field `pub in_subshell: bool` (`src/shell_state.rs:281`) + its init `in_subshell: false,` (`:389`).
- `call_completion_function` step 5 (`src/completion_spec.rs:294`): `let _outcome = crate::executor::call_function_body(func_name, pos_args, shell);`.
- `run_multi_stage` gate (`src/executor.rs:3125`): `let interactive = matches!(sink, StdoutSink::Terminal) && !shell.in_subshell;`.
- `run_subprocess` gate (`src/executor.rs:2897`): `let interactive = matches!(sink, StdoutSink::Terminal);`.
- pty harness model: `tests/subshell_pipeline_pty.rs` (uses `expectrl::session::OsSession`, `set_expect_timeout`, `.send()`, `.expect()`, skips on no-PTY).

---

## Task 1: the fix + pty regression test

**Files:**
- Modify: `src/shell_state.rs`, `src/completion_spec.rs`, `src/executor.rs`
- Create: `tests/completion_jobcontrol_pty.rs`

- [ ] **Step 1: Write the failing pty regression test**

Create `tests/completion_jobcontrol_pty.rs` (modeled on `tests/subshell_pipeline_pty.rs`). It registers a completer whose body runs an **external-producer pipeline** (the confirmed-hanging construct), triggers TAB completion, then checks the shell is still responsive via a sentinel:
```rust
//! PTY regression test for M-116: interactive TAB completion must NOT hang when
//! a completion function runs an external command/pipeline (bash-completion's
//! `_longopt` does `cmd --help | while read …`). huck used to run completer
//! subprocesses with job control (setpgid + give_terminal_to) mid-line-edit,
//! which wedged the shell. Spawns huck under a real PTY with a hard per-read
//! timeout: a hang => the sentinel never arrives => the test FAILS (instead of
//! wedging the suite). Skips (passes) if no PTY can be allocated.

use std::process::Command;
use std::time::Duration;
use expectrl::session::OsSession;
use expectrl::Expect;

#[test]
fn external_pipeline_completer_does_not_hang() {
    let cmd = Command::new(env!("CARGO_BIN_EXE_huck"));
    let mut session = match OsSession::spawn(cmd) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("completion_jobcontrol_pty: skipping — no PTY: {e}");
            return;
        }
    };
    session.set_expect_timeout(Some(Duration::from_secs(8)));

    // Helper: send a line + CR.
    macro_rules! line { ($s:expr) => {{ let _ = session.send($s); let _ = session.send("\r"); }}; }

    // 1. Register a completer whose body runs an EXTERNAL-producer pipeline.
    line!("ec(){ COMPREPLY=($(ls --help 2>&1 | while read -r l; do printf '%s\\n' \"$l\"; done | head -3)); }");
    line!("complete -F ec ecmd");
    // Sentinel proving the setup lines were processed (shell is alive here).
    line!("echo SETUP_OK_$((6*7))");
    assert!(session.expect("SETUP_OK_42").is_ok(), "setup never completed (shell dead before TAB)");

    // 2. Trigger completion: type 'ecmd ' then TAB (no CR). Pre-fix this hangs.
    let _ = session.send("ecmd \t");
    // 3. Clear the line (Ctrl-U) and run a fresh sentinel command. If TAB hung,
    //    these are never processed and the expect() below times out => FAIL.
    let _ = session.send("\x15");           // Ctrl-U: clear line
    let _ = session.send("echo TAB_DONE_$((7*8))\r");
    let responsive = session.expect("TAB_DONE_56").is_ok();

    // Dropping `session` kills any wedged child.
    drop(session);
    assert!(responsive, "TAB completion hung: shell unresponsive after invoking an external-pipeline completer (M-116)");
}
```
(If `expectrl`'s API differs slightly from `subshell_pipeline_pty.rs` — method names / `send` taking `&str` — mirror that file EXACTLY; it compiles against the project's `expectrl` version.)

- [ ] **Step 2: Run the test — confirm it FAILS (hangs → timeout) on the unfixed binary**

Run: `cargo test --test completion_jobcontrol_pty 2>&1 | tail -15`
Expected: FAIL with the "TAB completion hung" assertion (the `TAB_DONE_56` expect times out after 8s) — proving the test reproduces the bug. (If it SKIPS due to no PTY in the sandbox, note that and rely on manual verification in Step 7; the test still guards real environments.)

- [ ] **Step 3: Add `Shell.in_completion`**

In `src/shell_state.rs`, after the `in_subshell` field (`:281`), add:
```rust
    /// Set for the dynamic extent of a completion-function invocation
    /// (`call_completion_function`). Suppresses interactive job control for the
    /// completer's subprocesses/pipelines so they don't `setpgid` / hand the
    /// controlling terminal to a new process group mid-line-edit (that wedges
    /// the shell — M-116). bash runs completion functions without job control.
    pub in_completion: bool,
```
And in the initializer (beside `in_subshell: false,` at `:389`): add `in_completion: false,`.

- [ ] **Step 4: Set `in_completion` around the completer call (`src/completion_spec.rs`)**

In `call_completion_function`, replace step 5:
```rust
    let _outcome = crate::executor::call_function_body(func_name, pos_args, shell);
```
with:
```rust
    // Run the completer WITHOUT job control: a completer's external commands /
    // pipelines must not setpgid / hand off the controlling terminal while huck
    // is mid-line-edit (that deadlocks — M-116). bash runs completion functions
    // without job control.
    let saved_in_completion = shell.in_completion;
    shell.in_completion = true;
    let _outcome = crate::executor::call_function_body(func_name, pos_args, shell);
    shell.in_completion = saved_in_completion;
```

- [ ] **Step 5: Gate the two job-control sites (`src/executor.rs`)**

`run_multi_stage` (`:3125`): change
```rust
    let interactive = matches!(sink, StdoutSink::Terminal) && !shell.in_subshell;
```
to
```rust
    let interactive = matches!(sink, StdoutSink::Terminal) && !shell.in_subshell && !shell.in_completion;
```
`run_subprocess` (`:2897`): change
```rust
    let interactive = matches!(sink, StdoutSink::Terminal);
```
to
```rust
    let interactive = matches!(sink, StdoutSink::Terminal) && !shell.in_completion;
```
(Leave the `2>&1` dup-target `matches!(sink, StdoutSink::Terminal)` checks at `:2744`/`:2801` UNCHANGED — they are not job-control.)

- [ ] **Step 6: Run the test — confirm it PASSES**

Run: `cargo build --bin huck && cargo test --test completion_jobcontrol_pty 2>&1 | tail -10`
Expected: PASS (`TAB_DONE_56` arrives well within the timeout — the completer ran, no hang). If it skipped (no PTY), proceed to Step 7 for manual verification.

- [ ] **Step 7: Manual payoff verification (the real bash-completion case)**

```bash
cargo build --release 2>&1 | tail -1
python3 - <<'PY'
import os, pty, select, time
BIN=os.path.abspath("target/release/huck")
pid,fd=pty.fork()
if pid==0:
    os.environ["PS1"]="HK> "; os.execv(BIN,[BIN]); os._exit(127)
def drain(t):
    b=b""; e=time.time()+t
    while time.time()<e:
        r,_,_=select.select([fd],[],[],0.3)
        if r:
            try: d=os.read(fd,8192)
            except OSError: break
            if not d: break
            b+=d
    return b
def send(s): os.write(fd,s.encode())
time.sleep(0.5); drain(1.0)
send("source /usr/share/bash-completion/bash_completion\n"); drain(3.0)
send("ls -\t"); out=drain(6.0)
# After the fix, completion should produce output (option list / redisplay),
# NOT spin. Heuristic: huck redraws the line or offers options within the window.
send("\x15"); send("echo PAYOFF_OK\n"); resp=drain(4.0)
print("PAYOFF:", "OK (responsive after ls -<TAB>)" if b"PAYOFF_OK" in resp else "STILL HANGS")
os.write(fd,b"\nexit\n")
try: os.close(fd)
except OSError: pass
PY
```
Expected: `PAYOFF: OK (responsive after ls -<TAB>)`. Report the result.

- [ ] **Step 8: Job-control non-regression + full suite + clippy**

```bash
cargo test --test pty_interactive 2>&1 | tail -8        # Ctrl-Z / fg / bg / handoff must stay green
cargo test --test subshell_pipeline_pty 2>&1 | tail -5  # v108 must stay green
cargo test 2>&1 | grep -E "test result: FAILED" || echo "no failures"
cargo clippy --all-targets 2>&1 | tail -3
```
Expected: pty_interactive + subshell_pipeline_pty green; no FAILED; clippy clean. (If pty tests SKIP in the sandbox, note it.)

- [ ] **Step 9: Commit**

```bash
git add src/shell_state.rs src/completion_spec.rs src/executor.rs tests/completion_jobcontrol_pty.rs
git commit -m "$(cat <<'EOF'
fix: run completion functions without job control (M-116)

Interactive TAB completion hung whenever a completer ran an external command/
pipeline (bash-completion's _longopt: `cmd --help | while read …`): the completer
ran with a Terminal sink, so run_multi_stage/run_subprocess took the job-control
path (setpgid + give_terminal_to) mid-line-edit and wedged the shell (the v108
class). New Shell.in_completion flag, set around the call_function_body call in
call_completion_function, gates both job-control decisions so completer
subprocesses run foreground in huck's pgroup — as bash does. PTY regression test.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1 report
DONE/BLOCKED, commit SHA, the 3 source edits (flag + wrap + 2 gates), the pty test fail→pass transition (or skip note + manual payoff result), the pty_interactive/subshell_pipeline_pty non-regression lines, full-suite green, clippy status.

---

## Task 2: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Read the structures**

```bash
grep -n 'Last updated:\|Bugs (Tier 1) |\|^## Change log\|### M-115:\|### M-104:' docs/bash-divergences.md | head
grep -n '| v120 ' README.md
cargo test 2>&1 | awk '/test result:/{s+=$4} END{print "TESTCOUNT="s}'
```
Use the real TESTCOUNT for `<N>`.

- [ ] **Step 2: Add the M-116 entry (Tier 1)**

In `docs/bash-divergences.md` Tier-1 section (after the last `### M-11x:` entry), add a `### M-116:` entry `[fixed v121]` (high): interactive TAB completion hung whenever a completion function ran an external command / pipeline. Root cause: `call_completion_function` invoked the completer via `call_function_body` (Terminal sink), so `run_multi_stage`/`run_subprocess` took the interactive job-control path (`setpgid` + `give_terminal_to`/`tcsetpgrp`) while huck was mid-line-edit in raw mode → terminal-handoff wedge (the v108/M-104 class, triggered via the completion path rather than an explicit subshell; bash-completion's `_longopt` `cmd --help | while read …` is the trigger, so `ls<TAB>`/`grep<TAB>`/etc. hung the shell; builtin-producer completers dodged it). Fix (v121): new `Shell.in_completion`, set for the dynamic extent of the completer call, gating both job-control decisions (`run_multi_stage:` `&& !in_completion`; `run_subprocess:` `&& !in_completion`) so completer subprocesses run foreground in huck's process group, EOF'ing pipes normally — matching bash, which never job-controls completion functions. PTY regression test (`completion_jobcontrol_pty.rs`). Note the relationship to M-104 (same job-control-on-a-controlling-terminal hazard). Bump the Tier-1 count.

- [ ] **Step 3: Tier-1 count + Last-updated**

- `| Bugs (Tier 1) | <count> |`: bump by 1 (M-116, fixed). Append to the notes: `; M-116 interactive-completion job-control hang fixed v121`.
- "Last updated" → v121 (M-116 — completion functions no longer hang when they run an external command/pipeline; the v108-class job-control hazard, via the completion path).

- [ ] **Step 4: Change-log entry + README row**

Append a `2026-06-09` v121 change-log entry (root cause + the `in_completion` gate + the `_longopt` trigger + the M-104 relationship + the pty regression test + `<N>` tests). Add a v121 README row after v120 (the hang, the fix, "PTY regression test; full suite `<N>` tests pass, clippy clean"). Be honest: this fixes the hang; `mise<TAB>` candidate output is still gated by the separate 2.11-vs-2.12 bash-completion API mismatch.

- [ ] **Step 5: Verify + commit**

```bash
grep -n 'M-116\|fixed v121\|v121' docs/bash-divergences.md README.md | head
grep -n '<N>\|<count>' docs/bash-divergences.md README.md && echo "PLACEHOLDER LEFT" || echo "no placeholders"
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: v121 — suppress job control during completion (M-116 fixed)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2 report
DONE/BLOCKED, commit SHA, the grep proving M-116 `[fixed v121]`, no placeholder, the test count used.

---

## Final (after all tasks)
- [ ] Whole-branch review: `git log --oneline main..HEAD`, `git diff --stat main..HEAD`.
- [ ] `cargo test 2>&1 | grep -cE 'test result: ok'` (green, no FAILED), `cargo clippy --all-targets 2>&1 | tail -2` (clean).
- [ ] pty suites green (or skip-noted): `completion_jobcontrol_pty`, `pty_interactive`, `subshell_pipeline_pty`.
- [ ] **Payoff**: `ls -<TAB>` after sourcing bash-completion is responsive (no hang) — Task 1 Step 7.
- [ ] AskUserQuestion merge gate, then `git merge --no-ff` + push + delete branch, then update memory files (`project_huck_iterations.md` + `MEMORY.md`; MEMORY.md is near its cap — compress older entries while updating). **Tell the user to re-test `ls -<TAB>` / `mise<TAB>` live: the hang should be gone; mise candidates still need the 2.12 bash-completion (env), separate.**
