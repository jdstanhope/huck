# fd-plumbing Phase 0 (#128 + #129 + fd-torture net) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the two quick-win policy fixes of the fd-plumbing remediation — #128 (don't SIGHUP jobs on non-interactive exit) and #129 (background-pipeline stage-0 stdin follows the async rule) — and build the `fd_torture_diff_check.sh` regression net that guards the later structural phases.

**Architecture:** #128 self-gates `hangup_jobs` on `is_interactive && huponexit` (wiring the existing-but-unread shopt). #129 routes `run_background_sequence`'s stage-0 stdin through the existing `async_default_stdin` helper. The harness lands first as a green regression baseline, then each fix adds its own failing-first case and turns it green. No structural change (that starts at Phase 1).

**Tech Stack:** Rust (crate `huck-engine`), bash-diff harness shell script.

## Global Constraints

- Every commit ends with the trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` (the `(1M context)` parenthetical is canonical).
- Run `cargo fmt --all` before every commit (CI enforces `cargo fmt --all --check`).
- Build the binary with `cargo build -p huck` (NOT `--workspace`).
- Run tests per-crate, single-threaded: `cargo test -p <crate> --jobs 1 --lib -- --test-threads 1` (this box OOM-kills on `cargo test --workspace`).
- Guard any full bash-diff sweep with `ulimit -v 1500000` and `timeout`.
- Harnesses under `tests/scripts/*_diff_check.sh` are auto-discovered by `run_diff_checks.sh`; each uses its OWN default binary — do NOT override `HUCK_BIN`.
- Do not push to `main` or self-merge; work lands on a `v289-fd-plumbing-phase0` branch via a PR the user merges.
- Compat target: bash 5.2.21, non-interactive.
- **Model policy (maintainer directive, whole remediation arc): dispatch implementer AND reviewer subagents on Opus** — this is foundational multi-phase work; do not down-tier to save cost.

---

### Task 1: `fd_torture_diff_check.sh` — the green-now regression net

**Files:**
- Create: `tests/scripts/fd_torture_diff_check.sh`

**Interfaces:**
- Consumes: the `huck` debug binary at `target/debug/huck`.
- Produces: an executable harness that PASSES fully on the current binary (its role is a regression net for Phases 1–5). #128/#129 cases are added in Tasks 2/3. Auto-discovered by `run_diff_checks.sh`.

- [ ] **Step 1: Build the current huck binary**

Run: `cargo build -p huck`
Expected: `Finished`.

- [ ] **Step 2: Write the harness**

Create `tests/scripts/fd_torture_diff_check.sh` with this content:

```bash
#!/usr/bin/env bash
# fd-plumbing remediation regression net (review: docs/superpowers/reviews/
# 2026-07-13-engine-fd-plumbing-review.md). Concentrated fd/redirect/pipeline/
# background matrix, restricted to behavior huck ALREADY matches bash on, so it is
# green today and its job is to catch REGRESSIONS as Phases 1-5 rework the fd
# machinery. #128 (no-hangup) and #129 (bg stdin) cases are added by their tasks.
# Deliberately excluded until their fixing phase: `exec <&-; cat < file | cat`
# (#132) and stage redirect source-order (#50).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
printf 'FA\n' > "$WORK/inA"

norm() { sed -E 's#^([^:]*/)?(bash|huck): (line [0-9]+: )?##'; }

