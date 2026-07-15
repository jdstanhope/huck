# Pipeline-stage spawn failure — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement
> this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An external pipeline stage whose program can't be run prints `<name>: command not found` (127)
or `<name>: <reason>` (126) to its OWN redirected fd 2, exits that stage 126/127, and lets the pipeline
continue with a populated `PIPESTATUS` — matching bash 5.2.21.

**Architecture:** Detect an unrunnable stage before spawning (`classify_stage_runnability`), and fork a
diagnostic child (`spawn_command_error_stage`, modeled on v296's `spawn_failed_stage`) that installs the
stage's stdio + replays its `ChildRedirPlan`, then `write(2, diag)` and `_exit(code)`. Wire the check
into `spawn_external_with_fds` so it returns `Ok(pid)` for the diagnostic child — the caller treats it
as a normal stage, no abort. Extract the proven child stdio-install into a shared `install_child_stdio`.

**Tech Stack:** Rust, `libc` (fork/dup2/close/write/_exit/access), the existing `ChildStdio`/`ChildFd`
(`crate::child_fd`), `ChildRedirPlan`/`ChildRedirOp`, `builtins::search_path_for`, `crate::bash_io_error`,
`crate::emit_error_to`. Everything is in `crates/huck-engine/src/executor.rs` unless noted.

## Global Constraints

- Commit trailer, verbatim: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- `cargo fmt --all` before every commit; CI enforces `cargo fmt --all --check`.
- Build the binary with `cargo build -p huck` (never `--workspace`; the box OOMs). Engine lib tests:
  `( ulimit -v 2500000; timeout 900 cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 )`.
  Integration bins: `cargo test -p huck --test <name> --jobs 1 -- --test-threads 1`.
- Bash-diff harnesses assert BYTE-IDENTICAL output vs bash 5.2.21 (`ubuntu-24.04`); prologue
  normalization (`bash:`/`huck:` → `SH:`) is expected.
- Child-branch code after `fork()` must be async-signal-safe: only `setpgid`/`dup2`/`close`/`write`/
  `_exit`/`access` (raw libc), and NO `OwnedFd` destructor may run in the child (`into_raw()` first) —
  the `crate::child_fd` fork/`pre_exec` Drop-safety contract.
- Exit codes: **127** = command not found; **126** = found but not executable / is-a-directory.

---

## Task 1: Extract `install_child_stdio` (behavior-preserving refactor)

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (`fork_and_run_in_subshell` child branch, ~lines 7695–7742)

**Interfaces:**
- Produces: `unsafe fn install_child_stdio(stdio: ChildStdio) -> [RawFd; 3]` — installs the three stdio
  slots onto fds 0/1/2 in a just-forked child (overlap-safe), returns the ORIGINAL raw source fd numbers
  (`-1` for `Inherit`) for the caller's parent-fd close-exclusion. Consumed by Task 3.

- [ ] **Step 1: Add the shared helper.** Insert this free function just above `fn fork_and_run_in_subshell`
  (near the other spawn helpers). It is the verbatim Pass-1/Pass-2 logic currently inside the child branch:

```rust
/// Install a `ChildStdio` onto fds 0/1/2 inside a just-forked child, overlap-safe.
///
/// Returns the ORIGINAL raw source fd numbers (pre-move; `-1` for `Inherit`) so the
/// caller can exclude this child's own stdio sources when closing parent-held pipe
/// fds. async-signal-safe: `into_raw()` first (no `OwnedFd` Drop in the child), then
/// pure `fcntl`/`dup2`/`close`.
///
/// # Safety
/// Must be called only in the child after `fork()`, before any `OwnedFd` could drop.
unsafe fn install_child_stdio(stdio: ChildStdio) -> [RawFd; 3] {
    let ChildStdio {
        stdin,
        stdout,
        stderr,
    } = stdio;
    let mut plan: [(Option<RawFd>, RawFd); 3] = [
        (stdin.into_raw(), 0),
        (stdout.into_raw(), 1),
        (stderr.into_raw(), 2),
    ];
    let original_raws: [RawFd; 3] = [
        plan[0].0.unwrap_or(-1),
        plan[1].0.unwrap_or(-1),
        plan[2].0.unwrap_or(-1),
    ];
    // Pass 1 (PRE-MOVE): move any owned source in 0..=2 up to >=3, so pass 2's
    // dup2 always has source != target (clears FD_CLOEXEC): the moved copy must
    // survive exec if its install no-ops.
    for (src, _) in plan.iter_mut() {
        if let Some(s) = *src
            && s <= 2
        {
            let moved = unsafe { libc::fcntl(s, libc::F_DUPFD, 3) };
            if moved >= 0 {
                unsafe { libc::close(s) };
                *src = Some(moved);
            }
        }
    }
    // Pass 2 (INSTALL): sources now all >=3 and pairwise distinct (except the
    // pathological case where a pass-1 F_DUPFD failed and left an owned source at
    // its own slot — a no-op dup2 we must NOT then close).
    for (src, slot) in plan {
        if let Some(s) = src {
            unsafe { libc::dup2(s, slot) };
            if s != slot {
                unsafe { libc::close(s) };
            }
        }
    }
    original_raws
}
```

- [ ] **Step 2: Replace the inlined block in `fork_and_run_in_subshell`.** In the child branch, replace
  the `let ChildStdio { … }` destructure through the end of "Pass 2 (INSTALL)" loop (the block that
  builds `plan`, `original_raws`, Pass 1, Pass 2) with a single call. The subsequent "Pass 3" parent-fd
  close loop and the `stdout_dup_target`/`stderr_dup_target` dup2s are UNCHANGED and still use
  `original_raws`:

```rust
            // 3-5. Install stdio onto 0/1/2 (overlap-safe). Returns the original
            // raw source fds for the pass-3 close-exclusion below.
            let original_raws = unsafe { install_child_stdio(stdio) };
            // Pass 3: close every parent-held pipe fd, skipping this child's own
            // stdio sources by their ORIGINAL numbers.
            for &fd in parent_fds_to_close {
                if fd != original_raws[0] && fd != original_raws[1] && fd != original_raws[2] {
                    libc::close(fd);
                }
            }
```

  (Keep everything after — the `stdout_dup_target`/`stderr_dup_target` block, the `clear_for_subshell`,
  etc. — exactly as-is. The `unsafe` block boundaries may need adjusting: `install_child_stdio` is itself
  `unsafe fn`, call it inside the existing `unsafe { … }`.)

- [ ] **Step 3: Build + verify no behavior change.**

Run: `cargo build -p huck 2>&1 | grep -E "^error|^warning" || echo clean`
Expected: `clean`.

- [ ] **Step 4: Run the suites that cover the refactored path.**

Run:
```
cargo fmt --all
( ulimit -v 2500000; timeout 900 cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -2 )
for t in subshell_integration subshell_pipeline_integration pipeline_subshell_integration compound_redirects_integration heredoc_forked_writer_integration; do
  ( ulimit -v 2500000; timeout 200 cargo test -p huck --test $t --jobs 1 -- --test-threads 1 2>&1 | grep "test result:" )
done
```
Expected: engine lib `ok` (same count as before, ~1812), every integration bin `ok. N passed; 0 failed`.

- [ ] **Step 5: Commit.**

```bash
git add crates/huck-engine/src/executor.rs
git commit -m "refactor: extract install_child_stdio from fork_and_run_in_subshell (#78)

Behavior-preserving: hoist the overlap-safe stdio-onto-0/1/2 install (Pass 1
pre-move + Pass 2 dup2) into a shared unsafe fn, to be reused by the #78
diagnostic child. fork_and_run_in_subshell's Pass 3 + dup_target logic unchanged.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `classify_stage_runnability` + unit tests

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (add enum + fn near `classify_stage`, ~line 7885)
- Test: `crates/huck-engine/src/executor.rs` `#[cfg(test)] mod tests` (find it via
  `grep -n "mod tests" crates/huck-engine/src/executor.rs` — the file ends with `mod tests;` including
  an external file, OR an inline module; add the tests to the inline test module if present, else to
  `crates/huck-engine/src/executor/tests.rs`).

