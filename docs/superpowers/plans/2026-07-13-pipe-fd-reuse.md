# Pipeline pipe fds must not reuse a freed 0/1/2 (#130) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop `make_pipe()` from handing a pipeline pipe end a freed std fd (0/1/2), so a first pipeline stage after `exec <&-` inherits the closed fd 0 and errors like bash instead of reading the pipe and hanging ([#130](https://github.com/jdstanhope/huck/issues/130)).

**Architecture:** Add a `move_fd_above_stdio` helper and call it on both ends inside `make_pipe()` so pipe fds are always ≥ 3 (bash's `move_to_high_fd` discipline). This is a no-op whenever fds 0–2 are already open (the common case), so the hot path is unchanged.

**Tech Stack:** Rust (crate `huck-engine`), bash-diff harness shell script.

## Global Constraints

- Every commit ends with the trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` (the `(1M context)` parenthetical is canonical).
- Run `cargo fmt --all` before every commit (CI enforces `cargo fmt --all --check`).
- Build the binary with `cargo build -p huck` (NOT `--workspace`).
- Run tests per-crate, single-threaded: `cargo test -p <crate> --jobs 1 --lib -- --test-threads 1` (this box OOM-kills on `cargo test --workspace`).
- Guard any full bash-diff sweep with `ulimit -v 1500000` and `timeout`.
- Bash-diff harnesses under `tests/scripts/*_diff_check.sh` are auto-discovered by `run_diff_checks.sh`; each uses its OWN default binary — do NOT override `HUCK_BIN`.
- Do not push to `main` or self-merge; work lands on a `v288-pipe-fd-reuse` branch via a PR the user merges.
- Compat target: bash 5.2.21, non-interactive.

---

### Task 1: Failing bash-diff harness for the closed-fd pipeline hang

**Files:**
- Create: `tests/scripts/pipe_closed_fd_diff_check.sh`

**Interfaces:**
- Consumes: the `huck` debug binary at `target/debug/huck` (built via `cargo build -p huck`).
- Produces: an executable harness that FAILS before Task 2 (the `cat | cat` cases hang/diverge) and passes after. Auto-discovered by `run_diff_checks.sh` — no registration needed.

- [ ] **Step 1: Build the current huck binary**

Run: `cargo build -p huck`
Expected: `Finished` (binary at `target/debug/huck`).

- [ ] **Step 2: Write the harness**

Create `tests/scripts/pipe_closed_fd_diff_check.sh` with exactly this content:

```bash
#!/usr/bin/env bash
# v288 (#130): a pipeline pipe end must never reuse a freed std fd (0/1/2). After
# `exec <&-` frees fd 0, huck's make_pipe() used to hand the pipe read end fd 0,
# aliasing the first stage's stdin onto the pipe -> the stage read its own output
# and hung. bash keeps pipe fds >= 3, so its first stage inherits the closed fd 0
# and errors immediately. We compare EXTERNAL-command pipelines (byte-identical
# program messages) and add a functional no-hang check for the `read` repro (whose
# builtin error WORDING differs from bash for unrelated reasons — out of scope).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
printf 'file-line-1\n' > "$WORK/inA"

# Strip the shell's program-name/line prefix so only command output is compared.
norm() { sed -E 's#^([^:]*/)?(bash|huck): (line [0-9]+: )?##'; }

