//! Extended-glob (`shopt extglob`) pattern matcher. Pure: no shell, no FS.
//! Used by `[[`/`case`/`${}` only when extglob is on AND the pattern contains
//! an extglob operator (`has_extglob`). Plain globs keep using the `glob` crate.

#[derive(Debug, Clone, Copy, PartialEq)]
enum GroupKind {
    ZeroOrOne,
    ZeroOrMore,
    OneOrMore,
    ExactlyOne,
    Not,
}

#[derive(Debug, Clone)]
enum Item {
    Lit(char),
    AnyChar, // ?
    AnyRun,  // *
    Class {
        negated: bool,
        set: Vec<ClassAtom>,
    }, // [...]
    Group {
        kind: GroupKind,
        alts: Vec<Vec<Item>>,
    }, // ?( *( +( @( !(
}

#[derive(Debug, Clone)]
enum ClassAtom {
    Ch(char),
    Range(char, char),
}

/// True if `pattern` contains an extglob operator: one of `? * + @ !` directly
/// followed by `(` (scanning past `\`-escapes).
pub fn has_extglob(pattern: &str) -> bool {
    let b: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            '\\' => {
                i += 2;
                continue;
            }
            '?' | '*' | '+' | '@' | '!' if i + 1 < b.len() && b[i + 1] == '(' => return true,
            _ => i += 1,
        }
    }
    false
}

/// Matches `text` against extglob `pattern` (the WHOLE text must match).
pub fn extglob_match(pattern: &str, text: &str, case_insensitive: bool) -> bool {
    let chars: Vec<char> = pattern.chars().collect();
    let mut pos = 0;
    let pat = parse_seq(&chars, &mut pos, false);
    let txt: Vec<char> = text.chars().collect();
    match_here(&pat, &txt, case_insensitive)
}

/// Parses a sequence of `Item`s from `chars` starting at `*pos`. When
/// `in_group` is true, parsing stops (returns) at a top-level `|` or `)`
/// without consuming it, so the caller can handle alternation / close.
fn parse_seq(chars: &[char], pos: &mut usize, in_group: bool) -> Vec<Item> {
    let mut items = Vec::new();
    while *pos < chars.len() {
        let c = chars[*pos];
        if in_group && (c == '|' || c == ')') {
            return items;
        }
        match c {
            '\\' => {
                // Escaped char → literal of the next char (or a lone `\`).
                if *pos + 1 < chars.len() {
                    items.push(Item::Lit(chars[*pos + 1]));
                    *pos += 2;
                } else {
                    items.push(Item::Lit('\\'));
                    *pos += 1;
                }
            }
            '[' => {
                items.push(parse_class(chars, pos));
            }
            '?' | '*' | '+' | '@' | '!'
                if *pos + 1 < chars.len() && chars[*pos + 1] == '(' =>
            {
                let kind = match c {
                    '?' => GroupKind::ZeroOrOne,
                    '*' => GroupKind::ZeroOrMore,
                    '+' => GroupKind::OneOrMore,
                    '@' => GroupKind::ExactlyOne,
                    '!' => GroupKind::Not,
                    _ => unreachable!(),
                };
                *pos += 2; // consume prefix char and '('
                let mut alts: Vec<Vec<Item>> = Vec::new();
                loop {
                    let alt = parse_seq(chars, pos, true);
                    alts.push(alt);
                    if *pos < chars.len() && chars[*pos] == '|' {
                        *pos += 1; // consume '|', parse next alt
                        continue;
                    }
                    if *pos < chars.len() && chars[*pos] == ')' {
                        *pos += 1; // consume ')'
                    }
                    // (If we hit EOF without ')', just stop — unterminated.)
                    break;
                }
                items.push(Item::Group { kind, alts });
            }
            '?' => {
                items.push(Item::AnyChar);
                *pos += 1;
            }
            '*' => {
                items.push(Item::AnyRun);
                *pos += 1;
            }
            _ => {
                items.push(Item::Lit(c));
                *pos += 1;
            }
        }
    }
    items
}

