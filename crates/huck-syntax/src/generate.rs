//! Render a parsed `Command` AST back to re-parseable shell source. For the
//! `declare -f` / `type` (function-definition) context this matches bash
//! 5.2.21's `print_cmd.c` output byte-for-byte (see `declare_f_diff_check.sh`
//! and the `declf_*` tests); correctness is also round-trip idempotence (see
//! the `rt_*` tests). Built across v146 Tasks 1-3; bash-faithful port in v218.
use crate::command::{
    Assignment, CaseClause, CaseItem, CaseTerminator, Command, Connector, ElifBranch, ExecCommand,
    ForClause, IfClause, Pipeline, RedirectSlot, SelectClause, Sequence, SimpleCommand,
    TestBinaryOp, TestExpr, TestUnaryOp, WhileClause,
};
use crate::lexer::{
    CaseDirection, ParamModifier, SubscriptKind, SubstAnchor, TildeSpec, TransformOp, Word,
    WordPart,
};

/// Render a function definition for `declare -f`: `NAME ()\n<body>`.
pub fn function_to_source(name: &str, body: &Command) -> String {
    render_function_def(name, body, 0, false)
}

/// Append the 0/1/2 slot redirects to `s`, each prefixed with a space
/// (e.g. ` 1>&2`). Shared by the hoisted-brace-group path, the
/// `Command::Redirected` arm, and `exec_to_source` — all three emit
/// identical spacing/ordering (stdin → stdout → stderr, empty slots skipped).
fn append_slot_redirects(s: &mut String, redirects: &[crate::command::Redirection]) {
    let (stdin, stdout, stderr) = crate::command::slots_for_simple_path(redirects);
    if let Some(r) = &stdin {
        s.push(' ');
        s.push_str(&redirect_to_source(r, RedirDefault::Stdin));
    }
    if let Some(r) = &stdout {
        s.push(' ');
        s.push_str(&redirect_to_source(r, RedirDefault::Stdout));
    }
    if let Some(r) = &stderr {
        s.push(' ');
        s.push_str(&redirect_to_source(r, RedirDefault::Stderr));
    }
}

/// Render a function definition. `with_keyword` adds the leading `function `
/// that bash emits for NESTED defs; the outer named function (declare -f / type
/// entry point) passes `false`. A brace-group body — bare or carrying a redirect
/// — becomes the function's own braces, with any redirect hoisted to the close
/// brace (`} 1>&2`). Any other body is wrapped in fresh `{ }`.
fn render_function_def(name: &str, body: &Command, indent: usize, with_keyword: bool) -> String {
    let kw = if with_keyword { "function " } else { "" };
    let (group_seq, hoisted): (Sequence, String) = match body {
        Command::BraceGroup(seq) => ((**seq).clone(), String::new()),
        Command::Redirected { inner, redirects }
            if matches!(inner.as_ref(), Command::BraceGroup(_)) =>
        {
            let Command::BraceGroup(seq) = inner.as_ref() else {
                unreachable!()
            };
            let mut hoisted = String::new();
            append_slot_redirects(&mut hoisted, redirects);
            ((**seq).clone(), hoisted)
        }
        other => (
            Sequence {
                first: other.clone(),
                rest: Vec::new(),
                background: false,
            },
            String::new(),
        ),
    };
    format!(
        "{kw}{name} () \n{p}{{ \n{}{p}}}{hoisted}",
        group_body(&group_seq, indent + 1),
        p = pad(indent),
    )
}

