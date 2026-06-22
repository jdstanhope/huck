# v206 `Engine::exec` sandbox knobs — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add three per-call sandbox knobs to `ExecBuilder` — `.cwd(path)`, `.restricted(true)`, `.timeout(dur)` — so embedders can constrain generated/untrusted shell.

**Architecture:** Three independent state additions on `Shell` (`restricted: bool`, `timeout_flag: Arc<AtomicBool>`, `live_external_children: Arc<Mutex<Vec<libc::pid_t>>>`); three small helper modules (`cwd_scope.rs`, `restricted.rs`, `timeout.rs`); one enum widening (`ExecOutcome::Interrupted(InterruptReason)`); three builder methods that compose at `ExecBuilder::run_with_sinks` in a fixed order — sinks → timer → cwd → stdin → restricted → run → restore restricted → drop stdin → drop cwd → cancel timer → override 124.

**Tech Stack:** Rust 2021, `libc` for SIGTERM + pid_t, `std::thread` + `mpsc::channel` for the timer, `std::sync::{Arc, Mutex, atomic::AtomicBool}` for cross-thread state. No new deps.

**Branch:** Implement on `v206-engine-sandbox`. Each task ends with a green-suite commit.

**Spec:** `docs/superpowers/specs/2026-06-22-engine-sandbox-design.md`.

---

## File structure

**Create:**
- `crates/huck-engine/src/cwd_scope.rs` — `with_cwd(path, shell, f)` helper (RAII chdir + PWD/OLDPWD snapshot/restore).
- `crates/huck-engine/src/restricted.rs` — seven `check_*` helpers + `is_restricted(shell)` accessor.
- `crates/huck-engine/src/timeout.rs` — `spawn_timer(dur, flag, pids) -> TimerHandle` + `TimerHandle::cancel()`.
- `crates/huck-engine/tests/engine_sandbox_diff.rs` — Rust driver for the bash-diff harness.
- `tests/scripts/engine_sandbox_diff_check.sh` — bash-diff harness.

**Modify:**
- `crates/huck-engine/src/shell_state.rs` — add `restricted`, `timeout_flag`, `live_external_children` fields to `Shell`; init in `Shell::new()`.
- `crates/huck-engine/src/executor.rs` — widen `ExecOutcome::Interrupted` → `Interrupted(InterruptReason)`, update all existing sites; extend `check_interrupt` to poll `timeout_flag`; map exit codes (124 for Timeout, 130 for Sigint) at the top-level reducer; plumb PID registry into the 3 external-fork sites.
- `crates/huck-engine/src/builtins.rs` — call `restricted::check_*` from `builtin_cd`, `builtin_source`, `builtin_set` (`+r`), and `Shell::set`-callers for the special-var-assign check.
- `crates/huck-engine/src/exec_builder.rs` — add `cwd: Option<PathBuf>`, `restricted: bool`, `timeout: Option<Duration>` fields and the three builder methods; compose them in `run_with_sinks`.
- `crates/huck-engine/src/engine.rs` — append composition + sandbox unit tests; update the rustdoc example.
- `crates/huck-engine/src/lib.rs` — declare new modules `cwd_scope`, `restricted`, `timeout`.
- `docs/architecture.md` — short paragraph on the three new knobs.

---

## Task 1: Add `Shell` fields for sandbox state

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (add 3 fields + init in `Shell::new()`)

- [ ] **Step 1: Create the branch**

```bash
git checkout -b v206-engine-sandbox
```

- [ ] **Step 2: Add the three fields to `pub struct Shell`**

In `crates/huck-engine/src/shell_state.rs`, find the existing `sigint_flag` field around line 422 and add three siblings right after it:

```rust
pub sigint_flag: Arc<AtomicBool>,
/// Set by a timer thread when an `ExecBuilder::timeout` deadline elapses.
/// Polled by `executor::check_interrupt`; when seen, the executor aborts the
/// current run with `ExecOutcome::Interrupted(InterruptReason::Timeout)`.
pub timeout_flag: Arc<AtomicBool>,
/// PIDs of external children currently being waited on. Pushed at fork
/// sites, popped after `waitpid` success. The timeout timer thread iterates
/// this list to send SIGTERM when the deadline fires.
pub live_external_children: Arc<Mutex<Vec<libc::pid_t>>>,
/// True while the current `ExecBuilder::run`/`capture` call is running
/// under `.restricted(true)`. Snapshot-and-restored by the builder.
pub restricted: bool,
```

You'll need `std::sync::Mutex` in scope — check whether it's already imported (the file already uses `Arc`/`AtomicBool`); if not, add `use std::sync::Mutex;` near the top with the other `use std::sync::…` imports.

- [ ] **Step 3: Initialize the fields in `Shell::new()`**

Around line 723 (the `sigint_flag: Arc::new(...)` line in `Shell::new()`), add the three new fields immediately after:

```rust
sigint_flag: Arc::new(AtomicBool::new(false)),
timeout_flag: Arc::new(AtomicBool::new(false)),
live_external_children: Arc::new(Mutex::new(Vec::new())),
restricted: false,
```

- [ ] **Step 4: Add unit tests for the field defaults**

In the `#[cfg(test)] mod tests` block at the bottom of `shell_state.rs`, find the existing `new_initializes_sigint_flag_to_false` test (~line 2957) and add three siblings:

```rust
#[test]
fn new_initializes_timeout_flag_to_false() {
    let s = Shell::new();
    assert!(!s.timeout_flag.load(std::sync::atomic::Ordering::Relaxed));
}

#[test]
fn new_initializes_live_external_children_empty() {
    let s = Shell::new();
    assert!(s.live_external_children.lock().unwrap().is_empty());
}

#[test]
fn new_initializes_restricted_to_false() {
    let s = Shell::new();
    assert!(!s.restricted);
}
```

- [ ] **Step 5: Build + run**

```bash
cargo build --workspace -q
cargo test --workspace --quiet
```

Expected: green, three new tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/shell_state.rs
git commit -m "$(cat <<'EOF'
v206 task 1: add Shell fields for sandbox state (restricted, timeout, pids)

Three new fields on Shell with Default initialisers: `restricted: bool`,
`timeout_flag: Arc<AtomicBool>`, `live_external_children:
Arc<Mutex<Vec<libc::pid_t>>>`. Three unit tests pin the defaults. No
behaviour change — fields aren't read anywhere yet.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `cwd_scope.rs` — `with_cwd` RAII helper

**Files:**
- Create: `crates/huck-engine/src/cwd_scope.rs`
- Modify: `crates/huck-engine/src/lib.rs` (declare `pub(crate) mod cwd_scope;`)

- [ ] **Step 1: Create the module**

```rust
//! Run a closure with the process cwd temporarily set to `path`, restoring
//! the prior cwd (and the shell's `PWD`/`OLDPWD` vars) when the closure
//! returns or panics.
//!
//! Because cwd is process-global, callers MUST NOT invoke this concurrently
//! across threads — relies on Engine's `!Send + !Sync` contract; tests gate
//! on `test_support::CWD_LOCK`.

use crate::shell_state::Shell;
use std::path::{Path, PathBuf};

/// Run `f` with the process cwd set to `path`. On return (or panic), restore
/// the prior cwd and the shell's `PWD` / `OLDPWD` variables.
///
/// On chdir failure prints `huck: cwd: <path>: <err>` to real fd 2 and runs
/// `f` anyway with the embedder's original cwd (best-effort, matches the
/// `with_stdin_fd0` posture in v205).
pub fn with_cwd<R>(
    path: &Path,
    shell: &mut Shell,
    f: impl FnOnce(&mut Shell) -> R,
) -> R {
    let saved_os = std::env::current_dir().ok();
    let saved_pwd = shell.lookup_var("PWD");
    let saved_oldpwd = shell.lookup_var("OLDPWD");

    if let Err(e) = std::env::set_current_dir(path) {
        eprintln!("huck: cwd: {}: {e}", path.display());
        return f(shell);
    }

    // Compute the new PWD: canonicalize via the OS, falling back to the
    // input on failure.
    let new_pwd = std::env::current_dir()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| path.display().to_string());
    if let Some(prev) = &saved_pwd {
        shell.set("OLDPWD", prev.clone());
    }
    shell.set("PWD", new_pwd);

    struct Restore {
        saved_os: Option<PathBuf>,
        saved_pwd: Option<String>,
        saved_oldpwd: Option<String>,
        shell: *mut Shell,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(p) = &self.saved_os {
                let _ = std::env::set_current_dir(p);
            }
            // SAFETY: `with_cwd` holds `shell: &mut Shell` exclusively for
            // its scope; we cast to a raw pointer here only because the
            // closure `f` consumes the live borrow. The pointer is reborrowed
            // here in Drop, which runs before `with_cwd` returns, so the
            // original borrow is still live and valid. No aliasing because
            // `f` has already returned by the time Drop runs.
            let shell = unsafe { &mut *self.shell };
            match &self.saved_pwd {
                Some(v) => shell.set("PWD", v.clone()),
                None => shell.unset("PWD"),
            }
            match &self.saved_oldpwd {
                Some(v) => shell.set("OLDPWD", v.clone()),
                None => shell.unset("OLDPWD"),
            }
        }
    }
    let _restore = Restore {
        saved_os,
        saved_pwd,
        saved_oldpwd,
        shell: shell as *mut Shell,
    };
    f(shell)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::CWD_LOCK;

    #[test]
    fn with_cwd_runs_closure_in_path_and_restores() {
        let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let before = std::env::current_dir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let mut s = Shell::new();
        let inside = with_cwd(tmp.path(), &mut s, |_| {
            std::env::current_dir().unwrap()
        });
        let after = std::env::current_dir().unwrap();
        // canonicalize before comparing — macOS / Linux may add /private prefix
        assert_eq!(
            std::fs::canonicalize(&inside).unwrap(),
            std::fs::canonicalize(tmp.path()).unwrap()
        );
        assert_eq!(after, before);
    }

    #[test]
    fn with_cwd_sets_and_restores_pwd_oldpwd() {
        let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let mut s = Shell::new();
        s.set("PWD", "before".to_string());
        // No OLDPWD before the call.
        with_cwd(tmp.path(), &mut s, |s2| {
            // Inside: PWD is the canonical tmp path, OLDPWD is "before".
            let pwd = s2.lookup_var("PWD").unwrap();
            let canonical_tmp = std::fs::canonicalize(tmp.path()).unwrap();
            assert_eq!(
                std::fs::canonicalize(&pwd).unwrap(),
                canonical_tmp
            );
            assert_eq!(s2.lookup_var("OLDPWD").as_deref(), Some("before"));
        });
        // After: PWD restored, OLDPWD removed.
        assert_eq!(s.lookup_var("PWD").as_deref(), Some("before"));
        assert_eq!(s.lookup_var("OLDPWD"), None);
    }

    #[test]
    fn with_cwd_chdir_failure_is_best_effort() {
        let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = Shell::new();
        s.set("PWD", "before".to_string());
        let ran = with_cwd(Path::new("/no/such/huck/sandbox"), &mut s, |_| true);
        assert!(ran);
        // PWD unchanged on chdir failure.
        assert_eq!(s.lookup_var("PWD").as_deref(), Some("before"));
    }
}
```

