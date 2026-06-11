# v134 — fork a writer for heredoc/herestring bodies (M-120) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop heredoc/herestring bodies from deadlocking when they exceed the pipe buffer (or feed a backpressuring consumer), by feeding the body from a forked writer process instead of a blocking parent `write()`. The final `nvm ls-remote` blocker.

**Architecture:** One helper `spawn_heredoc_writer(bytes) -> (read_fd, writer_pid)` forks a writer process; the parent closes the write end immediately (so no later in-process forked stage inherits it) and uses the read end as the consumer's stdin. Wire it at all four heredoc-stdin sites and reap the writer pids at the foreground wait points.

**Tech Stack:** Rust, libc (pipe/fork/write/_exit/waitpid). Tests: watchdog-guarded cargo integration + a bash-diff harness + a network-gated `nvm ls-remote` PTY payoff.

**GIT SAFETY:** Do NOT `git checkout <sha>` — stay on `v134-heredoc-forked-writer`; edit, build, commit in place. Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`.

**Reference:** spec `docs/superpowers/specs/2026-06-11-heredoc-forked-writer-design.md`. Key locations: `write_pipe_for_stdin` (executor.rs:2543, the blocking-write helper to replace); `with_redirect_scope` stdin Heredoc/HereString arms (executor.rs:566/573, calls `write_pipe_for_stdin`, then `scope.redirect(new_fd, STDIN_FILENO)` + close); `run_multi_stage` heredoc write (executor.rs:~4072 `write_file.write_all(&bytes)`, fd from a make_pipe + `heredoc_write_fd`); `run_subprocess` (executor.rs:~3359 `child_stdin.write_all(&bytes)` from `pending_stdin_bytes`, child stdin is `Stdio::piped()`); `run_background_sequence` (executor.rs:~2013 `heredoc_pairs.push((w, bytes))` + :2039 write loop). The single-command non-captured heredoc currently works (the child drains its own stdout); the captured single (`r=$(cat << big)`) DEADLOCKS — so run_subprocess needs the fix too.

**CRITICAL learning (from the trace):** a writer THREAD fails — an InProcess pipeline stage forks-without-exec and inherits the thread's open write fd → no EOF. MUST fork a writer PROCESS.

---

### Task 1: `spawn_heredoc_writer` + `with_redirect_scope` (the nvm blocker)

**Files:**
- Create: `tests/heredoc_forked_writer_integration.rs`
- Modify: `src/executor.rs` (add `spawn_heredoc_writer`; rewire `with_redirect_scope`)

- [ ] **Step 1: Write the failing tests** — create `tests/heredoc_forked_writer_integration.rs` with the watchdog helper (reuse v133's `run_guarded`) and the compound + herestring cases:

```rust
//! v134: heredoc/herestring bodies are fed by a forked writer, so large bodies
//! (> pipe buffer) and backpressuring consumers no longer deadlock (M-120).
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
fn run_guarded(script: &str, secs: u64) -> Option<(String, String, i32)> {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    let pid = child.id();
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let (tx, rx) = mpsc::channel::<()>();
    let wd = thread::spawn(move || -> bool {
        if rx.recv_timeout(Duration::from_secs(secs)).is_err() {
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).status();
            true
        } else { false }
    });
    let out = child.wait_with_output().unwrap();
    let _ = tx.send(());
    if wd.join().unwrap() { None } else {
        Some((String::from_utf8_lossy(&out.stdout).into_owned(),
              String::from_utf8_lossy(&out.stderr).into_owned(),
              out.status.code().unwrap_or(-1)))
    }
}
// Builds a script that defines V as a 200000-char string then runs `frag`.
fn with_bigV(frag: &str) -> String {
    format!("V=$(printf 'x%.0s' $(seq 1 200000))\n{frag}\n")
}

#[test]
fn compound_heredoc_large_body() {
    let (o, _e, _c) = run_guarded(&with_bigV("{ wc -c; } << EOF\n$V\nEOF"), 10)
        .expect("HUNG: compound heredoc deadlocked");
    assert_eq!(o.trim(), "200001", "o: {o:?}");
}

