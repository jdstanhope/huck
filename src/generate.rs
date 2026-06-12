//! Render a parsed `Command` AST back to normalized, re-parseable shell source.
//! Output is a single consistent style (NOT byte-identical to bash); correctness
//! is round-trip idempotence (see tests). Built across v146 Tasks 1-3.
#![allow(dead_code)] // entry points wired in Task 4; some helpers land in Tasks 2-3
use crate::command::{
    Assignment, CaseClause, CaseItem, CaseTerminator, Command, Connector, ExecCommand, ForClause,
    IfClause, Pipeline, Redirect, SelectClause, Sequence, SimpleCommand, TestBinaryOp, TestExpr,
    TestUnaryOp, WhileClause,
};
use crate::lexer::{
    CaseDirection, ParamModifier, SubscriptKind, SubstAnchor, TildeSpec, TransformOp, Word,
    WordPart,
};

/// Render a function definition for `declare -f`: `NAME ()\n<body>`.
pub fn function_to_source(name: &str, body: &Command) -> String {
    command_to_source(&Command::FunctionDef { name: name.to_string(), body: Box::new(body.clone()) }, 0)
}

/// Render any command at nesting depth `indent` (4 spaces/level).
pub fn command_to_source(cmd: &Command, indent: usize) -> String {
    match cmd {
        Command::Pipeline(p) => pipeline_to_source(p, indent),
        Command::Simple(s) => simple_to_source(s),
        Command::If(c) => if_to_source(c, indent),
        Command::While(c) => while_to_source(c, indent),
        Command::For(c) => for_to_source(c, indent),
        Command::ArithFor(c) => arith_for_to_source(c, indent),
        Command::Select(c) => select_to_source(c, indent),
        Command::Case(c) => case_to_source(c, indent),
        Command::BraceGroup(seq) => format!(
            "{{\n{}{}}}",
            body_block(seq, indent + 1),
            pad(indent)
        ),
        Command::Subshell { body } => format!(
            "(\n{}{})",
            body_block(body, indent + 1),
            pad(indent)
        ),
        Command::Arith(word) => format!("(({}))", word_to_source(word)),
        Command::DoubleBracket {
            expr,
            inline_assignments,
        } => {
            let mut s = String::new();
            for a in inline_assignments {
                s.push_str(&assignment_to_source(a));
                s.push(' ');
            }
            s.push_str(&format!("[[ {} ]]", testexpr_to_source(expr)));
            s
        }
        Command::FunctionDef { name, body } => {
            format!("{name} ()\n{}", command_to_source(body, indent))
        }
        Command::Redirected {
            inner,
            stdin,
            stdout,
            stderr,
        } => {
            let mut s = command_to_source(inner, indent);
            if let Some(r) = stdin {
                s.push(' ');
                s.push_str(&redirect_to_source(r, RedirDefault::Stdin));
            }
            if let Some(r) = stdout {
                s.push(' ');
                s.push_str(&redirect_to_source(r, RedirDefault::Stdout));
            }
            if let Some(r) = stderr {
                s.push(' ');
                s.push_str(&redirect_to_source(r, RedirDefault::Stderr));
            }
            s
        }
    }
}

/// Render a compound-command body as one indented, `;`-terminated region:
/// `<pad(indent)><sequence>;\n`. The body's own multi-command separators are
/// handled by `sequence_to_source`; this just positions + terminates it.
fn body_block(seq: &Sequence, indent: usize) -> String {
    format!("{}{};\n", pad(indent), sequence_to_source(seq, indent))
}

/// Render a condition/header sequence inline (no indentation, no trailing
/// terminator) for use on a keyword line like `if <cond>; then`.
fn inline_seq(seq: &Sequence) -> String {
    sequence_to_source(seq, 0)
}

