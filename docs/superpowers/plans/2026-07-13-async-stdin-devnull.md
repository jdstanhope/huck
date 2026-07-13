# Async command stdin `/dev/null` default (#126, Path B) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a backgrounded command with no input redirection default its stdin to `/dev/null` (non-interactive) instead of inheriting the shell's stdin, fixing the `/bin/cat & wait` hang ([#126](https://github.com/jdstanhope/huck/issues/126)).

**Architecture:** A shared `async_default_stdin` helper encodes bash's rule (non-interactive async units get `/dev/null` unless they're a bare multi-stage pipeline; interactive shells always inherit). `run_background_subshell` — huck's Path B, reached by `(cmd) &`, `a && b &`, and every `cmd & …` separator-group background — resolves its child's stdin through the helper instead of hard-coding `STDIN_FILENO`. Its three callers pass an `inherit_stdin` flag; only the separator-group caller can present a bare multi-stage pipeline. Path A (`run_background_sequence`) is out of scope (deferred to #129, blocked by #128).

**Tech Stack:** Rust (crate `huck-engine`), bash-diff harness shell script.

## Global Constraints

- Every commit ends with the trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` (the `(1M context)` parenthetical is canonical).
- Run `cargo fmt --all` before every commit (CI enforces `cargo fmt --all --check`).
- Build the binary with `cargo build -p huck` (NOT `--workspace`).
- Run tests per-crate, single-threaded: `cargo test -p <crate> --jobs 1 --lib -- --test-threads 1` (this box OOM-kills on `cargo test --workspace`).
- Guard any full bash-diff sweep with `ulimit -v 1500000` and `timeout`.
- Bash-diff harnesses under `tests/scripts/*_diff_check.sh` run each fragment through bash and huck and assert byte-identical output; a harness uses its OWN default binary — do NOT override `HUCK_BIN`.
- Do not push to `main` or self-merge; work lands on a `v287-async-stdin` branch via a PR the user merges.
- Compat target: bash 5.2.21, non-interactive.

---

### Task 1: Failing bash-diff harness for async stdin

**Files:**
- Create: `tests/scripts/async_stdin_diff_check.sh`

**Interfaces:**
- Consumes: the `huck` debug binary at `target/debug/huck` (built via `cargo build -p huck`).
- Produces: an executable harness that (before Task 2) FAILS, and (after Task 2) passes 9/9. `run_diff_checks.sh` auto-discovers `tests/scripts/*_diff_check.sh`, so no registration step is needed.

- [ ] **Step 1: Build the current huck binary**

Run: `cargo build -p huck`
Expected: `Finished` (a binary at `target/debug/huck`).

- [ ] **Step 2: Write the harness**

Create `tests/scripts/async_stdin_diff_check.sh` with exactly this content:

```bash
#!/usr/bin/env bash
# v287 (#126): an async command with no input redirection must default its stdin
# to /dev/null (non-interactive), so it cannot steal the terminal / an open pipe.
# Each async child prints `readlink /proc/self/fd/0` — its fd0 identity — which
# must match bash byte-for-byte: "/dev/null" for the defaulted cases, the fixture
# path for the inherited cases. (readlink never reads fd0, so these cases can't
# hang.) A final functional guard runs a real `cat` under timeout to assert the
# #126 hang is gone.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
printf 'alpha\n' > "$WORK/inA"
printf 'beta\n'  > "$WORK/inB"

# Compare a fragment's output+rc between bash and huck. The shell's own stdin is
# taken from $WORK/inA so an inherited async fd0 resolves to a stable path; the
# mktemp dir is masked so the absolute fixture paths compare equal.
check() {
    local label="$1" frag="$2" b h
    b=$(cd "$WORK" && bash        -c "$frag" < "$WORK/inA" 2>&1; echo "rc=$?")
    h=$(cd "$WORK" && "$HUCK_BIN" -c "$frag" < "$WORK/inA" 2>&1; echo "rc=$?")
    b=${b//$WORK/@WORK@}; h=${h//$WORK/@WORK@}
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

R='readlink /proc/self/fd/0'
check "simple cmd &"             "$R & wait"
check "(subshell) &"             "($R) & wait"
check "{ group; } &"             "{ $R; } & wait"
check "and-or (true && x) &"     "true && $R & wait"
check "explicit input redirect"  "$R < inB & wait"
check "bare pipeline (inherit)"  "$R | cat & wait"
check "subshell-wrapped pipe"    "($R | cat) & wait"
check "3-stage pipeline"         "$R | cat | cat & wait"

# Functional anti-hang guard (#126 direct repro): a real `cat` with no redirect
# must get /dev/null and EOF (rc=0), not block on an open pipe. fd0 is a pipe fed
# by `sleep 30` (produces nothing, stays open); a `cat` that inherited it would
# block until timeout. `< <(...)` is bash-only outer syntax feeding the inner shell.
guard() { timeout 5 "$1" -c '/bin/cat & wait; echo "rc=$?"' < <(sleep 30) 2>&1; }
gb=$(guard bash); gh=$(guard "$HUCK_BIN")
if [[ "$gb" == "rc=0" && "$gh" == "rc=0" ]]; then
    printf 'PASS: cat & wait no-hang guard\n'; PASS=$((PASS+1))
else
    printf 'FAIL: cat & wait no-hang guard (bash=[%s] huck=[%s])\n' "$gb" "$gh"; FAIL=$((FAIL+1))
fi

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 3: Make it executable**

Run: `chmod +x tests/scripts/async_stdin_diff_check.sh`

- [ ] **Step 4: Run it against the current (unfixed) huck — confirm it FAILS**

Run: `bash tests/scripts/async_stdin_diff_check.sh`
Expected: overall FAIL. Specifically these must fail on today's huck:
- `simple cmd &`, `(subshell) &`, `{ group; } &`, `and-or (true && x) &`, `subshell-wrapped pipe` — huck prints `@WORK@/inA`, bash prints `/dev/null`.
- `cat & wait no-hang guard` — huck times out (no `rc=0`); bash prints `rc=0`.

These should already PASS even before the fix (they're non-regression guards): `explicit input redirect` (both `@WORK@/inB`), `bare pipeline (inherit)` and `3-stage pipeline` (both `@WORK@/inA`). If any of those three FAIL, stop and report — the harness itself is wrong.

- [ ] **Step 5: Commit the failing harness**

```bash
git add tests/scripts/async_stdin_diff_check.sh
git commit -m "test: failing async-stdin /dev/null diff harness (#126)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Default async stdin to `/dev/null` in `run_background_subshell`

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` — add `AsyncStdin` + `async_default_stdin` (near `run_background_subshell`, ~line 3056); change `run_background_subshell` (line 3057) signature + body; update its 3 callers (lines 264, 285, 563).
- Test: `tests/scripts/async_stdin_diff_check.sh` (from Task 1).

**Interfaces:**
- Consumes: `Shell` (field `is_interactive: bool`, method `job_control_active()`), `StdoutSink`, `StderrSink`, `err_writer(err_sink, sink)`, `crate::sh_error_to!`, `crate::bash_io_error`, `fork_and_run_in_subshell`, `NO_PGROUP`, `RawFd`, `libc`, `File`, `Command::Pipeline(p)` where `p.commands: Vec<..>`, `ExecOutcome::Continue`. `AndOrGroup` has fields `first: &Command`, `rest: Vec<(Connector, &Command)>`.
- Produces: `fn run_background_subshell(cmd: &Command, shell: &mut Shell, sink: &mut StdoutSink, err_sink: &mut StderrSink, inherit_stdin: bool, source: &str) -> ExecOutcome` (new `inherit_stdin` parameter, second-to-last).

- [ ] **Step 1: Add the `AsyncStdin` enum and helper**

Insert immediately ABOVE `fn run_background_subshell(` (currently line 3057, just after the `// ----- background pipeline ---` region's `run_background_sequence`). Add:

```rust
/// The stdin an async (`&`) child should start with. bash defaults async stdin
/// to `/dev/null` when the shell is non-interactive and the unit is not a bare
/// multi-stage pipeline; otherwise the child inherits the shell's stdin.
enum AsyncStdin {
    /// Inherit the shell's stdin (fd 0).
    Inherit,
    /// A freshly opened `/dev/null` fd; the caller closes it after forking.
    DevNull(RawFd),
}

/// Decide an async child's default stdin. `inherit` is true when the unit must
/// keep the shell's stdin regardless of interactivity (a bare multi-stage
/// pipeline). An interactive shell always inherits. Otherwise stdin defaults to
/// `/dev/null` (`O_RDONLY`); on open failure a `/dev/null: <error>` diagnostic is
/// emitted and `Err(())` returned.
fn async_default_stdin(
    inherit: bool,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<AsyncStdin, ()> {
    if inherit || shell.is_interactive {
        return Ok(AsyncStdin::Inherit);
    }
    use std::os::unix::io::IntoRawFd;
    match File::open("/dev/null") {
        Ok(f) => Ok(AsyncStdin::DevNull(f.into_raw_fd())),
        Err(e) => {
            let mut err = err_writer(err_sink, sink);
            crate::sh_error_to!(
                shell,
                &mut *err,
                None,
                "/dev/null: {}",
                crate::bash_io_error(&e)
            );
            Err(())
        }
    }
}
```

- [ ] **Step 2: Change `run_background_subshell`'s signature**

Change the parameter list (line 3057) to add `inherit_stdin: bool` before `source`:

```rust
fn run_background_subshell(
    cmd: &Command,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
    inherit_stdin: bool,
    source: &str,
) -> ExecOutcome {
```

- [ ] **Step 3: Replace the hard-coded `STDIN_FILENO` with the resolved stdin**

Replace the current comment + `match fork_and_run_in_subshell(...) {` block head (lines 3066–3078) — from the `// Inherit stdin from the terminal …` comment down to and including the `) {` that opens the match — with:

```rust
    // bash: an async command's stdin defaults to /dev/null (non-interactive, no
    // explicit input redirect) so it can't steal the terminal; a bare
    // multi-stage pipeline inherits instead (async_default_stdin, #126).
    let stdin_fd = match async_default_stdin(inherit_stdin, shell, sink, err_sink) {
        Ok(AsyncStdin::Inherit) => libc::STDIN_FILENO,
        Ok(AsyncStdin::DevNull(fd)) => fd,
        Err(()) => return ExecOutcome::Continue(1),
    };
    let fork_result = fork_and_run_in_subshell(
        cmd,
        shell,
        stdin_fd,
        libc::STDOUT_FILENO,
        libc::STDERR_FILENO,
        /*pgid_target=*/ if job_control { 0 } else { NO_PGROUP },
        /*parent_fds_to_close=*/ &[],
        None, // no Dup redirect at this call site
        None,
    );
    if stdin_fd != libc::STDIN_FILENO {
        // Parent drops its /dev/null copy; the child kept its own across fork.
        unsafe {
            libc::close(stdin_fd);
        }
    }
    match fork_result {
```

The existing `Ok(pid) => { … }` and `Err(e) => { … }` arms below stay unchanged (they now match on `fork_result`).

- [ ] **Step 4: Update the two fast-path callers (both `inherit_stdin = false`)**

At line 264 (`(cmd) &` explicit subshell), change:

```rust
                return run_background_subshell(&seq.first, shell, sink, err_sink, source);
```
to
```rust
                return run_background_subshell(&seq.first, shell, sink, err_sink, false, source);
```

At line 285 (collapsed `a && b &` / `a || b &`), change:

```rust
            return run_background_subshell(&subshell, shell, sink, err_sink, source);
```
to
```rust
            return run_background_subshell(&subshell, shell, sink, err_sink, false, source);
```

- [ ] **Step 5: Update the separator-group caller (compute the pipeline predicate)**

In `execute_sequence_body`, immediately BEFORE the `let source = group_display_label(group.first);` line (currently line 557), insert:

```rust
            // bash: a bare multi-stage pipeline backgrounded via `&` keeps the
            // shell's stdin on stage 0; every other async unit gets /dev/null.
            let inherit_stdin = group.rest.is_empty()
                && matches!(group.first, Command::Pipeline(p) if p.commands.len() > 1);
```

Then change the call at line 563:

```rust
            run_background_subshell(&subshell, shell, sink, err_sink, &source);
```
to
```rust
            run_background_subshell(&subshell, shell, sink, err_sink, inherit_stdin, &source);
```

- [ ] **Step 6: Format and build**

Run: `cargo fmt --all && cargo build -p huck`
Expected: builds clean, no warnings about `AsyncStdin`/`async_default_stdin` being unused.

- [ ] **Step 7: Run the async-stdin harness — confirm 9/9 PASS**

Run: `bash tests/scripts/async_stdin_diff_check.sh`
Expected: `Total: 9, Pass: 9, Fail: 0`.

- [ ] **Step 8: Run the engine + syntax lib tests**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`
Expected: all pass (≈1795 passed; 0 failed).

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1`
Expected: all pass (≈423 passed; 0 failed).

- [ ] **Step 9: Run the full bash-diff sweep (guarded)**

Run: `ulimit -v 1500000; timeout 300 bash tests/scripts/run_diff_checks.sh`
Expected: every harness green, including `async_stdin_diff_check.sh`.

- [ ] **Step 10: Manual interactive-gate check (document result)**

The interactive gate is not automatable in a byte-diff harness (needs a tty). Verify by hand that an interactive shell still inherits the terminal (no `/dev/null`):

```bash
script -qec 'bash -i        -c "readlink /proc/self/fd/0 > /tmp/ib.out & wait" < /etc/hostname' /dev/null >/dev/null 2>&1; sleep 0.3; echo "bash -i: $(cat /tmp/ib.out)"
script -qec "$(pwd)/target/debug/huck -i -c 'readlink /proc/self/fd/0 > /tmp/ih.out & wait' < /etc/hostname" /dev/null >/dev/null 2>&1; sleep 0.3; echo "huck -i: $(cat /tmp/ih.out)"
```
Expected: BOTH print a terminal/pts path (e.g. `/dev/pts/N`) or `/etc/hostname`, NOT `/dev/null` — confirming interactive async inherits. Record the observed values in the task report.

- [ ] **Step 11: Verify the bash-suite `redir` hang is cleared (document result)**

The `/bin/cat & wait` hang is at `redir.tests:162`. Confirm it is cleared by running the real runner from inside the tests dir (a `run-<cat>` invocation from another cwd fails instantly and lies — see the v286 lesson). If the bash 5.2.21 test tree is not present at `/tmp/bash-5.2.21`, note that and rely on the harness guard instead.

```bash
cd /tmp/bash-5.2.21/tests 2>/dev/null && \
  timeout 60 env THIS_SH="$(cd /home/john/projects/huck && pwd)/target/debug/huck" \
  sh ./run-redir >/tmp/redir.log 2>&1; echo "redir runner rc=$?"
```
Expected: the runner no longer hangs on the `/bin/cat & wait` line. Report honestly whether `redir` now passes fully or advances to a different blocker (it may still fail later — that's a separate issue, not a v287 regression).

- [ ] **Step 12: Commit**

```bash
git add crates/huck-engine/src/executor.rs
git commit -m "fix: default async command stdin to /dev/null in run_background_subshell (#126)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the whole-branch review

- The recurring huck lesson: the whole-branch review catches missed SIBLING sites that per-task review misses. For this change, confirm `run_background_subshell` has exactly three callers (lines ~264, ~285, ~563) and all three were updated, and that no OTHER background path hard-codes `STDIN_FILENO` for an async child.
- Confirm `async_default_stdin`'s `DevNull` fd is closed on every path in `run_background_subshell` (both the `Ok` and `Err` fork arms) and never leaked, and that `STDIN_FILENO` is never `close()`d.
- Path A (`run_background_sequence`) is intentionally untouched — deferred to #129 (blocked by #128). Do not "fix" it here.