#[test]
fn compound_awk_while_heredoc_nvm_shape() {
    // nvm's shape: a pipeline inside the brace group fed by a heredoc.
    let (o, _e, _c) = run_guarded(&with_bigV("{ command awk '{print}' | wc -l; } << EOF\n$V\nEOF"), 10)
        .expect("HUNG: awk|while compound heredoc deadlocked");
    assert_eq!(o.trim(), "1", "o: {o:?}");
}

#[test]
fn compound_herestring_large_body() {
    let (o, _e, _c) = run_guarded(&with_bigV("{ wc -c; } <<< \"$V\""), 10)
        .expect("HUNG: compound herestring deadlocked");
    assert_eq!(o.trim(), "200001", "o: {o:?}");
}

#[test]
fn small_compound_heredoc_no_regression() {
    let (o, _e, _c) = run_guarded("{ cat; } << EOF\nyo\nEOF\n", 10).expect("hung");
    assert_eq!(o, "yo\n", "o: {o:?}");
}
```

- [ ] **Step 2: Run to verify the hang is caught** — `cargo test --test heredoc_forked_writer_integration 2>&1 | tail -20`. Expected: `compound_heredoc_large_body`, `compound_awk_while_heredoc_nvm_shape`, `compound_herestring_large_body` FAIL (watchdog → None → panic "HUNG"); `small_compound_heredoc_no_regression` PASSES.

- [ ] **Step 3: Add `spawn_heredoc_writer`** in `src/executor.rs` (near `write_pipe_for_stdin` ~2543):

```rust
/// Feed `bytes` (an expanded heredoc/herestring body) to a child's stdin WITHOUT
/// the parent ever blocking on a full pipe. Forks a writer process that owns the
/// pipe's write end, writes the whole body, then `_exit`s. The parent closes the
/// write end immediately, so no later in-process forked stage inherits it (a
/// writer *thread* would fail there — CLOEXEC only fires on exec, and InProcess
/// stages fork without exec). Returns the READ end (→ consumer stdin) and the
/// writer PID (reap it at the consumer's wait point; ECHILD is fine).
fn spawn_heredoc_writer(bytes: &[u8]) -> Result<(RawFd, libc::pid_t), io::Error> {
    let mut fds: [libc::c_int; 2] = [-1, -1];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let (r, w) = (fds[0], fds[1]);
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        let e = io::Error::last_os_error();
        unsafe { libc::close(r); libc::close(w); }
        return Err(e);
    }
    if pid == 0 {
        // CHILD: async-signal-safe only. Close read end; write the body; _exit.
        unsafe { libc::close(r); }
        let mut off = 0usize;
        while off < bytes.len() {
            let n = unsafe {
                libc::write(w, bytes[off..].as_ptr() as *const libc::c_void, bytes.len() - off)
            };
            if n < 0 {
                let err = io::Error::last_os_error();
                // EINTR: retry. EPIPE (consumer closed early): stop, success.
                match err.raw_os_error() {
                    Some(libc::EINTR) => continue,
                    _ => break, // EPIPE or other: the consumer is gone; we're done.
                }
            }
            if n == 0 { break; }
            off += n as usize;
        }
        unsafe { libc::close(w); libc::_exit(0); }
    }
    // PARENT: close the write end so only the writer child holds it.
    unsafe { libc::close(w); }
    Ok((r, pid))
}
```
(If `bytes.is_empty()`, this still forks a writer that writes nothing and exits → the reader sees immediate EOF. That's fine; no special-case needed.)

- [ ] **Step 4: Rewire `with_redirect_scope` stdin (executor.rs:566/573).** Capture the writer pid and reap it after the inner body. The function currently returns `run_inner(...)`'s outcome after the scope drop; change it to capture the outcome, reap the writer pid, then return. Sketch:
  - Add `let mut heredoc_writer: Option<libc::pid_t> = None;` before the stdin block.
  - In the `Heredoc` arm: `let (rfd, pid) = spawn_heredoc_writer(&bytes).map_err(...)?; heredoc_writer = Some(pid); rfd` (use `rfd` as `new_fd`). Same for `HereString`. (On error, print `huck: pipe/fork: {e}` and `return ExecOutcome::Continue(1)`.)
  - Find where the function runs `run_inner` and returns. Wrap: `let outcome = <run_inner ...>; if let Some(pid) = heredoc_writer { let mut st = 0; unsafe { libc::waitpid(pid, &mut st, 0); } } outcome`. The `waitpid` must happen AFTER the inner body (the consumer) has run and drained the pipe, and after the scope restores stdin. Tolerate ECHILD implicitly (waitpid returns -1, ignored). Read the function body to place the reap correctly relative to the scope drop + outcome return.
  - DELETE `write_pipe_for_stdin` (now unused) — or leave it if any other caller remains (grep `write_pipe_for_stdin`; if `with_redirect_scope` was the only caller, remove it).

- [ ] **Step 5: Run the tests** — `cargo test --test heredoc_forked_writer_integration 2>&1 | tail -15` → all 4 pass.

- [ ] **Step 6: Build + FULL regression + clippy** — `cargo build 2>&1 | tail -3`; `cargo test 2>&1 | grep -E "FAILED|error\[|panicked|test result: FAILED" | head` (none — heredoc/herestring/compound-redirect tests stay green); `cargo clippy --all-targets 2>&1 | tail -3` (clean).

- [ ] **Step 7: Sanity vs bash** (report): the nvm-shape compound + a no-zombie check:
```
V=$(printf 'x%.0s' $(seq 1 200000))
for f in '{ wc -c; } << EOF' '{ command awk "{print}" | wc -l; } << EOF'; do :; done
timeout 8 ./target/debug/huck -c "V=\$(printf 'x%.0s' \$(seq 1 200000)); { command awk '{print}' | wc -l; } << EOF
\$V
EOF"
```
(report it prints `1`, not a hang). Also: `ps` is not reliable in tests; just confirm the command returns and the shell continues (the watchdog tests already prove no hang).

- [ ] **Step 8: Commit**
```bash
git add src/executor.rs tests/heredoc_forked_writer_integration.rs
git commit -m "$(cat <<'EOF'
fix(v134): forked-writer heredoc for compound redirects (M-120 part 1)

