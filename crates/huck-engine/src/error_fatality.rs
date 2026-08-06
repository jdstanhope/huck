//! The ONE place huck decides whether an error is fatal, and with what code.
//!
//! Before v358 this was answered ad hoc at two dozen sites, and the results
//! were not merely inconsistent — they were UNCORRELATED with bash, wrong in
//! both directions at once: huck exited the shell where bash carried on (#25,
//! a malformed backtick substitution) and carried on where bash abandoned the
//! command list (#116, `history` with too many arguments). A cluster that
//! errs in both directions is the signature of a decision with no owner.
//!
//! Every rule below is measured against bash 5.2.21. Two of them look wrong
//! and are not; both have a test rather than a comment, because a future
//! reader WILL want to "fix" them:
//!
//!   * a plain top-level syntax error exits 2 under `-c`, while every other
//!     fatal error substitutes 127 there;
//!   * of fifteen measured builtin errors, only `history` with too many
//!     arguments abandons the command list.
//!
//! `Shell::raise_fatal` / `raise_discard` are `pub(crate)` and reached through
//! `Shell::report_error`, so a site cannot quietly decide for itself.

use crate::shell_state::Shell;

/// What KIND of error occurred — a classification, never a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    /// Arithmetic failure, bad substitution, and the other expansion errors.
    Expansion,
    /// An unset variable under `set -u`.
    UnsetUnderNounset,
    /// A POSIX special builtin rejected its options or operands.
    ///
    /// This is about USAGE rejection, not about every error a special builtin
    /// can raise: bash continues past `shift a b` and `break 1 2` in both
    /// modes, so those are `BuiltinError`.
    SpecialBuiltinUsage,
    /// Any other builtin error. Measured: ALWAYS continues.
    BuiltinError,
    /// `history` with too many arguments — the only builtin error in bash that
    /// abandons the command list. It gets its own kind because a general rule
    /// fitted to a single data point would be a fabrication: `cd -Q`,
    /// `kill -Q`, `read -Q`, `getopts`, `umask a b`, `history -Q` and
    /// `history a` all continue.
    HistoryTooManyArgs,
    /// A syntax error inside a command substitution.
    ///
    /// `backtick` is load-bearing rather than cosmetic. A backtick body is
    /// parsed during EXPANSION, so bash reports the error and carries on;
    /// `$( )` is parsed with the script, so its error is a shell syntax error
    /// and a non-interactive shell dies of it.
    ComsubSyntax { backtick: bool },
    /// A syntax error in the script itself.
    Syntax,
}

/// The three outcomes bash actually has. `AbortList` is the one an outcome
/// probe under `-c` cannot see — it looks identical to `ExitShell` there,
/// which is why the harness drives a two-line script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fatality {
    /// Emit the diagnostic and carry on.
    Continue,
    /// Abandon the rest of the current command list; the shell survives.
    AbortList,
    /// Leave, with this status.
    ExitShell(i32),
}

/// `-c` substitutes 127 for the kind's own code; a script or stdin keeps it.
///
/// Reads the EXISTING `is_command_string` rather than new state. Script and
/// stdin were measured to behave identically, so one bool is the whole axis —
/// a three-way `Invocation` enum would encode a distinction that carries no
/// behaviour.
fn driver_code(base: i32, shell: &Shell) -> i32 {
    if shell.is_command_string { 127 } else { base }
}