fn if_to_source(c: &IfClause, indent: usize) -> String {
    let mut s = format!("if {}; then\n", inline_seq(&c.condition));
    s.push_str(&body_block(&c.then_body, indent + 1));
    for e in &c.elif_branches {
        s.push_str(&pad(indent));
        s.push_str(&format!("elif {}; then\n", inline_seq(&e.condition)));
        s.push_str(&body_block(&e.body, indent + 1));
    }
    if let Some(eb) = &c.else_body {
        s.push_str(&pad(indent));
        s.push_str("else\n");
        s.push_str(&body_block(eb, indent + 1));
    }
    s.push_str(&pad(indent));
    s.push_str("fi");
    s
}

fn while_to_source(c: &WhileClause, indent: usize) -> String {
    let kw = if c.until { "until" } else { "while" };
    let mut s = format!("{kw} {}; do\n", inline_seq(&c.condition));
    s.push_str(&body_block(&c.body, indent + 1));
    s.push_str(&pad(indent));
    s.push_str("done");
    s
}

fn for_to_source(c: &ForClause, indent: usize) -> String {
    let mut header = format!("for {}", c.var);
    if c.has_in {
        header.push_str(" in");
        for w in &c.words {
            header.push(' ');
            header.push_str(&word_to_source(w));
        }
    }
    let mut s = format!("{header}; do\n");
    s.push_str(&body_block(&c.body, indent + 1));
    s.push_str(&pad(indent));
    s.push_str("done");
    s
}

fn arith_for_to_source(c: &crate::command::ArithForClause, indent: usize) -> String {
    let sec = |w: &Option<crate::lexer::Word>| w.as_ref().map(word_to_source).unwrap_or_default();
    let mut s = format!(
        "for (({}; {}; {})); do\n",
        sec(&c.init),
        sec(&c.cond),
        sec(&c.step)
    );
    s.push_str(&body_block(&c.body, indent + 1));
    s.push_str(&pad(indent));
    s.push_str("done");
    s
}

fn select_to_source(c: &SelectClause, indent: usize) -> String {
    let mut header = format!("select {}", c.var);
    if let Some(words) = &c.words {
        header.push_str(" in");
        for w in words {
            header.push(' ');
            header.push_str(&word_to_source(w));
        }
    }
    let mut s = format!("{header}; do\n");
    s.push_str(&body_block(&c.body, indent + 1));
    s.push_str(&pad(indent));
    s.push_str("done");
    s
}

fn case_to_source(c: &CaseClause, indent: usize) -> String {
    let mut s = format!("case {} in\n", word_to_source(&c.subject));
    for item in &c.items {
        s.push_str(&case_item_to_source(item, indent + 1));
    }
    s.push_str(&pad(indent));
    s.push_str("esac");
    s
}

fn case_item_to_source(item: &CaseItem, indent: usize) -> String {
    let patterns = item
        .patterns
        .iter()
        .map(pattern_word_to_source)
        .collect::<Vec<_>>()
        .join(" | ");
    let mut s = format!("{}{patterns})\n", pad(indent));
    if let Some(body) = &item.body {
        s.push_str(&body_block(body, indent + 1));
    }
    let term = match item.terminator {
        CaseTerminator::Break => ";;",
        CaseTerminator::FallThrough => ";&",
        CaseTerminator::ContinueMatch => ";;&",
    };
    s.push_str(&pad(indent));
    s.push_str(term);
    s.push('\n');
    s
}

/// Render a `case` pattern Word. Like `word_to_source`, but unquoted `Literal`
/// parts keep their glob metacharacters (`* ? [ ]`) UNescaped — a case pattern
/// is a glob, so escaping `*` to `\*` would change its meaning AND break the
/// round-trip (re-parse turns `\*` into a quoted literal). Other shell-special
/// characters (whitespace, `)`, `;`, `&`, `|`, …) are still escaped so the
/// pattern re-parses as one word.
fn pattern_word_to_source(w: &Word) -> String {
    w.0.iter()
        .map(|part| match part {
            WordPart::Literal { text, quoted: false } => escape_bareword(text),
            other => part_to_source(other),
        })
        .collect()
}