- [ ] **Step 2: Register the module in `lib.rs`**

In `crates/huck-engine/src/lib.rs`, after the existing `pub(crate) mod stdin_pipe;` line (or wherever the v205 modules sit), add:

```rust
pub(crate) mod cwd_scope;
```

- [ ] **Step 3: Run the new tests + suite**

```bash
cargo test --workspace --quiet cwd_scope -- --nocapture
cargo test --workspace --quiet
```

Expected: 3 cwd_scope tests pass; full suite green.

- [ ] **Step 4: Commit**

```bash
git add crates/huck-engine/src/cwd_scope.rs crates/huck-engine/src/lib.rs
git commit -m "$(cat <<'EOF'
v206 task 2: cwd_scope::with_cwd RAII helper

with_cwd(path, shell, f) saves OS cwd + shell PWD/OLDPWD, chdirs, runs the
closure, restores all three on Drop (panic-safe). Chdir failure is
best-effort (logs + runs f with embedder's cwd). 3 tests cover round-trip,
PWD/OLDPWD snapshot, and chdir-failure path; gated on test_support::CWD_LOCK.
Used by ExecBuilder::cwd in task 6.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `restricted.rs` — policy + enforcement sites

**Files:**
- Create: `crates/huck-engine/src/restricted.rs`
- Modify: `crates/huck-engine/src/lib.rs` (declare `pub(crate) mod restricted;`)
- Modify: `crates/huck-engine/src/builtins.rs` (gates in `builtin_cd`, `builtin_source`, `builtin_set`)
- Modify: `crates/huck-engine/src/executor.rs` (gates in `run_exec_single` for `exec`, command-name resolution, redirect-open, assignment apply)

- [ ] **Step 1: Create `crates/huck-engine/src/restricted.rs`**

```rust
//! Restricted-mode policy checks. When `Shell.restricted` is true, the
//! enforcement sites consult these helpers to refuse operations that would
//! let a sandboxed script escape: cd, exec, slash-bearing command names,
//! slash-bearing source paths, absolute or `..` redirect paths, assignment
//! to SHELL/PATH/ENV/BASH_ENV, and `set +r`.
//!
//! Each helper returns Result so the caller can emit the diagnostic through
//! its in-scope `err: &mut dyn Write` writer via the `e!` macro.

use crate::shell_state::Shell;
use std::path::Path;

#[inline]
pub fn is_restricted(shell: &Shell) -> bool {
    shell.restricted
}

pub fn check_cd() -> Result<(), &'static str> {
    Err("huck: restricted: cd")
}

pub fn check_exec() -> Result<(), &'static str> {
    Err("huck: restricted: exec")
}

pub fn check_command_name(name: &str) -> Result<(), String> {
    if name.contains('/') {
        Err(format!("huck: restricted: {name}: restricted"))
    } else {
        Ok(())
    }
}

pub fn check_source_path(path: &str) -> Result<(), &'static str> {
    if path.contains('/') {
        Err("huck: restricted: source: paths with '/'")
    } else {
        Ok(())
    }
}

/// `path` is the redirect target string from the script source (e.g. `>file`
/// → `"file"`). An absolute path OR any `..` component is refused.
pub fn check_redirect_path(path: &str) -> Result<(), String> {
    if path.starts_with('/') || Path::new(path).components().any(|c| {
        matches!(c, std::path::Component::ParentDir)
    }) {
        Err(format!("huck: restricted: {path}"))
    } else {
        Ok(())
    }
}

pub fn check_special_assign(name: &str) -> Result<(), String> {
    if matches!(name, "SHELL" | "PATH" | "ENV" | "BASH_ENV") {
        Err(format!("huck: restricted: {name}: readonly variable"))
    } else {
        Ok(())
    }
}

