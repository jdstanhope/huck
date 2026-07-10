//! Lex a shell string and print the token stream.
//!
//! Run: `cargo run --example tokenize_dump -p huck-syntax -- 'echo hi | wc -l'`
//! (falls back to a built-in sample if no argument is given).

use huck_syntax::{Lexer, LexerOptions};

fn main() {
    let src = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "echo hello | wc -l".to_string());
    println!("source: {src:?}\n");

    let mut lx = Lexer::new(&src, &Default::default(), LexerOptions::default());
    loop {
        match lx.next() {
            Ok(Some(tok)) => println!("{:>4}:{:<3} {:?}", tok.span.line, tok.span.column, tok.kind),
            Ok(None) => break,
            Err(e) => {
                eprintln!("lex error: {e}");
                std::process::exit(1);
            }
        }
    }
}
