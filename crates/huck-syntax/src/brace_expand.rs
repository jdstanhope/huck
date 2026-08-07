//! Brace expansion (`{a,b,c}`, `{1..5}`, ...). Runs at the lexer
//! stage before any other expansion. Operates on a `&str` and
//! returns the list of expanded strings.
//!
//! Sentinels of the form `\u{E000}<idx>\u{E001}` (Unicode Private
//! Use Area) mark positions occupied by non-Literal WordParts and
//! are preserved verbatim through expansion. The PUA chars are
//! reserved by Unicode for internal application use and are not
//! expected to appear in real shell input.
//!
//! A third PUA sentinel, [`EMPTY_QUOTED_SENTINEL`], marks a char-range
//! element that bash's `\` (0x5C) range quirk (#318) produces as an EMPTY
//! but QUOTE-PROTECTED field — distinct from an ordinary empty string
//! (e.g. the middle item of `{a,,b}`), which is an unquoted empty word and
//! vanishes like any other unquoted-empty expansion. `split_on_sentinels`
//! (lexer.rs) turns this sentinel into a `WordPart::Literal { quoted: true,
//! text: "" }` so the field survives; a bare `""` string from `parse_body`
//! still produces zero `WordPart`s and disappears as before.

const MAX_ELEMENTS: usize = 65_536;

/// PUA sentinel (see module docs) standing in for an EMPTY but
/// QUOTE-PROTECTED brace-expansion element — bash's `\` (0x5C) char-range
/// quirk (#318). `split_on_sentinels` (lexer.rs) is the sole consumer.
pub(crate) const EMPTY_QUOTED_SENTINEL: char = '\u{E002}';

#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BraceError {
    TooManyElements,
}

impl std::fmt::Display for BraceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BraceError::TooManyElements => f.write_str("brace expansion: too many elements"),
        }
    }
}

impl std::error::Error for BraceError {}

pub fn expand(input: &str) -> Result<Vec<String>, BraceError> {
    let mut out = Vec::new();
    expand_into(input, &mut out)?;
    Ok(out)
}

fn expand_into(input: &str, out: &mut Vec<String>) -> Result<(), BraceError> {
    if out.len() > MAX_ELEMENTS {
        return Err(BraceError::TooManyElements);
    }
    let lbrace = match find_top_level_lbrace(input) {
        Some(i) => i,
        None => {
            out.push(input.to_string());
            return Ok(());
        }
    };
    let rbrace = match find_matching_rbrace(input, lbrace) {
        Some(i) => i,
        None => {
            // The `{` at lbrace has no matching `}` — it is literal. bash still
            // expands a LATER balanced brace (`a-{bdef-{g,i}-c` → `a-{bdef-g-c
            // a-{bdef-i-c`; `{a{b,c}` → `{ab {ac`). Emit through-and-including this
            // `{` as literal and re-scan the remainder (which is strictly shorter,
            // so no infinite recursion).
            let split = lbrace + '{'.len_utf8();
            let head = &input[..split];
            let rest = &input[split..];
            let mut tail = Vec::new();
            expand_into(rest, &mut tail)?;
            for t in tail {
                out.push(format!("{head}{t}"));
                if out.len() > MAX_ELEMENTS {
                    return Err(BraceError::TooManyElements);
                }
            }
            return Ok(());
        }
    };
    let prefix = &input[..lbrace];
    let body = &input[lbrace + 1..rbrace];
    let suffix = &input[rbrace + 1..];

    let items = match parse_body(body) {
        Some(items) => items,
        None => {
            // Outer {body} is not a brace expr (no top-level comma/range) → the
            // braces are LITERAL, but inner braces inside body still expand
            // (bash: `a-{b{d,e}}-c` → `a-{bd}-c a-{be}-c`). Recurse into body and
            // suffix and cross them, re-wrapping body in literal braces. Do NOT
            // re-feed `{be}` through expand_into — the literal braces would be
            // re-parsed as a top-level brace with no comma/range and recurse forever.
            let mut body_exp = Vec::new();
            expand_into(body, &mut body_exp)?;
            let mut suffix_exp = Vec::new();
            expand_into(suffix, &mut suffix_exp)?;
            for be in &body_exp {
                for se in &suffix_exp {
                    out.push(format!("{prefix}{{{be}}}{se}"));
                    if out.len() > MAX_ELEMENTS {
                        return Err(BraceError::TooManyElements);
                    }
                }
            }
            return Ok(());
        }
    };

    for item in items {
        let mut item_expansions = Vec::new();
        expand_into(&item, &mut item_expansions)?;
        for ie in item_expansions {
            let combined = format!("{prefix}{ie}{suffix}");
            expand_into(&combined, out)?;
            if out.len() > MAX_ELEMENTS {
                return Err(BraceError::TooManyElements);
            }
        }
    }
    Ok(())
}

