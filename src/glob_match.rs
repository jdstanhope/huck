//! Extended-glob (`shopt extglob`) pattern matcher. Pure: no shell, no FS.
//! Used by `[[`/`case`/`${}` only when extglob is on AND the pattern contains
//! an extglob operator (`has_extglob`). Plain globs keep using the `glob` crate.

use std::borrow::Cow;

/// Rewrite a class-leading `^` to `!` so the `glob` crate (which only honors
/// `[!…]`) treats `[^…]` as negation, matching bash (which accepts both). Only
/// the FIRST char inside an unescaped class-opening `[` is the negation slot; a
/// `^` anywhere else stays literal. Honors `\[` escapes and the literal-first-`]`
/// rule (`[^]x]` → `[!]x]`, `[]x]` unchanged). Returns the input borrowed when
/// there is nothing to change (zero-copy). (M-113)
pub(crate) fn translate_bracket_negation(pattern: &str) -> Cow<'_, str> {
    if !pattern.contains('[') {
        return Cow::Borrowed(pattern);
    }
    let chars: Vec<char> = pattern.chars().collect();
    let mut out: Option<String> = None; // built lazily on first change
    let mut in_class = false;
    let mut escaped = false;
    let mut pos_in_class = 0usize; // chars seen since '[' (1 = first content char)
    let mut negated = false;       // class opened with `!` or `^`
    for i in 0..chars.len() {
        let c = chars[i];
        let mut emit = c;
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if !in_class {
            if c == '[' {
                in_class = true;
                pos_in_class = 0;
                negated = false;
            }
            // '^' / ']' outside a class are literal — nothing to do.
        } else {
            pos_in_class += 1;
            if pos_in_class == 1 {
                // The negation slot (first char after `[`).
                if c == '^' {
                    emit = '!';
                    negated = true;
                    if out.is_none() {
                        out = Some(chars[..i].iter().collect());
                    }
                } else if c == '!' {
                    negated = true;
                }
                // A `]` here (`[]…`) is a LITERAL `]`; class stays open. Any
                // other char is ordinary class content.
            } else if pos_in_class == 2 && negated && c == ']' {
                // Literal `]` immediately after `[!` / `[^` — class stays open.
            } else if c == ']' {
                in_class = false;
            }
        }
        if let Some(o) = out.as_mut() {
            o.push(emit);
        }
    }
    match out {
        Some(s) => Cow::Owned(s),
        None => Cow::Borrowed(pattern),
    }
}

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
    Posix(PosixClass),
    Never, // unknown POSIX class name: matches nothing
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PosixClass {
    Alpha,
    Digit,
    Alnum,
    Upper,
    Lower,
    Space,
    Blank,
    Punct,
    Cntrl,
    Graph,
    Print,
    Xdigit,
}

fn posix_class_from_name(name: &str) -> Option<PosixClass> {
    use PosixClass::*;
    Some(match name {
        "alpha" => Alpha,
        "digit" => Digit,
        "alnum" => Alnum,
        "upper" => Upper,
        "lower" => Lower,
        "xdigit" => Xdigit,
        "punct" => Punct,
        "cntrl" => Cntrl,
        "graph" => Graph,
        "space" => Space,
        "blank" => Blank,
        "print" => Print,
        _ => return None,
    })
}