A heredoc/herestring body larger than the pipe buffer (or feeding a backpressuring
consumer) deadlocked because the parent blocking-wrote the whole body before the
consumer drained (write_pipe_for_stdin). Add spawn_heredoc_writer (forks a writer
process; parent closes the write end so no in-process stage inherits it) and use
it in with_redirect_scope; reap the writer pid after the inner body. Fixes the nvm
ls-remote `{ awk | while } << EOF` compound shape.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: the other three heredoc sites (pipeline / single / background)

**Files:**
- Modify: `src/executor.rs` (`run_multi_stage`, `run_subprocess`, `run_background_sequence`)
- Modify: `tests/heredoc_forked_writer_integration.rs` (add pipeline / captured-single / background tests)

- [ ] **Step 1: Add failing tests** — append:
```rust
#[test]
fn pipeline_heredoc_large_body() {
    let (o, _e, _c) = run_guarded(&with_bigV("cat << EOF | wc -c\n$V\nEOF"), 10)
        .expect("HUNG: pipeline heredoc deadlocked");
    assert_eq!(o.trim(), "200001", "o: {o:?}");
}
#[test]
fn captured_single_heredoc_large_body() {
    let (o, _e, _c) = run_guarded(&with_bigV("r=$(cat << EOF\n$V\nEOF\n); echo ${#r}"), 10)
        .expect("HUNG: captured single-command heredoc deadlocked");
    assert_eq!(o.trim(), "200000", "o: {o:?}");
}
#[test]
fn small_pipeline_heredoc_no_regression() {
    let (o, _e, _c) = run_guarded("cat << EOF | wc -c\nhi\nEOF\n", 10).expect("hung");
    assert_eq!(o.trim(), "3", "o: {o:?}");
}
#[test]
fn dollar_bang_unaffected_by_heredoc() {
    // The heredoc writer fork must NOT change $!.
    let (o, _e, _c) = run_guarded("sleep 0.2 & p=$!; cat << EOF >/dev/null\nx\nEOF\necho \"$p $!\"\n", 10).expect("hung");
    let parts: Vec<&str> = o.split_whitespace().collect();
    assert_eq!(parts.len(), 2, "o: {o:?}");
    assert_eq!(parts[0], parts[1], "heredoc writer changed $!; o: {o:?}");
}
```

