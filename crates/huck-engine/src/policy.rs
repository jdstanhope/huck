//! Restricted-mode policy. A `Policy` is the single authority on which
//! operations a shell may perform; enforcement sites ask it rather than
//! testing a flag and hand-rolling a refusal.
//!
//! Two restricted policies exist. `Rbash` mirrors bash's restricted shell
//! exactly (verified against 5.2.21). `Sandbox` is huck's embedding policy
//! (`ExecBuilder::restricted()`): it blocks escape from the working directory
//! but permits local work, so a hosted script can still write its own files.
//! Both speak bash's message vocabulary and differ only in what they deny.
//!
//! Messages are body-only (no invocation-name prefix) so the call site can
//! emit them through `sh_error!` / `sh_error_to!` — the "callers translate"
//! contract, same shape as `shell_state::declare_err_message`.
//!
//! Note what is NOT here: restricting `SHELL`/`PATH`/`HISTFILE`/`ENV`/
//! `BASH_ENV` is not an `Op`. bash marks those variables readonly when
//! restriction engages, so every write path (assignment, `+=`, `export`,
//! `read`, `declare`, `unset`) reports through ordinary readonly machinery
//! with that path's own wording. See `RESTRICTED_READONLY_VARS`.
//!
//! Also NOT here: `set +r` (leaving restricted mode). bash refuses it with
//! `set`'s own usage line and exit status, which a policy message body
//! cannot carry — that refusal lives in `set`'s option loop as a direct
//! `Policy::is_restricted()` test rather than as an `Op`. `Op` holds only
//! operations that produce a policy diagnostic.

use std::path::{Component, Path};

/// Variables bash marks readonly when restriction engages.
pub const RESTRICTED_READONLY_VARS: [&str; 5] = ["SHELL", "PATH", "HISTFILE", "ENV", "BASH_ENV"];

/// Which operations are permitted in this shell.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Policy {
    #[default]
    Unrestricted,
    /// bash's restricted shell (`rbash`, `-r`, `set -r`).
    Rbash,
    /// huck's embedding sandbox (`ExecBuilder::restricted()`).
    Sandbox,
}

