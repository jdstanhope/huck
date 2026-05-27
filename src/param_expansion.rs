//! Parameter-expansion modifier evaluation (`${var:-w}`, `${#var}`, etc.).

use crate::lexer::{ParamModifier, SubstAnchor, Word};
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
        ParamModifier::RemovePrefix { pattern, longest } => {
            let v = shell.get(name).unwrap_or("").to_string();
            let p = expand_word_to_string(pattern, shell);
            ExpansionResult::Value(remove_prefix(&v, &p, *longest))
        }
        ParamModifier::RemoveSuffix { pattern, longest } => {
            let v = shell.get(name).unwrap_or("").to_string();
            let p = expand_word_to_string(pattern, shell);
            ExpansionResult::Value(remove_suffix(&v, &p, *longest))
        }
        ParamModifier::Substitute { pattern, replacement, anchor, all } => {
            let v = shell.get(name).unwrap_or("").to_string();
            let pat = expand_word_to_string(pattern, shell);
            let rep = expand_word_to_string(replacement, shell);
            ExpansionResult::Value(substitute(&v, &pat, &rep, *anchor, *all))
        }
        ParamModifier::Substring { .. } => {
            // Filled in by Task 4.
            ExpansionResult::Value(shell.lookup_var(name).unwrap_or_default())
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

fn remove_prefix(value: &str, pattern: &str, longest: bool) -> String {
    let opts = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: false,
        require_literal_leading_dot: false,
    };
    let pat = match glob::Pattern::new(pattern) {
        Ok(p) => p,
        Err(_) => return value.to_string(),
    };
    let mut boundaries: Vec<usize> = value.char_indices().map(|(i, _)| i).collect();
    boundaries.push(value.len());

    if longest {
        for &end in boundaries.iter().rev() {
            if pat.matches_with(&value[..end], opts) {
                return value[end..].to_string();
            }
        }
    } else {
        for &end in &boundaries {
            if pat.matches_with(&value[..end], opts) {
                return value[end..].to_string();
            }
        }
    }
    value.to_string()
}

fn remove_suffix(value: &str, pattern: &str, longest: bool) -> String {
    let opts = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: false,
        require_literal_leading_dot: false,
    };
    let pat = match glob::Pattern::new(pattern) {
        Ok(p) => p,
        Err(_) => return value.to_string(),
    };
    let mut boundaries: Vec<usize> = value.char_indices().map(|(i, _)| i).collect();
    boundaries.push(value.len());

    if longest {
        for &start in &boundaries {
            if pat.matches_with(&value[start..], opts) {
                return value[..start].to_string();
            }
        }
    } else {
        for &start in boundaries.iter().rev() {
            if pat.matches_with(&value[start..], opts) {
                return value[..start].to_string();
            }
        }
    }
    value.to_string()
}

