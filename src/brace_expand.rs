//! Brace expansion (`{a,b,c}`, `{1..5}`, ...). Runs at the lexer
//! stage before any other expansion. Operates on a `&str` and
//! returns the list of expanded strings.
//!
//! Sentinels of the form `\u{0001}<idx>\u{0002}` mark positions
//! occupied by non-Literal WordParts and are preserved verbatim
//! through expansion.

const MAX_ELEMENTS: usize = 65_536;

#[derive(Debug, PartialEq, Eq)]
pub enum BraceError {
    TooManyElements,
}

pub fn expand(input: &str) -> Result<Vec<String>, BraceError> {
    let mut out = Vec::new();
    expand_into(input, &mut out)?;
    Ok(out)
}

fn expand_into(input: &str, out: &mut Vec<String>) -> Result<(), BraceError> {
    if out.len() > MAX_ELEMENTS {
        return Err(BraceError::TooManyElements);
    }
    let bytes = input.as_bytes();
    let lbrace = match find_top_level_lbrace(bytes) {
        Some(i) => i,
        None => {
            out.push(input.to_string());
            return Ok(());
        }
    };
    let rbrace = match find_matching_rbrace(bytes, lbrace) {
        Some(i) => i,
        None => {
            out.push(input.to_string());
            return Ok(());
        }
    };
    let prefix = &input[..lbrace];
    let body = &input[lbrace + 1..rbrace];
    let suffix = &input[rbrace + 1..];

    let items = match parse_body(body) {
        Some(items) => items,
        None => {
            // Body wasn't a valid brace expr; treat `{body}` as a
            // literal and continue scanning after it.
            let head = format!("{prefix}{{{body}}}");
            let mut tail = Vec::new();
            expand_into(suffix, &mut tail)?;
            for t in tail {
                out.push(format!("{head}{t}"));
                if out.len() > MAX_ELEMENTS {
                    return Err(BraceError::TooManyElements);
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

fn find_top_level_lbrace(bytes: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0x01 {
            // Skip sentinel block: \u{0001} <idx_bytes> \u{0002}
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != 0x02 {
                j += 1;
            }
            if j < bytes.len() {
                i = j + 1;
                continue;
            } else {
                return None;
            }
        }
        if b == b'{' {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_matching_rbrace(bytes: &[u8], lbrace: usize) -> Option<usize> {
    let mut depth: i32 = 1;
    let mut i = lbrace + 1;
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0x01 {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != 0x02 {
                j += 1;
            }
            if j < bytes.len() {
                i = j + 1;
                continue;
            } else {
                return None;
            }
        }
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
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
    let bytes = body.as_bytes();
    let mut depth: i32 = 0;
    let mut items: Vec<String> = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0x01 {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != 0x02 {
                j += 1;
            }
            if j < bytes.len() {
                i = j + 1;
                continue;
            } else {
                return None;
            }
        }
        match b {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            b',' if depth == 0 => {
                items.push(body[start..i].to_string());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
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
            None => if r >= l { 1i64 } else { -1i64 },
            Some(s) => match s.parse::<i64>() {
                Ok(0) => return None,
                Ok(n) if n > 0 => if r >= l { n } else { -n },
                _ => return None,
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
                if cur >= r { break; }
            } else {
                if cur <= r { break; }
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
            None => if r >= l { 1 } else { -1 },
            Some(s) => match s.parse::<i64>() {
                Ok(0) => return None,
                Ok(n) if n > 0 => if r >= l { n } else { -n },
                _ => return None,
            },
        };
        let mut out = Vec::new();
        let mut cur = l;
        loop {
            if let Some(c) = char::from_u32(cur as u32) {
                out.push(c.to_string());
            } else {
                return None;
            }
            if out.len() > MAX_ELEMENTS {
                return Some(out);
            }
            if step > 0 {
                if cur >= r { break; }
            } else {
                if cur <= r { break; }
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
        assert_eq!(expand("pre{a,b}post").unwrap(), vec!["preapost", "prebpost"]);
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
    fn char_range_ascending() {
        assert_eq!(expand("{a..e}").unwrap(), vec!["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn zero_padded_range() {
        assert_eq!(expand("{01..05}").unwrap(), vec!["01", "02", "03", "04", "05"]);
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
    fn invalid_range_falls_through() {
        assert_eq!(expand("{1..a}").unwrap(), vec!["{1..a}"]);
    }

    #[test]
    fn too_many_elements_errors() {
        let err = expand("{1..70000}").unwrap_err();
        assert_eq!(err, BraceError::TooManyElements);
    }
}
