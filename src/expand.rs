use crate::lexer::{Word, WordPart};
use crate::shell_state::Shell;

/// Expands a `Word` against the current `Shell` state into 0 or more
/// argument strings. Quoted variable references append their value verbatim;
/// unquoted references split on ASCII whitespace and can yield multiple
/// fields (or zero, for an empty value).
pub fn expand(word: &Word, shell: &Shell) -> Vec<String> {
    let mut current = String::new();
    let mut has_emitted = false;
    let mut result: Vec<String> = Vec::new();

    for part in &word.0 {
        match part {
            WordPart::Literal(s) => {
                current.push_str(s);
                has_emitted = true;
            }
            WordPart::Tilde => {
                if let Some(home) = shell.get("HOME") {
                    current.push_str(home);
                }
                has_emitted = true;
            }
            WordPart::Var { name, quoted: true } => {
                if let Some(value) = shell.get(name) {
                    current.push_str(value);
                }
                has_emitted = true;
            }
            WordPart::LastStatus { quoted: true } => {
                current.push_str(&shell.last_status().to_string());
                has_emitted = true;
            }
            WordPart::Var { name, quoted: false } => {
                let value = shell.get(name).unwrap_or("");
                emit_split(value, &mut current, &mut result, &mut has_emitted);
            }
            WordPart::LastStatus { quoted: false } => {
                let value = shell.last_status().to_string();
                emit_split(&value, &mut current, &mut result, &mut has_emitted);
            }
        }
    }

    if has_emitted {
        result.push(current);
    }
    result
}

/// Splits `value` on ASCII whitespace and integrates the fields into the
/// caller's accumulator state, following the standard word-splitting rule.
fn emit_split(
    value: &str,
    current: &mut String,
    result: &mut Vec<String>,
    has_emitted: &mut bool,
) {
    let fields: Vec<&str> = value.split_ascii_whitespace().collect();
    match fields.len() {
        0 => {
            // No fields — the unquoted empty expansion contributes nothing.
        }
        1 => {
            current.push_str(fields[0]);
            *has_emitted = true;
        }
        _ => {
            current.push_str(fields[0]);
            result.push(std::mem::take(current));
            for f in &fields[1..fields.len() - 1] {
                result.push((*f).to_string());
            }
            *current = fields[fields.len() - 1].to_string();
            *has_emitted = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(s: &str) -> Word {
        Word(vec![WordPart::Literal(s.to_string())])
    }

    #[test]
    fn expand_literal_word() {
        let shell = Shell::new();
        assert_eq!(expand(&lit("hello"), &shell), vec!["hello".to_string()]);
    }

    #[test]
    fn expand_empty_literal_yields_one_empty_arg() {
        let shell = Shell::new();
        assert_eq!(expand(&lit(""), &shell), vec!["".to_string()]);
    }

    #[test]
    fn expand_multiple_literals_concatenate() {
        let shell = Shell::new();
        let word = Word(vec![
            WordPart::Literal("foo".to_string()),
            WordPart::Literal("bar".to_string()),
        ]);
        assert_eq!(expand(&word, &shell), vec!["foobar".to_string()]);
    }
}
