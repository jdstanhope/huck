//! Restricted-mode policy checks. When `Shell.restricted` is true, the
//! enforcement sites consult these helpers to refuse operations that would
//! let a sandboxed script escape: cd, exec, slash-bearing command names,
//! slash-bearing source paths, absolute or `..` redirect paths, assignment
//! to SHELL/PATH/ENV/BASH_ENV, and `set +r`.
//!
//! Each helper returns `Result` with a body-only message (no invocation-name
//! prefix) so the caller can emit the diagnostic through the unified emitter
//! (`sh_error!`/`sh_error_to!`) — the "callers translate" contract, same
//! shape as `shell_state::declare_err_message`.

use crate::shell_state::Shell;
use std::path::Path;

#[inline]
pub fn is_restricted(shell: &Shell) -> bool {
    shell.restricted
}

pub fn check_cd() -> Result<(), &'static str> {
    Err("restricted: cd")
}

pub fn check_exec() -> Result<(), &'static str> {
    Err("restricted: exec")
}

pub fn check_command_name(name: &str) -> Result<(), String> {
    if name.contains('/') {
        Err(format!("restricted: {name}: restricted"))
    } else {
        Ok(())
    }
}

pub fn check_source_path(path: &str) -> Result<(), &'static str> {
    if path.contains('/') {
        Err("restricted: source: paths with '/'")
    } else {
        Ok(())
    }
}

/// `path` is the redirect target string from the script source (e.g. `>file`
/// → `"file"`). An absolute path OR any `..` component is refused.
pub fn check_redirect_path(path: &str) -> Result<(), String> {
    if path.starts_with('/')
        || Path::new(path)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        Err(format!("restricted: {path}"))
    } else {
        Ok(())
    }
}

pub fn check_special_assign(name: &str) -> Result<(), String> {
    if matches!(name, "SHELL" | "PATH" | "ENV" | "BASH_ENV") {
        Err(format!("restricted: {name}: readonly variable"))
    } else {
        Ok(())
    }
}

pub fn check_set_plus_r() -> Result<(), &'static str> {
    Err("restricted: cannot turn off restricted mode")
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