pub fn check_set_plus_r() -> Result<(), &'static str> {
    Err("huck: restricted: cannot turn off restricted mode")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_command_name_rejects_slash() {
        assert!(check_command_name("/bin/echo").is_err());
        assert!(check_command_name("./foo").is_err());
        assert!(check_command_name("bin/foo").is_err());
    }

    #[test]
    fn check_command_name_accepts_bare() {
        assert!(check_command_name("echo").is_ok());
        assert!(check_command_name("my-cmd").is_ok());
    }

    #[test]
    fn check_redirect_path_rejects_absolute() {
        assert!(check_redirect_path("/tmp/x").is_err());
        assert!(check_redirect_path("/etc/passwd").is_err());
    }

    #[test]
    fn check_redirect_path_rejects_parent() {
        assert!(check_redirect_path("../escape").is_err());
        assert!(check_redirect_path("foo/../bar").is_err());
    }

    #[test]
    fn check_redirect_path_accepts_relative_no_parent() {
        assert!(check_redirect_path("log").is_ok());
        assert!(check_redirect_path("sub/log").is_ok());
        assert!(check_redirect_path("./log").is_ok());
    }

    #[test]
    fn check_special_assign_rejects_listed() {
        for n in ["SHELL", "PATH", "ENV", "BASH_ENV"] {
            assert!(check_special_assign(n).is_err(), "expected {n} refused");
        }
    }

    #[test]
    fn check_special_assign_accepts_others() {
        for n in ["FOO", "BAR", "IFS", "HOME"] {
            assert!(check_special_assign(n).is_ok(), "expected {n} allowed");
        }
    }
}
```

- [ ] **Step 2: Register in `lib.rs`**

Add `pub(crate) mod restricted;` to `crates/huck-engine/src/lib.rs` near the other v206 modules.

- [ ] **Step 3: Add the `cd` gate in `builtin_cd`**

In `crates/huck-engine/src/builtins.rs`, `builtin_cd` (~line 319): immediately after the function opens (before any other logic), add:

```rust
pub(crate) fn builtin_cd(args: &[String], out: &mut dyn Write, err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if crate::restricted::is_restricted(shell) {
        if let Err(msg) = crate::restricted::check_cd() {
            e!(err, "{msg}");
            return ExecOutcome::Continue(1);
        }
    }
    // ... existing body ...
```

- [ ] **Step 4: Add the `source` gate in `builtin_source`**

In `builtins.rs`, `builtin_source` (~line 6011): after the function opens, BEFORE the args are processed (you'll need to peek at the args to apply the check), gate on `is_restricted` AND apply only when the path arg contains `/`:

```rust
fn builtin_source(args: &[String], err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if crate::restricted::is_restricted(shell) {
        if let Some(path) = args.first() {
            if let Err(msg) = crate::restricted::check_source_path(path) {
                e!(err, "{msg}");
                return ExecOutcome::Continue(1);
            }
        }
    }
    // ... existing body ...
```

- [ ] **Step 5: Add the `set +r` gate in `builtin_set`**

In `builtins.rs`, `builtin_set` (~line 4986): find the branch that handles `+r` / `-r` (search for `"+r"` or `"-r"` or the `r` letter mapping). If huck doesn't currently model `r` as a `set` option, we'll only need to refuse the `+r` literal arg. Add at the top of the arg parsing loop:

```rust
fn builtin_set(args: &[String], out: &mut dyn Write, err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if crate::restricted::is_restricted(shell) {
        // Bash rbash refuses `set +r` (cannot turn off restricted mode).
        if args.iter().any(|a| a == "+r") {
            if let Err(msg) = crate::restricted::check_set_plus_r() {
                e!(err, "{msg}");
                return ExecOutcome::Continue(1);
            }
        }
    }
    // ... existing body ...
```

- [ ] **Step 6: Add the `exec` gate in `run_exec_single`**

In `crates/huck-engine/src/executor.rs`, `run_exec_single` (~line 3626): find the branch that intercepts the `exec` builtin (search for `"exec"` near the top of the function — there's a comment "exec is intercepted by the executor"). At that intercept site, gate on `is_restricted`:

```rust
// After resolving the command but before applying the permanent dup2:
if crate::restricted::is_restricted(shell) {
    if let Err(msg) = crate::restricted::check_exec() {
        let mut err = crate::executor::err_writer(err_sink, sink);
        e!(&mut *err, "{msg}");
        return ExecOutcome::Continue(1);
    }
}
```

(The exact insertion point depends on where the `exec`-builtin branch is in `run_exec_single`. The check must run BEFORE the permanent dup2 / process-image replacement; before any side-effecting redirect opens.)

- [ ] **Step 7: Add the command-name `/` gate in command resolution**

After the executor resolves the command name (find the resolution call in `run_exec_single` — search for `resolve` or `program` near where the external command is dispatched). Just BEFORE the fork / dispatch:

```rust
if crate::restricted::is_restricted(shell) {
    if let Err(msg) = crate::restricted::check_command_name(&resolved.program) {
        let mut err = crate::executor::err_writer(err_sink, sink);
        e!(&mut *err, "{msg}");
        return ExecOutcome::Continue(1);
    }
}
```

- [ ] **Step 8: Add the redirect-path gate**

In `executor.rs`, find the redirect-open path. Look for `RedirectScope::apply` or `RedirectScope::apply_var` (or wherever redirects open files via `OpenOptions`). The check goes immediately before the file open, only for write-style redirects: `>`, `>>`, `>|`, `<>`, `&>`, `&>>`.

Search for the redirect-handling enum/variants:

```bash
grep -n 'enum Redirection\|enum RedirKind\|OpenOptions' crates/huck-engine/src/{executor,command,lexer}.rs
```

At the open site, add (before the `OpenOptions::new()…open(path)` call):

```rust
if crate::restricted::is_restricted(shell)
    && matches!(redir.kind, RedirKind::Output { .. } | RedirKind::Append { .. }
        | RedirKind::Clobber { .. } | RedirKind::ReadWrite { .. }
        | RedirKind::OutputBoth { .. } | RedirKind::AppendBoth { .. })
{
    if let Err(msg) = crate::restricted::check_redirect_path(target_path) {
        let mut err = crate::executor::err_writer(err_sink, sink);
        e!(&mut *err, "{msg}");
        return ExecOutcome::Continue(1);
    }
}
```

(Adjust the `RedirKind` variant names to match the actual enum in huck — read the file to confirm. Output, Append, Clobber, ReadWrite, OutputBoth (`&>`), AppendBoth (`&>>`) are the write-style operations bash rbash refuses; `<` (input) is not refused.)

- [ ] **Step 9: Add the special-var-assign gate in `Shell::set`**

In `crates/huck-engine/src/shell_state.rs`, find `impl Shell` `pub fn set(`. Add at the top:

```rust
pub fn set(&mut self, name: &str, value: String) {
    if self.restricted {
        if let Err(msg) = crate::restricted::check_special_assign(name) {
            // No `err` writer in this leaf; use the thread-local one.
            crate::err_thread_local::with_err(|err| crate::e!(err, "{msg}"));
            return; // Refuse the assignment.
        }
    }
    // ... existing body ...
}
```

(`Shell::set` is the lowest leaf — called from inline-assignment `apply_one_assignment` and from `declare`/`export`/etc. The thread-local err pointer is installed by the executor's `install_err_sinks_raw` at every top-level `execute_with_sink`. If `set` is called outside an executor scope, `with_err` falls through to `io::stderr()` — fine.)

- [ ] **Step 10: Add per-site enforcement unit tests in `engine.rs`**

Append to `crates/huck-engine/src/engine.rs::mod tests`:

```rust
#[test]
fn restricted_off_by_default() {
    let mut e = Engine::new();
    let out = e.exec("cd /tmp; echo $PWD").capture();
    assert_eq!(out.exit_code, 0, "stderr={:?}", out.stderr);
    assert_eq!(out.stdout, "/tmp\n");
}

#[test]
fn restricted_refuses_cd() {
    let _g = crate::test_support::CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut e = Engine::new();
    let out = e.exec("cd /tmp; echo \"$PWD\"").restricted(true).capture();
    assert!(out.stderr.contains("restricted: cd"), "stderr={:?}", out.stderr);
    // The script keeps running after the refused cd; echo still fires.
    assert!(out.stdout.ends_with("\n"));
}

#[test]
fn restricted_refuses_exec() {
    let mut e = Engine::new();
    let out = e.exec("exec /bin/true").restricted(true).capture();
    assert!(out.stderr.contains("restricted: exec"), "stderr={:?}", out.stderr);
}

#[test]
fn restricted_refuses_command_name_with_slash() {
    let mut e = Engine::new();
    let out = e.exec("/bin/echo hi").restricted(true).capture();
    assert!(out.stderr.contains("restricted:"), "stderr={:?}", out.stderr);
    assert_eq!(out.stdout, "");
}

#[test]
fn restricted_accepts_command_name_without_slash() {
    let mut e = Engine::new();
    let out = e.exec("true").restricted(true).capture();
    assert_eq!(out.exit_code, 0);
}

#[test]
fn restricted_refuses_source_with_slash() {
    let mut e = Engine::new();
    let out = e.exec(". /etc/profile").restricted(true).capture();
    assert!(out.stderr.contains("restricted: source"), "stderr={:?}", out.stderr);
}

#[test]
fn restricted_refuses_absolute_redirect() {
    let mut e = Engine::new();
    let out = e.exec("echo hi > /tmp/v206-restricted-test").restricted(true).capture();
    assert!(out.stderr.contains("restricted:"), "stderr={:?}", out.stderr);
    // The file MUST NOT have been written.
    assert!(!std::path::Path::new("/tmp/v206-restricted-test").exists()
        || std::fs::read("/tmp/v206-restricted-test").map(|b| b.is_empty()).unwrap_or(true),
        "the refused redirect wrote a file");
    let _ = std::fs::remove_file("/tmp/v206-restricted-test");
}

#[test]
fn restricted_refuses_parent_dir_redirect() {
    let mut e = Engine::new();
    let out = e.exec("echo hi > ../escape").restricted(true).capture();
    assert!(out.stderr.contains("restricted:"), "stderr={:?}", out.stderr);
}

#[test]
fn restricted_refuses_path_assignment() {
    let mut e = Engine::new();
    let out = e.exec("PATH=/tmp; echo done").restricted(true).capture();
    assert!(out.stderr.contains("restricted: PATH"), "stderr={:?}", out.stderr);
}

#[test]
fn restricted_refuses_shell_assignment() {
    let mut e = Engine::new();
    let out = e.exec("SHELL=/bin/bash; echo done").restricted(true).capture();
    assert!(out.stderr.contains("restricted: SHELL"), "stderr={:?}", out.stderr);
}

#[test]
fn restricted_refuses_set_plus_r() {
    let mut e = Engine::new();
    let out = e.exec("set +r; cd /tmp").restricted(true).capture();
    assert!(out.stderr.contains("restricted: cannot turn off"), "stderr={:?}", out.stderr);
    // cd should STILL be refused after the refused `set +r`.
    assert!(out.stderr.contains("restricted: cd"), "stderr={:?}", out.stderr);
}

#[test]
fn restricted_propagates_to_function() {
    let mut e = Engine::new();
    let out = e.exec("f() { cd /tmp; }; f").restricted(true).capture();
    assert!(out.stderr.contains("restricted: cd"), "stderr={:?}", out.stderr);
}

#[test]
fn restricted_lifts_after_call() {
    let _g = crate::test_support::CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut e = Engine::new();
    let _ = e.exec("cd /tmp; pwd").restricted(true).capture();
    // Next call, no restricted: cd works.
    let out = e.exec("cd /; pwd").capture();
    assert_eq!(out.stdout, "/\n", "stderr={:?}", out.stderr);
}
```

Note these tests don't yet exercise `.restricted(true)` because the builder method doesn't exist yet — Task 6 adds it. Mark these tests `#[ignore]` for now and Task 6 will un-ignore them. OR: skip writing them in Task 3 and add them in Task 6. **Recommended: skip these in Task 3; Task 6 writes them all when `.restricted()` exists.**

Replace step 10 with:

- [ ] **Step 10 (REVISED): No engine.rs tests in this task**

Defer the end-to-end `.restricted(true)` tests to Task 6 (which adds the builder method). Task 3's tests are just the unit tests on the `check_*` functions in `restricted.rs::tests`.

- [ ] **Step 11: Build + suite**

```bash
cargo build --workspace -q
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: green; clippy clean. The new `check_*` unit tests pass. The enforcement-site gates compile but are unreachable until Task 6 sets `shell.restricted = true`.

- [ ] **Step 12: Commit**

```bash
git add crates/huck-engine/src/restricted.rs \
        crates/huck-engine/src/lib.rs \
        crates/huck-engine/src/builtins.rs \
        crates/huck-engine/src/executor.rs \
        crates/huck-engine/src/shell_state.rs
git commit -m "$(cat <<'EOF'
v206 task 3: restricted-mode policy + enforcement sites

Adds restricted.rs with 7 check_* helpers (cd, exec, command_name with /,
source path with /, redirect-path absolute-or-parent, special var assign
SHELL/PATH/ENV/BASH_ENV, set +r) and is_restricted accessor. Threads gates
into builtin_cd, builtin_source, builtin_set (+r), run_exec_single (exec
intercept + command-name resolution), redirect-open path, and Shell::set
(special-var-assign — uses err_thread_local::with_err since set is leaf).
restricted check unit tests in restricted.rs::tests cover positive +
negative cases. End-to-end builder tests deferred to task 6.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Widen `ExecOutcome::Interrupted` → `Interrupted(InterruptReason)`

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (enum definition + ~20 existing match sites + the top-level exit-code reducer)

- [ ] **Step 1: Add the new enum**

Find `pub enum ExecOutcome` in `executor.rs` (early in the file). Add a sibling enum and update the variant:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptReason {
    Sigint,
    Timeout,
}

pub enum ExecOutcome {
    // ... existing variants ...
    Interrupted(InterruptReason),  // CHANGED from bare Interrupted
}
```

- [ ] **Step 2: Update every existing `ExecOutcome::Interrupted` site**

Use grep to enumerate:

```bash
grep -n 'ExecOutcome::Interrupted' crates/huck-engine/src/executor.rs
```

There are ~20 sites. Each falls into one of these patterns:

**Construction** (`Some(ExecOutcome::Interrupted)` or `return ExecOutcome::Interrupted`):
- Replace with `ExecOutcome::Interrupted(InterruptReason::Sigint)`.

**Match arm matching the bare variant** (`ExecOutcome::Interrupted =>`):
- Replace with `ExecOutcome::Interrupted(_) =>` (the reason isn't needed for most propagation arms).

**Match arm in the top-level reducer that maps to exit code 130**:
- Find the arm in `run_program_in_sinks` (or wherever the final code is computed — search for `=> 130`):
  ```bash
  grep -n '=> 130' crates/huck-engine/src/executor.rs
  ```
- Change it to discriminate on reason:
  ```rust
  ExecOutcome::Interrupted(InterruptReason::Sigint) => 130,
  ExecOutcome::Interrupted(InterruptReason::Timeout) => 124,
  ```

Walk each grep hit carefully — most are pattern-match arms in propagation chains (`match outcome { … ExecOutcome::Interrupted => return ExecOutcome::Interrupted, … }`). For those, the propagation is `(_)`-match → re-emit the same reason:

```rust
ExecOutcome::Interrupted(r) => return ExecOutcome::Interrupted(r),
```

- [ ] **Step 3: Build + run the suite**

```bash
cargo build --workspace -q
cargo test --workspace --quiet
```

Expected: green. All existing SIGINT tests still pass (they exercise the `Sigint` reason now).

- [ ] **Step 4: Commit**

```bash
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v206 task 4: widen ExecOutcome::Interrupted to carry InterruptReason

ExecOutcome::Interrupted becomes Interrupted(InterruptReason::{Sigint,Timeout}).
All existing sites updated to construct/match Sigint; the top-level reducer
maps Sigint -> 130 (today's behaviour) and Timeout -> 124 (new path, exercised
by task 5+). No observable behaviour change yet — the Timeout reason is
unreachable until task 5 wires the timeout flag.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: `timeout.rs` + `check_interrupt` poll + PID registry

**Files:**
- Create: `crates/huck-engine/src/timeout.rs`
- Modify: `crates/huck-engine/src/lib.rs` (declare `pub(crate) mod timeout;`)
- Modify: `crates/huck-engine/src/executor.rs` (extend `check_interrupt`; PID registry push/pop at 3 fork sites)

- [ ] **Step 1: Create `crates/huck-engine/src/timeout.rs`**

```rust
//! Timer thread that, after a deadline elapses, sets a shared atomic flag and
//! sends SIGTERM to all currently-live external children. Cancelled via a
//! channel send (so a script that finishes before the deadline doesn't leave
//! a dangling sleeping thread).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

pub struct TimerHandle {
    handle: JoinHandle<()>,
    cancel_tx: Sender<()>,
}

impl TimerHandle {
    /// Cancel the timer (if it hasn't fired yet) and join the thread.
    pub fn cancel(self) {
        let _ = self.cancel_tx.send(());
        let _ = self.handle.join();
    }
}

/// Spawn a timer thread. When `dur` elapses without a cancel, sets `flag` to
/// true and sends SIGTERM to every pid currently in `pids`.
pub fn spawn_timer(
    dur: Duration,
    flag: Arc<AtomicBool>,
    pids: Arc<Mutex<Vec<libc::pid_t>>>,
) -> TimerHandle {
    let (cancel_tx, cancel_rx) = channel::<()>();
    let handle = std::thread::spawn(move || {
        match cancel_rx.recv_timeout(dur) {
            Ok(_) | Err(RecvTimeoutError::Disconnected) => {
                // Cancelled before the deadline.
            }
            Err(RecvTimeoutError::Timeout) => {
                flag.store(true, Ordering::Relaxed);
                if let Ok(guard) = pids.lock() {
                    for &pid in guard.iter() {
                        unsafe {
                            libc::kill(pid, libc::SIGTERM);
                        }
                    }
                }
            }
        }
    });
    TimerHandle { handle, cancel_tx }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn timer_fires_after_deadline() {
        let flag = Arc::new(AtomicBool::new(false));
        let pids = Arc::new(Mutex::new(Vec::new()));
        let h = spawn_timer(Duration::from_millis(50), Arc::clone(&flag), Arc::clone(&pids));
        std::thread::sleep(Duration::from_millis(150));
        assert!(flag.load(Ordering::Relaxed), "flag should be set");
        h.cancel();
    }

    #[test]
    fn timer_cancel_prevents_fire() {
        let flag = Arc::new(AtomicBool::new(false));
        let pids = Arc::new(Mutex::new(Vec::new()));
        let h = spawn_timer(Duration::from_secs(60), Arc::clone(&flag), Arc::clone(&pids));
        let start = Instant::now();
        h.cancel();
        assert!(start.elapsed() < Duration::from_secs(1), "cancel should return immediately");
        std::thread::sleep(Duration::from_millis(50));
        assert!(!flag.load(Ordering::Relaxed), "flag should NOT be set after cancel");
    }

    #[test]
    fn timer_zero_duration_fires_immediately() {
        let flag = Arc::new(AtomicBool::new(false));
        let pids = Arc::new(Mutex::new(Vec::new()));
        let h = spawn_timer(Duration::ZERO, Arc::clone(&flag), Arc::clone(&pids));
        std::thread::sleep(Duration::from_millis(50));
        assert!(flag.load(Ordering::Relaxed));
        h.cancel();
    }
}
```

- [ ] **Step 2: Register in `lib.rs`**

Add `pub(crate) mod timeout;` to `crates/huck-engine/src/lib.rs`.

- [ ] **Step 3: Extend `check_interrupt` to poll `timeout_flag`**

In `crates/huck-engine/src/executor.rs`, find the existing `check_interrupt` (~line 81). The current body checks `sigint_flag`. Add a sibling check for `timeout_flag`:

```rust
pub(crate) fn check_interrupt(shell: &Shell) -> Option<ExecOutcome> {
    use std::sync::atomic::Ordering;

    // Existing SIGINT poll
    if shell
        .sigint_flag
        .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        return Some(ExecOutcome::Interrupted(InterruptReason::Sigint));
    }

    // NEW: timeout poll. Don't clear here — the builder reads and clears in
    // its epilogue to override the exit code to 124.
    if shell.timeout_flag.load(Ordering::Relaxed) {
        return Some(ExecOutcome::Interrupted(InterruptReason::Timeout));
    }
    None
}
```

Note: SIGINT is cleared with `compare_exchange` (latch + clear). Timeout is NOT cleared in `check_interrupt` — it's a one-shot signal the builder reads to override the exit code; clearing happens once at the call boundary via `swap(false)`.

- [ ] **Step 4: Plumb the PID registry into `run_subprocess`**

In `executor.rs`, find `run_subprocess` (~line 4493 per the v205 work). After a successful `ProcessCommand::spawn()` returning `child`, push the child's pid:

```rust
let pid = child.id() as libc::pid_t;
shell.live_external_children.lock().unwrap().push(pid);
```

After the `child.wait()` / `waitpid` returns, pop:

```rust
shell.live_external_children.lock().unwrap().retain(|&p| p != pid);
```

Place the pop in a scope guard if there are early-return paths — wrap the wait-and-pop in:

```rust
struct PidGuard<'s> {
    pids: &'s Arc<Mutex<Vec<libc::pid_t>>>,
    pid: libc::pid_t,
}
impl Drop for PidGuard<'_> {
    fn drop(&mut self) {
        let _ = self.pids.lock().map(|mut g| g.retain(|&p| p != self.pid));
    }
}
let _guard = PidGuard { pids: &shell.live_external_children, pid };
```

- [ ] **Step 5: Plumb the PID registry into the `Command::Subshell` fork site**

Same pattern at the subshell fork site (~line 447 per the v205 work). Push after a successful `fork_and_run_in_subshell` returning a pid; pop via the same `PidGuard` after waitpid.

- [ ] **Step 6: Plumb the PID registry into `run_multi_stage` pipeline stages**

For each pipeline stage that forks an external, push its pid into the registry. After the pipeline-wide `waitpid` loop, pop all stage pids. Because pipeline stages share the executor's `waitpid` reaping logic, the simplest pattern: push each stage's pid as it's spawned; after the `wait_pipeline_raw` returns, clear out all stage pids in one pass:

```rust
let mut stage_pids: Vec<libc::pid_t> = Vec::new();
// ... in the per-stage spawn loop:
stage_pids.push(child_pid);
shell.live_external_children.lock().unwrap().push(child_pid);
// ... after the pipeline wait:
{
    let mut guard = shell.live_external_children.lock().unwrap();
    guard.retain(|p| !stage_pids.contains(p));
}
```

- [ ] **Step 7: Build + run the suite**

```bash
cargo build --workspace -q
cargo test --workspace --quiet timeout -- --nocapture
cargo test --workspace --quiet
```

Expected: 3 `timeout::tests::*` pass; full suite green (PID registry push/pop is no-op when nobody touches the flag).

- [ ] **Step 8: Commit**

```bash
git add crates/huck-engine/src/timeout.rs \
        crates/huck-engine/src/lib.rs \
        crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v206 task 5: timeout timer module + check_interrupt poll + PID registry

timeout::spawn_timer(dur, flag, pids) -> TimerHandle: a thread that
recv_timeouts on a cancel channel; on Timeout sets the flag and SIGTERMs
every pid in the registry. TimerHandle::cancel sends + joins (no dangling
thread when the script finishes early). check_interrupt grows a load() poll
on Shell.timeout_flag returning Interrupted(Timeout). PID registry push/pop
plumbed into run_subprocess, the Command::Subshell branch, and
run_multi_stage. 3 timer unit tests cover fire / cancel-prevents-fire /
zero-duration. Builder method lands in task 6.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `ExecBuilder::cwd / .restricted / .timeout` + composition

**Files:**
- Modify: `crates/huck-engine/src/exec_builder.rs` (fields + methods + run_with_sinks composition)
- Modify: `crates/huck-engine/src/engine.rs` (unit tests + doc-example update)

- [ ] **Step 1: Add fields to `ExecBuilder`**

In `crates/huck-engine/src/exec_builder.rs`:

```rust
use std::path::PathBuf;
use std::time::Duration;

pub struct ExecBuilder<'a> {
    engine: &'a mut Engine,
    src: String,
    stdin: Option<Vec<u8>>,
    merge: bool,
    cwd: Option<PathBuf>,                 // NEW
    restricted: bool,                     // NEW
    timeout: Option<Duration>,            // NEW
}

impl<'a> ExecBuilder<'a> {
    pub(crate) fn new(engine: &'a mut Engine, src: String) -> Self {
        ExecBuilder {
            engine,
            src,
            stdin: None,
            merge: false,
            cwd: None,
            restricted: false,
            timeout: None,
        }
    }

    // ... existing stdin / merge_stderr ...

    /// Run the script with CWD = `path` for the duration of the call. The
    /// process's prior cwd plus `Shell.vars["PWD"]` / `["OLDPWD"]` are
    /// snapshot-and-restored on exit (including panic unwind).
    pub fn cwd(mut self, path: impl Into<PathBuf>) -> Self {
        self.cwd = Some(path.into());
        self
    }

    /// Enable restricted mode for this call only (bash `rbash` subset:
    /// refuses `cd`, `exec`, command-names containing `/`, `source` of paths
    /// containing `/`, write-redirects to absolute or `..` paths, assignment
    /// to `SHELL`/`PATH`/`ENV`/`BASH_ENV`, and `set +r`). Refused operations
    /// emit `huck: restricted: <op>` via the active stderr sink and return
    /// exit 1; the script keeps running unless `set -e` propagates the
    /// failure.
    pub fn restricted(mut self, on: bool) -> Self {
        self.restricted = on;
        self
    }

    /// Abort the script if it hasn't finished within `dur`. Returns exit
    /// 124 on timeout (matches GNU `timeout(1)`). In-flight external
    /// children receive SIGTERM; builtins finish their current command and
    /// then the next command-boundary check aborts.
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = Some(dur);
        self
    }
}
```

- [ ] **Step 2: Compose all knobs in `run_with_sinks`**

Replace the existing `run_with_sinks` body. The composition order from the spec:

```rust
fn run_with_sinks(self, out: &mut StdoutSink, err: &mut StderrSink) -> i32 {
    let ExecBuilder { engine, src, stdin, merge: _, cwd, restricted, timeout } = self;
    let cell = engine.shell_cell().clone();

    // 1. Timer setup (if requested).
    let timer = timeout.map(|dur| {
        let flag = cell.borrow().timeout_flag.clone();
        let pids = cell.borrow().live_external_children.clone();
        // Reset the flag in case a prior call left it set (we always swap at
        // the end, but defend against an unexpected path).
        flag.store(false, std::sync::atomic::Ordering::Relaxed);
        crate::timeout::spawn_timer(dur, flag, pids)
    });

    // The core run that needs to happen INSIDE the cwd / stdin guards and
    // with restricted set.
    let run_core = |out: &mut StdoutSink, err: &mut StderrSink| -> i32 {
        let label = cell.borrow().shell_argv0.clone();
        let args = cell.borrow().positional_args.clone();
        let code = crate::shell::run_program_in_sinks(
            &src, None, args, &label, false, out, err, &cell,
        );
        cell.borrow_mut().set_last_status(code);
        code
    };

    // Wrap with restricted snapshot+set+restore, then cwd, then stdin.
    let run_restricted_then_core = |out: &mut StdoutSink, err: &mut StderrSink| -> i32 {
        let prev_restricted = cell.borrow().restricted;
        cell.borrow_mut().restricted = restricted || prev_restricted;
        // Use a Drop guard so we restore even on panic.
        struct R<'c> { cell: &'c std::rc::Rc<std::cell::RefCell<crate::shell_state::Shell>>, prev: bool }
        impl Drop for R<'_> {
            fn drop(&mut self) { self.cell.borrow_mut().restricted = self.prev; }
        }
        let _r = R { cell: &cell, prev: prev_restricted };
        run_core(out, err)
    };

    let run_cwd_then_rest = |out: &mut StdoutSink, err: &mut StderrSink| -> i32 {
        match &cwd {
            Some(p) => {
                // with_cwd takes &mut Shell; we borrow it for the call.
                let mut shell_borrow = cell.borrow_mut();
                // The closure must NOT re-borrow `cell` — its borrow is held
                // by `shell_borrow`. So we drop the borrow before calling
                // run_restricted_then_core, which itself re-borrows. The
                // `with_cwd` helper handles the chdir/PWD/OLDPWD itself.
                let r = crate::cwd_scope::with_cwd(p, &mut shell_borrow, |_s| {
                    drop(shell_borrow);
                    // ^ Wait: can't drop a local move-bind here. Need a
                    // different shape — see Implementation Note below.
                    unreachable!()
                });
                r
            }
            None => run_restricted_then_core(out, err),
        }
    };

    // 4. Stdin wraps everything.
    let code = match stdin {
        Some(bytes) => crate::stdin_pipe::with_stdin_fd0(&bytes, || run_cwd_then_rest(out, err)),
        None => run_cwd_then_rest(out, err),
    };

    // 5. Cancel timer.
    if let Some(t) = timer { t.cancel(); }

    // 6. Timeout-flag override.
    if cell.borrow().timeout_flag.swap(false, std::sync::atomic::Ordering::Relaxed) {
        return 124;
    }
    code
}
```

**Implementation Note**: the cwd block above has a borrow-checker problem — `with_cwd` takes `&mut Shell` but `run_restricted_then_core` re-borrows `cell` via `borrow_mut`. There are two clean ways to resolve this:

**(A) Restructure `with_cwd`** to NOT hold the `&mut Shell` for the duration of the closure — instead, it: (1) snapshots OS cwd + PWD + OLDPWD via `&mut shell`, (2) drops the borrow, (3) calls the closure with NO shell arg, (4) reborrows via the Drop guard at the end.

**(B) Have `ExecBuilder` apply cwd entirely inline** without using `with_cwd` — duplicate its logic here, taking advantage of the fact that the builder has direct access to `cell` and can choose its borrow boundaries.

**Recommended: (A).** Rewrite `with_cwd` so the closure takes NO Shell argument:

```rust
// In cwd_scope.rs:
pub fn with_cwd<R>(
    path: &Path,
    shell: &mut Shell,
    f: impl FnOnce() -> R,    // CHANGED: no shell arg
) -> R {
    let saved_os = std::env::current_dir().ok();
    let saved_pwd = shell.lookup_var("PWD");
    let saved_oldpwd = shell.lookup_var("OLDPWD");

    if let Err(e) = std::env::set_current_dir(path) {
        eprintln!("huck: cwd: {}: {e}", path.display());
        return f();
    }
    let new_pwd = std::env::current_dir()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| path.display().to_string());
    if let Some(prev) = &saved_pwd { shell.set("OLDPWD", prev.clone()); }
    shell.set("PWD", new_pwd);

    // Restore guard: needs to mutate shell on Drop. We pass `shell` via raw
    // pointer because the closure (which we're about to call) doesn't take
    // shell, so `shell`'s &mut borrow ends here.
    struct Restore { /* same as before */ }
    // ... build Restore from saved_os/pwd/oldpwd and shell-as-pointer ...
    let _restore = Restore { /* ... */ };

    // Drop the &mut Shell borrow before calling f (which may re-borrow via
    // some other path). Local `shell: &mut Shell` is a reborrow of caller's
    // borrow; once we reach the closure call, no further uses of `shell`
    // exist in scope, so the borrow ends.
    f()
}
```

This shape lets the executor's run closure re-borrow `cell` via `borrow_mut()` inside `f()` without conflict — `with_cwd`'s `&mut shell` borrow has ended before `f` runs.

Update Task 2's `with_cwd` signature to match (closure takes no args), and update Task 2's tests accordingly.

- [ ] **Step 3: Rewrite Task 2's `with_cwd` to take a no-arg closure**

Edit `crates/huck-engine/src/cwd_scope.rs` to use the shape above. The Restore guard stays the same; only the closure signature changes (from `FnOnce(&mut Shell) -> R` to `FnOnce() -> R`). Update the three unit tests accordingly:

```rust
#[test]
fn with_cwd_runs_closure_in_path_and_restores() {
    let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let before = std::env::current_dir().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Shell::new();
    let inside = with_cwd(tmp.path(), &mut s, || {
        std::env::current_dir().unwrap()
    });
    let after = std::env::current_dir().unwrap();
    assert_eq!(
        std::fs::canonicalize(&inside).unwrap(),
        std::fs::canonicalize(tmp.path()).unwrap()
    );
    assert_eq!(after, before);
}

