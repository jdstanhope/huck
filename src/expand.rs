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

    fn var_unq(name: &str) -> Word {
        Word(vec![WordPart::Var { name: name.to_string(), quoted: false }])
    }
    fn var_q(name: &str) -> Word {
        Word(vec![WordPart::Var { name: name.to_string(), quoted: true }])
    }

    #[test]
    fn expand_unset_unquoted_yields_no_args() {
        let shell = Shell::new();
        assert!(expand(&var_unq("DEFINITELY_NOT_SET_XYZ"), &shell).is_empty());
    }

    #[test]
    fn expand_unset_quoted_yields_one_empty_arg() {
        let shell = Shell::new();
        assert_eq!(
            expand(&var_q("DEFINITELY_NOT_SET_XYZ"), &shell),
            vec!["".to_string()]
        );
    }

    #[test]
    fn expand_set_var_quoted_preserves_whitespace() {
        let mut shell = Shell::new();
        shell.set("SHUCK_T", "a b".to_string());
        assert_eq!(expand(&var_q("SHUCK_T"), &shell), vec!["a b".to_string()]);
    }

    #[test]
    fn expand_set_var_unquoted_splits_whitespace() {
        let mut shell = Shell::new();
        shell.set("SHUCK_T", "a b".to_string());
        assert_eq!(
            expand(&var_unq("SHUCK_T"), &shell),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn expand_unquoted_var_with_literal_prefix_merges_first_field() {
        let mut shell = Shell::new();
        shell.set("SHUCK_T", "x y".to_string());
        let word = Word(vec![
            WordPart::Literal("a".to_string()),
            WordPart::Var { name: "SHUCK_T".to_string(), quoted: false },
        ]);
        assert_eq!(
            expand(&word, &shell),
            vec!["ax".to_string(), "y".to_string()]
        );
    }

    #[test]
    fn expand_last_status_quoted() {
        let mut shell = Shell::new();
        shell.set_last_status(42);
        let word = Word(vec![WordPart::LastStatus { quoted: true }]);
        assert_eq!(expand(&word, &shell), vec!["42".to_string()]);
    }

    #[test]
    fn expand_tilde_uses_home() {
        let mut shell = Shell::new();
        shell.export_set("HOME", "/tmp/shuck_test".to_string());
        let word = Word(vec![
            WordPart::Tilde,
            WordPart::Literal("/foo".to_string()),
        ]);
        assert_eq!(
            expand(&word, &shell),
            vec!["/tmp/shuck_test/foo".to_string()]
        );
    }

    #[test]
    fn expand_unset_unquoted_returns_no_fields_for_redirect_check() {
        // executor.rs::expand_single uses fields.len() != 1 as the
        // "ambiguous redirect" signal. Confirm 0 fields fires it.
        let shell = Shell::new();
        assert_eq!(expand(&Word(vec![WordPart::Var {
            name: "DEFINITELY_NOT_SET_REDIR".to_string(),
            quoted: false,
        }]), &shell).len(), 0);
    }

    #[test]
    fn expand_unquoted_var_with_two_fields_returns_two_for_redirect_check() {
        // Two-field result must also fire the ambiguous-redirect path.
        let mut shell = Shell::new();
        shell.set("SHUCK_T_TWOFIELD", "a b".to_string());
        assert_eq!(expand(&Word(vec![WordPart::Var {
            name: "SHUCK_T_TWOFIELD".to_string(),
            quoted: false,
        }]), &shell).len(), 2);
    }
}
