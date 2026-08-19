//! The ONE place huck decides whether an error is fatal, and with what code.
//!
//! Before v358 this was answered ad hoc at two dozen sites, and the results
//! were not merely inconsistent — they were UNCORRELATED with bash, wrong in
//! both directions at once: huck exited the shell where bash carried on (#25,
//! a malformed backtick substitution) and carried on where bash abandoned the
//! command list (#116, `history` with too many arguments). A cluster that
//! errs in both directions is the signature of a decision with no owner.
//!
//! Every rule below is measured against bash 5.2.21. Three of them look like
//! inconsistencies and are not; each has a TEST rather than a comment, because
//! a future reader WILL want to "fix" them:
//!
//!   * a plain top-level syntax error exits 2 under `-c`, while every other
//!     fatal error substitutes 127 there;
//!   * of fifteen measured builtin errors, only `history` with too many
//!     arguments abandons the command list;
//!   * and that one kind's OUTCOME is driver-dependent, not just its code —
//!     under `-c` it ends the whole program, while its neighbours abandon the
//!     list and carry on to the next line.
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
    ///
    /// `status` is the builtin's OWN usage status — the builtin knows it, the
    /// classifier only decides whether it is fatal. Flattening it to a
    /// constant changed `export`'s bad-assignment status from 1 to 2 and was
    /// caught by a unit test.
    SpecialBuiltinUsage { status: i32 },
    /// A POSIX special builtin rejected an OPERAND (`. /nonexistent`).
    /// Separate from `SpecialBuiltinUsage` because it exits 1, not 2.
    SpecialBuiltinOperand,
    /// A readonly assignment used as a COMMAND PREFIX (`r=2 true`), which bash
    /// treats far more leniently than a standalone `r=2`.
    AssignmentPrefix,
    /// A readonly variable used as a `for` loop's iteration variable. bash
    /// CONTINUES outright outside posix — unlike a standalone `r=2`, which
    /// abandons the list — and exits in posix.
    ReadonlyForVar,
    /// Any other builtin error. Measured: ALWAYS continues.
    ///
    /// Deliberately unconstructed: `Continue` is the no-op verdict, so the ~360
    /// emit-only builtin sites reach it by simply not calling `report_error` at
    /// all. Kept because it names the measured default and
    /// `ordinary_builtin_errors_always_continue` pins it — a new builtin error
    /// site that DOES want to route through the classifier has an obvious,
    /// tested kind to reach for.
    #[allow(dead_code)]
    BuiltinError,
    /// `history` or `return` with too many arguments — the builtin errors in
    /// bash that abandon the command list rather than continuing.
    ///
    /// It is a KIND rather than a general rule because a rule fitted to these
    /// would be a fabrication: `cd -Q`, `kill -Q`, `read -Q`, `getopts`,
    /// `umask a b`, `history -Q`, `history a`, and even the special builtins
    /// `shift a b` and `break 1 2` all continue.
    ///
    /// v364 follow-on (#683) added `return`, and it is the same kind rather
    /// than a second one because the two are byte-identical on all five
    /// drivers — measured, not assumed:
    ///
    /// ```text
    ///           return 1 2                     history 1 2
    /// script :  message, B st=1, rc 0          message, B st=1, rc 0
    /// stdin  :  message, B st=1, rc 0          message, B st=1, rc 0
    /// -c     :  message, STOPS,  rc 1          message, STOPS,  rc 1
    /// source :  message, STOPS,  rc 1          message, STOPS,  rc 1
    /// eval   :  message, STOPS,  rc 1          message, STOPS,  rc 1
    /// ```
    ///
    /// Note the name is about the SHAPE of the error, not the builtin: adding a
    /// third member means measuring it against this table first.
    TooManyArgsAbortsList,
    /// A syntax error inside a command substitution.
    ///
    /// `backtick` is load-bearing rather than cosmetic. A backtick body is
    /// parsed during EXPANSION, so bash reports the error and carries on;
    /// `$( )` is parsed with the script, so its error is a shell syntax error
    /// and a non-interactive shell dies of it.
    ComsubSyntax { backtick: bool },
    /// A syntax error in the script itself.
    Syntax,
    /// A syntax error raised while a COMPOUND ASSIGNMENT was open — `v=(`,
    /// `v+=(`, `declare -a v=(`.
    ///
    /// The same outcome as `Syntax` with a different code, and the code is the
    /// only thing that differs: bash exits **1** here and 2 for every other
    /// syntax error, with no `-c` substitution. Measured on all five drivers:
    ///
    /// ```text
    /// script / stdin / -c :  v=(a        -> 1        echo "a      -> 2
    /// source              :  OUTER=1, caller survives (an ordinary syntax error gives OUTER=2)
    /// eval                :  AFTER=1                              -> 2
    /// ```
    ///
    /// Keyed on the CONTEXT, not on the error: `v=('abc` names the quote and
    /// `v=(${x` names the brace, and both still exit 1, while the same two
    /// errors inside a subshell `(`, a group `{` or a function body exit 2
    /// (#633).
    CompoundAssignmentSyntax,
    /// `set -u` on an unset variable, reported from the LENGTH form with a
    /// SUBSCRIPT — `${#nope[0]}`. The same error as `UnsetUnderNounset`
    /// except that bash's `-c` 127 substitution does NOT reach it. Measured:
    ///
    /// ```text
    /// bash -c 'set -u; echo $nope'          -> 127
    /// bash -c 'set -u; echo ${nope[0]}'     -> 127
    /// bash -c 'set -u; echo ${#nope[0]}'    ->   1   <- this one
    /// ```
    ///
    /// A script file gives 1 for all three, so the split is `-c`-only (#572).
    UnsetUnderNounsetLength,
    /// A bad substitution whose parameter is `$@` under the LENGTH prefix —
    /// `${#@:-D}`, `${#@#a}`, `${#@[0]}`. bash makes this one FATAL where every
    /// other bad substitution lets the script carry on. Measured:
    ///
    /// ```text
    /// script:  echo ${#@:-D}; echo SAME   -> message, rc 1, no SAME
    ///          echo ${#v:-D}; echo SAME   -> message, rc 0, SAME
    /// bash -c: echo ${#@:-D}              -> 127
    ///          echo ${#v:-D}              ->   1
    /// ```
    ///
    /// `$*` is NOT included (`${#*:-D}` is the ordinary non-fatal kind), nor is
    /// `$@` without the length prefix (`${@!}`) — the fatality is specific to
    /// the pair (#605).
    BadSubstAllArgsLength,
    /// The here-document limit (bash's `HEREDOC_MAX`) exceeded in a NESTED
    /// parse — a sourced file, an `eval`, or a function calling either.
    ///
    /// Its own kind because it is the one lex error bash treats as fatal to
    /// the WHOLE shell rather than to the parse context that raised it:
    /// `source bad; echo OUTER` prints nothing and exits 1, where an ordinary
    /// syntax error in the same file lets the caller carry on (#340). The
    /// status is a plain 1 with no `-c` substitution — measured under `-c`,
    /// a script file and a function.
    HeredocLimit,
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
        // ⚠️ NO driver substitution. Measured: `set -o posix; set -Q` and
        // `export -Q` exit 2 under `-c`, a script AND stdin alike — unlike the
        // expansion fatals, which become 127 under `-c`. An earlier draft
        // applied `driver_code` here and the harness still passed, because the
        // legacy `posix_fatal(2)` path was answering first; the bug was found
        // by measuring rather than by a red row.
        ErrorKind::SpecialBuiltinUsage { status } => {
            if posix && can_exit {
                Fatality::ExitShell(status)
            } else {
                Fatality::Continue
            }
        }
        // A special builtin rejecting an OPERAND rather than an option —
        // `. /nonexistent`. Measured: exits 1 in posix under every driver
        // (again no substitution), continues otherwise.
        ErrorKind::SpecialBuiltinOperand => {
            if posix && can_exit {
                Fatality::ExitShell(1)
            } else {
                Fatality::Continue
            }
        }
        // A readonly assignment used as a COMMAND PREFIX (`r=2 true`) rather
        // than as a standalone command. Measured: bash abandons the list in
        // posix and continues outright otherwise — it never exits, where a
        // standalone `r=2` in posix does. The distinction is the prefix.
        // ⚠️ Outside posix this CONTINUES; it does not abandon the list. The
        // site previously called `posix_fatal`, which was a NO-OP outside
        // posix, so classifying it as `Expansion` (which aborts) was a
        // regression — caught by `posix_exit_on_error_diff_check.sh`, not by
        // any unit test. Measured: `readonly i=1; for i in a b; do :; done;
        // echo AFTER` prints AFTER and exits 0.
        ErrorKind::ReadonlyForVar => {
            if posix && can_exit {
                Fatality::ExitShell(driver_code(1, shell))
            } else {
                Fatality::Continue
            }
        }
        ErrorKind::AssignmentPrefix => {
            if posix {
                Fatality::AbortList
            } else {
                Fatality::Continue
            }
        }
        ErrorKind::BuiltinError => Fatality::Continue,
        // ⚠️ Driver-dependent OUTCOME, not just a driver-dependent code, and
        // the only kind here that is. Under `-c` bash ends the whole program
        // with status 1 even when a further line follows; from a script or
        // stdin it abandons only the current list and carries on. Its
        // neighbours do NOT behave that way — an arithmetic or readonly error
        // under `-c` continues to the next line — so this cannot be folded
        // into a general rule. Measured 2026-08-06.
        ErrorKind::TooManyArgsAbortsList => {
            if shell.is_command_string && !shell.is_interactive {
                Fatality::ExitShell(1)
            } else {
                Fatality::AbortList
            }
        }
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
        // ⚠️ NO driver substitution, and deliberately 1 rather than `Syntax`'s 2.
        // Measured on script, stdin, `-c`, `source` and `eval` alike.
        ErrorKind::CompoundAssignmentSyntax => {
            if can_exit {
                Fatality::ExitShell(1)
            } else {
                Fatality::Continue
            }
        }
        ErrorKind::BadSubstAllArgsLength => {
            if can_exit {
                Fatality::ExitShell(driver_code(1, shell))
            } else {
                Fatality::AbortList
            }
        }
        ErrorKind::UnsetUnderNounsetLength => {
            if can_exit {
                Fatality::ExitShell(1)
            } else {
                Fatality::AbortList
            }
        }
        ErrorKind::HeredocLimit => {
            if can_exit {
                Fatality::ExitShell(1)
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
            fatality(
                ErrorKind::SpecialBuiltinUsage { status: 2 },
                &shell_with(false, false)
            ),
            Fatality::Continue
        );
        assert_eq!(
            fatality(
                ErrorKind::SpecialBuiltinUsage { status: 2 },
                &shell_with(true, false)
            ),
            Fatality::ExitShell(2)
        );
        // ⚠️ 2, NOT 127 — this kind takes no driver substitution. Measured:
        // `bash -c 'set -o posix; set -Q'` exits 2.
        assert_eq!(
            fatality(
                ErrorKind::SpecialBuiltinUsage { status: 2 },
                &shell_with(true, true)
            ),
            Fatality::ExitShell(2)
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
                fatality(ErrorKind::TooManyArgsAbortsList, &shell_with(posix, false)),
                Fatality::AbortList
            );
        }
    }

    #[test]
    fn history_too_many_args_ends_the_whole_program_under_dash_c() {
        // ⚠️ Driver-dependent OUTCOME, unique to this kind. `bash -c
        // 'history 1 2 3; echo SAME\necho NEXT'` prints nothing and exits 1,
        // while the same shape with an arithmetic or readonly error prints
        // NEXT and exits 0. A test rather than a comment because it looks like
        // an inconsistency and is not.
        assert_eq!(
            fatality(ErrorKind::TooManyArgsAbortsList, &shell_with(false, true)),
            Fatality::ExitShell(1)
        );
        assert_eq!(
            fatality(ErrorKind::Expansion, &shell_with(false, true)),
            Fatality::AbortList
        );
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
    fn a_compound_assignment_syntax_error_is_1_under_every_driver() {
        // #633. Same outcome as `Syntax`, different code, and NO `-c`
        // substitution — measured on script, stdin, `-c`, `source` and `eval`:
        // `v=(a` is 1 where `echo "a` is 2.
        // posix does not enter into it either, so all four combinations give 1.
        for posix in [true, false] {
            for dash_c in [true, false] {
                assert_eq!(
                    fatality(
                        ErrorKind::CompoundAssignmentSyntax,
                        &shell_with(posix, dash_c)
                    ),
                    Fatality::ExitShell(1),
                    "posix={posix} dash_c={dash_c}"
                );
            }
        }
        // An interactive shell returns to its prompt, like every other kind.
        let mut interactive = shell_with(false, false);
        interactive.is_interactive = true;
        assert_eq!(
            fatality(ErrorKind::CompoundAssignmentSyntax, &interactive),
            Fatality::Continue
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
    fn readonly_for_var_continues_outside_posix() {
        // ⚠️ Regression guard. Classifying this as `Expansion` made it abandon
        // the list outside posix, because the site it replaced (`posix_fatal`)
        // was a no-op there. bash prints AFTER and exits 0.
        assert_eq!(
            fatality(ErrorKind::ReadonlyForVar, &shell_with(false, false)),
            Fatality::Continue
        );
        assert_eq!(
            fatality(ErrorKind::ReadonlyForVar, &shell_with(true, false)),
            Fatality::ExitShell(1)
        );
        assert_eq!(
            fatality(ErrorKind::ReadonlyForVar, &shell_with(true, true)),
            Fatality::ExitShell(127)
        );
    }

    #[test]
    fn an_interactive_shell_is_never_killed_by_an_error() {
        for kind in [
            ErrorKind::Expansion,
            ErrorKind::UnsetUnderNounset,
            ErrorKind::SpecialBuiltinUsage { status: 2 },
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
