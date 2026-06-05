//! Parameter-expansion modifier evaluation (`${var:-w}`, `${#var}`, etc.).

use crate::lexer::{CaseDirection, ParamModifier, SubstAnchor, Word};
use crate::shell_state::Shell;

#[derive(Debug, PartialEq, Eq)]
pub enum ExpansionResult {
    Value(String),
    Empty,
    /// Fatal parameter-expansion error: the caller must abort the
    /// surrounding simple command and (in non-interactive mode) exit
    /// the shell. The message has already been printed by the arm that
    /// produced this; `status` is the exit code.
    Fatal { status: i32 },
    /// Word-list result for array-aware forms (`${a[@]}`, `${!a[@]}`,
    /// `${a[@]:o:l}`, and `${@:o:l}` / `${*:o:l}`). Consumers in the
    /// expansion pipeline decide how to materialise this: in a quoted
    /// `@`-style context, each word becomes its own field; in an
    /// unquoted context it's joined and word-split.
    WordList(Vec<String>),
}

pub fn expand_modifier(
    name: &str,
    modifier: &ParamModifier,
    shell: &mut Shell,
) -> ExpansionResult {
    expand_modifier_with_value(name, modifier, ParamLookup::Scalar, shell)
}

/// Source of the parameter value for `expand_modifier_with_value`.
/// Distinguishes between "scalar lookup" (consult `shell.get(name)`) and
/// "explicit array element" (the caller already resolved the element
/// and we must NOT fall back to the scalar view). The array-element
/// case carries an `Option`: `Some(v)` is "element exists with this
/// value" and `None` is "element is missing" (truly unset for modifier
/// purposes — `${a[i]:-W}` and `${a[i]-W}` both substitute the default).
#[derive(Debug, Clone, Copy)]
pub enum ParamLookup<'a> {
    Scalar,
    Element(Option<&'a str>),
}