**Interfaces:**
- Produces:
  ```rust
  enum StageRunnability { Runnable, NotRunnable { body: String, code: i32 } }
  fn classify_stage_runnability(program: &str, shell: &Shell) -> StageRunnability
  ```
  Consumed by Task 3.
- Consumes: `builtins::search_path_for(name, shell) -> Option<PathBuf>` and `crate::bash_io_error`.

- [ ] **Step 1: Write the failing tests.** Add to the executor test module. (Use `/etc` for the
  directory case and a `chmod 644` temp file for the non-executable case; `std::env::temp_dir()`.)

```rust
#[test]
fn classify_runnability_bare_not_found_is_127() {
    let shell = Shell::new();
    match classify_stage_runnability("definitely_no_such_cmd_xyz", &shell) {
        StageRunnability::NotRunnable { body, code } => {
            assert_eq!(code, 127);
            assert_eq!(body, "definitely_no_such_cmd_xyz: command not found");
        }
        _ => panic!("expected NotRunnable"),
    }
}

#[test]
fn classify_runnability_slash_not_found_is_127_no_such_file() {
    let shell = Shell::new();
    match classify_stage_runnability("/no/such/path/xyz", &shell) {
        StageRunnability::NotRunnable { body, code } => {
            assert_eq!(code, 127);
            assert_eq!(body, "/no/such/path/xyz: No such file or directory");
        }
        _ => panic!("expected NotRunnable"),
    }
}

#[test]
fn classify_runnability_directory_is_126_is_a_directory() {
    let shell = Shell::new();
    match classify_stage_runnability("/etc", &shell) {
        StageRunnability::NotRunnable { body, code } => {
            assert_eq!(code, 126);
            assert_eq!(body, "/etc: Is a directory");
        }
        _ => panic!("expected NotRunnable"),
    }
}

#[test]
fn classify_runnability_non_executable_is_126_permission_denied() {
    use std::io::Write as _;
    let mut p = std::env::temp_dir();
    p.push("huck_classify_noexec_test");
    {
        let mut f = std::fs::File::create(&p).unwrap();
        writeln!(f, "#x").unwrap();
    }
    std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    let shell = Shell::new();
    let ps = p.to_str().unwrap();
    match classify_stage_runnability(ps, &shell) {
        StageRunnability::NotRunnable { body, code } => {
            assert_eq!(code, 126);
            assert_eq!(body, format!("{ps}: Permission denied"));
        }
        other => panic!("expected NotRunnable, got {other:?}"),
    }
    let _ = std::fs::remove_file(&p);
}

#[test]
fn classify_runnability_existing_binary_is_runnable() {
    let shell = Shell::new();
    // /bin/sh exists and is executable on the CI target.
    assert!(matches!(
        classify_stage_runnability("/bin/sh", &shell),
        StageRunnability::Runnable
    ));
    // and a bare name found on PATH
    assert!(matches!(
        classify_stage_runnability("sh", &shell),
        StageRunnability::Runnable
    ));
}
```

  Add `#[derive(Debug)]` to `StageRunnability` so `panic!("{other:?}")` compiles.