/// The BASH_FUNC env-var VALUE form: `() {\n    <body>\n}` (no name prefix).
/// A child shell parses it after prepending the function name.
pub fn exported_function_value(body: &Command) -> String {
    format!("() {}", command_to_source(body, 0))
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
        Command::BraceGroup(seq) => {
            format!("{{ \n{}{}}}", group_body(seq, indent + 1), pad(indent))
        }
        Command::Subshell { body } => {
            // bash prints subshells inline at the SAME indent: `( <body> )`.
            format!("( {} )", sequence_to_source(body, indent))
        }
        Command::Arith(word) => format!("(({}))", arith_body_to_source(word)),
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
        Command::FunctionDef { name, body } => render_function_def(name, body, indent, true),
        Command::Coproc { name, body } => {
            let body_src = command_to_source(body, indent);
            if name == "COPROC" {
                format!("coproc {body_src}")
            } else {
                format!("coproc {name} {body_src}")
            }
        }
        Command::Redirected { inner, redirects } => {
            // Source regeneration uses the 0/1/2 slot fast-path (v156).
            // Regeneration of fd>2 / `<&` / `{var}` redirects is best-effort
            // (slot-collapsed).
            let mut s = command_to_source(inner, indent);
            append_slot_redirects(&mut s, redirects);
            s
        }
    }
}

/// Group / function / subshell / case body: indented sequence, NO trailing
/// `;`, terminated by a newline. (bash: these bodies never call `semicolon()`.)
fn group_body(seq: &Sequence, indent: usize) -> String {
    format!("{}{}\n", pad(indent), sequence_to_source(seq, indent))
}

/// if / while / until / for / arith-for / select body: indented sequence with
/// bash's `semicolon()` terminator — a trailing `;` UNLESS the rendered body
/// already ends in `&` (a background command) or `\n` (e.g. a heredoc).
fn loop_body(seq: &Sequence, indent: usize) -> String {
    let inner = sequence_to_source(seq, indent);
    let semi = if inner.ends_with('&') || inner.ends_with('\n') {
        ""
    } else {
        ";"
    };
    format!("{}{}{}\n", pad(indent), inner, semi)
}

/// Render a condition/header sequence inline (no indentation, no trailing
/// terminator) for use on a keyword line like `if <cond>; then`.
fn inline_seq(seq: &Sequence) -> String {
    sequence_to_source(seq, 0)
}

fn if_to_source(c: &IfClause, indent: usize) -> String {
    let mut s = format!("if {}; then\n", inline_seq(&c.condition));
    s.push_str(&loop_body(&c.then_body, indent + 1));
    s.push_str(&nested_elif(&c.elif_branches, &c.else_body, indent));
    s.push_str(&pad(indent));
    s.push_str("fi");
    s
}

/// bash has no `elif` node — it renders `elif` as a nested `else { if … fi; }`,
/// deepening one indent level per branch. The inner `fi` takes a `;` (the outer
/// `semicolon()`); the outermost `fi` (emitted by `if_to_source`) does not.
fn nested_elif(elifs: &[ElifBranch], else_body: &Option<Sequence>, indent: usize) -> String {
    if let Some((head, tail)) = elifs.split_first() {
        let inner = indent + 1;
        let mut s = format!("{}else\n", pad(indent));
        s.push_str(&pad(inner));
        s.push_str(&format!("if {}; then\n", inline_seq(&head.condition)));
        s.push_str(&loop_body(&head.body, inner + 1));
        s.push_str(&nested_elif(tail, else_body, inner));
        s.push_str(&pad(inner));
        s.push_str("fi;\n");
        s
    } else if let Some(eb) = else_body {
        format!("{}else\n{}", pad(indent), loop_body(eb, indent + 1))
    } else {
        String::new()
    }
}

fn while_to_source(c: &WhileClause, indent: usize) -> String {
    let kw = if c.until { "until" } else { "while" };
    let mut s = format!("{kw} {}; do\n", inline_seq(&c.condition));
    s.push_str(&loop_body(&c.body, indent + 1));
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
    } else {
        // bash desugars the no-`in` form to `in "$@"`; semantically identical.
        header.push_str(" in \"$@\"");
    }
    let mut s = format!("{header};\n{}do\n", pad(indent));
    s.push_str(&loop_body(&c.body, indent + 1));
    s.push_str(&pad(indent));
    s.push_str("done");
    s
}