/// Same as `expand_modifier`, except the caller may supply a
/// `ParamLookup::Element(...)` to evaluate the modifier against a
/// specific array element rather than the scalar view of `name`. Used
/// by the array-element path (`${a[i]:-default}` etc.) — `Element(None)`
/// represents a missing key, so `${a[unset]:-W}` correctly substitutes
/// the default instead of falling through to `a[0]`'s value (the bug
/// pattern logged on M-82 during v72 and fixed here).
pub fn expand_modifier_with_value(
    name: &str,
    modifier: &ParamModifier,
    source: ParamLookup,
    shell: &mut Shell,
) -> ExpansionResult {
    if shell.pending_fatal_pe_error.is_some() {
        return ExpansionResult::Empty;
    }
    // `get_raw` returns the value to test against null/unset. For
    // Scalar lookup it consults `shell.lookup_var(name)` (so positional
    // and special params resolve too); for Element it uses the
    // caller-supplied value verbatim (Some=set, None=unset).
    let get_raw = |sh: &Shell| -> Option<String> {
        match source {
            // `lookup_var` (not `get`) so positional (`$1`) and special
            // params resolve here too — `get` consults only named vars,
            // which silently dropped e.g. `${1#-a}` to empty (v93 fix).
            ParamLookup::Scalar => sh.lookup_var(name),
            ParamLookup::Element(Some(s)) => Some(s.to_string()),
            ParamLookup::Element(None) => None,
        }
    };
    let lookup_v = |sh: &Shell| -> String {
        match source {
            ParamLookup::Scalar => sh.lookup_var(name).unwrap_or_default(),
            ParamLookup::Element(Some(s)) => s.to_string(),
            ParamLookup::Element(None) => String::new(),
        }
    };
    match modifier {
        ParamModifier::None => {
            ExpansionResult::Value(get_raw(shell).unwrap_or_default())
        }
        ParamModifier::Length => {
            let n = match (source, name) {
                (ParamLookup::Scalar, "@") | (ParamLookup::Scalar, "*") => {
                    shell.positional_args.len()
                }
                _ => lookup_v(shell).chars().count(),
            };
            ExpansionResult::Value(n.to_string())
        }
        ParamModifier::IndirectKeys => {
            // The scalar (no-subscript) path is rejected at the lexer
            // (a bare `${!NAME}` returns InvalidBraceModifier). This
            // arm is reached only via the array dispatcher's fall-
            // through; emit empty so it stays a no-op.
            ExpansionResult::Value(String::new())
        }
        ParamModifier::UseDefault { word, colon } => {
            let raw = get_raw(shell);
            if condition_is_null(raw.as_deref(), *colon) {
                ExpansionResult::Value(expand_word_to_string(word, shell))
            } else {
                ExpansionResult::Value(raw.unwrap_or_default())
            }
        }
        ParamModifier::AssignDefault { word, colon } => {
            let raw = get_raw(shell);
            if condition_is_null(raw.as_deref(), *colon) {
                let v = expand_word_to_string(word, shell);
                // When operating on an array element, we do NOT mutate
                // the array via `try_set` (that would write the scalar
                // path). The caller (`expand_array_param`) handles
                // any element-write semantics; here we just return the
                // default value. For the scalar path behave as before.
                if matches!(source, ParamLookup::Scalar)
                    && shell.try_set(name, v.clone()).is_err()
                {
                    eprintln!("huck: {name}: readonly variable");
                    return ExpansionResult::Fatal { status: 1 };
                }
                ExpansionResult::Value(v)
            } else {
                ExpansionResult::Value(raw.unwrap_or_default())
            }
        }
        ParamModifier::ErrorIfUnset { word, colon } => {
            let raw = get_raw(shell);
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
                ExpansionResult::Fatal { status: 1 }
            } else {
                ExpansionResult::Value(raw.unwrap_or_default())
            }
        }
        ParamModifier::UseAlternate { word, colon } => {
            let raw = get_raw(shell);
            if condition_is_null(raw.as_deref(), *colon) {
                ExpansionResult::Empty
            } else {
                ExpansionResult::Value(expand_word_to_string(word, shell))
            }
        }
        ParamModifier::RemovePrefix { pattern, longest } => {
            let v = get_raw(shell).unwrap_or_default();
            let p = expand_word_to_string(pattern, shell);
            let extglob = shell.shopt_options.get("extglob").unwrap_or(false);
            ExpansionResult::Value(remove_prefix(&v, &p, *longest, extglob))
        }
        ParamModifier::RemoveSuffix { pattern, longest } => {
            let v = get_raw(shell).unwrap_or_default();
            let p = expand_word_to_string(pattern, shell);
            let extglob = shell.shopt_options.get("extglob").unwrap_or(false);
            ExpansionResult::Value(remove_suffix(&v, &p, *longest, extglob))
        }
        ParamModifier::Substitute { pattern, replacement, anchor, all } => {
            let v = get_raw(shell).unwrap_or_default();
            let pat = expand_word_to_string(pattern, shell);
            let rep = expand_word_to_string(replacement, shell);
            let extglob = shell.shopt_options.get("extglob").unwrap_or(false);
            ExpansionResult::Value(substitute(&v, &pat, &rep, *anchor, *all, extglob))
        }
        ParamModifier::Substring { offset, length } => {
            let value = lookup_v(shell);
            let off_n = match eval_arith_word(offset, shell) {
                Ok(n) => n,
                Err(()) => return ExpansionResult::Empty,
            };
            let len_n = match length {
                Some(w) => match eval_arith_word(w, shell) {
                    Ok(n) => Some(n),
                    Err(()) => return ExpansionResult::Empty,
                },
                None => None,
            };
            match substring(&value, off_n, len_n) {
                Ok(s) => ExpansionResult::Value(s),
                Err(msg) => {
                    eprintln!("huck: {}: {}", name, msg);
                    ExpansionResult::Fatal { status: 1 }
                }
            }
        }
        ParamModifier::Case { direction, all, pattern } => {
            let v = lookup_v(shell);
            let pat_string = pattern.as_ref().map(|w| expand_word_to_string(w, shell));
            let extglob = shell.shopt_options.get("extglob").unwrap_or(false);
            ExpansionResult::Value(case_modify(&v, *direction, *all, pat_string.as_deref(), extglob))
        }
        ParamModifier::Transform { op } => {
            let v = lookup_v(shell);
            let out = match op {
                crate::lexer::TransformOp::Upper => {
                    case_modify(&v, CaseDirection::Upper, true, None, false)
                }
                crate::lexer::TransformOp::Lower => {
                    case_modify(&v, CaseDirection::Lower, true, None, false)
                }
                crate::lexer::TransformOp::UpperFirst => {
                    case_modify(&v, CaseDirection::Upper, false, None, false)
                }
                crate::lexer::TransformOp::Quote => match get_raw(shell) {
                    // bash: `@Q` on a genuinely unset variable yields an
                    // empty string (no quotes); set-but-empty yields `''`.
                    // `get_raw` returns `None` for unset (Scalar via
                    // `lookup_var`, or `Element(None)` for a missing
                    // subscript), `Some` for set (incl. empty), so this
                    // also keeps `${a[1]@Q}` quoting correctly.
                    None => String::new(),
                    Some(val) => shell_quote(&val),
                },
                crate::lexer::TransformOp::EscapeExpand => {
                    crate::lexer::decode_ansi_c_escapes(&v)
                }
                crate::lexer::TransformOp::PromptExpand => {
                    crate::prompt::expand_prompt(&v, shell)
                }
            };
            ExpansionResult::Value(out)
        }
    }
}