- [ ] **Step 2: Run the tests to confirm they FAIL (fn not defined).**

Run: `( ulimit -v 2500000; cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 classify_runnability 2>&1 | tail -5 )`
Expected: compile error `cannot find function classify_stage_runnability` (RED).

- [ ] **Step 3: Implement the enum + function.** Add near `classify_stage`:

```rust
/// Whether an external pipeline stage's program can be run, and if not, the
/// bash diagnostic body + exit code (127 not-found, 126 found-but-not-executable).
#[derive(Debug)]
enum StageRunnability {
    Runnable,
    NotRunnable { body: String, code: i32 },
}

/// Classify a resolved program string the way bash's command search + `execve`
/// would, so an unrunnable stage becomes a 126/127 diagnostic child (#78).
fn classify_stage_runnability(program: &str, shell: &Shell) -> StageRunnability {
    if program.contains('/') {
        match std::fs::metadata(program) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => StageRunnability::NotRunnable {
                body: format!("{program}: No such file or directory"),
                code: 127,
            },
            Err(e) => StageRunnability::NotRunnable {
                body: format!("{program}: {}", crate::bash_io_error(&e)),
                code: 126,
            },
            Ok(md) if md.is_dir() => StageRunnability::NotRunnable {
                body: format!("{program}: Is a directory"),
                code: 126,
            },
            Ok(_) => {
                // Executable bit? Use a real access(X_OK) so the errno text matches libc.
                let c = std::ffi::CString::new(program).unwrap_or_default();
                if unsafe { libc::access(c.as_ptr(), libc::X_OK) } == 0 {
                    StageRunnability::Runnable
                } else {
                    let e = std::io::Error::last_os_error();
                    StageRunnability::NotRunnable {
                        body: format!("{program}: {}", crate::bash_io_error(&e)),
                        code: 126,
                    }
                }
            }
        }
    } else if builtins::search_path_for(program, shell).is_some() {
        StageRunnability::Runnable
    } else {
        StageRunnability::NotRunnable {
            body: format!("{program}: command not found"),
            code: 127,
        }
    }
}
```