# Byte-identical bash vs huck. Both are wrapped in `timeout` so a pre-fix hang
# surfaces as a FAIL (rc 124) instead of hanging the whole harness.
check() {
    local label="$1" frag="$2" b h
    b=$(cd "$WORK" && timeout 5 bash        -c "$frag" </dev/null 2>&1; echo "rc=$?"); b=$(printf '%s\n' "$b" | norm)
    h=$(cd "$WORK" && timeout 5 "$HUCK_BIN" -c "$frag" </dev/null 2>&1; echo "rc=$?"); h=$(printf '%s\n' "$h" | norm)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "closed fd0: cat | cat"        'exec <&-; cat | cat; echo end'
check "closed fd0: cat | grep"       'exec <&-; cat | grep nomatch; echo "end=$?"'
check "closed fd0: redirect override" 'exec <&-; cat < inA | cat; echo end'
check "baseline (no close) a | b"    'printf "hi\n" | cat; echo end'

# Functional no-hang check for the #130 repro. The `read` builtin's error wording
# differs from bash (out of scope), so compare only: huck did NOT hang (rc != 124)
# and huck's exit status equals bash's.
nohang() {
    local label="$1" frag="$2" brc hrc
    timeout 5 bash        -c "$frag" </dev/null >/dev/null 2>&1; brc=$?
    timeout 5 "$HUCK_BIN" -c "$frag" </dev/null >/dev/null 2>&1; hrc=$?
    if [[ "$hrc" != 124 && "$hrc" == "$brc" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s (bash rc=%s huck rc=%s; 124=hang)\n' "$label" "$brc" "$hrc"; FAIL=$((FAIL+1)); fi
}
nohang "closed fd0: read | cat no-hang" 'exec <&-; read x | cat; echo end'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 3: Make it executable**

Run: `chmod +x tests/scripts/pipe_closed_fd_diff_check.sh`

- [ ] **Step 4: Run it against the current (unfixed) huck — confirm it FAILS**

Run: `bash tests/scripts/pipe_closed_fd_diff_check.sh`
Expected: overall FAIL. Specifically, on today's huck:
- `closed fd0: cat | cat`, `closed fd0: cat | grep`, `closed fd0: redirect override`, and `closed fd0: read | cat no-hang` must FAIL (huck hangs → `timeout` kills it → rc 124 / diverging output; bash completes).
- `baseline (no close) a | b` must PASS (both print `hi` + `end`).

If `baseline (no close) a | b` FAILS, STOP and report — the harness itself is wrong. (Note: `redirect override` may already pass on today's huck if the explicit `< inA` avoids the fd-0 reuse; if so, that's fine — it is a guard. The decisive pre-fix failures are the `cat | cat`, `cat | grep`, and `read | cat` cases.)

- [ ] **Step 5: Commit the failing harness**

```bash
git add tests/scripts/pipe_closed_fd_diff_check.sh
git commit -m "test: failing closed-fd pipeline-hang diff harness (#130)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Move pipe fds above the stdio range in `make_pipe`

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` — add `move_fd_above_stdio` and rewrite `make_pipe` (currently lines 6789–6796).
- Test: `tests/scripts/pipe_closed_fd_diff_check.sh` (from Task 1).

**Interfaces:**
- Consumes: `RawFd`, `io` (both already imported in executor.rs), `libc` (`pipe`, `fcntl`, `F_DUPFD`, `close`).
- Produces: `fn move_fd_above_stdio(fd: RawFd) -> io::Result<RawFd>`; `make_pipe()` keeps its existing signature `fn make_pipe() -> io::Result<(RawFd, RawFd)>` and returned meaning `(read_end, write_end)`, but both ends are now guaranteed ≥ 3.

- [ ] **Step 1: Add `move_fd_above_stdio` and rewrite `make_pipe`**

Replace the current `make_pipe` (the doc comment + function body at lines 6788–6796):

```rust
/// Opens a `libc::pipe()` and returns `(read_end, write_end)` as raw fds.
fn make_pipe() -> io::Result<(RawFd, RawFd)> {
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((fds[0], fds[1]))
}
```

with:

```rust
/// Move `fd` above the stdio range (>= 3) so a freed 0/1/2 (e.g. after
/// `exec <&-`) is never silently reused as a pipeline pipe end, which would
/// alias a stage's std fd onto the pipe (issue #130). Returns `fd` unchanged
/// when it is already >= 3 (the common case). Uses `F_DUPFD` (NOT
/// `F_DUPFD_CLOEXEC`) to keep the moved fd's non-close-on-exec semantics
/// identical to the raw `libc::pipe()` ends the callers dup2/close by hand,
/// then closes the original low fd.
fn move_fd_above_stdio(fd: RawFd) -> io::Result<RawFd> {
    if fd > 2 {
        return Ok(fd);
    }
    let newfd = unsafe { libc::fcntl(fd, libc::F_DUPFD, 3) };
    if newfd < 0 {
        return Err(io::Error::last_os_error());
    }
    unsafe {
        libc::close(fd);
    }
    Ok(newfd)
}

/// Opens a `libc::pipe()` and returns `(read_end, write_end)` as raw fds, both
/// guaranteed >= 3 so a freed std fd cannot be aliased into a pipeline stage's
/// std fd (issue #130).
fn make_pipe() -> io::Result<(RawFd, RawFd)> {
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let (r0, w0) = (fds[0], fds[1]);
    let r = match move_fd_above_stdio(r0) {
        Ok(fd) => fd,
        Err(e) => {
            unsafe {
                libc::close(r0);
                libc::close(w0);
            }
            return Err(e);
        }
    };
    let w = match move_fd_above_stdio(w0) {
        Ok(fd) => fd,
        Err(e) => {
            unsafe {
                libc::close(r);
                libc::close(w0);
            }
            return Err(e);
        }
    };
    Ok((r, w))
}
```

- [ ] **Step 2: Format and build**

Run: `cargo fmt --all && cargo build -p huck`
Expected: builds clean, no warnings about `move_fd_above_stdio` being unused.

- [ ] **Step 3: Run the new harness — confirm 5/5 PASS**

Run: `bash tests/scripts/pipe_closed_fd_diff_check.sh`
Expected: `Total: 5, Pass: 5, Fail: 0`.

- [ ] **Step 4: Directly confirm the strace-level root cause is gone**

Run: `bash -c 'exec <&-; cat | cat; echo end' </dev/null` and the huck equivalent:
```bash
timeout 5 ./target/debug/huck -c 'exec <&-; cat | cat; echo end' </dev/null 2>&1
```
Expected: prints `cat: -: Bad file descriptor` / `cat: closing standard input: Bad file descriptor` then `end`, and returns (no hang).

- [ ] **Step 5: Run the engine + syntax lib tests**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`
Expected: all pass (≈1795 passed; 0 failed).

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1`
Expected: all pass (≈423 passed; 0 failed).

- [ ] **Step 6: Run the full bash-diff sweep (guarded)**

Run: `ulimit -v 1500000; timeout 300 bash tests/scripts/run_diff_checks.sh`
Expected: every harness green, including `pipe_closed_fd_diff_check.sh`. Pay attention that no existing `pipe_*` / `pipeline_*` harness regressed.

- [ ] **Step 7: Verify the bash-suite redir5.sub hang is cleared (document result)**

The redir5.sub hang is the final `read abcde 2>&1 | grep -q 'read error'` after `exec <&-`. Confirm it is cleared by running the real runner from inside the tests dir (a `run-<cat>` invocation from another cwd fails instantly and lies — v286 lesson). If the bash 5.2.21 tree is not at `/tmp/bash-5.2.21`, note that and rely on the harness + direct repro.

```bash
cd /tmp/bash-5.2.21/tests 2>/dev/null && \
  timeout 60 env THIS_SH="$(cd /home/john/projects/huck && pwd)/target/debug/huck" \
  sh ./run-redir >/tmp/redir.log 2>&1; echo "redir runner rc=$?"
```
Expected: the runner no longer hangs at redir5.sub's `read … | grep`. Report honestly whether `redir` now passes fully or advances to a different blocker (report the new blocker if any — do not claim a full pass unless the runner exits cleanly).

- [ ] **Step 8: Commit**

```bash
git add crates/huck-engine/src/executor.rs
git commit -m "fix: keep pipeline pipe fds above the stdio range (#130)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the whole-branch review

- Confirm `move_fd_above_stdio`'s two failure branches in `make_pipe` close exactly the right fds with no double-close and no leak (trace: end already ≥3 vs end moved).
- Confirm `F_DUPFD` (not `F_DUPFD_CLOEXEC`) is used, so moved pipe fds keep the same inherit-across-fork/exec semantics the callers rely on (they dup2 then manually close pipe ends in children).
- Confirm the change is a genuine no-op when fds 0–2 are open (the moved-fd branch is not taken), so no existing pipeline behavior shifts.
- Out of scope by design: the `read` builtin error wording, and the other raw `libc::pipe` sites (`procsub.rs`, `stdin_pipe.rs`, `wait_loop.rs`, executor.rs:4375).
