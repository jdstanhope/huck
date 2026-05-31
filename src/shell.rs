use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rustyline::error::ReadlineError;
use rustyline::history::FileHistory;
use rustyline::{CompletionType, Config, Editor};
use signal_hook::consts::{SIGCHLD, SIGINT};

use crate::builtins::ExecOutcome;
use crate::command::{self, ParseError};
use crate::completion::HuckHelper;
use crate::executor;
use crate::lexer::{self, LexError};
use crate::shell_state::Shell;

const DEFAULT_PS1: &str = "huck> ";
const DEFAULT_PS2: &str = "> ";

/// The outcome of reading one logical (possibly multi-line) command.
enum ReadResult {
    /// A finished command: `buffer` is fed to the executor, `history`
    /// is its single-line form for the history list.
    Ready { buffer: String, history: String },
    /// Ctrl-C — any partial command is discarded; the REPL loops.
    Interrupted,
    /// Ctrl-D at an empty first-line prompt — exit the shell cleanly.
    Eof,
    /// EOF while a partial command was pending — a truncated command.
    EofMidCommand,
    /// A rustyline read error — exit the shell.
    ReadError(String),
}

/// Runs the interactive shell loop. Returns the process exit code.
pub fn run() -> i32 {
    install_job_control_signals();

    let config = Config::builder()
        .completion_type(CompletionType::List)
        .build();
    let mut editor: Editor<HuckHelper, FileHistory> = match Editor::with_config(config) {
        Ok(editor) => editor,
        Err(e) => {
            eprintln!("huck: failed to initialize line editor: {e}");
            return 1;
        }
    };
    editor.set_helper(Some(HuckHelper::new()));

    let mut shell = Shell::new();
    install_sigint_handler(Arc::clone(&shell.sigint_flag));
    install_sigchld_handler(Arc::clone(&shell.sigchld_flag));

    shell.history.load();
    for (_, command) in shell.history.entries() {
        let _ = editor.add_history_entry(command);
    }

    loop {
        crate::jobs::reap_and_notify(&mut shell);
        crate::traps::dispatch_pending_traps(&mut shell);
        if let Some(helper) = editor.helper_mut() {
            helper.refresh(&shell);
        }
        if let Some(exit_code) = fire_prompt_command(&mut shell) {
            crate::traps::fire_exit_trap(&mut shell);
            shell.hangup_jobs();
            shell.history.save();
            return exit_code;
        }
        match read_logical_command(&mut editor, &mut shell) {
            ReadResult::Ready { buffer, history } => {
                if !history.trim().is_empty() {
                    shell.history.add(history.clone());
                    let _ = editor.add_history_entry(history.as_str());
                }
                let do_alias = shell.is_interactive
                    || std::env::var("HUCK_EXPAND_ALIASES").is_ok();
                match process_line(&buffer, &mut shell, do_alias) {
                    ExecOutcome::Exit(code) => {
                        crate::traps::fire_exit_trap(&mut shell);
                        shell.hangup_jobs();
                        shell.history.save();
                        return code;
                    }
                    ExecOutcome::Continue(status) => {
                        shell.set_last_status(status);
                        // Drain any fatal PE error. In non-interactive mode
                        // (stdin not a TTY), exit immediately with the fatal
                        // status. Interactive: $? already set; fall through
                        // to the next prompt iteration.
                        if let Some(fatal_status) = shell.take_pending_fatal_pe_error()
                            && !shell.is_interactive
                        {
                            crate::traps::fire_exit_trap(&mut shell);
                            shell.hangup_jobs();
                            shell.history.save();
                            return fatal_status;
                        }
                    }
                    ExecOutcome::LoopBreak | ExecOutcome::LoopContinue
                    | ExecOutcome::FunctionReturn(_) => {
                        shell.set_last_status(0)
                    }
                }
            }
            ReadResult::Interrupted => continue,
            ReadResult::Eof => {
                crate::traps::fire_exit_trap(&mut shell);
                shell.hangup_jobs();
                shell.history.save();
                return shell.last_status();
            }
            ReadResult::EofMidCommand => {
                eprintln!("huck: syntax error: unexpected end of input");
                crate::traps::fire_exit_trap(&mut shell);
                shell.hangup_jobs();
                shell.history.save();
                return 2;
            }
            ReadResult::ReadError(msg) => {
                eprintln!("huck: input error: {msg}");
                crate::traps::fire_exit_trap(&mut shell);
                shell.hangup_jobs();
                return 1;
            }
        }
    }
}

