use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use libc;
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

const PROMPT: &str = "huck> ";

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
        if let Some(helper) = editor.helper_mut() {
            helper.refresh(&shell);
        }
        match editor.readline(PROMPT) {
            Ok(line) => {
                let to_run = match crate::history::expand(&line, &shell.history) {
                    Ok(None) => line.clone(),
                    Ok(Some(expanded)) => {
                        println!("{expanded}");
                        expanded
                    }
                    Err(e) => {
                        eprintln!("huck: {e}");
                        shell.set_last_status(1);
                        continue;
                    }
                };
                if !to_run.trim().is_empty() {
                    shell.history.add(to_run.clone());
                    let _ = editor.add_history_entry(to_run.as_str());
                }
                match process_line(&to_run, &mut shell) {
                    ExecOutcome::Exit(code) => {
                        shell.history.save();
                        return code;
                    }
                    ExecOutcome::Continue(status) => shell.set_last_status(status),
                }
            }
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => {
                shell.history.save();
                return shell.last_status();
            }
            Err(e) => {
                eprintln!("huck: input error: {e}");
                return 1;
            }
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

/// Tokenizes, parses, and executes a single input line.
fn process_line(line: &str, shell: &mut Shell) -> ExecOutcome {
    let tokens = match lexer::tokenize(line) {
        Ok(tokens) => tokens,
        Err(e) => {
            eprintln!("huck: syntax error{}", lex_error_message(e));
            return ExecOutcome::Continue(2);
        }
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

fn parse_error_message(error: ParseError) -> &'static str {
    match error {
        ParseError::MissingCommand => "expected a command",
        ParseError::MissingRedirectTarget => "expected a filename after redirection",
        ParseError::RedirectTargetIsOperator => "expected a filename after redirection",
        ParseError::UnexpectedBackground => "'&' not allowed here",
        ParseError::BackgroundedMultiPipelineSequence => {
            "'&' on multi-command sequence not supported; use a single pipeline"
        }
    }
}

/// Renders a `LexError` into a message that includes its own leading
/// separator. Most variants start with `": "` so the caller's
/// `"huck: syntax error"` prefix reads naturally. Substitution-wrapper
/// variants start with `" in command substitution"` (no colon) so the
/// rendered line reads `"huck: syntax error in command substitution: ..."`.
fn lex_error_message(error: LexError) -> String {
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
        LexError::SubstitutionLexError(inner) => {
            format!(" in command substitution{}", lex_error_message(*inner))
        }
        LexError::SubstitutionParseError(inner) => {
            format!(" in command substitution: {}", parse_error_message(inner))
        }
    }
}
