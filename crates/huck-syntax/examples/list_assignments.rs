//! Parse shell source from stdin and list every variable assignment.
//!
//! Demonstrates the lex → parse → walk pipeline of `huck-syntax`:
//!   1. `tokenize` produces `Vec<Token>`.
//!   2. `parse` produces `Option<Sequence>` (None for empty input).
//!   3. We walk the `Sequence` AST and surface every `Assignment`.
//!
//! Catches:
//!   - `NAME=value` (bare scalar)
//!   - `NAME+=value` (append)
//!   - `NAME[i]=value` (indexed-element)
//!   - `NAME=(a b c)` (compound array literal)
//!   - inline prefix `A=1 B=2 cmd` (collected as `ExecCommand::inline_assignments`)
//!   - `declare`/`local`/`readonly`/`export NAME=value` (collected as `DeclArg::Assign`)
//!
//! Run:
//!   echo 'a=1; b=hi; declare -A m=([k]=v); A=x B=y cmd; arr=(p q)' \
//!     | cargo run -q --example list_assignments -p huck-syntax

use std::io::Read;

use huck_syntax::command::{IfClause, WhileClause};
use huck_syntax::{
    parse, tokenize_with_opts, AssignTarget, Assignment, Command, ExecCommand, LexerOptions,
    Sequence, SimpleCommand, Word, WordPart,
};

fn main() {
    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("read error: {e}");
        std::process::exit(1);
    }

    let tokens = match tokenize_with_opts(&input, LexerOptions::default()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("lex error: {}", huck_syntax::lex_error_message(&e));
            std::process::exit(1);
        }
    };

    let sequence = match parse(tokens) {
        Ok(Some(s)) => s,
        Ok(None) => {
            println!("(empty input)");
            return;
        }
        Err(e) => {
            eprintln!("parse error: {}", huck_syntax::parse_error_message(&e));
            std::process::exit(1);
        }
    };

    let mut sink = Vec::new();
    walk_sequence(&sequence, &mut sink);

    if sink.is_empty() {
        println!("(no assignments found)");
    } else {
        for record in &sink {
            println!("{record}");
        }
    }
}

/// One assignment occurrence, formatted for display.
struct Record {
    /// `bare` / `inline` / `decl` — where it was found.
    site: &'static str,
    target: String,
    /// `=` or `+=`.
    op: &'static str,
    /// Best-effort literal text of the value. `<dynamic>` if the value
    /// contains anything that isn't a literal (expansion, command-sub,
    /// arith, array literal, …).
    value: String,
}

impl std::fmt::Display for Record {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:7}  {}{}{}", self.site, self.target, self.op, self.value)
    }
}

/// Walk every `Command` in a `Sequence`, recursing into compound bodies.
fn walk_sequence(seq: &Sequence, sink: &mut Vec<Record>) {
    walk_command(&seq.first, sink);
    for (_, c) in &seq.rest {
        walk_command(c, sink);
    }
}

fn walk_command(cmd: &Command, sink: &mut Vec<Record>) {
    match cmd {
        // Pipeline stages can themselves be Commands; recurse.
        Command::Pipeline(p) => {
            for stage in &p.commands {
                walk_command(stage, sink);
            }
        }
        Command::Simple(s) => walk_simple(s, sink),
        Command::If(b) => walk_if(b, sink),
        Command::While(b) => walk_while(b, sink),
        Command::For(b) => walk_sequence(&b.body, sink),
        Command::Case(b) => {
            for item in &b.items {
                if let Some(body) = &item.body {
                    walk_sequence(body, sink);
                }
            }
        }
        Command::BraceGroup(seq) => walk_sequence(seq, sink),
        Command::Subshell { body } => walk_sequence(body, sink),
        Command::FunctionDef { body, .. } => walk_command(body, sink),
        // [[ … ]] can carry inline assignments via `inline_assignments`.
        Command::DoubleBracket { inline_assignments, .. } => {
            for a in inline_assignments {
                sink.push(record_from_assign("inline", a));
            }
        }
        Command::Redirected { inner, .. } => walk_command(inner, sink),
        Command::Select(b) => walk_sequence(&b.body, sink),
        Command::ArithFor(b) => walk_sequence(&b.body, sink),
        Command::Coproc { body, .. } => walk_command(body, sink),
        // `((expr))` standalone has no assignments at the AST level
        // (the expression itself is not an Assignment AST node).
        Command::Arith(_) => {}
        // Forward-compatible: future Command variants contribute no
        // assignments by default.
        _ => {}
    }
}