#[test]
fn with_cwd_sets_and_restores_pwd_oldpwd() {
    let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let mut s = Shell::new();
    s.set("PWD", "before".to_string());
    // The closure can't see s, so we can't assert inside. Instead, assert
    // by running through Engine in the engine.rs composition tests.
    with_cwd(tmp.path(), &mut s, || {});
    // After: PWD restored, OLDPWD removed.
    assert_eq!(s.lookup_var("PWD").as_deref(), Some("before"));
    assert_eq!(s.lookup_var("OLDPWD"), None);
}

#[test]
fn with_cwd_chdir_failure_is_best_effort() {
    let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = Shell::new();
    s.set("PWD", "before".to_string());
    let ran = with_cwd(Path::new("/no/such/huck/sandbox"), &mut s, || true);
    assert!(ran);
    assert_eq!(s.lookup_var("PWD").as_deref(), Some("before"));
}
```

(The "inside" snapshot of PWD is tested end-to-end via the engine.rs builder tests in step 6 instead.)

- [ ] **Step 4: Simplify the builder composition with the new `with_cwd` shape**

Now the executor side becomes much cleaner:

```rust
fn run_with_sinks(self, out: &mut StdoutSink, err: &mut StderrSink) -> i32 {
    let ExecBuilder { engine, src, stdin, merge: _, cwd, restricted, timeout } = self;
    let cell = engine.shell_cell().clone();

    // Timer setup.
    let timer = timeout.map(|dur| {
        let flag = cell.borrow().timeout_flag.clone();
        let pids = cell.borrow().live_external_children.clone();
        flag.store(false, std::sync::atomic::Ordering::Relaxed);
        crate::timeout::spawn_timer(dur, flag, pids)
    });

    // run_inner: the core that needs sinks + the cell.
    let run_inner = || -> i32 {
        let label = cell.borrow().shell_argv0.clone();
        let args = cell.borrow().positional_args.clone();
        let code = crate::shell::run_program_in_sinks(
            &src, None, args, &label, false, out, err, &cell,
        );
        cell.borrow_mut().set_last_status(code);
        code
    };

    // Wrap: restricted snapshot+set.
    let run_with_restricted = || -> i32 {
        let prev_restricted = cell.borrow().restricted;
        cell.borrow_mut().restricted = restricted || prev_restricted;
        struct R { cell: std::rc::Rc<std::cell::RefCell<crate::shell_state::Shell>>, prev: bool }
        impl Drop for R {
            fn drop(&mut self) { self.cell.borrow_mut().restricted = self.prev; }
        }
        let _r = R { cell: cell.clone(), prev: prev_restricted };
        run_inner()
    };

    // Wrap: cwd (if set).
    let run_with_cwd = || -> i32 {
        match &cwd {
            Some(p) => {
                let mut shell = cell.borrow_mut();
                crate::cwd_scope::with_cwd(p, &mut shell, || {
                    drop(shell);  // release borrow before reborrowing inside
                    run_with_restricted()
                })
            }
            None => run_with_restricted(),
        }
    };

    // Wrap: stdin (if set).
    let code = match stdin {
        Some(bytes) => crate::stdin_pipe::with_stdin_fd0(&bytes, || run_with_cwd()),
        None => run_with_cwd(),
    };

    if let Some(t) = timer { t.cancel(); }
    if cell.borrow().timeout_flag.swap(false, std::sync::atomic::Ordering::Relaxed) {
        return 124;
    }
    code
}
```

**Note on the `out` / `err` borrows.** The original `run_with_sinks` takes `out: &mut StdoutSink` and `err: &mut StderrSink`. The closures above need to capture these by mutable reference. But the closures are nested several layers deep, so the borrow checker may complain. **Resolution**: have the closures take `out` and `err` as parameters (not captures), threading them through each wrapper. Pseudocode:

```rust
let run_inner = |out: &mut StdoutSink, err: &mut StderrSink| -> i32 { ... };
let run_with_restricted = |out: &mut StdoutSink, err: &mut StderrSink| -> i32 { ... };
let run_with_cwd = |out: &mut StdoutSink, err: &mut StderrSink| -> i32 { ... };

