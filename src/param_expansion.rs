//! Parameter-expansion modifier evaluation (`${var:-w}`, `${#var}`, etc.).

use crate::lexer::{ParamModifier, Word};
use crate::shell_state::Shell;

#[derive(Debug, PartialEq, Eq)]
pub enum ExpansionResult {
    Value(String),
    Empty,
}

pub fn expand_modifier(
    name: &str,
    modifier: &ParamModifier,
    shell: &mut Shell,
) -> ExpansionResult {
    match modifier {
        ParamModifier::Length => {
            let v = shell.get(name).unwrap_or("");
            ExpansionResult::Value(v.chars().count().to_string())
        }
        ParamModifier::UseDefault { .. }
        | ParamModifier::AssignDefault { .. }
        | ParamModifier::ErrorIfUnset { .. }
        | ParamModifier::UseAlternate { .. } => {
            unreachable!("default-value family lands in Task 3");
        }
        ParamModifier::RemovePrefix { .. } | ParamModifier::RemoveSuffix { .. } => {
            unreachable!("prefix/suffix removal lands in Task 4");
        }
    }
}

pub(crate) fn condition_is_null(raw: Option<&str>, colon: bool) -> bool {
    match (raw, colon) {
        (None, _) => true,
        (Some(""), true) => true,
        (Some(_), _) => false,
    }
}

pub(crate) fn expand_word_to_string(word: &Word, shell: &mut Shell) -> String {
    crate::expand::expand_assignment(word, shell)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_of_unset_is_zero() {
        let mut shell = Shell::new();
        let r = expand_modifier("HUCK_TEST_PE_UNSET", &ParamModifier::Length, &mut shell);
        assert_eq!(r, ExpansionResult::Value("0".to_string()));
    }

    #[test]
    fn length_of_empty_is_zero() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_EMPTY", "".to_string());
        let r = expand_modifier("HUCK_TEST_PE_EMPTY", &ParamModifier::Length, &mut shell);
        assert_eq!(r, ExpansionResult::Value("0".to_string()));
    }

    #[test]
    fn length_of_set_value_is_char_count() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_LEN", "hello".to_string());
        let r = expand_modifier("HUCK_TEST_PE_LEN", &ParamModifier::Length, &mut shell);
        assert_eq!(r, ExpansionResult::Value("5".to_string()));
    }

    #[test]
    fn length_counts_unicode_chars_not_bytes() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_UNI", "é".to_string());
        let r = expand_modifier("HUCK_TEST_PE_UNI", &ParamModifier::Length, &mut shell);
        assert_eq!(r, ExpansionResult::Value("1".to_string()));
    }

    #[test]
    fn condition_is_null_table() {
        assert_eq!(condition_is_null(None, false), true);
        assert_eq!(condition_is_null(None, true), true);
        assert_eq!(condition_is_null(Some(""), false), false);
        assert_eq!(condition_is_null(Some(""), true), true);
        assert_eq!(condition_is_null(Some("x"), false), false);
        assert_eq!(condition_is_null(Some("x"), true), false);
    }
}