/// Parses a bracket class `[...]` starting at `chars[*pos] == '['`.
/// Handles leading `!`/`^` negation, a literal `]` if it's the first set
/// char, and `a-z` ranges. On a malformed (unterminated) class, treats the
/// `[` as a literal.
fn parse_class(chars: &[char], pos: &mut usize) -> Item {
    let start = *pos;
    let mut i = *pos + 1; // skip '['
    let mut negated = false;
    if i < chars.len() && (chars[i] == '!' || chars[i] == '^') {
        negated = true;
        i += 1;
    }
    let mut set: Vec<ClassAtom> = Vec::new();
    // A `]` as the very first class char is a literal.
    if i < chars.len() && chars[i] == ']' {
        set.push(ClassAtom::Ch(']'));
        i += 1;
    }
    let mut closed = false;
    while i < chars.len() {
        if chars[i] == ']' {
            closed = true;
            i += 1;
            break;
        }
        // Range: x-y (where y is not the closing ']').
        if i + 2 < chars.len() && chars[i + 1] == '-' && chars[i + 2] != ']' {
            set.push(ClassAtom::Range(chars[i], chars[i + 2]));
            i += 3;
        } else {
            set.push(ClassAtom::Ch(chars[i]));
            i += 1;
        }
    }
    if !closed {
        // Unterminated class — treat the original `[` as a literal char.
        *pos = start + 1;
        return Item::Lit('[');
    }
    *pos = i;
    Item::Class { negated, set }
}

fn lc(c: char) -> char {
    // Use the first lowercase char; adequate for ASCII-and-common matching.
    c.to_lowercase().next().unwrap_or(c)
}

fn eqc(a: char, b: char, ci: bool) -> bool {
    if ci {
        lc(a) == lc(b)
    } else {
        a == b
    }
}

fn class_matches(set: &[ClassAtom], negated: bool, c: char, ci: bool) -> bool {
    let mut hit = false;
    for atom in set {
        match atom {
            ClassAtom::Ch(x) => {
                if eqc(*x, c, ci) {
                    hit = true;
                    break;
                }
            }
            ClassAtom::Range(lo, hi) => {
                if ci {
                    let cl = lc(c);
                    if (lc(*lo)..=lc(*hi)).contains(&cl) || (*lo..=*hi).contains(&c) {
                        hit = true;
                        break;
                    }
                } else if (*lo..=*hi).contains(&c) {
                    hit = true;
                    break;
                }
            }
        }
    }
    hit ^ negated
}

/// True if any alternative matches the WHOLE `span`.
fn alt_matches_whole(alts: &[Vec<Item>], span: &[char], ci: bool) -> bool {
    alts.iter().any(|a| match_here(a, span, ci))
}

/// `*(...)`/`+(...)` repetition helper: zero-or-more reps of `alts`,
/// then `rest` must match the remainder.
fn match_star(alts: &[Vec<Item>], rest: &[Item], text: &[char], ci: bool) -> bool {
    if match_here(rest, text, ci) {
        return true;
    }
    (1..=text.len())
        .any(|k| alt_matches_whole(alts, &text[..k], ci) && match_star(alts, rest, &text[k..], ci))
}