fn posix_matches(pc: PosixClass, c: char, ci: bool) -> bool {
    use PosixClass::*;
    match pc {
        Alpha => c.is_ascii_alphabetic(),
        Digit => c.is_ascii_digit(),
        Alnum => c.is_ascii_alphanumeric(),
        // Under case-insensitive matching, upper/lower widen to any letter.
        Upper => {
            if ci {
                c.is_ascii_alphabetic()
            } else {
                c.is_ascii_uppercase()
            }
        }
        Lower => {
            if ci {
                c.is_ascii_alphabetic()
            } else {
                c.is_ascii_lowercase()
            }
        }
        Xdigit => c.is_ascii_hexdigit(),
        Punct => c.is_ascii_punctuation(),
        Cntrl => c.is_ascii_control(),
        Graph => c.is_ascii_graphic(),
        // POSIX `space` includes \v (0x0b), which Rust's is_ascii_whitespace omits.
        Space => matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{0b}' | '\u{0c}'),
        Blank => matches!(c, ' ' | '\t'),
        Print => c.is_ascii_graphic() || c == ' ',
    }
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

/// True if `pattern` contains a POSIX bracket class `[:name:]` (the
/// `[[:name:]]` form) — an unescaped `[:` followed later by `:]`. Liberal: a
/// false positive only routes a class-free pattern through the (faithful)
/// own-matcher, which is harmless.
pub fn has_posix_class(pattern: &str) -> bool {
    let b: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '\\' {
            i += 2;
            continue;
        }
        if b[i] == '[' && i + 1 < b.len() && b[i + 1] == ':' {
            let mut j = i + 2;
            while j + 1 < b.len() {
                if b[j] == ':' && b[j + 1] == ']' {
                    return true;
                }
                j += 1;
            }
        }
        i += 1;
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
        // POSIX class `[:name:]` (the inner `[:` of `[[:name:]]`).
        #[allow(clippy::collapsible_if)] // keep the explicit fall-through comment.
        if chars[i] == '[' && i + 1 < chars.len() && chars[i + 1] == ':' {
            if let Some(close) = (i + 2..chars.len().saturating_sub(1))
                .find(|&k| chars[k] == ':' && chars[k + 1] == ']')
            {
                let name: String = chars[i + 2..close].iter().collect();
                set.push(match posix_class_from_name(&name) {
                    Some(pc) => ClassAtom::Posix(pc),
                    None => ClassAtom::Never,
                });
                i = close + 2; // skip past ":]"
                continue;
            }
            // not a valid `[:...:]` — fall through to literal handling.
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
            ClassAtom::Posix(pc) => {
                if posix_matches(*pc, c, ci) {
                    hit = true;
                    break;
                }
            }
            ClassAtom::Never => {}
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

/// Filesystem pathname expansion for an extglob `pattern` (the `glob` crate
/// can't do extglob). Returns matched paths sorted lexicographically; empty if
/// nothing matches. Honors the dotfile rule, `nocaseglob`, and `dotglob`.
/// Per-component matching delegates to `extglob_match` (which also implements
/// `*`/`?`/`[…]`), so mixed patterns like `dir*/+(foo|bar).txt` work.
pub fn extglob_pathname_expand(pattern: &str, nocaseglob: bool, dotglob: bool) -> Vec<String> {
    let absolute = pattern.starts_with('/');
    let comps: Vec<String> = pattern
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    if comps.is_empty() {
        return Vec::new();
    }
    let start = if absolute { "/".to_string() } else { String::new() };
    let mut out = Vec::new();
    walk_components(&start, &comps, 0, nocaseglob, dotglob, &mut out);
    out.sort();
    out
}

/// True if a path component needs directory matching (vs literal descent):
/// it has a glob wildcard or an extglob operator.
fn component_needs_match(comp: &str) -> bool {
    comp.contains('*') || comp.contains('?') || comp.contains('[') || has_extglob(comp)
}

/// Joins `prefix` + `name` into a path: empty prefix → bare name (relative,
/// no `./`); root prefix → `/name`; else `prefix/name`.
fn join_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else if prefix == "/" {
        format!("/{name}")
    } else {
        format!("{prefix}/{name}")
    }
}

fn walk_components(
    prefix: &str,
    comps: &[String],
    idx: usize,
    nocaseglob: bool,
    dotglob: bool,
    out: &mut Vec<String>,
) {
    if idx == comps.len() {
        out.push(prefix.to_string());
        return;
    }
    let comp = &comps[idx];
    let is_last = idx + 1 == comps.len();

    // Literal component: descend (or include) only if the path exists on disk.
    if !component_needs_match(comp) {
        let next = join_path(prefix, comp);
        if std::path::Path::new(&next).exists() {
            walk_components(&next, comps, idx + 1, nocaseglob, dotglob, out);
        }
        return;
    }

    // Pattern component: list the directory and keep matching entries.
    let dir = if prefix.is_empty() { "." } else { prefix };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    // Dotfile rule: a leading-dot entry is matched only if `dotglob` is on or
    // the component's first char is a literal `.` (the pattern is dot-anchored).
    let dot_anchored = comp.starts_with('.');
    for entry in entries.flatten() {
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => continue, // skip non-UTF8 names
        };
        if name == "." || name == ".." {
            continue;
        }
        if name.starts_with('.') && !dotglob && !dot_anchored {
            continue;
        }
        if extglob_match(comp, &name, nocaseglob) {
            let next = join_path(prefix, &name);
            if is_last {
                out.push(next);
            } else if std::path::Path::new(&next).is_dir() {
                walk_components(&next, comps, idx + 1, nocaseglob, dotglob, out);
            }
        }
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

#[cfg(test)]
mod pathname_tests {
    use super::*;
    use std::fs;

    /// Builds a tempdir fixture and returns (TempDir, its absolute path string).
    fn fixture() -> (tempfile::TempDir, String) {
        let d = tempfile::tempdir().unwrap();
        for f in ["a", "b", "ab", "aab", "abc", "cd", "xy", ".hidden", ".ab"] {
            fs::write(d.path().join(f), b"").unwrap();
        }
        fs::create_dir(d.path().join("dir1")).unwrap();
        fs::create_dir(d.path().join("dir2")).unwrap();
        fs::write(d.path().join("dir1/foo.txt"), b"").unwrap();
        fs::write(d.path().join("dir1/bar.log"), b"").unwrap();
        fs::write(d.path().join("dir2/foo.txt"), b"").unwrap();
        let base = d.path().to_str().unwrap().to_string();
        (d, base)
    }

    /// Maps file names to absolute paths under `base`, sorted.
    fn abs(base: &str, names: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = names.iter().map(|n| format!("{base}/{n}")).collect();
        v.sort();
        v
    }

    #[test]
    fn plus_one_or_more_excludes_dotfiles() {
        let (_d, base) = fixture();
        let got = extglob_pathname_expand(&format!("{base}/+(a|b)"), false, false);
        assert_eq!(got, abs(&base, &["a", "aab", "ab", "b"]));
    }

    #[test]
    fn at_exactly_one() {
        let (_d, base) = fixture();
        let got = extglob_pathname_expand(&format!("{base}/@(a|cd)"), false, false);
        assert_eq!(got, abs(&base, &["a", "cd"]));
    }

    #[test]
    fn negation_excludes_listed_and_dotfiles() {
        let (_d, base) = fixture();
        let got = extglob_pathname_expand(&format!("{base}/!(a|ab)"), false, false);
        assert_eq!(got, abs(&base, &["aab", "abc", "b", "cd", "dir1", "dir2", "xy"]));
    }

    #[test]
    fn class_inside_extglob() {
        let (_d, base) = fixture();
        let got = extglob_pathname_expand(&format!("{base}/+([a-c])"), false, false);
        assert_eq!(got, abs(&base, &["a", "aab", "ab", "abc", "b"]));
    }

    #[test]
    fn explicit_dot_matches_dotfile() {
        let (_d, base) = fixture();
        let got = extglob_pathname_expand(&format!("{base}/.+(ab)"), false, false);
        assert_eq!(got, abs(&base, &[".ab"]));
    }

    #[test]
    fn nocaseglob_folds_case() {
        let (_d, base) = fixture();
        let got = extglob_pathname_expand(&format!("{base}/@(A|AB)"), true, false);
        assert_eq!(got, abs(&base, &["a", "ab"]));
    }

    #[test]
    fn multi_component() {
        let (_d, base) = fixture();
        let got = extglob_pathname_expand(&format!("{base}/dir*/+(foo|bar).txt"), false, false);
        assert_eq!(got, abs(&base, &["dir1/foo.txt", "dir2/foo.txt"]));
    }

    #[test]
    fn no_match_is_empty() {
        let (_d, base) = fixture();
        assert!(extglob_pathname_expand(&format!("{base}/+(zzz)"), false, false).is_empty());
    }
}

#[cfg(test)]
mod bracket_negation_tests {
    use super::translate_bracket_negation;
    use std::borrow::Cow;

    fn t(p: &str) -> String { translate_bracket_negation(p).into_owned() }

    #[test]
    fn leading_caret_becomes_bang() {
        assert_eq!(t("[^abc]"), "[!abc]");
        assert_eq!(t("[^0-9]"), "[!0-9]");
    }
    #[test]
    fn bang_unchanged() { assert_eq!(t("[!abc]"), "[!abc]"); }
    #[test]
    fn plain_class_unchanged() { assert_eq!(t("[abc]"), "[abc]"); }
    #[test]
    fn caret_not_leading_is_literal() {
        assert_eq!(t("[a^b]"), "[a^b]");
        assert_eq!(t("a^b"), "a^b");
        assert_eq!(t("^foo"), "^foo");
    }
    #[test]
    fn literal_first_bracket_after_neg() {
        assert_eq!(t("[^]x]"), "[!]x]");
        assert_eq!(t("[]x]"), "[]x]");
    }
    #[test]
    fn escaped_open_bracket_not_a_class() {
        assert_eq!(t(r"\[^a]"), r"\[^a]");
    }
    #[test]
    fn caret_inside_existing_class_is_literal() {
        // `[a[^b]` is one class containing a,[,^,b — the inner ^ is NOT leading.
        assert_eq!(t("[a[^b]"), "[a[^b]");
    }
    #[test]
    fn multiple_classes_each_converted() {
        assert_eq!(t("x[^0-9]y[^a]z"), "x[!0-9]y[!a]z");
    }
    #[test]
    fn posix_class_inner_brackets() {
        assert_eq!(t("[[:alpha:]]"), "[[:alpha:]]");      // no leading ^
        assert_eq!(t("[^[:digit:]]"), "[![:digit:]]");    // leading ^ converted
    }
    #[test]
    fn no_change_returns_borrowed() {
        assert!(matches!(translate_bracket_negation("[abc]"), Cow::Borrowed(_)));
        assert!(matches!(translate_bracket_negation("plain"), Cow::Borrowed(_)));
    }
}

#[cfg(test)]
mod posix_class_tests {
    use super::{extglob_match, has_posix_class};

    fn m(p: &str, t: &str) -> bool { extglob_match(p, t, false) }

    #[test]
    fn digit_alpha_space() {
        assert!(m("[[:digit:]]", "5"));
        assert!(!m("[[:digit:]]", "x"));
        assert!(m("[[:alpha:]]", "x"));
        assert!(!m("[[:alpha:]]", "5"));
        assert!(m("[[:space:]]", " "));
        assert!(m("[[:space:]]", "\u{0b}")); // vertical tab — POSIX space includes \v
        assert!(!m("[[:space:]]", "x"));
    }
    #[test]
    fn upper_lower_alnum_xdigit() {
        assert!(m("[[:upper:]]", "A") && !m("[[:upper:]]", "a"));
        assert!(m("[[:lower:]]", "a") && !m("[[:lower:]]", "A"));
        assert!(m("[[:alnum:]]", "Z") && m("[[:alnum:]]", "7") && !m("[[:alnum:]]", "_"));
        assert!(m("[[:xdigit:]]", "f") && m("[[:xdigit:]]", "9") && !m("[[:xdigit:]]", "g"));
    }
    #[test]
    fn punct_cntrl_graph_print_blank() {
        assert!(m("[[:punct:]]", "]") && m("[[:punct:]]", "!") && !m("[[:punct:]]", "a"));
        assert!(m("[[:cntrl:]]", "\u{01}") && !m("[[:cntrl:]]", "a"));
        assert!(m("[[:graph:]]", "!") && !m("[[:graph:]]", " "));
        assert!(m("[[:print:]]", " ") && m("[[:print:]]", "!") && !m("[[:print:]]", "\u{01}"));
        assert!(m("[[:blank:]]", " ") && m("[[:blank:]]", "\t") && !m("[[:blank:]]", "\n"));
    }
    #[test]
    fn negation_and_mixed() {
        assert!(m("[^[:digit:]]", "x") && !m("[^[:digit:]]", "5"));
        assert!(m("[[:digit:]_]", "5") && m("[[:digit:]_]", "_") && !m("[[:digit:]_]", "a"));
        assert!(m("[[:digit:]a-f]", "c") && m("[[:digit:]a-f]", "3") && !m("[[:digit:]a-f]", "z"));
    }
    #[test]
    fn unknown_class_matches_nothing() {
        assert!(!m("[[:bogus:]]", "x"));
        assert!(!m("[[:bogus:]]", ":"));
    }
    #[test]
    fn single_bracket_colon_is_literal_set() {
        // `[:y:]` (single bracket) is a literal set {':','y'}, NOT a class.
        assert!(m("[:y:]", ":") && m("[:y:]", "y") && !m("[:y:]", "z"));
    }
    #[test]
    fn has_posix_class_detection() {
        assert!(has_posix_class("[[:space:]]"));
        assert!(has_posix_class("x[[:digit:]]y"));
        assert!(has_posix_class("[^[:alpha:]]"));
        assert!(!has_posix_class("[abc]"));
        assert!(!has_posix_class("[a-z]"));
        assert!(!has_posix_class("plain*"));
        assert!(!has_posix_class("\\[[:x"));  // escaped, no close
    }
}
