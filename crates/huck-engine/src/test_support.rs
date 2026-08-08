//! Shared test-only synchronization (the cwd-changing tests must not race).
use std::sync::Mutex;
pub static CWD_LOCK: Mutex<()> = Mutex::new(());

/// Gate for tests that swap process-global fd 0 via `stdin_pipe::with_stdin_fd0`.
/// Tests using fd 0 redirection in parallel would clobber each other.
pub static STDIN_LOCK: Mutex<()> = Mutex::new(());

/// Lex `echo <input>` and return the first argument `Word`.
///
/// Avoids constructing `WordPart::ParamExpansion` literals by hand and keeps
/// tests aligned with what the lexer actually produces (matters for the
/// lexer-touching `${!a[@]}` shape). Was copied byte-identically into three
/// `expand/*_tests.rs` modules before it landed here.
#[cfg(test)]
pub fn first_arg_word(input: &str) -> crate::lexer::Word {
    use crate::command::{Command, SimpleCommand};
    let src = format!("echo {input}");
    let seq = crate::parser::parse_sequence(&mut crate::lexer::Lexer::new(
        &src,
        &Default::default(),
        crate::lexer::LexerOptions::default(),
    ))
    .expect("parse")
    .expect("non-empty");
    let pipeline = match seq.first {
        Command::Pipeline(p) => p,
        other => panic!("expected Pipeline, got {other:?}"),
    };
    match &pipeline.commands[0] {
        Command::Simple(SimpleCommand::Exec(e)) => e.args[0].clone(),
        other => panic!("expected SimpleCommand::Exec, got {other:?}"),
    }
}