- [ ] **Step 4: Run the tests to confirm they PASS.**

Run: `( ulimit -v 2500000; cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 classify_runnability 2>&1 | tail -4 )`
Expected: `test result: ok. 5 passed; 0 failed`.

- [ ] **Step 5: Commit.**

```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs
git commit -m "feat: classify_stage_runnability for pipeline spawn diagnostics (#78)

Classify a resolved program as Runnable or NotRunnable{body,code} mirroring
bash's command search + execve outcomes (127 not-found, 126 dir/non-exec),
with the errno text from a real stat/access so it matches libc. Unit-tested.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `spawn_command_error_stage` + wire into `spawn_external_with_fds` + diff-check harness

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (add `spawn_command_error_stage`; edit
  `spawn_external_with_fds`, ~lines 7938–7957)
- Create: `tests/scripts/pipeline_stage_spawn_fail_diff_check.sh`

**Interfaces:**
- Consumes: `install_child_stdio` (Task 1), `classify_stage_runnability`/`StageRunnability` (Task 2),
  `crate::emit_error_to`, `ChildRedirOp`, `replay_redir_ops`, `ChildStdio`.
- Produces: correct pipeline behavior (gated by the new harness).

- [ ] **Step 1: Write the failing harness.** Create `tests/scripts/pipeline_stage_spawn_fail_diff_check.sh`:

```bash
#!/usr/bin/env bash
# v304 (#78): an external pipeline stage whose program can't be run must print
# `<name>: <reason>` to its OWN redirected fd 2, exit 126/127, and let the
# pipeline continue with a populated PIPESTATUS — matching bash 5.2.21.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

NOEXEC=$(mktemp); printf '#x\n' > "$NOEXEC"; chmod 644 "$NOEXEC"
trap 'rm -f "$NOEXEC" /tmp/hk78_e' EXIT

FAIL=0
# Strip the shell-name + `line N:` prologue GLOBALLY (not just at line start):
# the 2>&1-capture case embeds the diagnostic inside `cap=[...]`, so an anchored
# match would miss it and the byte-compare would fail on the prog-name difference.
norm() { sed -e "s#$HUCK: line [0-9]*: #SH: #g" -e "s#$HUCK: #SH: #g" \
             -e 's#bash: line [0-9]*: #SH: #g' -e 's#bash: #SH: #g'; }
