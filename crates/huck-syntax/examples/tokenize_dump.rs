//! Tokenize shell source from stdin and pretty-print each token.
//!
//! Demonstrates the lexer side of `huck-syntax`:
//!   `tokenize_with_opts(input, LexerOptions { extglob: true, ... })`
//!     → `Result<Vec<Token>, LexError>`
//!
//! Run:
//!   echo 'name=alice; echo "hi ${name@U}"' | cargo run -q --example tokenize_dump -p huck-syntax
//!
//! Pipe a script:
//!   cat tests/scripts/some_script.sh | cargo run -q --example tokenize_dump -p huck-syntax

use std::io::Read;

use huck_syntax::lexer::{tokenize_with_opts, LexerOptions, Token, Word, WordPart};

fn main() {
    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("read error: {e}");
        std::process::exit(1);
    }

    let opts = LexerOptions { extglob: true };

    match tokenize_with_opts(&input, opts) {
        Ok(tokens) => print_tokens(&tokens),
        Err(e) => {
            let kind_dbg = format!("{e:?}");
            let msg = huck_syntax::lex_error_message(&e);
            eprintln!("lex error: {msg}");
            eprintln!("  kind: {kind_dbg}");
            std::process::exit(1);
        }
    }
}

fn print_tokens(tokens: &[Token]) {
    for (i, t) in tokens.iter().enumerate() {
        println!("[{i:3}] {}", token_label(t));
    }
}

fn token_label(t: &Token) -> String {
    match t {
        Token::Word(w) => format!("Word            {}", describe_word(w)),
        Token::Op(op) => format!("Op              {op:?}"),
        Token::Heredoc { expand, strip_tabs, body } => {
            // The lexer collapses `<<DELIM` + body lines into one Heredoc
            // token at the `<<` position. The delim itself is not emitted.
            format!(
                "Heredoc         expand={expand} strip_tabs={strip_tabs} body={}",
                describe_word(body),
            )
        }
        Token::Newline => "Newline".to_string(),
        Token::ArithBlock(body, _opts) => format!("ArithBlock      ((  {body}  ))"),
        Token::RedirFd(fd) => format!("RedirFd         {fd:?}"),
    }
}

/// Short, readable description of a Word's parts. We don't recurse fully
/// into `CommandSub`/`ProcessSub`/`Arith` — those carry a Sequence and
/// would balloon the output.
fn describe_word(Word(parts): &Word) -> String {
    if parts.len() == 1 {
        return part_label(&parts[0]);
    }
    let parts: Vec<String> = parts.iter().map(part_label).collect();
    format!("[{}]", parts.join(" + "))
}

fn part_label(p: &WordPart) -> String {
    match p {
        WordPart::Literal { text, quoted } => {
            let q = if *quoted { "Q" } else { "U" };
            format!("Lit({q},{text:?})")
        }
        WordPart::Tilde(_) => "Tilde(...)".to_string(),
        WordPart::Var { name, quoted } => {
            let q = if *quoted { "Q" } else { "U" };
            format!("Var({q},${name})")
        }
        WordPart::LastStatus { quoted } => {
            let q = if *quoted { "Q" } else { "U" };
            format!("LastStatus({q})")
        }
        WordPart::CommandSub { .. } => "CmdSub(...)".to_string(),
        WordPart::ProcessSub { dir, .. } => format!("ProcSub({dir:?})"),
        WordPart::Arith { .. } => "Arith(...)".to_string(),
        WordPart::ParamExpansion {
            name,
            modifier,
            subscript,
            indirect,
            quoted,
        } => {
            let q = if *quoted { "Q" } else { "U" };
            let ind = if *indirect { "!" } else { "" };
            let sub = match subscript {
                Some(s) => format!("[{s:?}]"),
                None => String::new(),
            };
            format!("Param({q},${ind}{name}{sub} {modifier:?})")
        }
        WordPart::AllArgs { quoted, joined } => {
            let sigil = if *joined { "$*" } else { "$@" };
            let q = if *quoted { "Q" } else { "U" };
            format!("AllArgs({q},{sigil})")
        }
        WordPart::AssignPrefix { target, append } => {
            let op = if *append { "+=" } else { "=" };
            format!("AssignPrefix({target:?}{op})")
        }
        WordPart::ArrayLiteral(elems) => {
            format!("ArrayLiteral({} elements)", elems.len())
        }
    }
}