fn substitute(
    value: &str,
    pattern: &str,
    replacement: &str,
    anchor: SubstAnchor,
    all: bool,
) -> String {
    // Bash treats an empty pattern as a no-op (`${var//}` → `$var`).
    if pattern.is_empty() {
        return value.to_string();
    }
    let opts = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: false,
        require_literal_leading_dot: false,
    };
    let pat = match glob::Pattern::new(pattern) {
        Ok(p) => p,
        Err(_) => return value.to_string(),
    };
    let mut boundaries: Vec<usize> = value.char_indices().map(|(i, _)| i).collect();
    boundaries.push(value.len());

    // Longest match at `start`: largest `end` (from boundaries) > start
    // such that value[start..end] matches. Returns None if no end works.
    // For empty-pattern callers this can return Some(start) (empty match).
    let longest_match_at = |start: usize| -> Option<usize> {
        // `boundaries` is ascending, so iter().rev() yields descending —
        // once we drop below `start`, every remaining entry is also below.
        for &end in boundaries.iter().rev() {
            if end < start { break; }
            if pat.matches_with(&value[start..end], opts) {
                return Some(end);
            }
        }
        None
    };

    match anchor {
        SubstAnchor::Prefix => {
            // Only try at index 0; longest match wins.
            if let Some(end) = longest_match_at(0) {
                let mut out = String::with_capacity(replacement.len() + value.len() - end);
                out.push_str(replacement);
                out.push_str(&value[end..]);
                out
            } else {
                value.to_string()
            }
        }
        SubstAnchor::Suffix => {
            // Smallest start such that value[start..] matches → longest
            // suffix match.
            for &start in &boundaries {
                if pat.matches_with(&value[start..], opts) {
                    let mut out = String::with_capacity(start + replacement.len());
                    out.push_str(&value[..start]);
                    out.push_str(replacement);
                    return out;
                }
            }
            value.to_string()
        }
        SubstAnchor::None => {
            let mut out = String::new();
            let mut cursor = 0;
            let mut bi = 0; // index into boundaries
            while bi < boundaries.len() {
                let start = boundaries[bi];
                if start < cursor {
                    bi += 1;
                    continue;
                }
                if let Some(end) = longest_match_at(start) {
                    if end == start && start == value.len() {
                        // Trailing empty match (e.g. `*` against the slot
                        // after the last char). Nothing left to substitute;
                        // matching bash, no extra replacement is emitted.
                        break;
                    }
                    out.push_str(&value[cursor..start]);
                    out.push_str(replacement);
                    if end == start {
                        // Empty match mid-string: advance one char so we
                        // don't re-enter at the same position.
                        let next = boundaries.iter().copied().find(|&b| b > start).unwrap_or(value.len());
                        out.push_str(&value[start..next]);
                        cursor = next;
                        bi += 1;
                    } else {
                        cursor = end;
                    }
                    if !all {
                        out.push_str(&value[cursor..]);
                        return out;
                    }
                } else {
                    bi += 1;
                }
            }
            out.push_str(&value[cursor..]);
            out
        }
    }
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
        assert!(condition_is_null(None, false));
        assert!(condition_is_null(None, true));
        assert!(!condition_is_null(Some(""), false));
        assert!(condition_is_null(Some(""), true));
        assert!(!condition_is_null(Some("x"), false));
        assert!(!condition_is_null(Some("x"), true));
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
    fn error_if_unset_empty_operand_uses_default_message() {
        // ${X:?} with no operand word — should still error and set status.
        let mut shell = Shell::new();
        let m = ParamModifier::ErrorIfUnset { word: Word(vec![]), colon: true };
        let r = expand_modifier("HUCK_TEST_PE_EU_EMPTY", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Empty);
        assert_eq!(shell.last_status(), 1);
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

    #[test]
    fn remove_prefix_shortest_match() {
        assert_eq!(remove_prefix("/path/to/file.txt", "*/", false), "path/to/file.txt");
    }

    #[test]
    fn remove_prefix_longest_match() {
        assert_eq!(remove_prefix("/path/to/file.txt", "*/", true), "file.txt");
    }

    #[test]
    fn remove_prefix_no_match_returns_value_unchanged() {
        assert_eq!(remove_prefix("hello", "world", false), "hello");
    }

    #[test]
    fn remove_prefix_empty_pattern_removes_nothing() {
        // The empty glob pattern matches the empty prefix; removing an
        // empty prefix leaves the value intact (matches bash `${var#}`).
        assert_eq!(remove_prefix("hello", "", false), "hello");
    }

    #[test]
    fn remove_prefix_invalid_glob_returns_value_unchanged() {
        assert_eq!(remove_prefix("hello", "[abc", false), "hello");
    }

    #[test]
    fn remove_prefix_literal_match() {
        assert_eq!(remove_prefix("hello world", "hello ", false), "world");
    }

    #[test]
    fn remove_prefix_glob_crosses_slash() {
        assert_eq!(remove_prefix("a/b/c", "*", true), "");
        assert_eq!(remove_prefix("a/b/c", "*/", true), "c");
    }

    #[test]
    fn remove_suffix_shortest_match() {
        assert_eq!(remove_suffix("file.tar.gz", ".*", false), "file.tar");
    }

    #[test]
    fn remove_suffix_longest_match() {
        assert_eq!(remove_suffix("file.tar.gz", ".*", true), "file");
    }

    #[test]
    fn remove_suffix_no_match() {
        assert_eq!(remove_suffix("hello", "world", false), "hello");
    }

    #[test]
    fn remove_suffix_handles_utf8_boundaries() {
        assert_eq!(remove_suffix("café.txt", ".txt", false), "café");
    }

    #[test]
    fn expand_modifier_remove_prefix_shortest() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_RP1", "/path/to/file.txt".to_string());
        let m = ParamModifier::RemovePrefix { pattern: lit("*/"), longest: false };
        let r = expand_modifier("HUCK_TEST_PE_RP1", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("path/to/file.txt".to_string()));
    }

    #[test]
    fn expand_modifier_remove_prefix_longest() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_RP2", "/path/to/file.txt".to_string());
        let m = ParamModifier::RemovePrefix { pattern: lit("*/"), longest: true };
        let r = expand_modifier("HUCK_TEST_PE_RP2", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("file.txt".to_string()));
    }

    #[test]
    fn expand_modifier_remove_suffix_longest() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_RS1", "file.tar.gz".to_string());
        let m = ParamModifier::RemoveSuffix { pattern: lit(".*"), longest: true };
        let r = expand_modifier("HUCK_TEST_PE_RS1", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("file".to_string()));
    }

    #[test]
    fn expand_modifier_remove_prefix_unset_returns_empty() {
        let mut shell = Shell::new();
        let m = ParamModifier::RemovePrefix { pattern: lit("*"), longest: true };
        let r = expand_modifier("HUCK_TEST_PE_UNSET_RP", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("".to_string()));
    }

    #[test]
    fn substitute_first_match_unanchored() {
        assert_eq!(substitute("foobar", "o", "X", SubstAnchor::None, false), "fXobar");
    }

    #[test]
    fn substitute_all_unanchored() {
        assert_eq!(substitute("foobar", "o", "X", SubstAnchor::None, true), "fXXbar");
    }

    #[test]
    fn substitute_first_unanchored_no_match_returns_value() {
        assert_eq!(substitute("foobar", "z", "X", SubstAnchor::None, false), "foobar");
    }

    #[test]
    fn substitute_all_with_empty_replacement_removes() {
        assert_eq!(substitute("aaa", "a", "", SubstAnchor::None, true), "");
    }

    #[test]
    fn substitute_anchored_prefix_hit() {
        assert_eq!(substitute("hello", "he", "HI", SubstAnchor::Prefix, false), "HIllo");
    }

    #[test]
    fn substitute_anchored_prefix_miss() {
        assert_eq!(substitute("hello", "xo", "HI", SubstAnchor::Prefix, false), "hello");
    }

    #[test]
    fn substitute_anchored_suffix_hit() {
        assert_eq!(substitute("hello", "lo", "LO", SubstAnchor::Suffix, false), "helLO");
    }

    #[test]
    fn substitute_anchored_suffix_miss() {
        assert_eq!(substitute("hello", "xo", "LO", SubstAnchor::Suffix, false), "hello");
    }

    #[test]
    fn substitute_glob_star_longest_match() {
        // `*` matches the whole tail at i=0; with all=true, the second pass
        // starts past the replacement and finds nothing more.
        assert_eq!(substitute("xyz", "*", "Q", SubstAnchor::None, true), "Q");
    }

    #[test]
    fn substitute_glob_question_mark() {
        assert_eq!(substitute("abc", "?", "X", SubstAnchor::None, false), "Xbc");
        assert_eq!(substitute("abc", "?", "X", SubstAnchor::None, true), "XXX");
    }

    #[test]
    fn substitute_unicode_boundaries() {
        assert_eq!(substitute("café", "é", "E", SubstAnchor::None, false), "cafE");
    }

    #[test]
    fn substitute_invalid_glob_returns_value_unchanged() {
        assert_eq!(substitute("hello", "[abc", "X", SubstAnchor::None, false), "hello");
    }

    #[test]
    fn substitute_empty_value_returns_empty() {
        assert_eq!(substitute("", "foo", "bar", SubstAnchor::None, true), "");
    }

    #[test]
    fn substitute_empty_pattern_is_noop_first() {
        // Bash: empty pattern is a no-op for both /first and //all.
        assert_eq!(substitute("abc", "", "X", SubstAnchor::None, false), "abc");
    }

    #[test]
    fn substitute_empty_pattern_is_noop_all() {
        assert_eq!(substitute("abc", "", "X", SubstAnchor::None, true), "abc");
    }

    #[test]
    fn substitute_glob_star_all_replaces_once_no_trailing_empty_match() {
        // `*` matches the whole string at i=0; after the replacement,
        // the empty-match guard must not emit a second replacement at
        // the trailing position.
        assert_eq!(substitute("xyz", "*", "Q", SubstAnchor::None, true), "Q");
    }

    #[test]
    fn substitute_glob_star_with_prefix_match_advances_past_match() {
        // `f*` against "foo bar foo" — greedy, all-mode still only one
        // replacement (matches whole tail from first `f`).
        assert_eq!(substitute("foo bar foo", "f*", "X", SubstAnchor::None, true), "X");
    }

    #[test]
    fn expand_modifier_substitute_first_match() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_SU1", "foobar".to_string());
        let m = ParamModifier::Substitute {
            pattern: lit("o"),
            replacement: lit("X"),
            anchor: SubstAnchor::None,
            all: false,
        };
        let r = expand_modifier("HUCK_TEST_PE_SU1", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("fXobar".to_string()));
    }

    #[test]
    fn expand_modifier_substitute_all() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_SU2", "foobar".to_string());
        let m = ParamModifier::Substitute {
            pattern: lit("o"),
            replacement: lit("X"),
            anchor: SubstAnchor::None,
            all: true,
        };
        let r = expand_modifier("HUCK_TEST_PE_SU2", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("fXXbar".to_string()));
    }

    #[test]
    fn expand_modifier_substitute_unset_var_returns_empty() {
        let mut shell = Shell::new();
        let m = ParamModifier::Substitute {
            pattern: lit("o"),
            replacement: lit("X"),
            anchor: SubstAnchor::None,
            all: false,
        };
        let r = expand_modifier("HUCK_TEST_PE_SU_UNSET", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("".to_string()));
    }

    #[test]
    fn expand_modifier_substitute_anchored_prefix() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_SU3", "hello".to_string());
        let m = ParamModifier::Substitute {
            pattern: lit("he"),
            replacement: lit("HI"),
            anchor: SubstAnchor::Prefix,
            all: false,
        };
        let r = expand_modifier("HUCK_TEST_PE_SU3", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("HIllo".to_string()));
    }
}