/// Reads one logical command, gathering continuation lines until the
/// accumulated buffer classifies as `Complete` or a genuine `Error`.
fn read_logical_command(
    editor: &mut Editor<HuckHelper, FileHistory>,
    shell: &mut Shell,
) -> ReadResult {
    use crate::continuation::{classify, joiner_for, Completeness};

    let mut buffer = String::new();
    let mut history = String::new();
    // The reason the buffer-so-far is incomplete, and the line that
    // caused it — together they pick the joiner for the next line.
    let mut pending: Option<(crate::continuation::ContinuationReason, String)> = None;

    loop {
        let (var_name, default) = if pending.is_none() {
            ("PS1", DEFAULT_PS1)
        } else {
            ("PS2", DEFAULT_PS2)
        };
        let template = shell
            .lookup_var(var_name)
            .unwrap_or_else(|| default.to_string());
        let expanded = crate::prompt::expand_prompt(&template, shell);
        match editor.readline(&expanded) {
            Ok(raw) => {
                // History expansion runs per physical line, as before.
                let line = match crate::history::expand(&raw, &shell.history) {
                    Ok(None) => raw,
                    Ok(Some(expanded)) => {
                        println!("{expanded}");
                        expanded
                    }
                    Err(e) => {
                        eprintln!("huck: {e}");
                        shell.set_last_status(1);
                        return ReadResult::Interrupted;
                    }
                };

                match pending.take() {
                    None => {
                        // First physical line.
                        buffer.push_str(&line);
                        history.push_str(&line);
                    }
                    Some((reason, prev_line)) => {
                        // `buffer` joins with a real newline, except a
                        // backslash continuation which joins with nothing.
                        if reason != crate::continuation::ContinuationReason::Backslash {
                            buffer.push('\n');
                        }
                        buffer.push_str(&line);
                        history.push_str(joiner_for(reason, &prev_line));
                        history.push_str(&line);
                    }
                }

                match classify(&buffer) {
                    Completeness::Complete | Completeness::Error => {
                        return ReadResult::Ready { buffer, history };
                    }
                    Completeness::Incomplete(reason) => {
                        if reason == crate::continuation::ContinuationReason::Backslash {
                            // Drop the unescaped trailing backslash from
                            // both accumulators before the next line.
                            buffer.pop();
                            history.pop();
                        }
                        pending = Some((reason, line));
                    }
                }
            }
            Err(ReadlineError::Interrupted) => return ReadResult::Interrupted,
            Err(ReadlineError::Eof) => {
                return if buffer.is_empty() {
                    ReadResult::Eof
                } else {
                    ReadResult::EofMidCommand
                };
            }
            Err(e) => return ReadResult::ReadError(e.to_string()),
        }
    }
}

/// Installs a SIGINT handler that sets the supplied flag. Called once at
/// startup after `Shell::new()`; the flag lives on the `Shell` so the wait
/// builtin can poll it to break out of its loop when Ctrl-C is pressed.
fn install_sigint_handler(flag: Arc<AtomicBool>) {
    if let Err(e) = signal_hook::flag::register(SIGINT, flag) {
        eprintln!("huck: warning: could not install SIGINT handler: {e}");
    }
}

/// Installs a SIGCHLD handler that toggles the supplied flag. Called once
/// at startup; the flag lives on the `Shell` so the reap path can poll it.
fn install_sigchld_handler(flag: Arc<AtomicBool>) {
    if let Err(e) = signal_hook::flag::register(SIGCHLD, flag) {
        eprintln!("huck: warning: could not install SIGCHLD handler: {e}");
    }
}