- [ ] **Step 2: Run to verify** — `cargo test --test heredoc_forked_writer_integration 2>&1 | tail -20`. Expected: `pipeline_heredoc_large_body`, `captured_single_heredoc_large_body` FAIL (hang); the small + `$!` tests behavior: `dollar_bang_unaffected_by_heredoc` may already pass (Task-1 didn't touch these paths). `small_pipeline_heredoc_no_regression` passes.

- [ ] **Step 3: `run_multi_stage` (executor.rs ~4072).** Read 3850–4080. For a stage whose stdin is a heredoc/herestring, the code currently `make_pipe()`s, stores `heredoc_write_fd`, and later `write_file.write_all(&bytes)`. Replace this with `spawn_heredoc_writer(&bytes)` → use the returned read fd as that stage's stdin (where `heredoc_write_fd`'s pipe read end was used); push the writer pid into a new `Vec<libc::pid_t> heredoc_writers`. Remove the make_pipe + the `write_all` for heredocs. After `wait_pipeline_raw(...)`, loop `for pid in heredoc_writers { unsafe { libc::waitpid(pid, &mut 0, 0); } }`. CAUTION: keep the existing fd-close discipline coherent — with the forked writer the PARENT no longer holds a write end, so the `heredoc_write_fd` close sites become unnecessary for heredocs (the parent never holds it). Make the minimal change that swaps the pipe+write for the helper and reaps; do not disturb non-heredoc stage handling.