/// Anchored, whole-text match of `items` against `text`.
fn match_here(items: &[Item], text: &[char], ci: bool) -> bool {
    let (item, rest) = match items.split_first() {
        Some(x) => x,
        None => return text.is_empty(),
    };
    match item {
        Item::Lit(c) => !text.is_empty() && eqc(text[0], *c, ci) && match_here(rest, &text[1..], ci),
        Item::AnyChar => !text.is_empty() && match_here(rest, &text[1..], ci),
        Item::AnyRun => (0..=text.len()).any(|k| match_here(rest, &text[k..], ci)),
        Item::Class { negated, set } => {
            !text.is_empty()
                && class_matches(set, *negated, text[0], ci)
                && match_here(rest, &text[1..], ci)
        }
        Item::Group { kind, alts } => match kind {
            GroupKind::ExactlyOne => (0..=text.len())
                .any(|k| alt_matches_whole(alts, &text[..k], ci) && match_here(rest, &text[k..], ci)),
            GroupKind::ZeroOrOne => {
                match_here(rest, text, ci)
                    || (1..=text.len()).any(|k| {
                        alt_matches_whole(alts, &text[..k], ci) && match_here(rest, &text[k..], ci)
                    })
            }
            GroupKind::ZeroOrMore => match_star(alts, rest, text, ci),
            GroupKind::OneOrMore => (1..=text.len())
                .any(|k| alt_matches_whole(alts, &text[..k], ci) && match_star(alts, rest, &text[k..], ci)),
            GroupKind::Not => (0..=text.len())
                .any(|k| !alt_matches_whole(alts, &text[..k], ci) && match_here(rest, &text[k..], ci)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str, t: &str) -> bool {
        extglob_match(p, t, false)
    }

    #[test]
    fn has_extglob_detects_ops() {
        for p in ["?(a)", "*(a)", "+(a)", "@(a)", "!(a)", "x+(y)z", "a@(b|c)"] {
            assert!(has_extglob(p), "should detect: {p}");
        }
        for p in ["abc", "*.txt", "a?b", "[a-z]+", "(a)", "a|b"] {
            assert!(!has_extglob(p), "should NOT detect: {p}");
        }
    }

    #[test]
    fn question_zero_or_one() {
        assert!(m("?(abc)", ""));
        assert!(m("?(abc)", "abc"));
        assert!(!m("?(abc)", "abcabc"));
    }

    #[test]
    fn star_zero_or_more() {
        assert!(m("*(ab)", ""));
        assert!(m("*(ab)", "ababab"));
        assert!(!m("*(ab)", "aba"));
    }

    #[test]
    fn plus_one_or_more() {
        assert!(!m("+(ab)", ""));
        assert!(m("+(ab)", "ab"));
        assert!(m("+(ab)", "abab"));
    }

    #[test]
    fn at_exactly_one() {
        assert!(m("@(ab|cd)", "ab"));
        assert!(m("@(ab|cd)", "cd"));
        assert!(!m("@(ab|cd)", "abcd"));
        assert!(!m("@(ab|cd)", ""));
    }

    #[test]
    fn not_negation() {
        assert!(m("!(bar)", "foo"));
        assert!(!m("!(bar)", "bar"));
        assert!(m("!(bar)", "")); // empty is not "bar"
        assert!(m("!(*.txt)", "a.md"));
        assert!(!m("!(*.txt)", "a.txt"));
    }

    #[test]
    fn alternation_and_composition() {
        assert!(m("a@(x|y)b", "axb"));
        assert!(m("a@(x|y)b", "ayb"));
        assert!(!m("a@(x|y)b", "azb"));
        assert!(m("a+(x|y)b", "axxb"));
        assert!(m("a+(x|y)b", "axyb"));
        assert!(m("+([a-z]).txt", "file.txt"));
        assert!(!m("+([a-z]).txt", "File.txt")); // uppercase excluded by class
    }

    #[test]
    fn nesting() {
        assert!(m("@(a*(b)c)", "abc"));
        assert!(m("@(a*(b)c)", "ac"));
        assert!(m("@(a*(b)c)", "abbbc"));
        assert!(!m("@(a*(b)c)", "adc"));
    }

    #[test]
    fn plain_glob_still_works_through_engine() {
        assert!(m("*.txt", "a.txt"));
        assert!(m("a?c", "abc"));
        assert!(m("[a-z]+(0|1)", "x01")); // mix class + extglob
    }

    #[test]
    fn case_insensitive() {
        assert!(extglob_match("@(ABC)", "abc", true));
        assert!(!extglob_match("@(ABC)", "abc", false));
    }
}