let code = match stdin {
    Some(bytes) => crate::stdin_pipe::with_stdin_fd0(&bytes, || run_with_cwd(out, err)),
    None => run_with_cwd(out, err),
};
```

For the `stdin_pipe::with_stdin_fd0` case, the inner closure can't take `out`/`err` as args because `with_stdin_fd0`'s signature is `FnOnce() -> R`. The closure captures `out`/`err` by mutable reference. As long as no other closure simultaneously holds a mutable borrow of the same sinks, this compiles. Restructure the wrappers as nested `if let Some(...)` blocks instead of named closures if needed:

```rust
let code = match stdin {
    Some(bytes) => crate::stdin_pipe::with_stdin_fd0(&bytes, || {
        match &cwd {
            Some(p) => {
                let mut shell = cell.borrow_mut();
                crate::cwd_scope::with_cwd(p, &mut shell, || {
                    drop(shell);
                    run_with_restricted_then_inner(&cell, restricted, out, err, &src)
                })
            }
            None => run_with_restricted_then_inner(&cell, restricted, out, err, &src),
        }
    }),
    None => match &cwd {
        Some(p) => {
            let mut shell = cell.borrow_mut();
            crate::cwd_scope::with_cwd(p, &mut shell, || {
                drop(shell);
                run_with_restricted_then_inner(&cell, restricted, out, err, &src)
            })
        }
        None => run_with_restricted_then_inner(&cell, restricted, out, err, &src),
    },
};
```

Where `run_with_restricted_then_inner` is a private helper function (not a closure) that takes all parameters explicitly. This avoids the closure-borrow pitfalls.

**The implementer should pick the shape that compiles cleanly**; the SEMANTICS (the order in the spec) are the contract.

- [ ] **Step 5: Add the end-to-end tests in `engine.rs`**

Append to `crates/huck-engine/src/engine.rs::mod tests` — ALL the tests deferred from Task 3, plus cwd/timeout/composition:

```rust
// ============== CWD ==============