/// Ignore SIGTSTP/SIGTTIN/SIGTTOU at the shell level so that:
///   - Ctrl-Z at the prompt does not suspend huck itself.
///   - `tcsetpgrp` from a non-foreground pgrp does not trigger SIGTTOU on us.
///   - Defensive: huck never reads `/dev/tty` directly today, but match bash.
///
/// NOTE: `SIG_IGN` is inherited across `execve`. Foreground children
/// spawned by the executor (Task 5) MUST reset these three signals to
/// `SIG_DFL` via a `CommandExt::pre_exec` hook — otherwise Ctrl-Z would
/// not stop `vim`/`less`/etc., and a backgrounded reader would never
/// get SIGTTIN.
fn install_job_control_signals() {
    for sig in [libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU] {
        let prev = unsafe { libc::signal(sig, libc::SIG_IGN) };
        if prev == libc::SIG_ERR {
            eprintln!("huck: warning: could not ignore signal {sig}");
        }
    }
}

/// Fires `$PROMPT_COMMAND` if set, non-empty, and the shell is
/// interactive. Returns `Some(exit_code)` when PROMPT_COMMAND
/// returns `ExecOutcome::Exit` (e.g. `PROMPT_COMMAND='exit 7'`) —
/// the outer REPL handles the shell-exit cleanup. Returns `None`
/// otherwise; on `Continue`, updates `shell.last_status` so PS1's
/// `\?` and the next user command's `$?` both reflect
/// PROMPT_COMMAND's exit code (matches bash). Non-interactive
/// shells skip entirely.
pub fn fire_prompt_command(shell: &mut Shell) -> Option<i32> {
    if !shell.is_interactive {
        return None;
    }
    let pc = match shell.lookup_var("PROMPT_COMMAND") {
        Some(s) if !s.is_empty() => s,
        _ => return None,
    };
    match process_line(&pc, shell, true) {
        ExecOutcome::Exit(code) => Some(code),
        ExecOutcome::Continue(status) => {
            shell.set_last_status(status);
            None
        }
        _ => None,
    }
}

/// Tokenizes, parses, and executes a single input line.
pub fn process_line(line: &str, shell: &mut Shell, expand_aliases: bool) -> ExecOutcome {
    let tokens = match lexer::tokenize(line) {
        Ok(tokens) => tokens,
        Err(e) => {
            eprintln!("huck: syntax error{}", lex_error_message(e));
            return ExecOutcome::Continue(2);
        }
    };
    let tokens = if expand_aliases {
        match crate::alias_expand::expand_aliases_in_tokens(tokens, &shell.aliases) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("huck: syntax error{}", lex_error_message(e));
                return ExecOutcome::Continue(2);
            }
        }
    } else {
        tokens
    };

    match command::parse(tokens) {
        Ok(Some(sequence)) => executor::execute(&sequence, shell, line),
        Ok(None) => ExecOutcome::Continue(0),
        Err(e) => {
            eprintln!("huck: syntax error: {}", parse_error_message(e));
            ExecOutcome::Continue(2)
        }
    }
}

pub(crate) fn parse_error_message(error: ParseError) -> String {
    match error {
        ParseError::MissingCommand => "expected a command".to_string(),
        ParseError::MissingRedirectTarget => "expected a filename after redirection".to_string(),
        ParseError::RedirectTargetIsOperator => "expected a filename after redirection".to_string(),
        ParseError::UnexpectedBackground => "'&' not allowed here".to_string(),
        ParseError::UnterminatedIf => "unterminated 'if' (expected 'then'/'fi')".to_string(),
        ParseError::UnexpectedKeyword(kw) => format!("unexpected '{kw}'"),
        ParseError::UnterminatedLoop => "unterminated loop (expected 'do'/'done')".to_string(),
        ParseError::UnexpectedToken => "unexpected token after command".to_string(),
        ParseError::ForVariable => "invalid variable name in 'for' loop".to_string(),
        ParseError::UnterminatedCase => "unterminated 'case' (expected 'esac')".to_string(),
        ParseError::UnterminatedBrace => "unterminated '{' (expected '}')".to_string(),
        ParseError::FunctionName => "invalid function name".to_string(),
        ParseError::FunctionBody => {
            "function definition: expected '()' and a compound-command body \
             (`if`/`while`/`for`/`case`/`{ … }`)".to_string()
        }
        ParseError::UnterminatedFunction => {
            "unterminated function definition (expected a compound-command body)".to_string()
        }
        ParseError::EmptySubshell => "empty subshell '()' is not allowed".to_string(),
        ParseError::UnterminatedSubshell => {
            "unterminated '(' (expected matching ')')".to_string()
        }
        ParseError::EmptyDoubleBracket => {
            "'[[ ]]' with empty body is not allowed".to_string()
        }
        ParseError::UnterminatedDoubleBracket => {
            "unterminated '[[ ]]' (missing ']]')".to_string()
        }
        ParseError::TestExprBadOperator(op) => {
            format!("unrecognised operator in '[[ ]]': '{op}'")
        }
        ParseError::TestExprMissingOperand => {
            "missing operand in '[[ ]]'".to_string()
        }
    }
}