/// The ONLY place an error's fatality is decided.
pub(crate) fn fatality(kind: ErrorKind, shell: &Shell) -> Fatality {
    // An interactive shell is never killed by one of these — it returns to the
    // prompt. Hoisted so no rule below has to restate it.
    let can_exit = !shell.is_interactive;
    let posix = shell.shell_options.posix;

    match kind {
        ErrorKind::Expansion => {
            if posix && can_exit {
                Fatality::ExitShell(driver_code(1, shell))
            } else {
                Fatality::AbortList
            }
        }
        ErrorKind::UnsetUnderNounset => {
            if can_exit {
                Fatality::ExitShell(driver_code(1, shell))
            } else {
                Fatality::AbortList
            }
        }
        ErrorKind::SpecialBuiltinUsage => {
            if posix && can_exit {
                Fatality::ExitShell(driver_code(2, shell))
            } else {
                Fatality::Continue
            }
        }
        ErrorKind::BuiltinError => Fatality::Continue,
        ErrorKind::HistoryTooManyArgs => Fatality::AbortList,
        ErrorKind::ComsubSyntax { backtick } => {
            if backtick || !can_exit {
                Fatality::Continue
            } else {
                Fatality::ExitShell(driver_code(2, shell))
            }
        }
        // ⚠️ The `-c` substitution deliberately does NOT apply here. bash
        // rejects a top-level syntax error before execution begins, so the
        // `-c` path never reaches the substitution: `bash -c 'if'` exits 2,
        // while `bash -c 'echo $(echo a; ; echo b)'` exits 127.
        ErrorKind::Syntax => {
            if can_exit {
                Fatality::ExitShell(2)
            } else {
                Fatality::Continue
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell_state::Shell;

    /// `dash_c` is `Shell::is_command_string` — the EXISTING field recording
    /// `huck -c`. Script and stdin both spell it `false`, which is the
    /// measured truth rather than a simplification.
    fn shell_with(posix: bool, dash_c: bool) -> Shell {
        let mut s = Shell::new();
        s.shell_options.posix = posix;
        s.is_interactive = false;
        s.is_command_string = dash_c;
        s
    }

    #[test]
    fn expansion_error_aborts_the_list_outside_posix() {
        let s = shell_with(false, false);
        assert_eq!(fatality(ErrorKind::Expansion, &s), Fatality::AbortList);
    }

    #[test]
    fn expansion_error_exits_in_posix_with_the_drivers_code() {
        assert_eq!(
            fatality(ErrorKind::Expansion, &shell_with(true, false)),
            Fatality::ExitShell(1)
        );
        assert_eq!(
            fatality(ErrorKind::Expansion, &shell_with(true, true)),
            Fatality::ExitShell(127)
        );
    }

    #[test]
    fn nounset_always_exits_with_the_drivers_code() {
        assert_eq!(
            fatality(ErrorKind::UnsetUnderNounset, &shell_with(false, false)),
            Fatality::ExitShell(1)
        );
        assert_eq!(
            fatality(ErrorKind::UnsetUnderNounset, &shell_with(false, true)),
            Fatality::ExitShell(127)
        );
    }

    #[test]
    fn special_builtin_usage_is_fatal_only_in_posix() {
        assert_eq!(
            fatality(ErrorKind::SpecialBuiltinUsage, &shell_with(false, false)),
            Fatality::Continue
        );
        assert_eq!(
            fatality(ErrorKind::SpecialBuiltinUsage, &shell_with(true, false)),
            Fatality::ExitShell(2)
        );
        assert_eq!(
            fatality(ErrorKind::SpecialBuiltinUsage, &shell_with(true, true)),
            Fatality::ExitShell(127)
        );
    }

    #[test]
    fn ordinary_builtin_errors_always_continue() {
        for posix in [false, true] {
            for dash_c in [false, true] {
                assert_eq!(
                    fatality(ErrorKind::BuiltinError, &shell_with(posix, dash_c)),
                    Fatality::Continue
                );
            }
        }
    }

    #[test]
    fn history_too_many_args_aborts_the_list_in_both_modes() {
        // The ONLY builtin error in bash that abandons the list. Measured
        // across 15 cases; every other builtin error continues, including the
        // special builtins `shift a b` and `break 1 2`.
        for posix in [false, true] {
            assert_eq!(
                fatality(ErrorKind::HistoryTooManyArgs, &shell_with(posix, false)),
                Fatality::AbortList
            );
        }
    }

    #[test]
    fn backtick_comsub_syntax_error_continues_but_dollar_paren_exits() {
        let s = shell_with(false, false);
        assert_eq!(
            fatality(ErrorKind::ComsubSyntax { backtick: true }, &s),
            Fatality::Continue
        );
        assert_eq!(
            fatality(ErrorKind::ComsubSyntax { backtick: false }, &s),
            Fatality::ExitShell(2)
        );
    }

    #[test]
    fn dollar_paren_syntax_error_takes_127_under_dash_c() {
        assert_eq!(
            fatality(
                ErrorKind::ComsubSyntax { backtick: false },
                &shell_with(false, true)
            ),
            Fatality::ExitShell(127)
        );
    }

    #[test]
    fn plain_syntax_error_keeps_2_under_every_driver() {
        // THE EXCEPTION, and the reason this is a test and not a comment:
        // bash rejects a top-level syntax error before execution begins, so
        // the `-c` substitution never applies. `bash -c 'if'` exits 2.
        for dash_c in [true, false] {
            assert_eq!(
                fatality(ErrorKind::Syntax, &shell_with(false, dash_c)),
                Fatality::ExitShell(2)
            );
        }
    }

    #[test]
    fn an_interactive_shell_is_never_killed_by_an_error() {
        for kind in [
            ErrorKind::Expansion,
            ErrorKind::UnsetUnderNounset,
            ErrorKind::SpecialBuiltinUsage,
            ErrorKind::ComsubSyntax { backtick: false },
            ErrorKind::Syntax,
        ] {
            let mut s = shell_with(true, false);
            s.is_interactive = true;
            assert!(
                !matches!(fatality(kind, &s), Fatality::ExitShell(_)),
                "{kind:?} killed an interactive shell"
            );
        }
    }
}