#[test]
fn exec_cwd_runs_script_in_path() {
    let _g = crate::test_support::CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let canonical = std::fs::canonicalize(tmp.path()).unwrap();
    let mut e = Engine::new();
    let out = e.exec("pwd").cwd(tmp.path()).capture();
    assert_eq!(
        out.stdout.trim(),
        canonical.display().to_string(),
        "stderr={:?}", out.stderr
    );
}

#[test]
fn exec_cwd_restores_engine_pwd() {
    let _g = crate::test_support::CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let mut e = Engine::new();
    e.set_var("PWD", "before");
    let _ = e.exec("cd /; echo \"in:$PWD\"").cwd(tmp.path()).capture();
    assert_eq!(e.var("PWD").as_deref(), Some("before"));
}

#[test]
fn exec_cwd_chdir_failure_is_best_effort() {
    let _g = crate::test_support::CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut e = Engine::new();
    let out = e.exec("echo hi").cwd("/no/such/huck/v206").capture();
    assert!(out.stderr.contains("huck: cwd:"), "stderr={:?}", out.stderr);
    assert_eq!(out.stdout, "hi\n");
    assert_eq!(out.exit_code, 0);
}

// ============== RESTRICTED ============== (same as Task 3 step 10, included here in full)

// (paste the 12 restricted_* tests from Task 3 step 10 verbatim here)