fn testexpr_to_source(e: &TestExpr) -> String {
    match e {
        TestExpr::Unary { op, operand } => {
            format!("{} {}", unary_op_token(*op), word_to_source(operand))
        }
        TestExpr::Binary { op, lhs, rhs } => format!(
            "{} {} {}",
            word_to_source(lhs),
            binary_op_token(*op),
            word_to_source(rhs)
        ),
        TestExpr::Regex { lhs, pattern } => {
            format!("{} =~ {}", word_to_source(lhs), word_to_source(pattern))
        }
        TestExpr::Not(inner) => format!("! {}", testexpr_to_source(inner)),
        TestExpr::And(a, b) => {
            format!("{} && {}", testexpr_to_source(a), testexpr_to_source(b))
        }
        TestExpr::Or(a, b) => format!("{} || {}", testexpr_to_source(a), testexpr_to_source(b)),
    }
}

/// Invert the `try_unary_op` parse table (src/command.rs).
fn unary_op_token(op: TestUnaryOp) -> &'static str {
    match op {
        TestUnaryOp::FileExists => "-e",
        TestUnaryOp::IsRegFile => "-f",
        TestUnaryOp::IsDir => "-d",
        TestUnaryOp::IsReadable => "-r",
        TestUnaryOp::IsWritable => "-w",
        TestUnaryOp::IsExecutable => "-x",
        TestUnaryOp::IsNonEmpty => "-s",
        TestUnaryOp::IsSymlink => "-L",
        TestUnaryOp::StringNonEmpty => "-n",
        TestUnaryOp::StringEmpty => "-z",
        TestUnaryOp::VarSet => "-v",
        TestUnaryOp::OptEnabled => "-o",
        TestUnaryOp::IsFifo => "-p",
        TestUnaryOp::IsSocket => "-S",
        TestUnaryOp::IsBlockDev => "-b",
        TestUnaryOp::IsCharDev => "-c",
        TestUnaryOp::OwnedByEuid => "-O",
        TestUnaryOp::OwnedByEgid => "-G",
        TestUnaryOp::NewerThanRead => "-N",
        TestUnaryOp::IsSticky => "-k",
        TestUnaryOp::IsSetuid => "-u",
        TestUnaryOp::IsSetgid => "-g",
        TestUnaryOp::IsTerminal => "-t",
    }
}

/// Invert the binary-operator parse table (src/command.rs `parse_test_atom`).
fn binary_op_token(op: TestBinaryOp) -> &'static str {
    match op {
        TestBinaryOp::StringEq => "==",
        TestBinaryOp::StringNe => "!=",
        TestBinaryOp::StringLt => "<",
        TestBinaryOp::StringGt => ">",
        TestBinaryOp::IntEq => "-eq",
        TestBinaryOp::IntNe => "-ne",
        TestBinaryOp::IntLt => "-lt",
        TestBinaryOp::IntGt => "-gt",
        TestBinaryOp::IntLe => "-le",
        TestBinaryOp::IntGe => "-ge",
        TestBinaryOp::NewerThan => "-nt",
        TestBinaryOp::OlderThan => "-ot",
        TestBinaryOp::SameFile => "-ef",
    }
}

fn pipeline_to_source(p: &Pipeline, indent: usize) -> String {
    let mut s = if p.negate {
        "! ".to_string()
    } else {
        String::new()
    };
    let stages: Vec<String> = p
        .commands
        .iter()
        .map(|c| command_to_source(c, indent))
        .collect();
    s.push_str(&stages.join(" | "));
    s
}

fn simple_to_source(s: &SimpleCommand) -> String {
    match s {
        SimpleCommand::Assign(assigns) => assigns
            .iter()
            .map(assignment_to_source)
            .collect::<Vec<_>>()
            .join(" "),
        SimpleCommand::Exec(e) => exec_to_source(e),
    }
}

/// Render a command list: connectors, backgrounding, and the leading command.
fn sequence_to_source(seq: &Sequence, indent: usize) -> String {
    let mut out = command_to_source(&seq.first, indent);
    for (conn, cmd) in &seq.rest {
        match conn {
            Connector::Semi => {
                out.push_str(";\n");
                out.push_str(&pad(indent));
            }
            Connector::And => out.push_str(" && "),
            Connector::Or => out.push_str(" || "),
            Connector::Amp => {
                out.push_str(" &\n");
                out.push_str(&pad(indent));
            }
        }
        out.push_str(&command_to_source(cmd, indent));
    }
    if seq.background {
        out.push_str(" &");
    }
    out
}

