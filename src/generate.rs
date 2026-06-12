//! Render a parsed `Command` AST back to normalized, re-parseable shell source.
//! Output is a single consistent style (NOT byte-identical to bash); correctness
//! is round-trip idempotence (see tests). Built across v146 Tasks 1-3.
#![allow(dead_code)] // entry points wired in Task 4; some helpers land in Tasks 2-3
use crate::command::{Command, Sequence};
use crate::lexer::{
    CaseDirection, ParamModifier, SubscriptKind, SubstAnchor, TildeSpec, TransformOp, Word,
    WordPart,
};

/// Render a function definition for `declare -f`: `NAME ()\n<body>`.
pub fn function_to_source(name: &str, body: &Command) -> String {
    format!("{name} ()\n{}", command_to_source(body, 0))
}

/// Render any command at nesting depth `indent` (4 spaces/level).
pub fn command_to_source(cmd: &Command, _indent: usize) -> String {
    match cmd {
        Command::Simple(crate::command::SimpleCommand::Exec(e)) => exec_to_source(e),
        // A simple command parses as a single-stage, non-negated pipeline.
        // Unwrap it here so word-level round-trip works; Task 2/3 flesh out
        // the rest (negation, multi-stage, connectors, compound commands).
        Command::Pipeline(p)
            if !p.negate
                && p.commands.len() == 1
                && matches!(
                    p.commands[0],
                    Command::Simple(crate::command::SimpleCommand::Exec(_))
                ) =>
        {
            command_to_source(&p.commands[0], _indent)
        }
        _ => String::new(), // Tasks 2-3 fill these; OK for word-level round-trip
    }
}

/// Render a command list. Implemented enough here for command-substitution
/// round-trip (single simple command); Task 2 completes connectors/redirects.
fn sequence_to_source(seq: &Sequence, indent: usize) -> String {
    command_to_source(&seq.first, indent) // Task 2 adds rest/background
}

fn pad(indent: usize) -> String {
    "    ".repeat(indent)
}

fn exec_to_source(e: &crate::command::ExecCommand) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(word_to_source(&e.program));
    for w in &e.args {
        parts.push(word_to_source(w));
    }
    parts.join(" ")
}

fn word_to_source(w: &Word) -> String {
    w.0.iter().map(part_to_source).collect()
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
/// `Literal` round-trips as a single unquoted part. Empty → `''`.
fn escape_bareword(text: &str) -> String {
    if text.is_empty() {
        return "''".to_string();
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            ' ' | '\t' | '\n' | '\'' | '"' | '\\' | '$' | ';' | '&' | '|' | '<' | '>' | '('
            | ')' | '`' | '*' | '?' | '[' | ']' | '{' | '}' | '~' | '#' => {
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
}