check() {
  local label=$1 frag=$2 b h
  b=$( { timeout 10 bash    -c "$frag"; echo "rc=$?"; } 2>&1 | norm )
  h=$( { timeout 10 "$HUCK" -c "$frag"; echo "rc=$?"; } 2>&1 | norm )
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"; echo "  bash: $(printf '%s' "$b" | tr '\n' '|')"; echo "  huck: $(printf '%s' "$h" | tr '\n' '|')"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

check 'notfound-last'   'echo hi | nosuchcmd; echo "rc=$? ps=${PIPESTATUS[*]}"'
check 'notfound-middle' 'echo hi | nosuchcmd | cat; echo "ps=${PIPESTATUS[*]}"'
check 'capture-2>&1'    'x=$(echo hi | nosuchcmd 2>&1); echo "cap=[$x] rc=$?"'
check 'redir-2>file'    'echo hi | nosuchcmd 2>/tmp/hk78_e; echo "rc=$?"; echo "efile=[$(cat /tmp/hk78_e)]"'
check 'nonexec-126'     "echo hi | $NOEXEC; echo \"rc=\$? ps=\${PIPESTATUS[*]}\""
check 'directory-126'   'echo hi | /etc; echo "rc=$? ps=${PIPESTATUS[*]}"'
check 'slash-notfound'  'echo hi | /no/such/x; echo "rc=$? ps=${PIPESTATUS[*]}"'
check 'pipefail'        'set -o pipefail; echo hi | nosuchcmd | cat; echo "rc=$?"'

if [ $FAIL -ne 0 ]; then echo "pipeline_stage_spawn_fail_diff_check FAILED" >&2; exit 1; fi
echo "pipeline_stage_spawn_fail_diff_check OK"
```

- [ ] **Step 2: Confirm the harness FAILS on the current binary (RED).**

Run: `cargo build -p huck && chmod +x tests/scripts/pipeline_stage_spawn_fail_diff_check.sh && tests/scripts/pipeline_stage_spawn_fail_diff_check.sh; echo "harness-rc=$?"`
Expected: multiple `FAIL [...]` lines, `harness-rc=1`. (If the prologue `line N:` differs between the
two shells on these cases, extend `norm` to strip `line N:` — note it in the report; do NOT change the
assertions themselves.)

- [ ] **Step 3: Add `spawn_command_error_stage`.** Place it next to `spawn_failed_stage`:

```rust
/// #78: fork a stand-in child for an external stage whose program can't be run.
/// It installs the stage's stdio + replays the stage's redirect plan (so fd 2
/// points wherever `2>&1`/`2>file`/the pipe put it), writes the bash-formatted
/// `diag` to fd 2, and `_exit`s `exit_code` (127 not-found / 126 not-executable).
/// This mirrors bash's child-side diagnostic and lets the pipeline continue with
/// a populated PIPESTATUS. `held` (the plan's opened redirect-target fds) is
/// inherited by the child and dropped in the parent after fork.
fn spawn_command_error_stage(
    stdio: ChildStdio,
    pgid_target: i32,
    parent_fds_to_close: &[RawFd],
    replay_ops: Vec<ChildRedirOp>,
    held: Vec<std::os::fd::OwnedFd>,
    diag: Vec<u8>,
    exit_code: i32,
) -> Result<i32, io::Error> {
    flush_stdout();
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        unsafe {
            if pgid_target >= 0 {
                libc::setpgid(0, pgid_target);
            }
            // Install stdio onto 0/1/2 (overlap-safe); returns original sources
            // for the parent-fd close-exclusion.
            let original_raws = install_child_stdio(stdio);
            // Replay the stage's redirects (2>&1 / 2>file / fd>2 / close) in
            // source order, AFTER stdio install so 2>&1 sees the piped fd 1.
            let _ = replay_redir_ops(&replay_ops);
            // Fds this stage's replay claimed as targets — don't close them.
            let extra_targets: Vec<RawFd> = replay_ops
                .iter()
                .map(|op| match *op {
                    ChildRedirOp::Dup { target, .. } | ChildRedirOp::Close { target } => target,
                })
                .collect();
            for &fd in parent_fds_to_close {
                if fd != original_raws[0]
                    && fd != original_raws[1]
                    && fd != original_raws[2]
                    && !extra_targets.contains(&fd)
                {
                    libc::close(fd);
                }
            }
            // Write the diagnostic to fd 2 (now redirected as the stage asked).
            if !diag.is_empty() {
                libc::write(2, diag.as_ptr() as *const libc::c_void, diag.len());
            }
            libc::_exit(exit_code);
        }
    }
    // PARENT: the child inherited its own copies of `held`; drop ours.
    drop(held);
    if pgid_target >= 0 {
        unsafe {
            libc::setpgid(pid, pgid_target);
        }
    }
    Ok(pid)
}
```

- [ ] **Step 4: Wire the check into `spawn_external_with_fds`.** Right AFTER the `resolve(...)` call
  (currently `let resolved = resolve(...)?;`) and BEFORE the `if shell.shell_options.xtrace { ... }`
  block, insert:

```rust
    // #78: if the program can't be run, don't spawn — fork a diagnostic child
    // that prints `<name>: <reason>` to the stage's own (redirected) fd 2 and
    // exits 126/127, so the message routes correctly and PIPESTATUS is populated
    // (matching bash) instead of leaking a raw error and aborting the pipeline.
    if let StageRunnability::NotRunnable { body, code } =
        classify_stage_runnability(&resolved.program, shell)
    {
        let mut diag: Vec<u8> = Vec::new();
        crate::emit_error_to(shell, &mut diag, None, format_args!("{body}"));
        return spawn_command_error_stage(
            stdio,
            pgid_target,
            parent_fds_to_close,
            plan.ops,
            plan.held,
            diag,
            code,
        );
    }
