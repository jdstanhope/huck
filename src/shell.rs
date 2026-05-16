use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use signal_hook::consts::{SIGCHLD, SIGINT};

use crate::builtins::ExecOutcome;
use crate::command::{self, ParseError};
use crate::executor;
use crate::lexer::{self, LexError};
use crate::shell_state::Shell;

const PROMPT: &str = "shuck> ";

/// Runs the interactive shell loop. Returns the process exit code.
pub fn run() -> i32 {
    install_sigint_handler();

    let mut editor = match DefaultEditor::new() {
        Ok(editor) => editor,
        Err(e) => {
            eprintln!("shuck: failed to initialize line editor: {e}");
            return 1;
        }
    };

    let mut shell = Shell::new();
    install_sigchld_handler(Arc::clone(&shell.sigchld_flag));

    loop {
        crate::jobs::reap_and_notify(&mut shell);
        match editor.readline(PROMPT) {
            Ok(line) => {
                if !line.trim().is_empty() {
                    let _ = editor.add_history_entry(line.as_str());
                }
                match process_line(&line, &mut shell) {
                    ExecOutcome::Exit(code) => return code,
                    ExecOutcome::Continue(status) => shell.set_last_status(status),
                }
            }
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => return shell.last_status(),
            Err(e) => {
                eprintln!("shuck: input error: {e}");
                return 1;
            }
        }
    }
}

/// Installs a SIGINT handler so the shell survives Ctrl-C while a child
/// process runs. The handler is a real handler (not SIG_IGN), so a spawned
/// child resets SIGINT to its default disposition on exec and is terminated
/// normally. The flag itself is not read; registering the handler is the
/// whole point.
fn install_sigint_handler() {
    let flag = Arc::new(AtomicBool::new(false));
    if let Err(e) = signal_hook::flag::register(SIGINT, flag) {
        eprintln!("shuck: warning: could not install SIGINT handler: {e}");
    }
}

/// Installs a SIGCHLD handler that toggles the supplied flag. Called once
/// at startup; the flag lives on the `Shell` so the reap path can poll it.
fn install_sigchld_handler(flag: Arc<AtomicBool>) {
    if let Err(e) = signal_hook::flag::register(SIGCHLD, flag) {
        eprintln!("shuck: warning: could not install SIGCHLD handler: {e}");
    }
}

/// Tokenizes, parses, and executes a single input line.
fn process_line(line: &str, shell: &mut Shell) -> ExecOutcome {
    let tokens = match lexer::tokenize(line) {
        Ok(tokens) => tokens,
        Err(e) => {
            eprintln!("shuck: syntax error{}", lex_error_message(e));
            return ExecOutcome::Continue(2);
        }
    };

    match command::parse(tokens) {
        Ok(Some(sequence)) => executor::execute(&sequence, shell, line),
        Ok(None) => ExecOutcome::Continue(0),
        Err(e) => {
            eprintln!("shuck: syntax error: {}", parse_error_message(e));
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
/// `"shuck: syntax error"` prefix reads naturally. Substitution-wrapper
/// variants start with `" in command substitution"` (no colon) so the
/// rendered line reads `"shuck: syntax error in command substitution: ..."`.
fn lex_error_message(error: LexError) -> String {
    match error {
        LexError::UnterminatedQuote => ": unterminated quote".to_string(),
        LexError::InvalidVarName => ": invalid variable name in '${...}'".to_string(),
        LexError::UnterminatedBrace => ": unterminated '${...}'".to_string(),
        LexError::UnterminatedSubstitution => ": unterminated command substitution".to_string(),
        LexError::SubstitutionLexError(inner) => {
            format!(" in command substitution{}", lex_error_message(*inner))
        }
        LexError::SubstitutionParseError(inner) => {
            format!(" in command substitution: {}", parse_error_message(inner))
        }
    }
}