/// Consume the remainder of a `\u{E000}…\u{E001}` sentinel span from
/// `iter`, which has already yielded the opening `\u{E000}` character.
/// Returns `true` if the closing `\u{E001}` was found, `false` if the
/// iterator was exhausted before seeing it.
fn skip_sentinel<I: Iterator<Item = (usize, char)>>(iter: &mut I) -> bool {
    for (_, nc) in iter.by_ref() {
        if nc == '\u{E001}' {
            return true;
        }
    }
    false
}

fn find_top_level_lbrace(s: &str) -> Option<usize> {
    let mut iter = s.char_indices();
    while let Some((i, c)) = iter.next() {
        if c == '\u{E000}' {
            if !skip_sentinel(&mut iter) {
                return None;
            }
            continue;
        }
        if c == '{' {
            return Some(i);
        }
    }
    None
}

fn find_matching_rbrace(s: &str, lbrace: usize) -> Option<usize> {
    let mut depth: i32 = 1;
    let mut iter = s[lbrace + 1..].char_indices();
    while let Some((rel_i, c)) = iter.next() {
        let i = lbrace + 1 + rel_i;
        if c == '\u{E000}' {
            if !skip_sentinel(&mut iter) {
                return None;
            }
            continue;
        }
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_body(body: &str) -> Option<Vec<String>> {
    if let Some(items) = split_top_level_commas(body)
        && items.len() >= 2
    {
        return Some(items);
    }
    if let Some(items) = parse_range(body) {
        return Some(items);
    }
    None
}

fn split_top_level_commas(body: &str) -> Option<Vec<String>> {
    let mut depth: i32 = 0;
    let mut items: Vec<String> = Vec::new();
    let mut start = 0;
    let mut iter = body.char_indices();
    while let Some((i, c)) = iter.next() {
        if c == '\u{E000}' {
            if !skip_sentinel(&mut iter) {
                return None;
            }
            continue;
        }
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                items.push(body[start..i].to_string());
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    items.push(body[start..].to_string());
    Some(items)
}

fn parse_range(body: &str) -> Option<Vec<String>> {
    // Look for `..` at top-level (no nested braces or sentinels).
    let parts: Vec<&str> = body.split("..").collect();
    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }
    let left = parts[0];
    let right = parts[1];
    let step_str = parts.get(2).copied();

    // Try integer range.
    if let (Ok(l), Ok(r)) = (left.parse::<i64>(), right.parse::<i64>()) {
        let step = match step_str {
            None => {
                if r >= l {
                    1i64
                } else {
                    -1i64
                }
            }
            Some(s) => match s.parse::<i64>() {
                Ok(0) => return None,
                Ok(n) => {
                    // bash ignores the step's SIGN — magnitude only, direction from the
                    // endpoints (`{10..1..-2}` == `{10..1..2}` → 10 8 6 4 2). (#318)
                    // `checked_abs` guards `i64::MIN`, whose magnitude has no `i64`
                    // representation (`i64::MIN.abs()` panics) — bash also leaves a
                    // step that extreme un-expanded (verified against bash 5.2.21:
                    // `{1..2..-9223372036854775808}` prints literally, rc 0), so
                    // falling back to `None` (literal) matches.
                    let m = n.checked_abs()?;
                    if r >= l { m } else { -m }
                }
                Err(_) => return None,
            },
        };
        let pad_width = compute_pad_width(left, right);
        let mut out = Vec::new();
        let mut cur = l;
        loop {
            let s = if let Some(w) = pad_width {
                if cur < 0 {
                    format!("-{:0>width$}", -cur, width = w.saturating_sub(1))
                } else {
                    format!("{:0>width$}", cur, width = w)
                }
            } else {
                cur.to_string()
            };
            out.push(s);
            if out.len() > MAX_ELEMENTS {
                return Some(out);
            }
            if step > 0 {
                if cur >= r {
                    break;
                }
            } else {
                if cur <= r {
                    break;
                }
            }
            cur = match cur.checked_add(step) {
                Some(n) => n,
                None => break,
            };
            if (step > 0 && cur > r) || (step < 0 && cur < r) {
                break;
            }
        }
        return Some(out);
    }

    // Try char range. Require both endpoints to be single ASCII
    // letters; mixed-type ranges like `{1..a}` fall through as
    // literal (matches bash).
    let left_chars: Vec<char> = left.chars().collect();
    let right_chars: Vec<char> = right.chars().collect();
    if left_chars.len() == 1
        && right_chars.len() == 1
        && left_chars[0].is_ascii_alphabetic()
        && right_chars[0].is_ascii_alphabetic()
    {
        let l = left_chars[0] as i64;
        let r = right_chars[0] as i64;
        let step: i64 = match step_str {
            None => {
                if r >= l {
                    1
                } else {
                    -1
                }
            }
            Some(s) => match s.parse::<i64>() {
                Ok(0) => return None,
                Ok(n) => {
                    // bash ignores the step's SIGN — magnitude only, direction from the
                    // endpoints (`{10..1..-2}` == `{10..1..2}` → 10 8 6 4 2). (#318)
                    // `checked_abs` guards `i64::MIN`, whose magnitude has no `i64`
                    // representation (`i64::MIN.abs()` panics) — bash also leaves a
                    // step that extreme un-expanded (verified against bash 5.2.21:
                    // `{1..2..-9223372036854775808}` prints literally, rc 0), so
                    // falling back to `None` (literal) matches.
                    let m = n.checked_abs()?;
                    if r >= l { m } else { -m }
                }
                Err(_) => return None,
            },
        };
        let mut out = Vec::new();
        let mut cur = l;
        loop {
            let c = char::from_u32(cur as u32)?;
            // bash emits an EMPTY (but quote-protected — see module docs)
            // element for `\` (0x5C) in a char range (#318); every other
            // char in the byte span is emitted literally.
            if c == '\\' {
                out.push(EMPTY_QUOTED_SENTINEL.to_string());
            } else {
                out.push(c.to_string());
            }
            if out.len() > MAX_ELEMENTS {
                return Some(out);
            }
            if step > 0 {
                if cur >= r {
                    break;
                }
            } else {
                if cur <= r {
                    break;
                }
            }
            cur += step;
            if (step > 0 && cur > r) || (step < 0 && cur < r) {
                break;
            }
        }
        return Some(out);
    }

    None
}

fn compute_pad_width(left: &str, right: &str) -> Option<usize> {
    let l_pad = left.starts_with('0') && left.len() >= 2;
    let r_pad = right.starts_with('0') && right.len() >= 2;
    if l_pad || r_pad {
        let l_len = left.trim_start_matches('-').len();
        let r_len = right.trim_start_matches('-').len();
        Some(l_len.max(r_len))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comma_list_simple() {
        assert_eq!(expand("{a,b,c}").unwrap(), vec!["a", "b", "c"]);
    }

    #[test]
    fn comma_list_with_prefix_suffix() {
        assert_eq!(
            expand("pre{a,b}post").unwrap(),
            vec!["preapost", "prebpost"]
        );
    }

    #[test]
    fn integer_range_ascending() {
        assert_eq!(expand("{1..5}").unwrap(), vec!["1", "2", "3", "4", "5"]);
    }

    #[test]
    fn integer_range_descending() {
        assert_eq!(expand("{5..1}").unwrap(), vec!["5", "4", "3", "2", "1"]);
    }

    #[test]
    fn integer_range_with_step() {
        assert_eq!(expand("{1..10..2}").unwrap(), vec!["1", "3", "5", "7", "9"]);
    }

    #[test]
    fn integer_range_negative_step_sign_ignored() {
        // bash ignores the step's sign — magnitude only, direction from the
        // endpoints (#318): {10..1..-2} == {10..1..2}.
        assert_eq!(
            expand("{10..1..-2}").unwrap(),
            vec!["10", "8", "6", "4", "2"]
        );
    }

    #[test]
    fn integer_range_step_i64_min_stays_literal() {
        // (#318 fix-round-1) i64::MIN's magnitude has no i64 representation
        // (`i64::MIN.abs()` panics); `checked_abs` must fall back to treating
        // the whole `{...}` as literal, matching bash (verified against bash
        // 5.2.21: this prints literally, rc 0 — no expansion, no error).
        assert_eq!(
            expand("{1..2..-9223372036854775808}").unwrap(),
            vec!["{1..2..-9223372036854775808}"]
        );
    }

    #[test]
    fn char_range_ascending() {
        assert_eq!(expand("{a..e}").unwrap(), vec!["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn char_range_step_i64_min_stays_literal() {
        // Char arm of the same (#318 fix-round-1) guard.
        assert_eq!(
            expand("{a..z..-9223372036854775808}").unwrap(),
            vec!["{a..z..-9223372036854775808}"]
        );
    }

    #[test]
    fn zero_padded_range() {
        assert_eq!(
            expand("{01..05}").unwrap(),
            vec!["01", "02", "03", "04", "05"]
        );
    }

    #[test]
    fn nested_brace() {
        assert_eq!(expand("{a,{b,c}}").unwrap(), vec!["a", "b", "c"]);
    }

    #[test]
    fn cartesian_two_braces() {
        assert_eq!(expand("{a,b}{c,d}").unwrap(), vec!["ac", "ad", "bc", "bd"]);
    }

    #[test]
    fn invalid_brace_is_literal() {
        assert_eq!(expand("{a").unwrap(), vec!["{a"]);
    }

    #[test]
    fn unmatched_brace_still_expands_a_later_balanced_one() {
        // bash treats an unmatched `{` as literal but still expands a LATER
        // balanced brace elsewhere in the string (#318).
        assert_eq!(
            expand("a-{bdef-{g,i}-c").unwrap(),
            vec!["a-{bdef-g-c", "a-{bdef-i-c"]
        );
        assert_eq!(expand("{a{b,c}").unwrap(), vec!["{ab", "{ac"]);
        assert_eq!(expand("{{a,b}").unwrap(), vec!["{a", "{b"]);
    }

    #[test]
    fn invalid_range_falls_through() {
        assert_eq!(expand("{1..a}").unwrap(), vec!["{1..a}"]);
    }

    #[test]
    fn too_many_elements_errors() {
        let err = expand("{1..70000}").unwrap_err();
        assert_eq!(err, BraceError::TooManyElements);
    }

    #[test]
    fn soh_stx_in_input_pass_through_cleanly() {
        // \u{0001} (SOH) and \u{0002} (STX) used to be our sentinels
        // and would collide with user input containing them. After
        // switching to PUA sentinels (\u{E000}/\u{E001}), these
        // control chars pass through brace expansion unchanged.
        let result = expand("X\u{0001}Y{a,b}\u{0002}Z").unwrap();
        assert_eq!(
            result,
            vec![
                "X\u{0001}Ya\u{0002}Z".to_string(),
                "X\u{0001}Yb\u{0002}Z".to_string(),
            ]
        );
    }
}