// ============== TIMEOUT ==============

#[test]
fn exec_timeout_kills_infinite_loop() {
    use std::time::{Duration, Instant};
    let mut e = Engine::new();
    let start = Instant::now();
    let code = e.exec("while true; do :; done").timeout(Duration::from_millis(100)).run();
    let elapsed = start.elapsed();
    assert_eq!(code, 124, "expected 124, got {code}");
    assert!(elapsed < Duration::from_millis(500), "took too long: {elapsed:?}");
}

#[test]
fn exec_timeout_short_script_completes_normally() {
    use std::time::Duration;
    let mut e = Engine::new();
    let out = e.exec("echo hi").timeout(Duration::from_secs(5)).capture();
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "hi\n");
}

#[test]
fn exec_timeout_kills_sleeping_external() {
    use std::time::{Duration, Instant};
    let mut e = Engine::new();
    let start = Instant::now();
    let code = e.exec("/bin/sleep 5").timeout(Duration::from_millis(100)).run();
    let elapsed = start.elapsed();
    assert_eq!(code, 124);
    assert!(elapsed < Duration::from_millis(500), "took too long: {elapsed:?}");
}

#[test]
fn exec_timeout_exit_code_overrides_natural() {
    use std::time::Duration;
    let mut e = Engine::new();
    // Long sleep then `exit 0` — the timeout fires first.
    let code = e.exec("/bin/sleep 5; exit 0").timeout(Duration::from_millis(100)).run();
    assert_eq!(code, 124);
}

#[test]
fn exec_timeout_zero_returns_124() {
    use std::time::Duration;
    let mut e = Engine::new();
    let out = e.exec("echo hi").timeout(Duration::ZERO).capture();
    assert_eq!(out.exit_code, 124);
    assert_eq!(out.stdout, "");
}

// ============== COMPOSITION ==============

#[test]
fn exec_all_knobs_compose() {
    use std::time::Duration;
    let _g = crate::test_support::CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _g2 = crate::test_support::STDIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let mut e = Engine::new();
    let out = e
        .exec("read x; echo \"got:$x\"")
        .cwd(tmp.path())
        .restricted(true)
        .timeout(Duration::from_secs(2))
        .stdin(b"hello\n".to_vec())
        .capture();
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "got:hello\n");
}

#[test]
fn exec_cwd_and_restricted() {
    let _g = crate::test_support::CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let mut e = Engine::new();
    // `pwd` works inside restricted; `cd` doesn't.
    let out = e.exec("pwd; cd /").cwd(tmp.path()).restricted(true).capture();
    assert!(out.stderr.contains("restricted: cd"), "stderr={:?}", out.stderr);
    assert!(out.stdout.contains(&tmp.path().display().to_string())
        || out.stdout.contains(&std::fs::canonicalize(tmp.path()).unwrap().display().to_string()));
}

#[test]
fn exec_stdin_with_timeout_blocking_read_times_out() {
    use std::time::Duration;
    let _g = crate::test_support::STDIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut e = Engine::new();
    // Empty stdin: read blocks; timeout fires.
    let code = e.exec("read x; echo \"$x\"")
        .stdin(Vec::<u8>::new())
        .timeout(Duration::from_millis(100))
        .run();
    assert_eq!(code, 124);
}
```

- [ ] **Step 6: Update the rustdoc example on `Engine::exec`**

Find the existing example on `Engine::exec` (added in v205). Append:

```rust
//! // Sandboxed run: tmpdir cwd, restricted mode, 5-second budget.
//! # let sandbox_dir = std::env::temp_dir();
//! # let generated_script = "echo hi";
//! let out = e.exec(generated_script)
//!     .cwd(sandbox_dir)
//!     .restricted(true)
//!     .timeout(std::time::Duration::from_secs(5))
//!     .capture();
```

- [ ] **Step 7: Build + run the suite + clippy + doc tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet
cargo test --workspace --doc --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: all green, all new builder tests pass, doc example compiles.

- [ ] **Step 8: Commit**

```bash
git add crates/huck-engine/src/exec_builder.rs \
        crates/huck-engine/src/engine.rs \
        crates/huck-engine/src/cwd_scope.rs
git commit -m "$(cat <<'EOF'
v206 task 6: ExecBuilder gains .cwd / .restricted / .timeout

Three new methods on ExecBuilder consuming Self. ExecBuilder.run_with_sinks
composes them in the spec-fixed order: build sinks -> spawn timer -> cwd
guard -> stdin guard -> set shell.restricted -> run -> drop restricted (RAII)
-> drop stdin -> drop cwd -> cancel timer -> if timeout flag set, override
exit code to 124. with_cwd's closure signature simplified to FnOnce()->R so
the executor's run-core can re-borrow shell_cell without conflicting with
the cwd-guard borrow. ~20 new engine.rs unit tests cover cwd round-trip,
12 restricted refusals, timeout fire/cancel/zero, and several composition
shapes. Doc example updated to show the full sandbox chain.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Bash-diff harness `engine_sandbox_diff_check.sh`

**Files:**
- Create: `crates/huck-engine/examples/engine_sandbox_diff.rs`
- Create: `tests/scripts/engine_sandbox_diff_check.sh`

- [ ] **Step 1: Create the Rust driver**

```rust
//! Driver for the `engine_sandbox_diff_check.sh` bash-diff harness.
//!
//! Argv: `<mode> <fragment>` where mode is:
//!   - `bare`           — `.capture()` only.
//!   - `restricted`     — `.restricted(true).capture()`.
//!   - `cwd:<path>`     — `.cwd(<path>).capture()`.
//!   - `cwd:<path>:r`   — `.cwd(<path>).restricted(true).capture()`.
//!
//! Output format (same as v205's engine_capture_diff):
//!   STDOUT:<n>\n<bytes>STDERR:<n>\n<bytes>EXIT:<code>\n

use huck_engine::Engine;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().expect("mode arg");
    let fragment = args.next().expect("fragment arg");

    let mut e = Engine::new();
    let out = match mode.as_str() {
        "bare" => e.exec(&fragment).capture(),
        "restricted" => e.exec(&fragment).restricted(true).capture(),
        m if m.starts_with("cwd:") => {
            let body = &m[4..];
            let (path, restricted) = if let Some(stripped) = body.strip_suffix(":r") {
                (PathBuf::from(stripped), true)
            } else {
                (PathBuf::from(body), false)
            };
            let mut b = e.exec(&fragment).cwd(path);
            if restricted { b = b.restricted(true); }
            b.capture()
        }
        _ => panic!("unknown mode: {mode}"),
    };

    let stdout = std::io::stdout();
    let mut h = stdout.lock();
    writeln!(h, "STDOUT:{}", out.stdout.len()).unwrap();
    h.write_all(out.stdout.as_bytes()).unwrap();
    writeln!(h, "STDERR:{}", out.stderr.len()).unwrap();
    h.write_all(out.stderr.as_bytes()).unwrap();
    writeln!(h, "EXIT:{}", out.exit_code).unwrap();
}
```

- [ ] **Step 2: Create the bash harness**