# Byte-identical bash vs huck, both timeout-wrapped so a regression that
# reintroduces a hang FAILs instead of wedging. cwd is $WORK for relative paths.
check() {
    local label="$1" frag="$2" b h
    b=$(cd "$WORK" && timeout 5 bash        -c "$frag" </dev/null 2>&1; echo "rc=$?"); b=$(printf '%s\n' "$b" | norm)
    h=$(cd "$WORK" && timeout 5 "$HUCK_BIN" -c "$frag" </dev/null 2>&1; echo "rc=$?"); h=$(printf '%s\n' "$h" | norm)
    b=${b//$WORK/@W@}; h=${h//$WORK/@W@}
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- freed std fds x pipelines (post-v288 correct) ---
check "freed fd0: cat|cat"        'exec <&-; cat | cat; echo end'
check "freed fd0: cat|cat|cat"    'exec <&-; cat | cat | cat; echo end'
check "freed fd2: err|cat"        'exec 2>&-; ls /no_such_xyz | cat; echo "end=$?"'
# --- per-stage redirects on a non-last stage ---
check "stage stdout redirect"     'echo hi > f | cat; cat f'
check "stage stderr redirect"     'ls /no_such_xyz 2>e | cat; cat e'
check "fd>2 dup"                  'exec 3>f; echo x >&3; exec 3>&-; cat f'
# --- heredoc / here-string into a stage ---
check "heredoc into stage"        $'cat <<EOF | cat\nh1\nh2\nEOF'
check "herestring into stage"     'cat <<< "hs" | cat'
# --- subshell / group redirects ---
check "subshell redirect"         '(echo hi) 2>&1 | cat'
check "group redirect"            '{ echo hi; } > f; cat f'
# --- dup / close / merge ---
check "1>&2 to stderr"            'echo hi 1>&2 2>/dev/null; echo done'
check "2>&1 merge into pipe"      'ls /no_such_xyz 2>&1 | cat'
check "&> merge to file"          'echo hi &> f; cat f'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 3: Make it executable**

Run: `chmod +x tests/scripts/fd_torture_diff_check.sh`

- [ ] **Step 4: Run it — it MUST be fully green on the current binary**

Run: `bash tests/scripts/fd_torture_diff_check.sh`
Expected: `Total: 13, Pass: 13, Fail: 0`.
If any case FAILS, it is either a real current divergence (then REMOVE that case and note it in the report — the net must be green-now) or a harness bug (fix it). Do not commit a red net.

- [ ] **Step 5: Commit**

```bash
git add tests/scripts/fd_torture_diff_check.sh
git commit -m "test: fd-torture regression net for the fd-plumbing remediation

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: #128 — don't SIGHUP jobs on non-interactive exit

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` — `hangup_jobs` (line 3046).
- Modify: `tests/scripts/fd_torture_diff_check.sh` — add the no-hangup functional check.

**Interfaces:**
- Consumes: `self.is_interactive: bool`, `self.shopt_options.get("huponexit") -> Option<bool>` (both on the `Shell`/shell-state type that owns `hangup_jobs`).
- Produces: `hangup_jobs` becomes a no-op unless `is_interactive && huponexit`.

- [ ] **Step 1: Add the failing no-hangup check to the harness**

Append to `tests/scripts/fd_torture_diff_check.sh` BEFORE the final `echo`/`exit`:

```bash
# --- #128: a non-interactive shell must NOT SIGHUP background jobs at exit ---
# The child writes a file after a short sleep while the shell exits immediately;
# bash leaves it running (file appears), huck must match. Poll after exit.
nohangup() {
    local label="$1" shbin="$2" out
    rm -f "$WORK/bg.out"
    timeout 5 "$shbin" -c 'sleep 0.3 && echo alive > "'"$WORK"'/bg.out" & exit 0' </dev/null >/dev/null 2>&1
    for _ in 1 2 3 4 5 6 7 8 9 10; do [ -s "$WORK/bg.out" ] && break; sleep 0.1; done
    [ -s "$WORK/bg.out" ] && echo alive || echo KILLED
}
b=$(nohangup bg bash); h=$(nohangup bg "$HUCK_BIN")
if [[ "$b" == alive && "$h" == "$b" ]]; then printf 'PASS: #128 bg child survives non-interactive exit\n'; PASS=$((PASS+1))
else printf 'FAIL: #128 bg child survives (bash=%s huck=%s)\n' "$b" "$h"; FAIL=$((FAIL+1)); fi
```

- [ ] **Step 2: Run the harness — confirm the #128 case FAILS**

Run: `bash tests/scripts/fd_torture_diff_check.sh`
Expected: overall FAIL, with `FAIL: #128 bg child survives … (bash=alive huck=KILLED)`. The 13 baseline cases still PASS.

- [ ] **Step 3: Gate `hangup_jobs`**

In `crates/huck-engine/src/shell_state.rs`, at the top of `hangup_jobs` (line 3046, immediately inside the `{`), insert:

```rust
        // bash: SIGHUP jobs at exit only for an interactive shell with `huponexit`
        // set (default off). Non-interactive shells (and interactive shells without
        // huponexit) leave background jobs running. This also wires the huponexit
        // shopt, which was defined but never read. (#128)
        if !(self.is_interactive && self.shopt_options.get("huponexit").unwrap_or(false)) {
            return;
        }
```

The rest of `hangup_jobs` (the per-job `should_hangup` loop) is unchanged.

- [ ] **Step 4: Format, build, and confirm the harness is green**

Run: `cargo fmt --all && cargo build -p huck`
Then: `bash tests/scripts/fd_torture_diff_check.sh`
Expected: `Total: 14, Pass: 14, Fail: 0`.

- [ ] **Step 5: Confirm the existing `should_hangup` unit tests still pass**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 shell_state`
Expected: pass (the per-job predicate is unchanged; no new unit test — the guard sends real signals and is covered functionally by the harness).

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/shell_state.rs tests/scripts/fd_torture_diff_check.sh
git commit -m "fix: don't SIGHUP jobs on non-interactive exit; wire huponexit (#128)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: #129 — background-pipeline stage-0 stdin follows the async rule

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` — `run_background_sequence` (the `devnull_fd` block at ~3189–3208 and its uses at ~3237, ~3473, ~3477).
- Modify: `tests/scripts/fd_torture_diff_check.sh` — add the Path-A bg-stdin cases.

**Interfaces:**
- Consumes: `async_default_stdin(inherit: bool, shell, sink, err_sink) -> Result<AsyncStdin, ()>` and `enum AsyncStdin { Inherit, DevNull(RawFd) }` (executor.rs ~3060), `pipeline.commands.len()`, `libc::STDIN_FILENO`, `parent_held: Vec<RawFd>`.
- Produces: `run_background_sequence` stage-0 stdin obeys the async rule (bare multi-stage pipeline / interactive → inherit; single-stage non-interactive → `/dev/null`).

- [ ] **Step 1: Add the failing Path-A bg-stdin cases (observable now that #128 landed)**

Append to `tests/scripts/fd_torture_diff_check.sh` before the final `echo`/`exit`. These use a TRAILING `&` at end-of-input (Path A = `run_background_sequence`; `& wait` would instead hit the already-correct Path B), and poll the output file after the shell exits (the child survives post-#128):

```bash
# --- #129: run_background_sequence stage-0 stdin async rule (Path A) ---
# Shell stdin is a real file ($WORK/inA); the async child prints readlink of its
# fd0. Single-stage async -> /dev/null (non-interactive); bare multi-stage pipeline
# -> inherits the shell's stdin. Trailing `&` at EOF => Path A; poll after exit.
poll_fd0() {
    local shbin="$1" frag="$2" out
    rm -f "$WORK/fd0.out"
    timeout 5 "$shbin" -c "$frag" < "$WORK/inA" >/dev/null 2>&1
    for _ in 1 2 3 4 5 6 7 8 9 10; do [ -s "$WORK/fd0.out" ] && break; sleep 0.1; done
    cat "$WORK/fd0.out" 2>/dev/null | sed "s#$WORK#@W@#g"
}
p129() {
    local label="$1" frag="$2" b h
    b=$(poll_fd0 bash "$frag"); h=$(poll_fd0 "$HUCK_BIN" "$frag")
    if [[ -n "$b" && "$h" == "$b" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s (bash=[%s] huck=[%s])\n' "$label" "$b" "$h"; FAIL=$((FAIL+1)); fi
}
p129 "#129 single-stage & -> /dev/null" 'readlink /proc/self/fd/0 > "'"$WORK"'/fd0.out" &'
p129 "#129 multi-stage a|b & -> inherit" 'readlink /proc/self/fd/0 | cat > "'"$WORK"'/fd0.out" &'
```

- [ ] **Step 2: Run the harness — confirm the multi-stage #129 case FAILS**

Run: `cargo build -p huck && bash tests/scripts/fd_torture_diff_check.sh`
Expected: overall FAIL. The `#129 multi-stage a|b & -> inherit` case FAILs (bash prints `@W@/inA` (inherited), huck prints `/dev/null`). The `#129 single-stage & -> /dev/null` case already PASSes (huck's current unconditional `/dev/null` happens to match bash for the single-stage non-interactive case). All prior cases still pass.

- [ ] **Step 3: Route stage-0 stdin through `async_default_stdin`**

In `crates/huck-engine/src/executor.rs`, `run_background_sequence`, replace the `devnull_fd` open block (lines ~3189–3208):

```rust
    // Open /dev/null once for the first stage's default stdin.
    let devnull_fd: RawFd = {
        use std::os::unix::io::IntoRawFd;
        match File::open("/dev/null") {
            Ok(f) => f.into_raw_fd(),
            Err(e) => {
                {
                    let mut err = err_writer(err_sink, sink);
                    crate::sh_error_to!(
                        shell,
                        &mut *err,
                        None,
                        "/dev/null: {}",
                        crate::bash_io_error(&e)
                    );
                }
                return ExecOutcome::Continue(1);
            }
        }
    };
    parent_held.push(devnull_fd);
```

with (bash: async stage-0 stdin defaults to /dev/null unless a bare multi-stage
pipeline or an interactive shell, in which case it inherits — the same rule
`run_background_subshell` uses):

```rust
    // Stage-0 stdin default (async rule, shared with run_background_subshell): a
    // bare multi-stage pipeline's stage 0 inherits the shell's stdin; a single
    // async command gets /dev/null when non-interactive; interactive always
    // inherits. (#129)
    let stage0_stdin_default: RawFd =
        match async_default_stdin(pipeline.commands.len() > 1, shell, sink, err_sink) {
            Ok(AsyncStdin::Inherit) => libc::STDIN_FILENO,
            Ok(AsyncStdin::DevNull(fd)) => {
                parent_held.push(fd);
                fd
            }
            Err(()) => return ExecOutcome::Continue(1),
        };
```

Then rename the three `devnull_fd` uses to `stage0_stdin_default`:
- line ~3237: `let stdin_fd = devnull_fd;` → `let stdin_fd = stage0_stdin_default;`
- line ~3473: `_ => prev_pipe_read.take().unwrap_or(devnull_fd),` → `... .unwrap_or(stage0_stdin_default),`
- line ~3477: `prev_pipe_read.take().unwrap_or(devnull_fd)` → `... .unwrap_or(stage0_stdin_default)`

(`STDIN_FILENO` is never pushed to `parent_held`, so teardown never closes fd 0; the `DevNull` fd is pushed and closed exactly as `devnull_fd` was.)

- [ ] **Step 4: Format, build, and confirm the harness is green**

Run: `cargo fmt --all && cargo build -p huck`
Then: `bash tests/scripts/fd_torture_diff_check.sh`
Expected: `Total: 16, Pass: 16, Fail: 0`.

- [ ] **Step 5: Manual interactive-gate spot check (document result)**

Not automatable in a byte-diff harness (needs a tty). Verify via a `script` pty that an interactive huck lets a bare `cmd &` inherit the terminal (fd0 is a pts, not `/dev/null`), matching bash. Record the observed values in the task report; not a gate.

- [ ] **Step 6: Run the engine lib tests + full sweep**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`
Expected: all pass (≈1795 passed; 0 failed).

Run: `ulimit -v 1500000; timeout 300 bash tests/scripts/run_diff_checks.sh`
Expected: every harness green, including `fd_torture_diff_check.sh`; no `pipe_*`/`pipeline_*`/`func_*` regression.

- [ ] **Step 7: Commit**

```bash
git add crates/huck-engine/src/executor.rs tests/scripts/fd_torture_diff_check.sh
git commit -m "fix: bg-pipeline stage-0 stdin follows the async rule (#129)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the whole-branch review

- Confirm the #128 guard reads `is_interactive && huponexit` and that BOTH exit callers (`shell.rs:281`, `repl.rs:284`) get correct behavior from the single guard; confirm no SIGHUP-signal-handler path needs an unconditional hangup (there is none today — only clean-exit callers).
- Confirm #129 threads the `DevNull` fd into `parent_held` and closes it exactly where `devnull_fd` was, and that `STDIN_FILENO` (Inherit) is never closed — mirror of the run_multi_stage/run_background_subshell lifecycle.
- Confirm the fd_torture net is green and its excluded cases (#132, #50) are documented in the harness header, not silently dropped.
- Out of scope by design: the `login_shell` refinement of huponexit; any structural fd change (Phases 1–5).
