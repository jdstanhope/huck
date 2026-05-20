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
        ParamModifier::UseDefault { word, colon } => {
            let raw = shell.get(name).map(|s| s.to_string());
            if condition_is_null(raw.as_deref(), *colon) {
                ExpansionResult::Value(expand_word_to_string(word, shell))
            } else {
                ExpansionResult::Value(raw.unwrap_or_default())
            }
        }
        ParamModifier::AssignDefault { word, colon } => {
            let raw = shell.get(name).map(|s| s.to_string());
            if condition_is_null(raw.as_deref(), *colon) {
                let v = expand_word_to_string(word, shell);
                shell.set(name, v.clone());
                ExpansionResult::Value(v)
            } else {
                ExpansionResult::Value(raw.unwrap_or_default())
            }
        }
        ParamModifier::ErrorIfUnset { word, colon } => {
            let raw = shell.get(name).map(|s| s.to_string());
            if condition_is_null(raw.as_deref(), *colon) {
                let msg = expand_word_to_string(word, shell);
                if msg.is_empty() {
                    let default = if *colon {
                        "parameter null or not set"
                    } else {
                        "parameter not set"
                    };
                    eprintln!("huck: {}: {}", name, default);
                } else {
                    eprintln!("huck: {}: {}", name, msg);
                }
                shell.set_last_status(1);
                ExpansionResult::Empty
            } else {
                ExpansionResult::Value(raw.unwrap_or_default())
            }
        }
        ParamModifier::UseAlternate { word, colon } => {
            let raw = shell.get(name);
            if condition_is_null(raw, *colon) {
                ExpansionResult::Empty
            } else {
                ExpansionResult::Value(expand_word_to_string(word, shell))
            }
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

    use crate::lexer::{Word, WordPart};

    fn lit(s: &str) -> Word {
        Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }])
    }

    #[test]
    fn use_default_colon_unset_uses_default() {
        let mut shell = Shell::new();
        let m = ParamModifier::UseDefault { word: lit("default"), colon: true };
        let r = expand_modifier("HUCK_TEST_PE_UD1", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("default".to_string()));
    }

    #[test]
    fn use_default_colon_empty_uses_default() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_UD2", "".to_string());
        let m = ParamModifier::UseDefault { word: lit("default"), colon: true };
        let r = expand_modifier("HUCK_TEST_PE_UD2", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("default".to_string()));
    }

    #[test]
    fn use_default_no_colon_empty_returns_empty_value_not_default() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_UD3", "".to_string());
        let m = ParamModifier::UseDefault { word: lit("default"), colon: false };
        let r = expand_modifier("HUCK_TEST_PE_UD3", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("".to_string()));
    }

    #[test]
    fn use_default_set_nonempty_returns_value() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_UD4", "actual".to_string());
        let m = ParamModifier::UseDefault { word: lit("default"), colon: true };
        let r = expand_modifier("HUCK_TEST_PE_UD4", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("actual".to_string()));
    }

    #[test]
    fn assign_default_colon_unset_mutates_shell() {
        let mut shell = Shell::new();
        let m = ParamModifier::AssignDefault { word: lit("set!"), colon: true };
        let r = expand_modifier("HUCK_TEST_PE_AD1", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("set!".to_string()));
        assert_eq!(shell.get("HUCK_TEST_PE_AD1"), Some("set!"));
    }

    #[test]
    fn assign_default_already_set_does_not_mutate() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_AD2", "keep".to_string());
        let m = ParamModifier::AssignDefault { word: lit("override"), colon: true };
        let r = expand_modifier("HUCK_TEST_PE_AD2", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("keep".to_string()));
        assert_eq!(shell.get("HUCK_TEST_PE_AD2"), Some("keep"));
    }

    #[test]
    fn error_if_unset_colon_null_returns_empty_and_sets_status() {
        let mut shell = Shell::new();
        let m = ParamModifier::ErrorIfUnset { word: lit("msg"), colon: true };
        let r = expand_modifier("HUCK_TEST_PE_EU1", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Empty);
        assert_eq!(shell.last_status(), 1);
    }

    #[test]
    fn error_if_unset_set_returns_value_no_status_change() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_EU2", "ok".to_string());
        let m = ParamModifier::ErrorIfUnset { word: lit("msg"), colon: true };
        let r = expand_modifier("HUCK_TEST_PE_EU2", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("ok".to_string()));
        assert_eq!(shell.last_status(), 0);
    }

    #[test]
    fn use_alternate_set_returns_alternate() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_UA1", "anything".to_string());
        let m = ParamModifier::UseAlternate { word: lit("alt"), colon: true };
        let r = expand_modifier("HUCK_TEST_PE_UA1", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("alt".to_string()));
    }

    #[test]
    fn use_alternate_unset_returns_empty() {
        let mut shell = Shell::new();
        let m = ParamModifier::UseAlternate { word: lit("alt"), colon: true };
        let r = expand_modifier("HUCK_TEST_PE_UA2", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Empty);
    }

    #[test]
    fn use_alternate_colon_empty_returns_empty() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_UA3", "".to_string());
        let m = ParamModifier::UseAlternate { word: lit("alt"), colon: true };
        let r = expand_modifier("HUCK_TEST_PE_UA3", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Empty);
    }

    #[test]
    fn use_alternate_no_colon_empty_returns_alternate() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_UA4", "".to_string());
        let m = ParamModifier::UseAlternate { word: lit("alt"), colon: false };
        let r = expand_modifier("HUCK_TEST_PE_UA4", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("alt".to_string()));
    }
}