/// Renders a `LexError` into a message that includes its own leading
/// separator. Most variants start with `": "` so the caller's
/// `"huck: syntax error"` prefix reads naturally. Substitution-wrapper
/// variants start with `" in command substitution"` (no colon) so the
/// rendered line reads `"huck: syntax error in command substitution: ..."`.
pub(crate) fn lex_error_message(error: LexError) -> String {
    match error {
        LexError::UnterminatedQuote => ": unterminated quote".to_string(),
        LexError::InvalidVarName => ": invalid variable name in '${...}'".to_string(),
        LexError::UnterminatedBrace => ": unterminated '${...}'".to_string(),
        LexError::UnterminatedSubstitution => ": unterminated command substitution".to_string(),
        LexError::UnterminatedArith => ": unterminated arithmetic expansion".to_string(),
        LexError::ArithParse(msg) => format!(": arithmetic expansion: {msg}"),
        LexError::InvalidBraceModifier(c) => format!(": invalid parameter-expansion modifier: {c}"),
        LexError::EmptyParamName => ": parameter expansion with empty name".to_string(),
        LexError::InvalidBraceOperand => ": invalid operator in parameter-expansion operand".to_string(),
        LexError::Substitution(inner) => {
            format!(" in command substitution{}", lex_error_message(*inner))
        }
        LexError::SubstitutionParseError(inner) => {
            format!(" in command substitution: {}", parse_error_message(inner))
        }
        LexError::UnterminatedHeredoc => ": unterminated here-document".to_string(),
        LexError::AnsiCInvalidCodepoint(v) => {
            format!(": invalid Unicode codepoint in $'...' escape: U+{:04X}", v)
        }
        LexError::BraceExpansionLimit => ": brace expansion: too many elements".to_string(),
    }
}

#[cfg(test)]
mod prompt_command_tests {
    use super::*;
    use crate::shell_state::Shell;

    fn interactive_shell() -> Shell {
        let mut shell = Shell::new();
        shell.is_interactive = true;
        shell
    }

    #[test]
    fn fires_when_set() {
        let mut shell = interactive_shell();
        shell.set("PROMPT_COMMAND", "true".to_string());
        assert_eq!(fire_prompt_command(&mut shell), None);
        assert_eq!(shell.last_status(), 0);
    }

    #[test]
    fn last_status_reflects_pc() {
        let mut shell = interactive_shell();
        shell.set("PROMPT_COMMAND", "false".to_string());
        assert_eq!(fire_prompt_command(&mut shell), None);
        assert_eq!(shell.last_status(), 1);
    }

    #[test]
    fn no_op_when_unset() {
        let mut shell = interactive_shell();
        shell.set_last_status(42);
        assert_eq!(fire_prompt_command(&mut shell), None);
        assert_eq!(shell.last_status(), 42);
    }

    #[test]
    fn no_op_when_empty() {
        let mut shell = interactive_shell();
        shell.set("PROMPT_COMMAND", String::new());
        shell.set_last_status(42);
        assert_eq!(fire_prompt_command(&mut shell), None);
        assert_eq!(shell.last_status(), 42);
    }

    #[test]
    fn propagates_exit() {
        let mut shell = interactive_shell();
        shell.set("PROMPT_COMMAND", "exit 7".to_string());
        assert_eq!(fire_prompt_command(&mut shell), Some(7));
    }

    #[test]
    fn silent_when_non_interactive() {
        let mut shell = Shell::new();
        shell.is_interactive = false;
        shell.set("PROMPT_COMMAND", "false".to_string());
        shell.set_last_status(42);
        assert_eq!(fire_prompt_command(&mut shell), None);
        // last_status unchanged since PC didn't run.
        assert_eq!(shell.last_status(), 42);
    }
}
