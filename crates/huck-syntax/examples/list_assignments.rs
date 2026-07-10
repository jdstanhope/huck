//! Parse a shell string and print every assignment in the first command.
//!
//! Run: `cargo run --example list_assignments -p huck-syntax -- 'a=1 b+=2 echo hi'`
//! (falls back to a built-in sample if no argument is given).

use huck_syntax::command::{Command, SimpleCommand};
use huck_syntax::{Assignment, parse};

fn main() {
    let src = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "a=1 b+=2 echo hi".to_string());
    println!("source: {src:?}\n");

    let seq = match parse(&src) {
        Ok(Some(seq)) => seq,
        Ok(None) => {
            println!("(no command — empty or comment-only input)");
            return;
        }
        Err(e) => {
            eprintln!("parse error: {e}");
            std::process::exit(1);
        }
    };

    let assigns = collect_assignments(&seq.first);
    if assigns.is_empty() {
        println!("(no assignments found)");
    }
    for a in assigns {
        let op = if a.append { "+=" } else { "=" };
        println!("{}{}{}", a.target.name(), op, "…");
    }
}

/// Pull the assignments off the first simple command (inline `a=1 cmd` prefix
/// assignments and bare `a=1` assignment-only commands).
///
/// A lone simple command is always wrapped as a single-stage `Pipeline` by
/// the parser (only an actual `a | b` gets more than one stage), so unwrap
/// that one level before matching on `Command::Simple`.
fn collect_assignments(cmd: &Command) -> Vec<&Assignment> {
    match cmd {
        Command::Simple(SimpleCommand::Exec(exec)) => exec.inline_assignments.iter().collect(),
        Command::Simple(SimpleCommand::Assign(list, _line)) => list.iter().collect(),
        Command::Pipeline(p) if p.commands.len() == 1 => collect_assignments(&p.commands[0]),
        _ => Vec::new(),
    }
}