fn arith_for_to_source(c: &crate::command::ArithForClause, indent: usize) -> String {
    let sec =
        |w: &Option<crate::lexer::Word>| w.as_ref().map(arith_body_to_source).unwrap_or_default();
    let mut s = format!(
        "for (({}; {}; {}))\n{}do\n",
        sec(&c.init),
        sec(&c.cond),
        sec(&c.step),
        pad(indent)
    );
    s.push_str(&loop_body(&c.body, indent + 1));
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
    let mut s = format!("{header};\n{}do\n", pad(indent));
    s.push_str(&loop_body(&c.body, indent + 1));
    s.push_str(&pad(indent));
    s.push_str("done");
    s
}

fn case_to_source(c: &CaseClause, indent: usize) -> String {
    let mut s = format!("case {} in \n", word_to_source(&c.subject));
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
        s.push_str(&group_body(body, indent + 1)); // case body: no trailing `;`
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
            WordPart::Literal {
                text,
                quoted: false,
            } => escape_bareword(text),
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
        SimpleCommand::Assign(assigns, _) => assigns
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
            Connector::Amp => out.push_str(" & "),
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
    // 0/1/2 slot fast-path for source regeneration (v156, best-effort).
    append_slot_redirects(&mut s, &e.redirects);
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

fn redirect_to_source(r: &RedirectSlot, which: RedirDefault) -> String {
    let fd_prefix = match which {
        RedirDefault::Stderr => "2",
        RedirDefault::Stdin | RedirDefault::Stdout => "",
    };
    match r {
        RedirectSlot::Read(w) => format!("< {}", word_to_source(w)),
        RedirectSlot::Truncate(w) => format!("{fd_prefix}> {}", word_to_source(w)),
        RedirectSlot::Append(w) => format!("{fd_prefix}>> {}", word_to_source(w)),
        RedirectSlot::Clobber(w) => format!("{fd_prefix}>| {}", word_to_source(w)),
        RedirectSlot::Dup { fd, source } => format!("{fd}>&{}", word_to_source(source)),
        RedirectSlot::HereString(w) => format!("<<< {}", word_to_source(w)),
        RedirectSlot::Heredoc {
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

/// Render an arithmetic body Word as raw expression text. The lexer marks
/// arith literal/expansion parts `quoted: true` (so expansion-time quote
/// removal applies), but bash's `print_cmd.c` prints the expression WITHOUT
/// those quotes (`(( i < 3 ))`, not `((" i < 3 "))`). Emit each part bare:
/// Literal text verbatim, expansions via their `$…` source form.
fn arith_body_to_source(w: &Word) -> String {
    let mut out = String::new();
    for part in &w.0 {
        match part {
            WordPart::Literal { text, .. } => out.push_str(text),
            WordPart::Var { name, .. } => out.push_str(&format!("${name}")),
            WordPart::LastStatus { .. } => out.push_str("$?"),
            WordPart::AllArgs { joined, .. } => out.push_str(if *joined { "$*" } else { "$@" }),
            WordPart::CommandSub { sequence, .. } => out.push_str(&format!(
                "$({})",
                sequence_to_source(sequence, 0).trim_end()
            )),
            WordPart::Arith { body, .. } => {
                out.push_str(&format!("$(({}))", arith_body_to_source(body)))
            }
            WordPart::ParamExpansion {
                name,
                modifier,
                subscript,
                indirect,
                ..
            } => out.push_str(&param_expansion_to_source(
                name,
                modifier,
                subscript.as_ref(),
                *indirect,
            )),
            other => out.push_str(&part_to_source(other)),
        }
    }
    out
}

/// Render one part inside a Single/Backslash/AnsiC quoted run: a `Literal`
/// contributes its text verbatim (the run owns the quotes); an expansion renders
/// via its normal source form WITHOUT re-quoting. (The `Double` style escapes
/// literals itself, so it does not use this helper.)
fn quoted_inner_to_source(part: &WordPart) -> String {
    match part {
        WordPart::Literal { text, .. } => text.clone(),
        other => part_to_source(other),
    }
}

/// Render a part that appears inside `"…"` — without the outer `"…"` wrapper
/// that `quote_if(true, …)` would normally add; the caller provides the
/// surrounding double-quotes. Literals are re-escaped for the double-quote
/// context; expansions are rendered without re-wrapping.
fn part_to_source_in_double(part: &WordPart) -> String {
    match part {
        WordPart::Literal { text, .. } => crate::escape_double_quote_value(text),
        WordPart::Var { name, .. } => format!("${name}"),
        WordPart::LastStatus { .. } => "$?".to_string(),
        WordPart::AllArgs { joined, .. } => (if *joined { "$*" } else { "$@" }).to_string(),
        WordPart::CommandSub { sequence, .. } => {
            format!("$({})", sequence_to_source(sequence, 0).trim_end())
        }
        WordPart::Arith { body, .. } => format!("$(({}))", arith_body_to_source(body)),
        WordPart::ParamExpansion {
            name,
            modifier,
            subscript,
            indirect,
            ..
        } => param_expansion_to_source(name, modifier, subscript.as_ref(), *indirect),
        // Nested Quoted or anything else: delegate to the full renderer.
        other => part_to_source(other),
    }
}

/// ANSI-C re-escape for `$'…'`: turn control chars back into their escapes.
fn ansi_c_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
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
                    format!("\"{}\"", crate::escape_double_quote_value(text))
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
        WordPart::CommandSub { sequence, quoted } => quote_if(
            *quoted,
            format!("$({})", sequence_to_source(sequence, 0).trim_end()),
        ),
        WordPart::Arith { body, quoted } => {
            quote_if(*quoted, format!("$(({}))", arith_body_to_source(body)))
        }
        WordPart::Tilde { spec: t, .. } => match t {
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
        WordPart::ProcessSub { sequence, dir } => {
            let prefix = match dir {
                crate::lexer::ProcDir::In => "<(",
                crate::lexer::ProcDir::Out => ">(",
            };
            format!("{}{})", prefix, sequence_to_source(sequence, 0).trim_end())
        }
        WordPart::Quoted { style, parts } => {
            use crate::lexer::QuoteStyle;
            match style {
                QuoteStyle::Double => {
                    let inner: String = parts.iter().map(part_to_source_in_double).collect();
                    format!("\"{inner}\"")
                }
                QuoteStyle::Single => {
                    let inner: String = parts.iter().map(quoted_inner_to_source).collect();
                    format!("'{inner}'")
                }
                QuoteStyle::Backslash => {
                    let inner: String = parts.iter().map(quoted_inner_to_source).collect();
                    format!("\\{inner}")
                }
                QuoteStyle::AnsiC => {
                    let inner: String = parts.iter().map(quoted_inner_to_source).collect();
                    format!("$'{}'", ansi_c_escape(&inner))
                }
            }
        }
    }
}

fn quote_if(quoted: bool, body: String) -> String {
    if quoted { format!("\"{body}\"") } else { body }
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
                "[{}]{}={}",
                word_to_source(sub),
                if e.append { "+" } else { "" },
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
        // `${!prefix*}` / `${!prefix@}` — prefix-name expansion (the `!` is a
        // prefix and `*`/`@` a suffix, so it doesn't fit the generic shape).
        ParamModifier::PrefixNames { at } => {
            format!("${{!{name}{}}}", if *at { "@" } else { "*" })
        }
        // `${…}` bad substitution: `raw` is the full `${…}` text already;
        // reproduce it verbatim so `declare -f` / `type` round-trips cleanly.
        ParamModifier::BadSubst { raw } => raw.clone(),
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
        ParamModifier::PrefixNames { .. } => {
            unreachable!("PrefixNames handled by param_expansion_to_source")
        }
        ParamModifier::BadSubst { raw } => {
            // Handled by param_expansion_to_source before reaching here;
            // this arm exists only for exhaustiveness.
            raw.clone()
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
            format!(
                "{}{}",
                if *longest { "##" } else { "#" },
                word_to_source(pattern)
            )
        }
        ParamModifier::RemoveSuffix { pattern, longest } => {
            format!(
                "{}{}",
                if *longest { "%%" } else { "%" },
                word_to_source(pattern)
            )
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
                TransformOp::AssignDecl => 'A',
                TransformOp::KvString => 'K',
                TransformOp::KvWords => 'k',
                TransformOp::AttrFlags => 'a',
            };
            format!("@{c}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn live_parse(src: &str) -> crate::command::Sequence {
        use crate::{lexer, parser};
        parser::parse_sequence(&mut lexer::Lexer::new(
            src,
            &Default::default(),
            lexer::LexerOptions::default(),
        ))
        .expect("parse")
        .expect("non-empty")
    }
    fn rt(src: &str) -> (String, String) {
        let a = live_parse(src);
        let s1 = sequence_to_source(&a, 0);
        let b = live_parse(&s1);
        let s2 = sequence_to_source(&b, 0);
        (s1, s2)
    }
    #[test]
    fn exported_function_value_form() {
        use crate::command;
        let seq = live_parse("f(){ echo hi; }");
        let body = match seq.first {
            command::Command::FunctionDef { body, .. } => body,
            _ => panic!(),
        };
        let v = exported_function_value(&body);
        assert!(v.starts_with("() "), "{v}");
        assert!(v.contains("echo hi"), "{v}");
        // re-parseable when prefixed with a name:
        let reparse = live_parse(&format!("f {v}"));
        assert!(matches!(
            reparse.first,
            command::Command::FunctionDef { .. }
        ));
    }

    fn assert_rt(src: &str) {
        let (s1, s2) = rt(src);
        assert_eq!(s1, s2, "not idempotent for {src:?}\n s1={s1:?}\n s2={s2:?}");
        assert!(!s1.trim().is_empty(), "empty output for {src:?}");
    }
    fn assert_rt_ast_eq(src: &str) {
        assert_rt(src);
        let a = live_parse(src);
        let s1 = sequence_to_source(&a, 0);
        let b = live_parse(&s1);
        // Compare structure in canonical source form: generate->parse legitimately
        // reflows physical lines (`;`-joined commands onto separate lines), so AST
        // `line` metadata differs even when structure is identical. The source
        // fixpoint (parse(s1) regenerates to s1) is the line-agnostic check.
        assert_eq!(
            sequence_to_source(&b, 0),
            s1,
            "AST changed across round-trip for {src:?}"
        );
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

    #[test]
    fn renders_process_substitution_both_directions() {
        use crate::command;

        // input direction: <(...)
        let seq_in = live_parse("f() { cat <(echo a); }");
        let body_in = match seq_in.first {
            command::Command::FunctionDef { body, .. } => body,
            _ => panic!("expected FunctionDef"),
        };
        let rendered_in = function_to_source("f", &body_in);
        assert!(rendered_in.contains("<(echo a)"), "got: {rendered_in}");

        // output direction: >(...)
        let seq_out = live_parse("g() { tee >(cat); }");
        let body_out = match seq_out.first {
            command::Command::FunctionDef { body, .. } => body,
            _ => panic!("expected FunctionDef"),
        };
        let rendered_out = function_to_source("g", &body_out);
        assert!(rendered_out.contains(">(cat)"), "got: {rendered_out}");
    }

    #[test]
    fn arith_command_renders_unquoted() {
        use crate::command;
        let seq = live_parse("f(){ (( i < 3 )); }");
        let command::Command::FunctionDef { name, body } = seq.first else {
            panic!()
        };
        let s = function_to_source(&name, &body);
        assert!(s.contains("(( i < 3 ))"), "got: {s:?}");
        assert!(!s.contains("((\""), "spurious quote in: {s:?}");
    }

    #[test]
    fn arith_expansion_renders_unquoted() {
        use crate::command;
        let seq = live_parse("f(){ i=$(( i + 1 )); }");
        let command::Command::FunctionDef { name, body } = seq.first else {
            panic!()
        };
        let s = function_to_source(&name, &body);
        assert!(s.contains("$(( i + 1 ))"), "got: {s:?}");
        assert!(!s.contains("$((\""), "spurious quote in: {s:?}");
    }

    #[test]
    fn arith_with_var_renders_unquoted() {
        use crate::command;
        let seq = live_parse("f(){ (( x + $y )); }");
        let command::Command::FunctionDef { name, body } = seq.first else {
            panic!()
        };
        let s = function_to_source(&name, &body);
        assert!(s.contains("(( x + $y ))"), "got: {s:?}");
    }

    // ── Task 2 (v218): bash print_cmd.c exact-match tests ──
    fn declf(src: &str) -> String {
        use crate::command;
        let seq = live_parse(src);
        let command::Command::FunctionDef { name, body } = seq.first else {
            panic!("expected a function def")
        };
        function_to_source(&name, &body)
    }

    #[test]
    fn declf_simple_last_semi_suppressed() {
        assert_eq!(
            declf("f(){ echo a; echo b; }"),
            "f () \n{ \n    echo a;\n    echo b\n}"
        );
    }
    #[test]
    fn declf_subshell_inline() {
        assert_eq!(declf("f(){ ( exit 1 ); }"), "f () \n{ \n    ( exit 1 )\n}");
    }
    #[test]
    fn declf_subshell_multi() {
        assert_eq!(declf("f(){ ( a; b ); }"), "f () \n{ \n    ( a;\n    b )\n}");
    }
    #[test]
    fn declf_group_multiline() {
        assert_eq!(
            declf("f(){ { echo a; }; }"),
            "f () \n{ \n    { \n        echo a\n    }\n}"
        );
    }
    #[test]
    fn declf_andor_inline() {
        assert_eq!(
            declf("f(){ a && b || c; }"),
            "f () \n{ \n    a && b || c\n}"
        );
    }
    #[test]
    fn declf_mid_background_inline() {
        assert_eq!(
            declf("f(){ echo bg >/dev/null & echo next; }"),
            "f () \n{ \n    echo bg > /dev/null & echo next\n}"
        );
    }
    #[test]
    fn declf_if() {
        assert_eq!(
            declf("f(){ if a; then b; fi; }"),
            "f () \n{ \n    if a; then\n        b;\n    fi\n}"
        );
    }
    #[test]
    fn declf_if_elif_else() {
        assert_eq!(
            declf("f(){ if a; then b; elif c; then d; else e; fi; }"),
            "f () \n{ \n    if a; then\n        b;\n    else\n        if c; then\n            d;\n        else\n            e;\n        fi;\n    fi\n}"
        );
    }
    #[test]
    fn declf_while() {
        assert_eq!(
            declf("f(){ while a; do b; done; }"),
            "f () \n{ \n    while a; do\n        b;\n    done\n}"
        );
    }
    #[test]
    fn declf_until() {
        assert_eq!(
            declf("f(){ until a; do b; done; }"),
            "f () \n{ \n    until a; do\n        b;\n    done\n}"
        );
    }
    #[test]
    fn declf_for_in() {
        assert_eq!(
            declf("f(){ for x in 1 2; do echo $x; done; }"),
            "f () \n{ \n    for x in 1 2;\n    do\n        echo $x;\n    done\n}"
        );
    }
    #[test]
    fn declf_for_noin() {
        assert_eq!(
            declf("f(){ for x; do echo $x; done; }"),
            "f () \n{ \n    for x in \"$@\";\n    do\n        echo $x;\n    done\n}"
        );
    }
    #[test]
    fn declf_arith_for() {
        assert_eq!(
            declf("f(){ for ((i=0; i<3; i++)); do echo $i; done; }"),
            "f () \n{ \n    for ((i=0; i<3; i++))\n    do\n        echo $i;\n    done\n}"
        );
    }
    #[test]
    fn declf_select() {
        assert_eq!(
            declf("f(){ select x in a b; do echo $x; done; }"),
            "f () \n{ \n    select x in a b;\n    do\n        echo $x;\n    done\n}"
        );
    }
    #[test]
    fn declf_case() {
        assert_eq!(
            declf("f(){ case $x in a) echo A;; b|c) echo BC;; esac; }"),
            "f () \n{ \n    case $x in \n        a)\n            echo A\n        ;;\n        b | c)\n            echo BC\n        ;;\n    esac\n}"
        );
    }
    #[test]
    fn declf_loop_bg_tail_no_semi() {
        assert_eq!(
            declf("f(){ while a; do b & done; }"),
            "f () \n{ \n    while a; do\n        b &\n    done\n}"
        );
    }
    #[test]
    fn declf_nested_compound_tail_gets_semi() {
        assert_eq!(
            declf("f(){ while a; do for x in 1; do b; done; done; }"),
            "f () \n{ \n    while a; do\n        for x in 1;\n        do\n            b;\n        done;\n    done\n}"
        );
    }
    #[test]
    fn declf_subshell_body_wrapped() {
        assert_eq!(declf("f() ( echo hi )"), "f () \n{ \n    ( echo hi )\n}");
    }

    // ── v222: outer-vs-nested keyword split + redirect hoist ──
    #[test]
    fn declf_outer_no_function_keyword_all_forms() {
        for src in [
            "f(){ echo a; }",
            "function f { echo a; }",
            "function f() { echo a; }",
        ] {
            let s = declf(src);
            assert!(s.starts_with("f () \n"), "outer must omit keyword: {s:?}");
            assert!(
                !s.starts_with("function "),
                "outer must not start with `function `: {s:?}"
            );
        }
    }

    #[test]
    fn declf_nested_def_gets_function_keyword_all_forms() {
        // All three nested forms render identically as `function f3 () `.
        for inner in [
            "function f3() { echo b; }",
            "function f3 { echo b; }",
            "f3() { echo b; }",
        ] {
            let s = declf(&format!("outer(){{ echo a; {inner}; }}"));
            assert!(
                s.contains("function f3 () \n"),
                "nested def needs keyword (inner={inner:?}): {s:?}"
            );
            assert!(
                s.starts_with("outer () \n"),
                "outer still keyword-free: {s:?}"
            );
        }
    }

    #[test]
    fn declf_outer_redirected_brace_body_hoists() {
        // `{ …; } 1>&2` body → unwrapped, redirect on the function close brace.
        assert_eq!(
            declf("f(){ echo a; echo b; } 1>&2"),
            "f () \n{ \n    echo a;\n    echo b\n} 1>&2"
        );
    }

    #[test]
    fn declf_nested_redirected_brace_body_hoists() {
        let s = declf("outer(){ f3() { echo b; } 1>&2; }");
        assert!(
            s.contains("function f3 () \n    { \n        echo b\n    } 1>&2"),
            "nested redirected brace body must hoist: {s:?}"
        );
    }

    #[test]
    fn declf_subshell_body_with_redirect_not_hoisted() {
        // A subshell body keeps its redirect INSIDE the function braces.
        assert_eq!(
            declf("funcc() ( echo c ) 2>&1"),
            "funcc () \n{ \n    ( echo c ) 2>&1\n}"
        );
    }

    // ── Quoted variant rendering ──
    #[test]
    fn render_quoted_single() {
        use crate::lexer::{QuoteStyle, Word, WordPart};
        let w = Word(vec![WordPart::Quoted {
            style: QuoteStyle::Single,
            parts: vec![WordPart::Literal {
                text: "what a window".into(),
                quoted: true,
            }],
        }]);
        assert_eq!(word_to_source(&w), "'what a window'");
    }
    #[test]
    fn render_quoted_double_span() {
        use crate::lexer::{QuoteStyle, Word, WordPart};
        let w = Word(vec![WordPart::Quoted {
            style: QuoteStyle::Double,
            parts: vec![
                WordPart::Var {
                    name: "a".into(),
                    quoted: true,
                },
                WordPart::Literal {
                    text: " ".into(),
                    quoted: true,
                },
                WordPart::Var {
                    name: "b".into(),
                    quoted: true,
                },
            ],
        }]);
        assert_eq!(word_to_source(&w), "\"$a $b\"");
    }
    #[test]
    fn render_quoted_backslash() {
        use crate::lexer::{QuoteStyle, Word, WordPart};
        let w = Word(vec![
            WordPart::Quoted {
                style: QuoteStyle::Backslash,
                parts: vec![WordPart::Literal {
                    text: "$".into(),
                    quoted: true,
                }],
            },
            WordPart::Literal {
                text: "PWD".into(),
                quoted: false,
            },
        ]);
        assert_eq!(word_to_source(&w), "\\$PWD");
    }
    #[test]
    fn render_quoted_adjacent_double() {
        use crate::lexer::{QuoteStyle, Word, WordPart};
        let run = |t: &str| WordPart::Quoted {
            style: QuoteStyle::Double,
            parts: vec![WordPart::Literal {
                text: t.into(),
                quoted: true,
            }],
        };
        let w = Word(vec![run("a b"), run("c d")]);
        assert_eq!(word_to_source(&w), "\"a b\"\"c d\"");
    }
    #[test]
    fn render_quoted_double_escapes_specials() {
        use crate::lexer::{QuoteStyle, Word, WordPart};
        let w = Word(vec![WordPart::Quoted {
            style: QuoteStyle::Double,
            parts: vec![WordPart::Literal {
                text: "a\"b$c".into(),
                quoted: true,
            }],
        }]);
        // inside "...", a literal " and $ must be backslash-escaped
        assert_eq!(word_to_source(&w), "\"a\\\"b\\$c\"");
    }
    #[test]
    fn render_quoted_ansic_newline() {
        use crate::lexer::{QuoteStyle, Word, WordPart};
        let w = Word(vec![WordPart::Quoted {
            style: QuoteStyle::AnsiC,
            parts: vec![WordPart::Literal {
                text: "i\n".into(),
                quoted: true,
            }],
        }]);
        assert_eq!(word_to_source(&w), "$'i\\n'");
    }

    // ── Task 2 (v219): end-to-end byte-exact reconstruction of quoted words ──
    fn declf_word(body_word_src: &str) -> String {
        use crate::command;
        let src = format!("f(){{ echo {body_word_src}; }}");
        let seq = live_parse(&src);
        let command::Command::FunctionDef { name, body } = seq.first else {
            panic!()
        };
        function_to_source(&name, &body)
    }

    #[test]
    fn rt_quote_single() {
        assert!(declf_word("'what a window'").contains("echo 'what a window'"));
    }
    #[test]
    fn rt_quote_dq_span() {
        assert!(declf_word("\"$a $b\"").contains("echo \"$a $b\""));
    }
    #[test]
    fn rt_quote_backslash() {
        assert!(declf_word("\\$PWD").contains("echo \\$PWD"));
    }
    #[test]
    fn rt_quote_adjacent() {
        assert!(declf_word("\"a b\"\"c d\"").contains("echo \"a b\"\"c d\""));
    }
    #[test]
    fn rt_quote_mixed() {
        assert!(declf_word("ab'cd'ef").contains("echo ab'cd'ef"));
    }
    #[test]
    fn rt_quote_specials() {
        assert!(declf_word("\\&\\|'()'").contains("echo \\&\\|'()'"));
    }
}
