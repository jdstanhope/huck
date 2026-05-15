use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use signal_hook::consts::SIGINT;

use crate::builtins::ExecOutcome;
use crate::command::{self, ParseError};
use crate::executor;
use crate::lexer::{self, LexError};

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

    // Tracks the exit status of the last command, so Ctrl-D (EOF) exits with
    // it — standard shell behavior, and the consumer of `ExecOutcome::Continue`'s
    // status. Room to grow into `$?` later.
    let mut last_status: i32 = 0;

    loop {
        match editor.readline(PROMPT) {
            Ok(line) => {
                if !line.trim().is_empty() {
                    let _ = editor.add_history_entry(line.as_str());
                }
                match process_line(&line) {
                    ExecOutcome::Exit(code) => return code,
                    ExecOutcome::Continue(status) => last_status = status,
                }
            }
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => return last_status,
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

/// Tokenizes, parses, and executes a single input line.
fn process_line(line: &str) -> ExecOutcome {
    let tokens = match lexer::tokenize(line) {
        Ok(tokens) => tokens,
        Err(e) => {
            eprintln!("shuck: syntax error: {}", lex_error_message(e));
            return ExecOutcome::Continue(2);
        }
    };

    match command::parse(tokens) {
        Ok(Some(pipeline)) => executor::execute(&pipeline),
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
    }
}

fn lex_error_message(error: LexError) -> &'static str {
    match error {
        LexError::UnterminatedQuote => "unterminated quote",
        LexError::BareAmpersand => "unexpected '&'",
    }
}