```bash
#!/usr/bin/env bash
# Bash-diff harness for v206 sandbox knobs.
# Compares huck Engine (via the engine_sandbox_diff example binary) against
# bash on the same fragments. For restricted-mode fragments uses
# `bash --restricted -c '…'`.
#
# Requires: bash 5+, the huck workspace built (`cargo build`).
set -u

cd "$(dirname "$0")/../.." || exit 1
cargo build --quiet --example engine_sandbox_diff -p huck-engine >/dev/null 2>&1
DRIVER=target/debug/examples/engine_sandbox_diff
if [ ! -x "$DRIVER" ]; then
    echo "FAIL: could not locate engine_sandbox_diff driver at $DRIVER" >&2
    exit 1
fi

# Output capture helpers — mirror the v205 protocol.
emit_capture() {
    local out_file=$1 err_file=$2 exit_code=$3
    local out_bytes err_bytes
    out_bytes=$(wc -c <"$out_file")
    err_bytes=$(wc -c <"$err_file")
    printf 'STDOUT:%s\n' "$out_bytes"
    cat "$out_file"
    printf 'STDERR:%s\n' "$err_bytes"
    cat "$err_file"
    printf 'EXIT:%s\n' "$exit_code"
}

run_bash() {
    local flags=$1 frag=$2
    local out_file err_file exit_code
    out_file=$(mktemp)
    err_file=$(mktemp)
    # shellcheck disable=SC2086
    bash $flags -c "$frag" >"$out_file" 2>"$err_file"
    exit_code=$?
    emit_capture "$out_file" "$err_file" "$exit_code"
    rm -f "$out_file" "$err_file"
}

FAIL=0
check() {
    local label=$1 huck_mode=$2 bash_flags=$3 frag=$4
    local huck_out bash_out
    huck_out=$("$DRIVER" "$huck_mode" "$frag")
    bash_out=$(run_bash "$bash_flags" "$frag")
    if [ "$huck_out" != "$bash_out" ]; then
        echo "FAIL [$label]"
        diff <(printf '%s' "$huck_out") <(printf '%s' "$bash_out") || true
        FAIL=1
    else
        echo "PASS [$label]"
    fi
}

# Bare fragments (sanity — exercise the driver itself)
check 'bare-echo'              bare       ''         'echo hi'

# Restricted fragments (compare against `bash --restricted -c '…'`)
# NOTE: bash --restricted's exact error wording differs from huck's. We only
# compare EXIT codes for these via a stripped-stderr variant; full byte-equality
# is enforced only for the bare baselines.
#
# Implementation: run a parallel `_exit_only` check for restricted refusals.
check_exit_only() {
    local label=$1 huck_mode=$2 bash_flags=$3 frag=$4
    local huck_code bash_code
    huck_code=$("$DRIVER" "$huck_mode" "$frag" | sed -n 's/^EXIT://p')
    bash $bash_flags -c "$frag" >/dev/null 2>&1
    bash_code=$?
    if [ "$huck_code" != "$bash_code" ]; then
        echo "FAIL [$label] huck_exit=$huck_code bash_exit=$bash_code"
        FAIL=1
    else
        echo "PASS [$label] exit=$huck_code"
    fi
}

# Restricted: cd refused (both should exit nonzero).
check_exit_only 'r-cd'            restricted --restricted   'cd /tmp'
# Restricted: exec refused.
check_exit_only 'r-exec'          restricted --restricted   'exec /bin/true'
# Restricted: slash command refused.
check_exit_only 'r-slash-cmd'     restricted --restricted   '/bin/echo hi'
# Restricted: bare command works under restricted.
check 'r-bare-true' restricted --restricted  'true; echo ok'
# Restricted: source with slash refused.
check_exit_only 'r-source-slash'  restricted --restricted   '. /etc/profile'

# CWD fragment: bash equivalent uses `cd $tmp` prefix.
TMP=$(mktemp -d)
check_exit_only 'cwd-pwd'         "cwd:$TMP"  ''         "cd $TMP; pwd"

if [ -d "$TMP" ]; then rm -rf "$TMP"; fi

if [ $FAIL -ne 0 ]; then
    echo "engine_sandbox_diff_check FAILED" >&2
    exit 1
fi
echo "engine_sandbox_diff_check OK"
```

`chmod +x tests/scripts/engine_sandbox_diff_check.sh`.

**Note** on the byte-vs-exit comparison: for restricted refusals, bash's error wording (`bash: restricted: …`) differs from huck's (`huck: restricted: …`). Exact stderr-byte parity is impossible. We compare:
- **Bare fragments**: full byte parity (stdout + stderr + exit) — same as v205 harness.
- **Restricted-refusal fragments**: exit-code only via `check_exit_only`. This catches "huck didn't refuse" (exit 0 vs bash's nonzero) without false-positiving on the program-name prefix.

- [ ] **Step 3: Run the harness**

```bash
chmod +x tests/scripts/engine_sandbox_diff_check.sh
bash tests/scripts/engine_sandbox_diff_check.sh
```

Expected: all checks PASS.

- [ ] **Step 4: Run the full suite + clippy**

```bash
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: green; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add tests/scripts/engine_sandbox_diff_check.sh \
        crates/huck-engine/examples/engine_sandbox_diff.rs
git commit -m "$(cat <<'EOF'
v206 task 7: bash-diff harness for engine sandbox knobs

Rust driver engine_sandbox_diff takes a mode (bare/restricted/cwd:<path>) and a
fragment, runs through Engine, emits the v205-format STDOUT/STDERR/EXIT report.
Harness runs ~8 fragments against bash (with --restricted as appropriate). Bare
fragments enforce full byte parity; restricted refusals enforce exit-code parity
only (bash and huck's restricted: error wording diverges by program-name prefix).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Final verify + architecture.md

**Files:**
- Modify: `docs/architecture.md`

- [ ] **Step 1: Update `docs/architecture.md`**

Find the existing v205 paragraph on `huck-engine` (added by v205 Task 9) — it mentions `ExecBuilder`, `.stdin()`, `.merge_stderr()`, `StderrSink`, and `stdin_pipe.rs`. Append after the closing parenthesis on that paragraph:

```
Sandbox knobs (v206) layer on top: `.cwd(path)` chdirs for the call (RAII via
`cwd_scope.rs`, snapshotting OS cwd + shell `PWD`/`OLDPWD`); `.restricted(true)`
enables a bash `rbash`-subset policy (refuses `cd`/`exec`/slash-bearing
command names/slash-bearing `source` paths/absolute-or-`..`-redirect targets/
assignment to SHELL/PATH/ENV/BASH_ENV/`set +r`) via `restricted.rs`;
`.timeout(dur)` spawns a timer thread (`timeout.rs`) that, on deadline, sets
`Shell.timeout_flag` (polled by `executor::check_interrupt`) and SIGTERMs every
pid in `Shell.live_external_children`, with the call returning exit 124.
`ExecOutcome::Interrupted` carries an `InterruptReason::{Sigint,Timeout}`
discriminator so the top-level reducer can map to 130 (SIGINT) or 124 (timeout).
```

- [ ] **Step 2: Final full-suite + harness sweep**

```bash
cargo test --workspace --quiet
cargo test --workspace --doc --quiet
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --workspace --quiet
bash tests/scripts/engine_sandbox_diff_check.sh
bash tests/scripts/engine_capture_diff_check.sh
# Run every existing bash-diff harness:
for h in tests/scripts/*_diff_check.sh; do
    bash "$h" > /tmp/h.out 2>&1
    if [ $? -ne 0 ]; then
        echo "FAIL: $h"
        tail -20 /tmp/h.out
    fi
done
```

Expected: all green. Release binary builds. No existing-harness regressions.

- [ ] **Step 3: Headless CLI smoke test (sanity — v206 doesn't touch the CLI)**

```bash
./target/release/huck -c 'echo hello'
echo "exit=$?"
# Expected: 'hello', exit=0 — identical to v205/v204.
```

- [ ] **Step 4: Commit**

```bash
git add docs/architecture.md
git commit -m "$(cat <<'EOF'
v206 task 8: architecture.md note on sandbox knobs

Architecture doc gains a paragraph on cwd_scope.rs, restricted.rs,
timeout.rs and the InterruptReason discriminator, layered onto the v205
ExecBuilder paragraph. No bash-divergences.md change needed (sandbox knobs
are embedder-facing and don't change shell semantics).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 5: Confirm with user before merging to main**

Per CLAUDE.md: ask the user before:

```bash
git checkout main
git merge --no-ff v206-engine-sandbox -m "Merge v206: Engine sandbox knobs (cwd + restricted + timeout)"
git push origin main
git branch -d v206-engine-sandbox
```

After merge: update `project_huck_iterations.md` and `MEMORY.md` with the v206 entry.

---

## Self-review

**Spec coverage:**
- Public API (`.cwd`, `.restricted`, `.timeout` on `ExecBuilder`): Task 6.
- `Shell` field additions (`restricted`, `timeout_flag`, `live_external_children`): Task 1.
- `cwd_scope.rs` + `with_cwd`: Tasks 2 + 6 (signature simplification).
- `restricted.rs` + enforcement at 7 sites: Task 3.
- `timeout.rs` + check_interrupt poll + PID registry: Task 5.
- `ExecOutcome::Interrupted(InterruptReason)` widening: Task 4.
- Composition order (sinks → timer → cwd → stdin → restricted → run → restore → cancel → 124-override): Task 6.
- 32 unit tests + composition tests: Tasks 2, 3, 5, 6 (concentrated in 6).
- Doc example update: Task 6.
- Bash-diff harness: Task 7.
- Architecture doc: Task 8.

**Placeholder scan:** No "TBD" / "implement later". Each code block is complete enough to type-check. The two implementation-shape notes in Task 6 (the `with_cwd` no-arg closure decision + the closure-vs-helper-function decision in `run_with_sinks`) are EXPLICIT design choices the implementer is told to pick the cleanest form of — not vague hand-waves.

**Type consistency:** `InterruptReason::{Sigint, Timeout}` used consistently in Tasks 4, 5, 6. `Shell.timeout_flag: Arc<AtomicBool>` and `Shell.live_external_children: Arc<Mutex<Vec<libc::pid_t>>>` consistent in Tasks 1, 5, 6. `TimerHandle { handle, cancel_tx }` consistent in Task 5. `with_cwd(path: &Path, shell: &mut Shell, f: impl FnOnce() -> R) -> R` consistent after Task 6's Step 3 update.

**One revised step**: Task 3 originally had end-to-end engine.rs tests for restricted; revised to defer them to Task 6 (which has the builder method). The plan calls out the revision explicitly.

**Audit-scale note**: The restricted enforcement-sites are 7 small gates threaded into existing code (no large mechanical sweep). The Interrupted widening (Task 4) is a one-enum-variant change but ~20 call sites updated. Both are mechanical and reviewable per site.