fn pad(indent: usize) -> String {
    "    ".repeat(indent)
}

fn exec_to_source(e: &ExecCommand) -> String {
    let mut parts: Vec<String> = Vec::new();
    for a in &e.inline_assignments {
        parts.push(assignment_to_source(a));
    }
    parts.push(word_to_source(&e.program));
    for w in &e.args {
        parts.push(word_to_source(w));
    }
    let mut s = parts.join(" ");
    if let Some(r) = &e.stdin {
        s.push(' ');
        s.push_str(&redirect_to_source(r, RedirDefault::Stdin));
    }
    if let Some(r) = &e.stdout {
        s.push(' ');
        s.push_str(&redirect_to_source(r, RedirDefault::Stdout));
    }
    if let Some(r) = &e.stderr {
        s.push(' ');
        s.push_str(&redirect_to_source(r, RedirDefault::Stderr));
    }
    s
}

fn assignment_to_source(a: &Assignment) -> String {
    format!(
        "{}{}={}",
        assign_target_to_source(&a.target),
        if a.append { "+" } else { "" },
        word_to_source(&a.value)
    )
}

/// Which standard fd a redirect attaches to, so we know whether to emit a
/// leading `2` for stderr. Stdin/stdout use the bare operator.
enum RedirDefault {
    Stdin,
    Stdout,
    Stderr,
}

fn redirect_to_source(r: &Redirect, which: RedirDefault) -> String {
    let fd_prefix = match which {
        RedirDefault::Stderr => "2",
        RedirDefault::Stdin | RedirDefault::Stdout => "",
    };
    match r {
        Redirect::Read(w) => format!("< {}", word_to_source(w)),
        Redirect::Truncate(w) => format!("{fd_prefix}> {}", word_to_source(w)),
        Redirect::Append(w) => format!("{fd_prefix}>> {}", word_to_source(w)),
        Redirect::Clobber(w) => format!("{fd_prefix}>| {}", word_to_source(w)),
        Redirect::Dup { fd, source } => format!("{fd}>&{}", word_to_source(source)),
        Redirect::HereString(w) => format!("<<< {}", word_to_source(w)),
        Redirect::Heredoc {
            body,
            expand,
            strip_tabs,
        } => {
            // DECISION: full heredoc round-trip via a fixed delimiter. The body
            // word is rendered verbatim (tabs already stripped at lex time for
            // `<<-`). Quote the delimiter when !expand so the re-parse keeps the
            // literal (non-expanding) mode. Emit `<<DELIM\n<body>\nDELIM`.
            let body_text = heredoc_body_to_source(body);
            let delim = pick_heredoc_delim(&body_text);
            let opener = if *strip_tabs { "<<-" } else { "<<" };
            let d = if *expand {
                delim.clone()
            } else {
                format!("'{delim}'")
            };
            // The body is raw text: each Literal part is emitted verbatim (NOT
            // bareword-escaped/quoted) and the body already carries its trailing
            // newline, so the closing delimiter follows directly. Expansion parts
            // ($VAR, $(...)) render through their normal source form so an
            // expanding heredoc keeps them.
            format!("{opener}{d}\n{body_text}{delim}")
        }
    }
}

/// Choose a heredoc closing delimiter that does NOT appear as a standalone
/// body line, so the round-trip re-parses the same body. Start with `EOF_GEN`
/// and bump a numeric suffix (`EOF_GEN_1`, `EOF_GEN_2`, …) until no body line
/// equals the candidate. A heredoc terminator must match the whole line, so we
/// compare against each line of the body verbatim.
fn pick_heredoc_delim(body_text: &str) -> String {
    let collides = |cand: &str| body_text.lines().any(|line| line == cand);
    let base = "EOF_GEN";
    if !collides(base) {
        return base.to_string();
    }
    let mut n = 1u32;
    loop {
        let cand = format!("{base}_{n}");
        if !collides(&cand) {
            return cand;
        }
        n += 1;
    }
}