```

  Note: this consumes `stdio`, `plan.ops`, `plan.held` on the NotRunnable path (early return), so it
  must sit before those are moved into `replay_ops`/`held`/the `ProcessCommand`. Since it returns early,
  the xtrace block and the whole spawn body are skipped (bash does not xtrace a not-found command).

- [ ] **Step 5: Build.**

Run: `cargo build -p huck 2>&1 | grep -E "^error|^warning" || echo clean`
Expected: `clean`.

- [ ] **Step 6: Confirm the harness now PASSES (GREEN).**

Run: `tests/scripts/pipeline_stage_spawn_fail_diff_check.sh`
Expected: all 8 `PASS [...]`, `pipeline_stage_spawn_fail_diff_check OK`.

- [ ] **Step 7: Confirm the v296 sibling harness still passes (no regression to the redirect-failure path).**

Run: `tests/scripts/pipeline_stage_redirect_fail_diff_check.sh 2>&1 | tail -3`
Expected: its existing `OK`.

- [ ] **Step 8: Commit.**

```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs tests/scripts/pipeline_stage_spawn_fail_diff_check.sh
git commit -m "feat(#78): pipeline-stage spawn failure prints child-side diagnostic, no abort

An unrunnable external stage now forks a diagnostic child (spawn_command_error_stage)
that installs the stage's fds + replays its redirect plan, writes `<name>: <reason>`
to its own fd 2, and _exits 126/127. Wired into spawn_external_with_fds via
classify_stage_runnability so the caller treats it as a normal stage: the message
routes to 2>&1/2>file/the pipe, PIPESTATUS is populated, and the pipeline continues
— matching bash. Gated by pipeline_stage_spawn_fail_diff_check.sh (8 cases, RED->GREEN).

Closes #78

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Whole-branch verification

**Files:** none (verification only).

- [ ] **Step 1: fmt + both binaries.**

Run:
```
cargo fmt --all --check && echo FMT-CLEAN
cargo build -p huck 2>&1 | grep -E "^error|^warning" || echo "debug clean"
cargo build --release --locked -p huck 2>&1 | grep -E "^error|^warning" || echo "release clean"
```
Expected: `FMT-CLEAN`, `debug clean`, `release clean`.

- [ ] **Step 2: Engine lib suite.**

Run: `( ulimit -v 2500000; timeout 900 cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | grep "test result:" | tail -1 )`
Expected: `ok. N passed; 0 failed` (N ≈ 1817 = prior 1812 + 5 new).

- [ ] **Step 3: Pipeline/redirect integration bins.**

Run:
```
for t in subshell_pipeline_integration pipeline_subshell_integration subshell_integration compound_redirects_integration external_fd_redirects_integration captured_pipeline_drain_integration pipefail_integration; do
  ( ulimit -v 2500000; timeout 200 cargo test -p huck --test $t --jobs 1 -- --test-threads 1 2>&1 | grep "test result:" )
done
```
Expected: every line `ok. N passed; 0 failed`.

- [ ] **Step 4: Full diff sweep, both binaries.**

Run: `( ulimit -v 1500000; timeout 1100 tests/scripts/run_diff_checks.sh 2>&1 | tail -3 )`
Expected: `Diff-check sweep: N passed, 0 failed` (N = prior 198 + 1 new = 199), including
`pipeline_stage_spawn_fail_diff_check.sh` and the unchanged `pipeline_stage_redirect_fail_diff_check.sh`.

- [ ] **Step 5: No commit** (verification only). Report the numbers.

---

## Self-review notes (author)

- **Spec coverage:** Task 2 = Component 1; Task 3 = Components 2+3 + testing; Task 1 = the shared-helper
  extraction the spec's Component 2 step 2 requires. All spec sections map to a task.
- **Type consistency:** `StageRunnability`/`classify_stage_runnability` (Task 2) are consumed verbatim in
  Task 3 step 4; `install_child_stdio` (Task 1) is consumed in Task 3 step 3; `spawn_command_error_stage`
  signature matches its call in step 4 (`stdio, pgid_target, parent_fds_to_close, plan.ops, plan.held,
  diag, code`).
- **Residuals (from spec):** post-pre-check `process.spawn()` failures keep the existing caller bail path;
  ENOEXEC unaddressed. Single-command `2>file`/rc-126 gaps → separate follow-on issue (controller files it).