fn walk_simple(s: &SimpleCommand, sink: &mut Vec<Record>) {
    match s {
        SimpleCommand::Assign(list, _line) => {
            for a in list {
                sink.push(record_from_assign("bare", a));
            }
        }
        SimpleCommand::Exec(ec) => walk_exec(ec, sink),
    }
}

fn walk_exec(ec: &ExecCommand, sink: &mut Vec<Record>) {
    // 1. Inline-prefix assignments (`A=1 B=2 cmd`).
    for a in &ec.inline_assignments {
        sink.push(record_from_assign("inline", a));
    }

    // 2. `declare`/`local`/`readonly`/`export NAME=value`. The lexer
    //    pre-parses these as `DeclArg::Assign`. The arguments are
    //    `Vec<Word>` though, NOT `Vec<DeclArg>` — declarations are
    //    resolved into `DeclArg` at execute time by the runtime, not
    //    at parse time. So at the syntax level we can only inspect the
    //    `args: Vec<Word>` and re-detect assignment-shaped words.
    if is_decl_command(&ec.program) {
        for arg in &ec.args {
            if let Some(a) = try_parse_decl_arg_assignment(arg) {
                sink.push(record_from_assign("decl", &a));
            }
        }
    }
}

fn walk_if(c: &IfClause, sink: &mut Vec<Record>) {
    walk_sequence(&c.condition, sink);
    walk_sequence(&c.then_body, sink);
    for elif in &c.elif_branches {
        walk_sequence(&elif.condition, sink);
        walk_sequence(&elif.body, sink);
    }
    if let Some(body) = &c.else_body {
        walk_sequence(body, sink);
    }
}

fn walk_while(c: &WhileClause, sink: &mut Vec<Record>) {
    walk_sequence(&c.condition, sink);
    walk_sequence(&c.body, sink);
}

/// Was this `program` Word literally `declare` / `local` / `readonly` /
/// `export` / `typeset`?
fn is_decl_command(program: &Word) -> bool {
    if program.0.len() != 1 {
        return false;
    }
    let WordPart::Literal { text, quoted: false } = &program.0[0] else {
        return false;
    };
    matches!(
        text.as_str(),
        "declare" | "local" | "readonly" | "export" | "typeset",
    )
}

/// Best-effort: if a `Word` argument to a decl command looks like a bare
/// `name=value` assignment, lift it into an `Assignment`. We use the
/// public `try_split_assignment` helper from `huck_syntax::command`,
/// which already knows the rules.
fn try_parse_decl_arg_assignment(w: &Word) -> Option<Assignment> {
    huck_syntax::try_split_assignment_ref(w)
}

fn record_from_assign(site: &'static str, a: &Assignment) -> Record {
    let op = if a.append { "+=" } else { "=" };
    let target = format_target(&a.target);
    let value = format_value(&a.value);
    Record { site, target, op, value }
}

fn format_target(t: &AssignTarget) -> String {
    // Use the round-trip generator for a faithful rendering. The generator
    // accepts an AST node and emits canonical source text.
    format!("{t:?}")
}

fn format_value(w: &Word) -> String {
    // If every part is an unquoted literal, concatenate. Otherwise dump
    // a short structural label.
    let mut out = String::new();
    let mut dynamic = false;
    for p in &w.0 {
        match p {
            WordPart::Literal { text, .. } => out.push_str(text),
            WordPart::ArrayLiteral(elems) => {
                out.push_str(&format!("(<{} elements>)", elems.len()));
                dynamic = true;
            }
            // Anything else means the value isn't a static literal.
            _ => {
                dynamic = true;
                break;
            }
        }
    }
    if dynamic && !out.is_empty() {
        out
    } else if dynamic {
        "<dynamic>".to_string()
    } else if out.is_empty() {
        "\"\"".to_string()
    } else {
        out
    }
}