- [ ] **Step 4: `run_subprocess` (executor.rs ~3359).** Read 3120–3360. Currently the child's stdin is `Stdio::piped()` and the parent writes `pending_stdin_bytes` via `child_stdin.write_all`. Change: when `pending_stdin_bytes` is `Some(bytes)`, call `spawn_heredoc_writer(&bytes)` and set the child's stdin to the read fd via `Stdio::from(unsafe { std::os::fd::OwnedFd::from_raw_fd(r) })` (instead of piped + write). Record the writer pid; after `child.wait()`/`wait_with_output`, `waitpid(writer_pid)`. (Confirm how run_subprocess obtains the child's exit status and reap the writer right after.) Keep the no-heredoc path (no `pending_stdin_bytes`) unchanged.

- [ ] **Step 5: `run_background_sequence` (executor.rs ~2013/2039).** Read 2000–2050. Currently it collects `heredoc_pairs: Vec<(RawFd, Vec<u8>)>` and writes them after the spawn loop. Replace: for each heredoc stage, `spawn_heredoc_writer(&bytes)` → read fd is the stage's stdin; the writer pids are NOT waited synchronously (the pipeline is backgrounded) — they are internal forks reaped by the existing SIGCHLD reaper, NOT registered as jobs. Remove the `heredoc_pairs` write loop. Confirm the backgrounded job's pid list / `$!` is the pipeline's last stage (NOT a writer).

- [ ] **Step 6: Run tests** — `cargo test --test heredoc_forked_writer_integration 2>&1 | tail -20` → all pass (8 tests).

- [ ] **Step 7: Build + FULL regression + clippy + interactive PTY** — `cargo build 2>&1 | tail -3`; `cargo test 2>&1 | grep -E "FAILED|error\[|panicked|test result: FAILED" | head` (none); `cargo test --test pty_interactive --test subshell_pipeline_pty --test subshell_tty_pty 2>&1 | tail -8` (green); `cargo clippy --all-targets 2>&1 | tail -3` (clean).

- [ ] **Step 8: Sanity vs bash** (report):
```
V=$(printf 'x%.0s' $(seq 1 200000))
for f in 'cat << EOF | wc -c' 'r=$(cat << EOF'; do :; done
for frag in 'cat << H | wc -c' 'r=$(cat << H'; do
  s="V=\$(printf 'x%.0s' \$(seq 1 200000))
$frag
\$V
H
$([ "$frag" = 'r=$(cat << H' ] && echo '); echo ${#r}')"
  b=$(timeout 10 bash -c "$s" 2>&1); h=$(timeout 10 ./target/debug/huck -c "$s" 2>&1)
  [ "$b" = "$h" ] && echo "MATCH" || { echo "DIFF/HANG: $frag"; diff <(echo "$b") <(echo "$h"); }
done
```
(report MATCH for both).

- [ ] **Step 9: Commit**
```bash
git add src/executor.rs tests/heredoc_forked_writer_integration.rs
git commit -m "$(cat <<'EOF'
fix(v134): forked-writer heredoc for pipeline/single/background (M-120 part 2)

Use spawn_heredoc_writer at the remaining heredoc-stdin sites: run_multi_stage
(pipeline stage), run_subprocess (single command, incl. the captured case that
deadlocked), and run_background_sequence. Reap foreground writer pids at the wait
points; background writers go through the existing reaper. Writer pids never
register as jobs / set $! / affect $PIPESTATUS.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Bash-diff harness + nvm ls-remote payoff + docs

**Files:**
- Create: `tests/scripts/heredoc_pipeline_diff_check.sh`
- Modify: `docs/bash-divergences.md`

- [ ] **Step 1: Bash-diff harness** — create `tests/scripts/heredoc_pipeline_diff_check.sh`:
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v134: heredoc/herestring bodies fed by a
# forked writer never deadlock (M-120). timeout-guarded so a regression FAILS.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(timeout 15 bash -c "$frag" 2>&1; echo "EXIT:$?")
    h=$(timeout 15 "$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
BIG='V=$(printf "x%.0s" $(seq 1 200000))'
check "compound large"        "$BIG"$'\n''{ wc -c; } << E\n$V\nE'
check "compound awk pipe"     "$BIG"$'\n''{ command awk "{print}" | wc -l; } << E\n$V\nE'
check "pipeline large"        "$BIG"$'\n''cat << E | wc -c\n$V\nE'
check "captured single large" "$BIG"$'\n''r=$(cat << E\n$V\nE\n); echo ${#r}'
check "herestring compound"   "$BIG"$'\n''{ wc -c; } <<< "$V"'
check "small compound"        $'{ cat; } << E\nhi\nE'
check "small pipeline"        $'cat << E | wc -c\nhi\nE'
check "pipestatus heredoc"    $'false << E | true\nx\nE\necho "${PIPESTATUS[*]}"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```
`chmod +x tests/scripts/heredoc_pipeline_diff_check.sh`. Run it: `cargo build 2>&1 | tail -2; bash tests/scripts/heredoc_pipeline_diff_check.sh` → expect `Fail: 0`. Report any diff.

- [ ] **Step 2: nvm ls-remote PTY payoff (REQUIRED — verify before claiming).** Write a python `pty.fork` harness (to /tmp) that spawns interactive huck (NO `-i`), sends `source ~/.nvm/nvm.sh` (NOT `~/.bashrc` — creds), waits ~3s, sends `nvm ls-remote`, then `echo DONE_$((6*7))`, reads with a ~60s timeout, strips ANSI. Report VERBATIM: did `DONE_42` arrive (completed, no hang)? Did version lines (`v18.`/`v20.`/`iojs`/an LTS alias) appear? If no network, say so. DO NOT claim success unless DONE_42 arrived AND version lines appeared. Also run the network-free `timeout 12 ./target/debug/huck -c 'V=$(printf "x%.0s" $(seq 1 200000)); { command awk "{print}" | wc -l; } << E
$V
E'` → expect `1`. Clean up the temp file.

- [ ] **Step 3: Optional network-gated PTY test** — if expectrl/OsSession is the repo's PTY idiom (see tests/subshell_tty_pty.rs), add `tests/nvm_ls_remote_pty.rs` that runs the payoff and SKIPS cleanly when `~/.nvm/nvm.sh` is absent or no network (don't fail CI offline). If a robust skip is hard, SKIP this step and rely on the Step-2 manual payoff + the synthetics — note which you did.