/// bash `${v@Q}`: shell-quote `v` so the result re-reads as the same
/// value. If `v` contains a control char (`\0`..`\x1F` or `\x7F`) it uses
/// the `$'…'` ANSI-C form (escaping `\`, `'`, and control chars); empty
/// and ordinary strings use single quotes with `'` rewritten as `'\''`.
fn shell_quote(v: &str) -> String {
    if v.chars().any(|c| c.is_control()) {
        let mut out = String::from("$'");
        for c in v.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '\'' => out.push_str("\\'"),
                '\x07' => out.push_str("\\a"),
                '\x08' => out.push_str("\\b"),
                '\t' => out.push_str("\\t"),
                '\n' => out.push_str("\\n"),
                '\x0B' => out.push_str("\\v"),
                '\x0C' => out.push_str("\\f"),
                '\r' => out.push_str("\\r"),
                '\x1B' => out.push_str("\\E"),
                c if (c as u32) < 0x20 || c == '\x7F' => {
                    out.push_str(&format!("\\{:03o}", c as u32));
                }
                c => out.push(c),
            }
        }
        out.push('\'');
        out
    } else {
        format!("'{}'", crate::builtins::escape_alias_value(v))
    }
}

/// Expands `word` to a string (no field-splitting), parses it as
/// arithmetic, evaluates it. On any error, prints `huck: arithmetic: <msg>`
/// and sets `$? = 1`, returning `Err(())`.
fn eval_arith_word(word: &Word, shell: &mut Shell) -> Result<i64, ()> {
    let s = crate::expand::expand_assignment(word, shell);
    let expr = match crate::arith::parse(&s) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("huck: arithmetic: {}", e);
            shell.set_last_status(1);
            return Err(());
        }
    };
    match crate::arith::eval(&expr, shell) {
        Ok(n) => Ok(n),
        Err(e) => {
            eprintln!("huck: arithmetic: {}", e);
            shell.set_last_status(1);
            Err(())
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

/// Pattern match for parameter expansion: extglob engine when enabled+applicable,
/// else the `glob` crate (preserving current behavior). `case_sensitive` mirrors
/// the existing `MatchOptions`.
fn pe_pattern_matches(pattern: &str, text: &str, extglob: bool, case_sensitive: bool) -> bool {
    if extglob && crate::glob_match::has_extglob(pattern) {
        crate::glob_match::extglob_match(pattern, text, !case_sensitive)
    } else {
        match glob::Pattern::new(pattern) {
            Ok(p) => p.matches_with(
                text,
                glob::MatchOptions {
                    case_sensitive,
                    require_literal_separator: false,
                    require_literal_leading_dot: false,
                },
            ),
            Err(_) => false,
        }
    }
}

fn remove_prefix(value: &str, pattern: &str, longest: bool, extglob: bool) -> String {
    // `${}` pattern stripping is always case-sensitive (bash `nocasematch`
    // does not affect parameter expansion).
    if glob::Pattern::new(pattern).is_err() && !(extglob && crate::glob_match::has_extglob(pattern))
    {
        return value.to_string();
    }
    let mut boundaries: Vec<usize> = value.char_indices().map(|(i, _)| i).collect();
    boundaries.push(value.len());

    if longest {
        for &end in boundaries.iter().rev() {
            if pe_pattern_matches(pattern, &value[..end], extglob, true) {
                return value[end..].to_string();
            }
        }
    } else {
        for &end in &boundaries {
            if pe_pattern_matches(pattern, &value[..end], extglob, true) {
                return value[end..].to_string();
            }
        }
    }
    value.to_string()
}

fn remove_suffix(value: &str, pattern: &str, longest: bool, extglob: bool) -> String {
    if glob::Pattern::new(pattern).is_err() && !(extglob && crate::glob_match::has_extglob(pattern))
    {
        return value.to_string();
    }
    let mut boundaries: Vec<usize> = value.char_indices().map(|(i, _)| i).collect();
    boundaries.push(value.len());

    if longest {
        for &start in &boundaries {
            if pe_pattern_matches(pattern, &value[start..], extglob, true) {
                return value[..start].to_string();
            }
        }
    } else {
        for &start in boundaries.iter().rev() {
            if pe_pattern_matches(pattern, &value[start..], extglob, true) {
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
    extglob: bool,
) -> String {
    // Bash treats an empty pattern as a no-op (`${var//}` → `$var`).
    if pattern.is_empty() {
        return value.to_string();
    }
    if glob::Pattern::new(pattern).is_err() && !(extglob && crate::glob_match::has_extglob(pattern))
    {
        return value.to_string();
    }
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
            if pe_pattern_matches(pattern, &value[start..end], extglob, true) {
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
                if pe_pattern_matches(pattern, &value[start..], extglob, true) {
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

/// Applies bash-style case modification to `value`. The `direction`
/// (Upper/Lower) and `all` flag together determine whether every char
/// or only the first matching char gets converted. `pattern` filters
/// per-character — if `None`, every char matches; if `Some(p)`, only
/// chars matching the glob `p` get converted. Glob compile errors
/// return `value` unchanged (silent fallthrough, matches v32's
/// `substitute`). Unicode-aware via Rust's `char::to_uppercase` /
/// `char::to_lowercase` iterators — handles multi-char expansions
/// like `'ß'.to_uppercase()` → "SS".
fn case_modify(
    value: &str,
    direction: CaseDirection,
    all: bool,
    pattern: Option<&str>,
    extglob: bool,
) -> String {
    // Validate the pattern, if any. On a glob compile failure that is not a
    // valid extglob pattern, return value unchanged (matches v32 substitute's
    // silent-no-op convention).
    if pattern
        .is_some_and(|p| glob::Pattern::new(p).is_err() && !(extglob && crate::glob_match::has_extglob(p)))
    {
        return value.to_string();
    }

    let should_modify = |c: char| -> bool {
        match pattern {
            None => true,
            Some(p) => pe_pattern_matches(p, &c.to_string(), extglob, true),
        }
    };

    let apply = |c: char| -> String {
        match direction {
            CaseDirection::Upper => c.to_uppercase().collect(),
            CaseDirection::Lower => c.to_lowercase().collect(),
        }
    };

    let mut out = String::with_capacity(value.len());
    if all {
        for c in value.chars() {
            if should_modify(c) {
                out.push_str(&apply(c));
            } else {
                out.push(c);
            }
        }
    } else {
        let mut done = false;
        for c in value.chars() {
            if !done && should_modify(c) {
                out.push_str(&apply(c));
                done = true;
            } else {
                out.push(c);
            }
        }
    }
    out
}

/// Bash substring semantics for `${var:offset[:length]}`. Char-counting
/// throughout (Unicode codepoints), consistent with the existing `${#var}`
/// divergence (L-04). Returns `Err("substring expression < 0")` only when
/// a negative `length` produces a computed length < 0.
fn substring(value: &str, offset: i64, length: Option<i64>) -> Result<String, &'static str> {
    let chars: Vec<char> = value.chars().collect();
    let strlen = chars.len() as i64;

    let eff_off: i64 = if offset >= 0 {
        offset.min(strlen)
    } else {
        (strlen + offset).max(0)
    };

    let eff_len: i64 = match length {
        None => strlen - eff_off,
        Some(n) if n >= 0 => n.min(strlen - eff_off),
        Some(n) => {
            // n < 0: count from end of string.
            let computed = strlen + n - eff_off;
            if computed < 0 {
                return Err("substring expression < 0");
            }
            computed
        }
    };

    let start = eff_off as usize;
    let end = (eff_off + eff_len) as usize;
    Ok(chars[start..end].iter().collect())
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
    fn expand_modifier_length_at_returns_positional_count() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "bb".to_string(), "ccc".to_string()];
        let m = ParamModifier::Length;
        let r = expand_modifier("@", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("3".to_string()));
    }

    #[test]
    fn expand_modifier_length_star_returns_positional_count() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "bb".to_string()];
        let m = ParamModifier::Length;
        let r = expand_modifier("*", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("2".to_string()));
    }

    #[test]
    fn expand_modifier_length_positional_returns_char_count() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["hello".to_string()];
        let m = ParamModifier::Length;
        let r = expand_modifier("1", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("5".to_string()));
    }

    #[test]
    fn expand_modifier_length_unset_positional_returns_zero() {
        let mut shell = Shell::new();
        // positional_args is empty by default; ${#5} → 0.
        let m = ParamModifier::Length;
        let r = expand_modifier("5", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("0".to_string()));
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
        // v34: ErrorIfUnset now returns Fatal instead of Empty + $?=1.
        assert_eq!(r, ExpansionResult::Fatal { status: 1 });
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
        // v34: ErrorIfUnset now returns Fatal instead of Empty + $?=1.
        assert_eq!(r, ExpansionResult::Fatal { status: 1 });
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
        assert_eq!(remove_prefix("/path/to/file.txt", "*/", false, false), "path/to/file.txt");
    }

    #[test]
    fn remove_prefix_longest_match() {
        assert_eq!(remove_prefix("/path/to/file.txt", "*/", true, false), "file.txt");
    }

    #[test]
    fn remove_prefix_no_match_returns_value_unchanged() {
        assert_eq!(remove_prefix("hello", "world", false, false), "hello");
    }

    #[test]
    fn remove_prefix_empty_pattern_removes_nothing() {
        // The empty glob pattern matches the empty prefix; removing an
        // empty prefix leaves the value intact (matches bash `${var#}`).
        assert_eq!(remove_prefix("hello", "", false, false), "hello");
    }

    #[test]
    fn remove_prefix_invalid_glob_returns_value_unchanged() {
        assert_eq!(remove_prefix("hello", "[abc", false, false), "hello");
    }

    #[test]
    fn remove_prefix_literal_match() {
        assert_eq!(remove_prefix("hello world", "hello ", false, false), "world");
    }

    #[test]
    fn remove_prefix_glob_crosses_slash() {
        assert_eq!(remove_prefix("a/b/c", "*", true, false), "");
        assert_eq!(remove_prefix("a/b/c", "*/", true, false), "c");
    }

    #[test]
    fn remove_suffix_shortest_match() {
        assert_eq!(remove_suffix("file.tar.gz", ".*", false, false), "file.tar");
    }

    #[test]
    fn remove_suffix_longest_match() {
        assert_eq!(remove_suffix("file.tar.gz", ".*", true, false), "file");
    }

    #[test]
    fn remove_suffix_no_match() {
        assert_eq!(remove_suffix("hello", "world", false, false), "hello");
    }

    #[test]
    fn remove_suffix_handles_utf8_boundaries() {
        assert_eq!(remove_suffix("café.txt", ".txt", false, false), "café");
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
        assert_eq!(substitute("foobar", "o", "X", SubstAnchor::None, false, false), "fXobar");
    }

    #[test]
    fn substitute_all_unanchored() {
        assert_eq!(substitute("foobar", "o", "X", SubstAnchor::None, true, false), "fXXbar");
    }

    #[test]
    fn substitute_first_unanchored_no_match_returns_value() {
        assert_eq!(substitute("foobar", "z", "X", SubstAnchor::None, false, false), "foobar");
    }

    #[test]
    fn substitute_all_with_empty_replacement_removes() {
        assert_eq!(substitute("aaa", "a", "", SubstAnchor::None, true, false), "");
    }

    #[test]
    fn substitute_anchored_prefix_hit() {
        assert_eq!(substitute("hello", "he", "HI", SubstAnchor::Prefix, false, false), "HIllo");
    }

    #[test]
    fn substitute_anchored_prefix_miss() {
        assert_eq!(substitute("hello", "xo", "HI", SubstAnchor::Prefix, false, false), "hello");
    }

    #[test]
    fn substitute_anchored_suffix_hit() {
        assert_eq!(substitute("hello", "lo", "LO", SubstAnchor::Suffix, false, false), "helLO");
    }

    #[test]
    fn substitute_anchored_suffix_miss() {
        assert_eq!(substitute("hello", "xo", "LO", SubstAnchor::Suffix, false, false), "hello");
    }

    #[test]
    fn substitute_glob_star_longest_match() {
        // `*` matches the whole tail at i=0; with all=true, the second pass
        // starts past the replacement and finds nothing more.
        assert_eq!(substitute("xyz", "*", "Q", SubstAnchor::None, true, false), "Q");
    }

    #[test]
    fn substitute_glob_question_mark() {
        assert_eq!(substitute("abc", "?", "X", SubstAnchor::None, false, false), "Xbc");
        assert_eq!(substitute("abc", "?", "X", SubstAnchor::None, true, false), "XXX");
    }

    #[test]
    fn substitute_unicode_boundaries() {
        assert_eq!(substitute("café", "é", "E", SubstAnchor::None, false, false), "cafE");
    }

    #[test]
    fn substitute_invalid_glob_returns_value_unchanged() {
        assert_eq!(substitute("hello", "[abc", "X", SubstAnchor::None, false, false), "hello");
    }

    #[test]
    fn substitute_empty_value_returns_empty() {
        assert_eq!(substitute("", "foo", "bar", SubstAnchor::None, true, false), "");
    }

    #[test]
    fn substitute_empty_pattern_is_noop_first() {
        // Bash: empty pattern is a no-op for both /first and //all.
        assert_eq!(substitute("abc", "", "X", SubstAnchor::None, false, false), "abc");
    }

    #[test]
    fn substitute_empty_pattern_is_noop_all() {
        assert_eq!(substitute("abc", "", "X", SubstAnchor::None, true, false), "abc");
    }

    #[test]
    fn substitute_glob_star_all_replaces_once_no_trailing_empty_match() {
        // `*` matches the whole string at i=0; after the replacement,
        // the empty-match guard must not emit a second replacement at
        // the trailing position.
        assert_eq!(substitute("xyz", "*", "Q", SubstAnchor::None, true, false), "Q");
    }

    #[test]
    fn substitute_glob_star_with_prefix_match_advances_past_match() {
        // `f*` against "foo bar foo" — greedy, all-mode still only one
        // replacement (matches whole tail from first `f`).
        assert_eq!(substitute("foo bar foo", "f*", "X", SubstAnchor::None, true, false), "X");
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

    #[test]
    fn substring_no_length_full() {
        assert_eq!(substring("abc", 0, None), Ok("abc".to_string()));
    }

    #[test]
    fn substring_no_length_offset_one() {
        assert_eq!(substring("abc", 1, None), Ok("bc".to_string()));
    }

    #[test]
    fn substring_offset_equals_strlen_is_empty() {
        assert_eq!(substring("abc", 3, None), Ok("".to_string()));
    }

    #[test]
    fn substring_offset_beyond_strlen_clamps_to_empty() {
        assert_eq!(substring("abc", 5, None), Ok("".to_string()));
    }

    #[test]
    fn substring_negative_offset_counts_from_end() {
        assert_eq!(substring("abc", -1, None), Ok("c".to_string()));
        assert_eq!(substring("abc", -3, None), Ok("abc".to_string()));
    }

    #[test]
    fn substring_negative_offset_beyond_start_clamps_to_zero() {
        // eff_off = max(3 + -5, 0) = 0; eff_len = strlen - 0 = 3.
        assert_eq!(substring("abc", -5, None), Ok("abc".to_string()));
    }

    #[test]
    fn substring_positive_length_clamps_to_remaining() {
        assert_eq!(substring("abc", 1, Some(5)), Ok("bc".to_string()));
    }

    #[test]
    fn substring_positive_length_within_range() {
        assert_eq!(substring("abcdef", 1, Some(3)), Ok("bcd".to_string()));
    }

    #[test]
    fn substring_negative_length_counts_from_end() {
        // eff_len = strlen + length - eff_off = 3 + -1 - 1 = 1.
        assert_eq!(substring("abc", 1, Some(-1)), Ok("b".to_string()));
    }

    #[test]
    fn substring_negative_length_yields_empty_when_zero() {
        // eff_len = 3 + -3 - 0 = 0.
        assert_eq!(substring("abc", 0, Some(-3)), Ok("".to_string()));
    }

    #[test]
    fn substring_negative_length_below_zero_is_error() {
        // eff_len = 3 + -4 - 0 = -1, below zero.
        assert_eq!(substring("abc", 0, Some(-4)), Err("substring expression < 0"));
    }

    #[test]
    fn substring_empty_value_returns_empty() {
        assert_eq!(substring("", 0, None), Ok("".to_string()));
        assert_eq!(substring("", 0, Some(3)), Ok("".to_string()));
    }

    #[test]
    fn substring_unicode_counts_codepoints_not_bytes() {
        // café: 4 codepoints, é is 2 bytes. Slice indices are by codepoint.
        assert_eq!(substring("café", 1, Some(2)), Ok("af".to_string()));
        assert_eq!(substring("café", 3, Some(1)), Ok("é".to_string()));
        assert_eq!(substring("café", -1, None), Ok("é".to_string()));
    }

    #[test]
    fn substring_zero_length_is_empty() {
        assert_eq!(substring("abc", 1, Some(0)), Ok("".to_string()));
    }

    #[test]
    fn expand_modifier_substring_scalar_var() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_SS1", "hello".to_string());
        let m = ParamModifier::Substring {
            offset: lit("1"),
            length: Some(lit("3")),
        };
        let r = expand_modifier("HUCK_TEST_PE_SS1", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("ell".to_string()));
    }

    #[test]
    fn expand_modifier_substring_no_length() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_SS2", "hello".to_string());
        let m = ParamModifier::Substring {
            offset: lit("2"),
            length: None,
        };
        let r = expand_modifier("HUCK_TEST_PE_SS2", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("llo".to_string()));
    }

    #[test]
    fn expand_modifier_substring_unset_var_returns_empty() {
        let mut shell = Shell::new();
        let m = ParamModifier::Substring {
            offset: lit("0"),
            length: Some(lit("3")),
        };
        let r = expand_modifier("HUCK_TEST_PE_SS_UNSET", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("".to_string()));
    }

    #[test]
    fn expand_modifier_substring_negative_offset() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_SS3", "hello".to_string());
        let m = ParamModifier::Substring {
            offset: lit("-2"),
            length: None,
        };
        let r = expand_modifier("HUCK_TEST_PE_SS3", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("lo".to_string()));
    }

    #[test]
    fn expand_modifier_substring_negative_length_below_zero_errors_and_empty() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_SS4", "abc".to_string());
        let m = ParamModifier::Substring {
            offset: lit("0"),
            length: Some(lit("-4")),
        };
        let r = expand_modifier("HUCK_TEST_PE_SS4", &m, &mut shell);
        // v34: Substring negative-length now returns Fatal instead of Empty + $?=1.
        assert_eq!(r, ExpansionResult::Fatal { status: 1 });
    }

    #[test]
    fn expand_modifier_substring_bad_arith_returns_empty_sets_status() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_SS5", "abc".to_string());
        let m = ParamModifier::Substring {
            offset: lit("@@@"), // not a valid arith expression
            length: None,
        };
        let r = expand_modifier("HUCK_TEST_PE_SS5", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Empty);
        assert_eq!(shell.last_status(), 1);
    }

    #[test]
    fn expand_modifier_substring_positional_lookup() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["hello".to_string()];
        let m = ParamModifier::Substring {
            offset: lit("0"),
            length: Some(lit("3")),
        };
        let r = expand_modifier("1", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("hel".to_string()));
    }

    #[test]
    fn expand_modifier_error_if_unset_returns_fatal() {
        let mut shell = Shell::new();
        let m = ParamModifier::ErrorIfUnset {
            word: lit("missing"),
            colon: true,
        };
        let r = expand_modifier("HUCK_TEST_PE_FATAL1", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Fatal { status: 1 });
    }

    #[test]
    fn expand_modifier_error_if_unset_with_message_returns_fatal_and_prints() {
        let mut shell = Shell::new();
        let m = ParamModifier::ErrorIfUnset {
            word: lit("custom message"),
            colon: false,
        };
        let r = expand_modifier("HUCK_TEST_PE_FATAL2", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Fatal { status: 1 });
        // We can't easily capture stderr here — the integration tests
        // in Task 5 verify the printed message. The unit test confirms
        // only the return shape.
    }

    #[test]
    fn expand_modifier_error_if_unset_when_set_returns_value() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_FATAL3", "set".to_string());
        let m = ParamModifier::ErrorIfUnset {
            word: lit("missing"),
            colon: true,
        };
        let r = expand_modifier("HUCK_TEST_PE_FATAL3", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("set".to_string()));
    }

    #[test]
    fn expand_modifier_substring_negative_computed_length_returns_fatal() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_FATAL4", "abc".to_string());
        let m = ParamModifier::Substring {
            offset: lit("0"),
            length: Some(lit("-4")),
        };
        let r = expand_modifier("HUCK_TEST_PE_FATAL4", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Fatal { status: 1 });
    }

    #[test]
    fn expand_modifier_substring_bad_arith_stays_empty_not_fatal() {
        // Regression guard: bad arith in offset stays non-fatal (matches
        // bash: arithmetic errors inside ${var:off:len} operands don't
        // exit the script).
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_FATAL5", "hello".to_string());
        let m = ParamModifier::Substring {
            offset: lit("@@@"),
            length: None,
        };
        let r = expand_modifier("HUCK_TEST_PE_FATAL5", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Empty);
        assert_eq!(shell.last_status(), 1);
        assert_eq!(shell.pending_fatal_pe_error, None);
    }

    #[test]
    fn expand_modifier_short_circuits_when_pending_fatal_is_set() {
        // Entry guard: if a previous expansion already set the fatal
        // flag, expand_modifier returns Empty immediately without doing
        // work — no eprintln, no side-effects.
        let mut shell = Shell::new();
        shell.pending_fatal_pe_error = Some(1);
        shell.export_set("HUCK_TEST_PE_FATAL6", "abc".to_string());
        let m = ParamModifier::Substring {
            offset: lit("0"),
            length: Some(lit("-4")), // would normally be fatal
        };
        let r = expand_modifier("HUCK_TEST_PE_FATAL6", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Empty);
        // The flag must remain set (not cleared by the guard).
        assert_eq!(shell.pending_fatal_pe_error, Some(1));
    }

    #[test]
    fn case_modify_upper_all_no_pattern() {
        assert_eq!(case_modify("hello", CaseDirection::Upper, true, None, false), "HELLO");
    }

    #[test]
    fn case_modify_upper_first_no_pattern() {
        assert_eq!(case_modify("hello", CaseDirection::Upper, false, None, false), "Hello");
    }

    #[test]
    fn case_modify_lower_all_no_pattern() {
        assert_eq!(case_modify("HELLO", CaseDirection::Lower, true, None, false), "hello");
    }

    #[test]
    fn case_modify_lower_first_no_pattern() {
        assert_eq!(case_modify("HELLO", CaseDirection::Lower, false, None, false), "hELLO");
    }

    #[test]
    fn case_modify_upper_all_with_pattern_filters_chars() {
        // [aeiou] — only vowels upper-cased.
        assert_eq!(case_modify("hello", CaseDirection::Upper, true, Some("[aeiou]"), false), "hEllO");
    }

    #[test]
    fn case_modify_upper_first_with_pattern_picks_first_match() {
        // Only the first MATCHING char (the `e`) gets upper-cased.
        assert_eq!(case_modify("hello", CaseDirection::Upper, false, Some("[aeiou]"), false), "hEllo");
    }

    #[test]
    fn case_modify_unicode_handles_multichar_uppercase() {
        // Rust's `'ß'.to_uppercase()` yields two chars: 'S', 'S'.
        assert_eq!(case_modify("straße", CaseDirection::Upper, true, None, false), "STRASSE");
    }

    #[test]
    fn case_modify_empty_value_returns_empty() {
        assert_eq!(case_modify("", CaseDirection::Upper, true, None, false), "");
    }

    #[test]
    fn case_modify_invalid_glob_returns_value_unchanged() {
        // `[abc` (unclosed bracket) — glob::Pattern::new returns Err.
        assert_eq!(case_modify("hello", CaseDirection::Upper, true, Some("[abc"), false), "hello");
    }

    #[test]
    fn case_modify_no_match_first_form_returns_unchanged() {
        // No char in "hello" matches [xyz]; all=false → return unchanged.
        assert_eq!(case_modify("hello", CaseDirection::Upper, false, Some("[xyz]"), false), "hello");
    }

    #[test]
    fn expand_modifier_case_upper_all_named_var() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_CASE1", "hello".to_string());
        let m = ParamModifier::Case {
            direction: CaseDirection::Upper,
            all: true,
            pattern: None,
        };
        let r = expand_modifier("HUCK_TEST_PE_CASE1", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("HELLO".to_string()));
    }

    #[test]
    fn expand_modifier_case_upper_positional_lookup() {
        // Verifies the arm uses lookup_var (so digit names resolve).
        let mut shell = Shell::new();
        shell.positional_args = vec!["hello".to_string()];
        let m = ParamModifier::Case {
            direction: CaseDirection::Upper,
            all: true,
            pattern: None,
        };
        let r = expand_modifier("1", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("HELLO".to_string()));
    }

    #[test]
    fn expand_modifier_case_unset_var_returns_empty() {
        let mut shell = Shell::new();
        let m = ParamModifier::Case {
            direction: CaseDirection::Upper,
            all: true,
            pattern: None,
        };
        let r = expand_modifier("HUCK_TEST_PE_CASE_UNSET", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("".to_string()));
    }
}