fn word_to_source(w: &Word) -> String {
    w.0.iter().map(part_to_source).collect()
}

/// Render a heredoc body Word as raw text: `Literal` parts are emitted
/// verbatim (no quoting/escaping — heredoc bodies are literal lines), while
/// expansion parts ($VAR, $(...), etc.) use their normal source form so an
/// expanding heredoc preserves them. The body already carries its trailing
/// newline from the lexer.
fn heredoc_body_to_source(w: &Word) -> String {
    let mut out = String::new();
    for part in &w.0 {
        match part {
            WordPart::Literal { text, .. } => out.push_str(text),
            other => out.push_str(&part_to_source(other)),
        }
    }
    out
}

fn part_to_source(part: &WordPart) -> String {
    match part {
        WordPart::Literal { text, quoted } => {
            if *quoted {
                if text.is_empty() {
                    "''".to_string()
                } else {
                    format!("\"{}\"", crate::builtins::escape_double_quote_value(text))
                }
            } else {
                escape_bareword(text)
            }
        }
        WordPart::Var { name, quoted } => quote_if(*quoted, format!("${name}")),
        WordPart::LastStatus { quoted } => quote_if(*quoted, "$?".to_string()),
        WordPart::AllArgs { quoted, joined } => {
            quote_if(*quoted, (if *joined { "$*" } else { "$@" }).to_string())
        }
        WordPart::CommandSub { sequence, quoted } => {
            quote_if(*quoted, format!("$({})", sequence_to_source(sequence, 0).trim_end()))
        }
        WordPart::Arith { body, quoted } => {
            quote_if(*quoted, format!("$(({}))", word_to_source(body)))
        }
        WordPart::Tilde(t) => match t {
            TildeSpec::Home => "~".to_string(),
            TildeSpec::User(u) => format!("~{u}"),
            TildeSpec::Pwd => "~+".to_string(),
            TildeSpec::OldPwd => "~-".to_string(),
        },
        WordPart::ParamExpansion {
            name,
            modifier,
            quoted,
            subscript,
            indirect,
        } => quote_if(
            *quoted,
            param_expansion_to_source(name, modifier, subscript.as_ref(), *indirect),
        ),
        WordPart::AssignPrefix { target, append } => format!(
            "{}{}=",
            assign_target_to_source(target),
            if *append { "+" } else { "" }
        ),
        WordPart::ArrayLiteral(elems) => array_literal_to_source(elems),
    }
}

fn quote_if(quoted: bool, body: String) -> String {
    if quoted {
        format!("\"{body}\"")
    } else {
        body
    }
}