/// An operation a policy may refuse. Borrowed, so checking allocates nothing
/// on the permitted path.
#[derive(Debug)]
pub enum Op<'a> {
    Cd,
    Exec,
    CommandName(&'a str),
    SourcePath(&'a str),
    /// A redirect whose target resolved to a FILE. Fd-duplication (`>&2`,
    /// `2>&1`) never constructs this, which is why bash permits it.
    RedirectFile {
        path: &'a str,
    },
}

impl Policy {
    #[inline]
    pub fn is_restricted(&self) -> bool {
        !matches!(self, Policy::Unrestricted)
    }

    /// Ask whether `op` is permitted. `Err` carries the body-only diagnostic.
    pub fn check(&self, op: Op<'_>) -> Result<(), String> {
        // The unrestricted fast path: one branch, no per-op work.
        let policy = match self {
            Policy::Unrestricted => return Ok(()),
            p => p,
        };
        match op {
            Op::Cd => Err("cd: restricted".to_string()),
            Op::Exec => Err("exec: restricted".to_string()),
            Op::CommandName(name) => {
                if name.contains('/') {
                    Err(format!(
                        "{name}: restricted: cannot specify `/' in command names"
                    ))
                } else {
                    Ok(())
                }
            }
            Op::SourcePath(path) => {
                if path.contains('/') {
                    Err(format!(".: {path}: restricted"))
                } else {
                    Ok(())
                }
            }
            Op::RedirectFile { path } => {
                // The one place the two policies genuinely differ in logic:
                // Rbash refuses every file target, Sandbox only escaping ones.
                let refuse = match policy {
                    Policy::Rbash => true,
                    Policy::Sandbox => escapes_cwd(path),
                    Policy::Unrestricted => unreachable!("handled above"),
                };
                if refuse {
                    Err(format!("{path}: restricted: cannot redirect output"))
                } else {
                    Ok(())
                }
            }
        }
    }
}

/// True when `path` could write outside the current directory tree.
fn escapes_cwd(path: &str) -> bool {
    path.starts_with('/')
        || Path::new(path)
            .components()
            .any(|c| matches!(c, Component::ParentDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The full decision matrix. Adding an `Op` variant should force a new
    /// row here — that is the point of the enum over scattered conditionals.
    #[test]
    fn matrix_unrestricted_allows_everything() {
        let p = Policy::Unrestricted;
        assert!(p.check(Op::Cd).is_ok());
        assert!(p.check(Op::Exec).is_ok());
        assert!(p.check(Op::CommandName("/bin/echo")).is_ok());
        assert!(p.check(Op::SourcePath("/etc/profile")).is_ok());
        assert!(p.check(Op::RedirectFile { path: "/tmp/x" }).is_ok());
    }

    #[test]
    fn matrix_rbash_denies_all_guarded_ops() {
        let p = Policy::Rbash;
        assert!(p.check(Op::Cd).is_err());
        assert!(p.check(Op::Exec).is_err());
        assert!(p.check(Op::CommandName("/bin/echo")).is_err());
        assert!(p.check(Op::SourcePath("/etc/profile")).is_err());
        // Rbash denies EVERY file-target redirect, relative ones included.
        assert!(p.check(Op::RedirectFile { path: "log" }).is_err());
        assert!(p.check(Op::RedirectFile { path: "/tmp/x" }).is_err());
    }

    #[test]
    fn matrix_rbash_allows_bare_names_and_slashless_source() {
        let p = Policy::Rbash;
        assert!(p.check(Op::CommandName("echo")).is_ok());
        assert!(p.check(Op::CommandName("my-cmd")).is_ok());
        assert!(p.check(Op::SourcePath("profile")).is_ok());
    }

    #[test]
    fn matrix_sandbox_denies_escaping_redirects_only() {
        let p = Policy::Sandbox;
        // Escape attempts refused.
        assert!(p.check(Op::RedirectFile { path: "/tmp/x" }).is_err());
        assert!(p.check(Op::RedirectFile { path: "../escape" }).is_err());
        assert!(p.check(Op::RedirectFile { path: "foo/../bar" }).is_err());
        // Local work permitted — this is the one behavioral difference from Rbash.
        assert!(p.check(Op::RedirectFile { path: "log" }).is_ok());
        assert!(p.check(Op::RedirectFile { path: "sub/log" }).is_ok());
        assert!(p.check(Op::RedirectFile { path: "./log" }).is_ok());
    }

    #[test]
    fn matrix_sandbox_matches_rbash_on_non_redirect_ops() {
        let p = Policy::Sandbox;
        assert!(p.check(Op::Cd).is_err());
        assert!(p.check(Op::Exec).is_err());
        assert!(p.check(Op::CommandName("/bin/echo")).is_err());
        assert!(p.check(Op::CommandName("echo")).is_ok());
        assert!(p.check(Op::SourcePath("/etc/profile")).is_err());
        assert!(p.check(Op::SourcePath("profile")).is_ok());
    }

    /// Message bodies are bash's, verbatim. These strings are asserted
    /// byte-for-byte by tests/scripts/rbash_diff_check.sh against the real
    /// shell; if you change one, change it there too.
    #[test]
    fn messages_match_bash_wording() {
        let p = Policy::Rbash;
        assert_eq!(p.check(Op::Cd).unwrap_err(), "cd: restricted");
        assert_eq!(p.check(Op::Exec).unwrap_err(), "exec: restricted");
        assert_eq!(
            p.check(Op::CommandName("/bin/echo")).unwrap_err(),
            "/bin/echo: restricted: cannot specify `/' in command names"
        );
        assert_eq!(
            p.check(Op::SourcePath("/etc/profile")).unwrap_err(),
            ".: /etc/profile: restricted"
        );
        assert_eq!(
            p.check(Op::RedirectFile { path: "f" }).unwrap_err(),
            "f: restricted: cannot redirect output"
        );
    }

    /// Both policies share bash's vocabulary; they differ only in WHAT they deny.
    #[test]
    fn sandbox_uses_bash_wording_too() {
        let p = Policy::Sandbox;
        assert_eq!(p.check(Op::Cd).unwrap_err(), "cd: restricted");
        assert_eq!(
            p.check(Op::RedirectFile { path: "/tmp/x" }).unwrap_err(),
            "/tmp/x: restricted: cannot redirect output"
        );
    }

    #[test]
    fn is_restricted_reflects_policy() {
        assert!(!Policy::Unrestricted.is_restricted());
        assert!(Policy::Rbash.is_restricted());
        assert!(Policy::Sandbox.is_restricted());
    }

    #[test]
    fn readonly_var_set_matches_bash() {
        // bash marks exactly these five readonly when restriction engages.
        assert_eq!(
            RESTRICTED_READONLY_VARS,
            ["SHELL", "PATH", "HISTFILE", "ENV", "BASH_ENV"]
        );
    }
}
