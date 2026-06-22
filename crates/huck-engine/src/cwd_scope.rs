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
///
/// The closure takes no arguments — it's expected to re-borrow whatever
/// shared state it needs through normal channels (e.g. `Rc<RefCell<Shell>>`).
/// This shape lets callers drop the `&mut Shell` borrow before invoking `f`,
/// avoiding borrow conflicts when `f` itself needs to re-borrow the shell.
#[allow(dead_code)]
pub fn with_cwd<R>(path: &Path, shell: &mut Shell, f: impl FnOnce() -> R) -> R {
    let saved_os = std::env::current_dir().ok();
    let saved_pwd = shell.lookup_var("PWD");
    let saved_oldpwd = shell.lookup_var("OLDPWD");

    if let Err(e) = std::env::set_current_dir(path) {
        eprintln!("huck: cwd: {}: {e}", path.display());
        return f();
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
            // closure `f` doesn't take Shell as a parameter, which lets the
            // `&mut shell` borrow END before `f` runs. Drop runs after `f`
            // returns but BEFORE `with_cwd` returns, so the pointer reborrows
            // the same memory the caller's `&mut shell` covers — single-
            // threaded by Engine contract.
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
    // Drop the `&mut Shell` borrow by not using `shell` again before f().
    // (The reborrow happens later in Restore::drop, after f has returned.)
    f()
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
        let inside = with_cwd(tmp.path(), &mut s, || std::env::current_dir().unwrap());
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
        // Ensure OLDPWD starts unset so we can assert the restore path removes it.
        // (Shell::new() imports env vars; an inherited OLDPWD would otherwise
        // mask the unset-restore behaviour.)
        s.unset("OLDPWD");
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
}