/// Backslash-escape characters that are special OUTSIDE quotes so a bareword
/// `Literal` round-trips as a single unquoted part. Glob/brace metacharacters
/// (`* ? [ ] { }`) are deliberately NOT escaped: an unquoted `Literal` is
/// glob-intended (the AST stores escaped/quoted forms as `quoted: true`, handled
/// by the quoted branch), so escaping `*` to `\*` would both change its meaning
/// and break the round-trip (re-parse turns `\*` into a quoted literal). This is
/// the single bareword escaper, shared by ordinary words and `case` patterns
/// (both want glob metachars preserved). An empty UNQUOTED literal carries no
/// content (it appears e.g. as the synthetic prefix fragment before an
/// `ArrayLiteral` in `a=(…)`), so it renders to nothing — emitting `''` would
/// mean a *quoted* empty word, which is the `quoted` branch's job.
fn escape_bareword(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            ' ' | '\t' | '\n' | '\'' | '"' | '\\' | '$' | ';' | '&' | '|' | '<' | '>' | '('
            | ')' | '`' | '~' | '#' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

fn assign_target_to_source(target: &crate::command::AssignTarget) -> String {
    match target {
        crate::command::AssignTarget::Bare(n) => n.clone(),
        crate::command::AssignTarget::Indexed { name, subscript } => {
            format!("{name}[{}]", word_to_source(subscript))
        }
    }
}

fn array_literal_to_source(elems: &[crate::lexer::ArrayLiteralElement]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for e in elems {
        match &e.subscript {
            Some(sub) => parts.push(format!(
                "[{}]={}",
                word_to_source(sub),
                word_to_source(&e.value)
            )),
            None => parts.push(word_to_source(&e.value)),
        }
    }
    format!("({})", parts.join(" "))
}

fn subscript_to_source(sub: &SubscriptKind) -> String {
    match sub {
        SubscriptKind::All => "[@]".to_string(),
        SubscriptKind::Star => "[*]".to_string(),
        SubscriptKind::Index(w) => format!("[{}]", word_to_source(w)),
    }
}

fn param_expansion_to_source(
    name: &str,
    modifier: &ParamModifier,
    subscript: Option<&SubscriptKind>,
    indirect: bool,
) -> String {
    let bang = if indirect { "!" } else { "" };
    let sub = subscript.map(subscript_to_source).unwrap_or_default();

    match modifier {
        // `${#name…}` — length PREFIXES the name.
        ParamModifier::Length => format!("${{#{bang}{name}{sub}}}"),
        // `${!name[@]}` / `${!name[*]}` — array keys form. The `!` here is
        // emitted regardless of `indirect` (the AST keeps indirect=false but
        // the construct is written with a leading `!`).
        ParamModifier::IndirectKeys => format!("${{!{name}{sub}}}"),
        _ => {
            let suffix = modifier_suffix(modifier);
            format!("${{{bang}{name}{sub}{suffix}}}")
        }
    }
}

/// Render the modifier portion that follows `${name[sub]`. Length and
/// IndirectKeys are handled by the caller (they don't follow this shape).
fn modifier_suffix(modifier: &ParamModifier) -> String {
    match modifier {
        ParamModifier::None => String::new(),
        ParamModifier::Length | ParamModifier::IndirectKeys => {
            unreachable!("Length/IndirectKeys handled by param_expansion_to_source")
        }
        ParamModifier::UseDefault { word, colon } => {
            format!("{}-{}", if *colon { ":" } else { "" }, word_to_source(word))
        }
        ParamModifier::AssignDefault { word, colon } => {
            format!("{}={}", if *colon { ":" } else { "" }, word_to_source(word))
        }
        ParamModifier::ErrorIfUnset { word, colon } => {
            format!("{}?{}", if *colon { ":" } else { "" }, word_to_source(word))
        }
        ParamModifier::UseAlternate { word, colon } => {
            format!("{}+{}", if *colon { ":" } else { "" }, word_to_source(word))
        }
        ParamModifier::RemovePrefix { pattern, longest } => {
            format!("{}{}", if *longest { "##" } else { "#" }, word_to_source(pattern))
        }
        ParamModifier::RemoveSuffix { pattern, longest } => {
            format!("{}{}", if *longest { "%%" } else { "%" }, word_to_source(pattern))
        }
        ParamModifier::Substitute {
            pattern,
            replacement,
            anchor,
            all,
        } => {
            let lead = if *all {
                "//".to_string()
            } else {
                match anchor {
                    SubstAnchor::None => "/".to_string(),
                    SubstAnchor::Prefix => "/#".to_string(),
                    SubstAnchor::Suffix => "/%".to_string(),
                }
            };
            format!(
                "{}{}/{}",
                lead,
                word_to_source(pattern),
                word_to_source(replacement)
            )
        }
        ParamModifier::Substring { offset, length } => {
            let mut s = format!(":{}", word_to_source(offset));
            if let Some(len) = length {
                s.push_str(&format!(":{}", word_to_source(len)));
            }
            s
        }
        ParamModifier::Case {
            direction,
            all,
            pattern,
        } => {
            let op = match (direction, all) {
                (CaseDirection::Upper, true) => "^^",
                (CaseDirection::Upper, false) => "^",
                (CaseDirection::Lower, true) => ",,",
                (CaseDirection::Lower, false) => ",",
            };
            let pat = pattern.as_ref().map(word_to_source).unwrap_or_default();
            format!("{op}{pat}")
        }
        ParamModifier::Transform { op } => {
            let c = match op {
                TransformOp::PromptExpand => 'P',
                TransformOp::Quote => 'Q',
                TransformOp::Upper => 'U',
                TransformOp::Lower => 'L',
                TransformOp::UpperFirst => 'u',
                TransformOp::EscapeExpand => 'E',
            };
            format!("@{c}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rt(src: &str) -> (String, String) {
        use crate::{command, lexer};
        let a = command::parse(lexer::tokenize(src).expect("lex"))
            .expect("parse")
            .expect("non-empty");
        let s1 = sequence_to_source(&a, 0);
        let b = command::parse(lexer::tokenize(&s1).expect("lex s1"))
            .expect("parse s1")
            .expect("non-empty s1");
        let s2 = sequence_to_source(&b, 0);
        (s1, s2)
    }
    fn assert_rt(src: &str) {
        let (s1, s2) = rt(src);
        assert_eq!(s1, s2, "not idempotent for {src:?}\n s1={s1:?}\n s2={s2:?}");
        assert!(!s1.trim().is_empty(), "empty output for {src:?}");
    }
    fn assert_rt_ast_eq(src: &str) {
        use crate::{command, lexer};
        assert_rt(src);
        let a = command::parse(lexer::tokenize(src).unwrap()).unwrap().unwrap();
        let s1 = sequence_to_source(&a, 0);
        let b = command::parse(lexer::tokenize(&s1).unwrap()).unwrap().unwrap();
        assert_eq!(a, b, "AST changed across round-trip for {src:?}");
    }

    #[test]
    fn rt_simple_word() {
        assert_rt_ast_eq("echo hello");
    }
    #[test]
    fn rt_double_quoted() {
        assert_rt("echo \"a  b\"");
    }
    #[test]
    fn rt_single_quoted() {
        assert_rt("echo 'a  b'");
    }
    #[test]
    fn rt_escaped_space() {
        assert_rt("echo a\\ b");
    }
    #[test]
    fn rt_var() {
        assert_rt_ast_eq("echo $HOME");
    }
    #[test]
    fn rt_braced_var() {
        assert_rt("echo ${HOME}");
    }
    #[test]
    fn rt_last_status() {
        assert_rt_ast_eq("echo $?");
    }
    #[test]
    fn rt_all_args() {
        assert_rt("echo \"$@\"");
    }
    #[test]
    fn rt_cmdsub() {
        assert_rt("echo $(date)");
    }
    #[test]
    fn rt_arith() {
        assert_rt("echo $((1 + 2))");
    }
    #[test]
    fn rt_param_default() {
        assert_rt("echo ${x:-def}");
    }
    #[test]
    fn rt_param_alt() {
        assert_rt("echo ${x:+alt}");
    }
    #[test]
    fn rt_param_remove_suffix() {
        assert_rt("echo ${x%.txt}");
    }
    #[test]
    fn rt_param_subst() {
        assert_rt("echo ${x/a/b}");
    }
    #[test]
    fn rt_param_substring() {
        assert_rt("echo ${x:1:2}");
    }
    #[test]
    fn rt_param_length() {
        assert_rt("echo ${#x}");
    }
    #[test]
    fn rt_array_index() {
        assert_rt("echo ${a[2]}");
    }
    #[test]
    fn rt_array_all() {
        assert_rt("echo \"${a[@]}\"");
    }
    #[test]
    fn rt_transform_q() {
        assert_rt("echo ${x@Q}");
    }
    #[test]
    fn rt_tilde() {
        assert_rt("echo ~");
    }
    #[test]
    fn rt_mixed() {
        assert_rt("echo pre$HOME\"post $x\"$(id)");
    }

    // ── Task 2: command-list serialization ──
    #[test]
    fn rt_args() {
        assert_rt_ast_eq("ls -l /tmp");
    }
    #[test]
    fn rt_assign_prefix() {
        assert_rt("FOO=bar BAZ=1 cmd a b");
    }
    #[test]
    fn rt_bare_assign() {
        assert_rt("x=1");
    }
    #[test]
    fn rt_append_assign() {
        assert_rt("x+=tail");
    }
    #[test]
    fn rt_array_assign() {
        assert_rt("a=(1 2 3)");
    }
    #[test]
    fn rt_indexed_assign() {
        assert_rt("a[2]=v");
    }
    #[test]
    fn rt_pipeline() {
        assert_rt_ast_eq("a | b | c");
    }
    #[test]
    fn rt_negated_pipeline() {
        assert_rt_ast_eq("! grep x f");
    }
    #[test]
    fn rt_semi() {
        assert_rt_ast_eq("a; b; c");
    }
    #[test]
    fn rt_and_or() {
        assert_rt_ast_eq("a && b || c");
    }
    #[test]
    fn rt_background() {
        assert_rt("sleep 1 &");
    }
    #[test]
    fn rt_redir_trunc() {
        assert_rt("echo hi > out");
    }
    #[test]
    fn rt_redir_append() {
        assert_rt("echo hi >> out");
    }
    #[test]
    fn rt_redir_read() {
        assert_rt("cat < in");
    }
    #[test]
    fn rt_redir_clobber() {
        assert_rt("echo hi >| out");
    }
    #[test]
    fn rt_redir_dup() {
        assert_rt("cmd 2>&1");
    }
    #[test]
    fn rt_redir_dup_file() {
        assert_rt("cmd > out 2>&1");
    }
    #[test]
    fn rt_herestring() {
        assert_rt("cat <<< word");
    }
    #[test]
    fn rt_heredoc() {
        assert_rt("cat <<EOF\nhi\nEOF");
    }

    // ── Task 3: compound commands + TestExpr ──
    #[test]
    fn rt_if() {
        assert_rt("if a; then b; fi");
    }
    #[test]
    fn rt_if_else() {
        assert_rt("if a; then b; else c; fi");
    }
    #[test]
    fn rt_if_elif() {
        assert_rt("if a; then b; elif c; then d; else e; fi");
    }
    #[test]
    fn rt_while() {
        assert_rt("while a; do b; done");
    }
    #[test]
    fn rt_until() {
        assert_rt("until a; do b; done");
    }
    #[test]
    fn rt_for() {
        assert_rt("for x in 1 2 3; do echo $x; done");
    }
    #[test]
    fn rt_for_noin() {
        assert_rt("for x; do echo $x; done");
    }
    #[test]
    fn rt_arith_for() {
        assert_rt("for ((i=0; i<3; i++)); do echo $i; done");
    }
    #[test]
    fn rt_select() {
        assert_rt("select x in a b; do echo $x; done");
    }
    #[test]
    fn rt_case() {
        assert_rt("case $x in a) echo A;; b|c) echo BC;; esac");
    }
    #[test]
    fn rt_case_fallthrough() {
        assert_rt("case $x in a) echo A;& *) echo D;; esac");
    }
    #[test]
    fn rt_subshell() {
        assert_rt("(a; b)");
    }
    #[test]
    fn rt_brace_group() {
        assert_rt("{ a; b; }");
    }
    #[test]
    fn rt_arith_cmd() {
        assert_rt("((x + 1))");
    }
    #[test]
    fn rt_dbracket_unary() {
        assert_rt("[[ -f /etc/passwd ]]");
    }
    #[test]
    fn rt_dbracket_binary() {
        assert_rt("[[ $x == y ]]");
    }
    #[test]
    fn rt_dbracket_regex() {
        assert_rt("[[ $x =~ ^a ]]");
    }
    #[test]
    fn rt_dbracket_logic() {
        assert_rt("[[ -n $x && $y == z ]]");
    }
    #[test]
    fn rt_redirected_compound() {
        assert_rt("while a; do b; done > out");
    }
    #[test]
    fn rt_nested() {
        assert_rt("if a; then for x in 1 2; do echo $x; done; fi");
    }
    #[test]
    fn rt_heredoc_collision() {
        assert_rt("cat <<EOF\nEOF_GEN\nhi\nEOF");
    }
}