- [ ] **Step 4: Delete M-120 + add the read-perf deferral in docs/bash-divergences.md.** Find `### M-120` (Tier-1). Delete the entire entry. Decrement the Tier-1 count: `| Bugs (Tier 1) | 2 | … (M-114, M-120). |` → `| Bugs (Tier 1) | 1 | … (M-114). |`. Then ADD a Tier-4 `[deferred]` perf entry (e.g. L-31) for the `read`-builtin per-byte syscall slowness: `the read builtin issues one libc::read per byte (builtins.rs:2027), so reading a large body via `read` is O(n) syscalls — slow (not a hang). Off the nvm path (nvm feeds awk). Fix: buffer read's stdin.` and bump the Tier-4 count by 1. Search the doc for other M-120 references.

- [ ] **Step 5: Verify docs** — `grep -n "M-120" docs/bash-divergences.md` → none; `grep -n "Bugs (Tier 1) | 1" docs/bash-divergences.md` → present; the new L-31 present.

- [ ] **Step 6: Full regression + clippy** — `cargo test 2>&1 | grep -E "FAILED|error\[|test result: FAILED" | head` (none); `cargo clippy --all-targets 2>&1 | tail -3` (clean); smoke `bash tests/scripts/captured_pipeline_drain_diff_check.sh | tail -1` (v133 harness still green).

- [ ] **Step 7: Commit**
```bash
git add tests/scripts/heredoc_pipeline_diff_check.sh docs/bash-divergences.md
# add tests/nvm_ls_remote_pty.rs if you created it in Step 3
git commit -m "$(cat <<'EOF'
test+docs(v134): heredoc-forked-writer harness; nvm payoff; resolve M-120

Add the bash-diff harness (all heredoc shapes, small + >64KB). nvm ls-remote now
completes end-to-end (verified). Delete M-120; log the read-builtin per-byte
slowness as a deferred perf item.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```
(If the Step-2 nvm payoff did NOT complete, STOP and report DONE_WITH_CONCERNS — do not commit a message claiming nvm works.)

---

## Self-review notes
- **Spec coverage:** Task 1 = the helper + with_redirect_scope (Blocker 1, the nvm compound shape) + reaping; Task 2 = the other 3 sites + reaping + `$!` no-disturb; Task 3 = harness + the REQUIRED nvm payoff + docs (delete M-120, defer read-perf).
- **Type/symbol consistency:** `spawn_heredoc_writer(&[u8]) -> Result<(RawFd, libc::pid_t), io::Error>` defined in Task 1, used at all 4 sites; replaces `write_pipe_for_stdin`. Writer pids reaped via `libc::waitpid` at each foreground wait point.
- **CRITICAL (forked writer, not thread):** every site forks a writer PROCESS; the parent closes the write end immediately. A thread would be inherited by InProcess fork-without-exec stages → no EOF (proven in the trace).
- **No-regress:** small heredocs + non-heredoc paths unchanged; `$!`/`$PIPESTATUS`/`$?` unaffected (writer pids are internal, never jobs); watchdog-guarded tests make a regression a timeout-FAIL.
- **Honesty gate:** Task 3 requires the real `nvm ls-remote` to print versions before claiming (v132/v133 lesson).
