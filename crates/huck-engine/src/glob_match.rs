//! Pattern matching, one chokepoint for every consumer (#717): `[[ == ]]`,
//! `case`, `${…#…}`/`${…/…}`, completion's `-X` filter and pathname
//! expansion all go through [`pattern_matches`] (or [`normalize`] + the
//! `glob` crate's directory walk for pathnames). It picks the engine — the
//! own matcher below for extglob, `[:class:]`, `[=c=]` and `[.c.]`, the
//! `glob` crate otherwise — and applies bash's bracket rules first, so a
//! pattern can no longer be "invalid": an unmatched `[` is an ordinary
//! character (`sm_loop.c` BRACKMATCH), exactly as in bash.

use std::borrow::Cow;

/// How a pattern is matched.
#[derive(Debug, Clone, Copy)]
pub struct MatchOpts {
    /// `shopt -s extglob`: whether `@(…)`-style groups are operators. (`[[ ]]`
    /// passes `true` unconditionally — bash always recognises them there.)
    pub extglob: bool,
    pub case_insensitive: bool,
}

/// Does `text` match `pattern`, in full? The one place the engine is chosen
/// and bash's bracket rules are applied.
pub fn pattern_matches(pattern: &str, text: &str, opts: MatchOpts) -> bool {
    // Engine choice is made on the RAW pattern, as bash's PATSCAN runs before
    // any bracket handling: an unmatched `[` inside `@(…)` swallows the `)`
    // and the group is not one. The own matcher applies the bracket rules
    // itself; only the `glob` crate needs the pattern rewritten.
    if needs_own_matcher(pattern, opts.extglob) {
        return extglob_match(pattern, text, opts.case_insensitive);
    }
    let Some(normalized) = normalize(pattern) else {
        return false;
    };
    compile_glob(&normalized).matches_with(
        text,
        glob::MatchOptions {
            case_sensitive: !opts.case_insensitive,
            require_literal_separator: false,
            require_literal_leading_dot: false,
        },
    )
}

/// Whether `pattern` needs the own matcher: an extended-glob group (when
/// `extglob` recognises them), or a `[:name:]`, `[=c=]` or `[.c.]` the
/// `glob` crate cannot express.
pub fn needs_own_matcher(pattern: &str, extglob: bool) -> bool {
    (extglob && has_extglob(pattern))
        || has_posix_class(pattern)
        || has_collating_symbol(pattern)
        || has_equivalence_class(pattern)
        || has_escaped_class_member(pattern)
}

/// A closed bracket expression with a `\c` member — `[a\-z]` is the three
/// members `a`, `-`, `z` in bash, which the `glob` crate (no escapes) would
/// read as a range once unescaped. The own matcher takes those.
fn has_escaped_class_member(pattern: &str) -> bool {
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            '[' => match bracket_close(&chars, i) {
                BracketEnd::Closed(close) => {
                    if chars[i..close].contains(&'\\') {
                        return true;
                    }
                    i = close + 1;
                }
                _ => i += 1,
            },
            _ => i += 1,
        }
    }
    false
}

/// bash's bracket rules, applied to a pattern before the `glob` crate sees it:
/// `[^…]` becomes `[!…]`, and a `[` whose bracket expression never closes is
/// rewritten as the one-member class `[[]` so it matches itself and matching
/// continues with the text after it — `[x` matches `[x`, `[*` matches `[`
/// then anything, `[]` matches `[]`. Borrowed back when nothing changes.
pub fn normalize(pattern: &str) -> Option<Cow<'_, str>> {
    // Literalize FIRST: `[^a` with no closing `]` is the three characters
    // `[^a`, not a negated class, so the `^` must not be rewritten.
    let lit = literalize_unmatched_brackets(pattern)?;
    Some(match lit {
        Cow::Borrowed(p) => translate_bracket_negation(p),
        Cow::Owned(p) => Cow::Owned(translate_bracket_negation(&p).into_owned()),
    })
}

/// Compile a NORMALIZED pattern for the `glob` crate. Cannot fail in practice
/// (normalization removed the one thing the crate rejects); should the crate
/// still object, the pattern matches itself literally rather than nothing.
pub(crate) fn compile_glob(normalized: &str) -> glob::Pattern {
    glob::Pattern::new(normalized).unwrap_or_else(|_| {
        glob::Pattern::new(&glob::Pattern::escape(normalized)).expect("an escaped pattern compiles")
    })
}

/// How a `[` ends, per bash's `sm_loop.c` BRACKMATCH.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BracketEnd {
    /// The index of the `]` that closes the bracket expression.
    Closed(usize),
    /// The pattern ended first: the `[` matches itself and matching
    /// continues with the text after it.
    Literal,
    /// A range whose end is missing (`[a-` at the end of the pattern, or
    /// `[a-\`): bash returns "no match" for ANY subject, not a literal.
    NeverMatches,
}

/// Port of the scan in bash's `sm_loop.c` BRACKMATCH from the `[` at `open`.
/// After an optional `!`/`^`, a first `]` is a member; `[:name:]`, `[=c=]`
/// and `[.c.]` are atomic (a `[:` with no `:]` is a plain `[` member); `\`
/// quotes the next member; a `-` range consumes its end member.
fn bracket_close(chars: &[char], open: usize) -> BracketEnd {
    let n = chars.len();
    let mut i = open + 1;
    if i < n && (chars[i] == '!' || chars[i] == '^') {
        i += 1;
    }
    let mut first = true;
    while i < n {
        let c = chars[i];
        if c == ']' && !first {
            return BracketEnd::Closed(i);
        }
        first = false;
        if c == '[' && i + 1 < n && matches!(chars[i + 1], ':' | '=' | '.') {
            let kind = chars[i + 1];
            // Find the matching `kind]`; without one the `[` is a plain member.
            let mut j = i + 2;
            let mut closed = None;
            while j + 1 < n {
                if chars[j] == kind && chars[j + 1] == ']' {
                    closed = Some(j + 1);
                    break;
                }
                j += 1;
            }
            if let Some(end) = closed {
                i = end + 1;
                continue;
            }
            i += 1;
            continue;
        }
        if c == '\\' {
            i += 2;
            continue;
        }
        // A range: `x-y` consumes `y` as a member too (unless `-` is last
        // before the `]`, when it is a literal `-`). A range with no end at
        // all — the pattern stops after the `-` (or after `-\`) — can never
        // match anything.
        if i + 1 < n && chars[i + 1] == '-' {
            if i + 2 >= n {
                return BracketEnd::NeverMatches;
            }
            if chars[i + 2] != ']' {
                i += 2;
                if chars[i] == '\\' {
                    if i + 1 >= n {
                        return BracketEnd::NeverMatches;
                    }
                    i += 1;
                }
            }
        }
        i += 1;
    }
    BracketEnd::Literal
}

/// bash's `gm_loop.c` MATCHLEN: the FIXED number of characters `pattern`
/// matches, or `None` when it can match any length (a `*`, or an extended
/// group). `?`, a backslash-escaped character and a closed bracket expression
/// each count one; an unterminated `[` counts one per character it swallowed
/// (so `[*` is two). `${v/pat/rep}` uses it to bound its search — bash tries
/// only substrings of exactly this length, which is why `${v/[*/X}` replaces
/// exactly two characters.
pub fn fixed_match_len(pattern: &str) -> Option<usize> {
    let chars: Vec<char> = pattern.chars().collect();
    let n = chars.len();
    let mut len = 0usize;
    let mut i = 0;
    while i < n {
        match chars[i] {
            '\\' => {
                len += 1;
                i += 2;
            }
            '*' => return None,
            '?' | '+' | '!' | '@' if i + 1 < n && chars[i + 1] == '(' => return None,
            '[' => match bracket_close(&chars, i) {
                BracketEnd::Closed(close) => {
                    len += 1;
                    i = close + 1;
                }
                // bash counts every character from the `[` to the end and
                // stops scanning there.
                BracketEnd::Literal | BracketEnd::NeverMatches => {
                    len += n - i;
                    return Some(len);
                }
            },
            _ => {
                len += 1;
                i += 1;
            }
        }
    }
    Some(len)
}

/// Rewrites bash-form pattern text for the `glob` crate, which has no
/// escape character: outside a bracket expression `\c` becomes the one-member
/// class `[c]` when `c` is a wildcard (`* ? [ ]`) and plain `c` otherwise; a
/// closed bracket expression is copied with its `\c` members unescaped; an
/// unmatched `[` becomes `[[]`. `None` when a bracket expression can never
/// match (see `BracketEnd`).
fn literalize_unmatched_brackets(pattern: &str) -> Option<Cow<'_, str>> {
    if !pattern.contains('[') && !pattern.contains('\\') {
        return Some(Cow::Borrowed(pattern));
    }
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len() + 4);
    let mut changed = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            changed = true;
            match chars.get(i + 1) {
                Some(&q) if matches!(q, '*' | '?' | '[' | ']') => {
                    out.push('[');
                    out.push(q);
                    out.push(']');
                }
                Some(&q) => out.push(q),
                // A trailing backslash is a literal one.
                None => out.push('\\'),
            }
            i += 2;
            continue;
        }
        if c == '[' {
            match bracket_close(&chars, i) {
                BracketEnd::Closed(close) => {
                    // Copy the expression, unescaping `\c` members.
                    let mut j = i;
                    while j <= close {
                        if chars[j] == '\\' && j < close {
                            out.push(chars[j + 1]);
                            changed = true;
                            j += 2;
                        } else {
                            out.push(chars[j]);
                            j += 1;
                        }
                    }
                    i = close + 1;
                }
                BracketEnd::Literal => {
                    out.push_str("[[]");
                    changed = true;
                    i += 1;
                }
                BracketEnd::NeverMatches => return None,
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    Some(if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(pattern)
    })
}

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
    let mut negated = false; // class opened with `!` or `^`
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
    // `ascii` is a glibc `wctype`/fnmatch extension beyond the 12 POSIX.2
    // classes (v119's original scope) — bash's own posixpat.tests exercises
    // it (`[[:alpha:]][[=b=]][[:ascii:]]`), so it needs to resolve too or
    // that line never flips even with equivalence classes wired up.
    Ascii,
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
        "ascii" => Ascii,
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
        Ascii => c.is_ascii(),
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
            // A bracket expression is opaque to group detection; an
            // unmatched `[` is just a character (bash's BRACKMATCH).
            '[' => match bracket_close(&b, i) {
                BracketEnd::Closed(close) => i = close + 1,
                _ => i += 1,
            },
            '?' | '*' | '+' | '@' | '!'
                if i + 1 < b.len() && b[i + 1] == '(' && group_close(&b, i + 2).is_some() =>
            {
                return true;
            }
            _ => i += 1,
        }
    }
    false
}

/// bash's PATSCAN: from just after a group's `(`, the index of the `)` that
/// closes it — nested groups balance, and `(`/`)`/`|` inside a bracket
/// expression are not special, so a `[` that never closes swallows the rest
/// of the pattern and the group is not one (`@([x)` is five literal
/// characters).
pub(crate) fn group_close(chars: &[char], from: usize) -> Option<usize> {
    let n = chars.len();
    let mut depth = 0usize;
    let mut i = from;
    while i < n {
        match chars[i] {
            '\\' => i += 2,
            '[' => match bracket_close(chars, i) {
                BracketEnd::Closed(close) => i = close + 1,
                _ => return None,
            },
            '(' => {
                depth += 1;
                i += 1;
            }
            ')' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
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

/// POSIX.2 table 2.8 collating-symbol names → their C-locale character.
/// Authored from the POSIX standard (NOT copied from bash's GPL collsyms.h).
/// Upper/lower letters are intentionally omitted — single-char passthrough in
/// `collsym` covers them; digits are listed by name here.
static POSIX_COLLSYMS: &[(&str, char)] = &[
    ("NUL", '\0'),
    ("SOH", '\u{01}'),
    ("STX", '\u{02}'),
    ("ETX", '\u{03}'),
    ("EOT", '\u{04}'),
    ("ENQ", '\u{05}'),
    ("ACK", '\u{06}'),
    ("alert", '\u{07}'),
    ("BS", '\u{08}'),
    ("backspace", '\u{08}'),
    ("HT", '\t'),
    ("tab", '\t'),
    ("LF", '\n'),
    ("newline", '\n'),
    ("VT", '\u{0b}'),
    ("vertical-tab", '\u{0b}'),
    ("FF", '\u{0c}'),
    ("form-feed", '\u{0c}'),
    ("CR", '\r'),
    ("carriage-return", '\r'),
    ("SO", '\u{0e}'),
    ("SI", '\u{0f}'),
    ("DLE", '\u{10}'),
    ("DC1", '\u{11}'),
    ("DC2", '\u{12}'),
    ("DC3", '\u{13}'),
    ("DC4", '\u{14}'),
    ("NAK", '\u{15}'),
    ("SYN", '\u{16}'),
    ("ETB", '\u{17}'),
    ("CAN", '\u{18}'),
    ("EM", '\u{19}'),
    ("SUB", '\u{1a}'),
    ("ESC", '\u{1b}'),
    ("IS4", '\u{1c}'),
    ("FS", '\u{1c}'),
    ("IS3", '\u{1d}'),
    ("GS", '\u{1d}'),
    ("IS2", '\u{1e}'),
    ("RS", '\u{1e}'),
    ("IS1", '\u{1f}'),
    ("US", '\u{1f}'),
    ("space", ' '),
    ("exclamation-mark", '!'),
    ("quotation-mark", '"'),
    ("number-sign", '#'),
    ("dollar-sign", '$'),
    ("percent-sign", '%'),
    ("ampersand", '&'),
    ("apostrophe", '\''),
    ("left-parenthesis", '('),
    ("right-parenthesis", ')'),
    ("asterisk", '*'),
    ("plus-sign", '+'),
    ("comma", ','),
    ("hyphen", '-'),
    ("hyphen-minus", '-'),
    ("minus", '-'),
    ("dash", '-'),
    ("period", '.'),
    ("full-stop", '.'),
    ("slash", '/'),
    ("solidus", '/'),
    ("zero", '0'),
    ("one", '1'),
    ("two", '2'),
    ("three", '3'),
    ("four", '4'),
    ("five", '5'),
    ("six", '6'),
    ("seven", '7'),
    ("eight", '8'),
    ("nine", '9'),
    ("colon", ':'),
    ("semicolon", ';'),
    ("less-than-sign", '<'),
    ("equals-sign", '='),
    ("greater-than-sign", '>'),
    ("question-mark", '?'),
    ("commercial-at", '@'),
    ("left-square-bracket", '['),
    ("backslash", '\\'),
    ("reverse-solidus", '\\'),
    ("right-square-bracket", ']'),
    ("circumflex", '^'),
    ("circumflex-accent", '^'),
    ("underscore", '_'),
    ("grave-accent", '`'),
    ("left-brace", '{'),
    ("left-curly-bracket", '{'),
    ("vertical-line", '|'),
    ("right-brace", '}'),
    ("right-curly-bracket", '}'),
    ("tilde", '~'),
    ("DEL", '\u{7f}'),
];

/// Resolve a POSIX.2 collating-symbol name: table lookup, else a single
/// character is a collating element for itself, else invalid (`None`).
fn collsym(name: &str) -> Option<char> {
    if let Some(&(_, c)) = POSIX_COLLSYMS.iter().find(|&&(n, _)| n == name) {
        return Some(c);
    }
    let mut it = name.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// True if `pattern` contains a collating symbol `[.` … `.]` (unescaped `[`).
/// Mirrors `has_posix_class`.
pub fn has_collating_symbol(pattern: &str) -> bool {
    let b: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '\\' {
            i += 2;
            continue;
        }
        if b[i] == '[' && i + 1 < b.len() && b[i + 1] == '.' {
            let mut j = i + 2;
            while j + 1 < b.len() {
                if b[j] == '.' && b[j + 1] == ']' {
                    return true;
                }
                j += 1;
            }
        }
        i += 1;
    }
    false
}

/// True if `pattern` contains an equivalence class `[=` … `=]` (unescaped
/// `[`). Mirrors `has_collating_symbol`.
pub fn has_equivalence_class(pattern: &str) -> bool {
    let b: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '\\' {
            i += 2;
            continue;
        }
        if b[i] == '[' && i + 1 < b.len() && b[i + 1] == '=' {
            let mut j = i + 2;
            while j + 1 < b.len() {
                if b[j] == '=' && b[j + 1] == ']' {
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
                // bash's BRACKMATCH: a `[` whose bracket expression never
                // closes matches itself, and matching continues after it.
                match bracket_close(chars, *pos) {
                    BracketEnd::Closed(_) => items.push(parse_class(chars, pos)),
                    BracketEnd::Literal => {
                        items.push(Item::Lit('['));
                        *pos += 1;
                    }
                    BracketEnd::NeverMatches => {
                        // An empty, non-negated class matches nothing, ever.
                        items.push(Item::Class {
                            negated: false,
                            set: Vec::new(),
                        });
                        *pos = chars.len();
                    }
                }
            }
            '?' | '*' | '+' | '@' | '!'
                if *pos + 1 < chars.len()
                    && chars[*pos + 1] == '('
                    && group_close(chars, *pos + 2).is_some() =>
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
    // Whether an atom is SHAPE-eligible to sit on one side of a `-` range,
    // as distinct from whether it resolved to a valid char. A `[:class:]` is
    // structurally never a range endpoint (bash leaks the `-`/next-atom as
    // literals after it). A plain char or a `[.sym.]` collating token IS
    // shape-eligible even when the symbol name is invalid — bash then
    // consumes the whole `atom '-' atom` span as a single failed (no-match)
    // range rather than leaking the `-`/second atom as literals.
    enum RangeEp {
        NotEligible,
        Eligible(Option<char>),
    }

    // Parse one bracket atom at `k`: a `[:class:]`, a `[.sym.]`, or a plain
    // char. Returns (standalone atom, range-endpoint eligibility/value, index
    // past the atom). Assumes chars[k] != ']' (the caller handles the
    // closing bracket).
    fn parse_atom(chars: &[char], k: usize) -> (ClassAtom, RangeEp, usize) {
        // [:name:] POSIX class — not a range endpoint.
        #[allow(clippy::collapsible_if)] // keep the explicit fall-through comment.
        if chars[k] == '[' && k + 1 < chars.len() && chars[k + 1] == ':' {
            if let Some(close) = (k + 2..chars.len().saturating_sub(1))
                .find(|&j| chars[j] == ':' && chars[j + 1] == ']')
            {
                let name: String = chars[k + 2..close].iter().collect();
                let atom = match posix_class_from_name(&name) {
                    Some(pc) => ClassAtom::Posix(pc),
                    None => ClassAtom::Never,
                };
                return (atom, RangeEp::NotEligible, close + 2);
            }
        }
        // [.name.] collating symbol — always range-eligible; an invalid name
        // yields Eligible(None) so a range attempt still consumes the span.
        #[allow(clippy::collapsible_if)] // keep the explicit fall-through comment.
        if chars[k] == '[' && k + 1 < chars.len() && chars[k + 1] == '.' {
            if let Some(close) = (k + 2..chars.len().saturating_sub(1))
                .find(|&j| chars[j] == '.' && chars[j + 1] == ']')
            {
                let name: String = chars[k + 2..close].iter().collect();
                let val = collsym(&name);
                let atom = match val {
                    Some(c) => ClassAtom::Ch(c),
                    None => ClassAtom::Never,
                };
                return (atom, RangeEp::Eligible(val), close + 2);
            }
        }
        // [=name=] equivalence class — in the C/POSIX locale each char is its
        // own class, so this resolves to the SAME char as a `[.name.]`
        // collating symbol (via the same `collsym` lookup), but it is NOT a
        // range endpoint (unlike a collating symbol) — bash leaks the `-`/
        // next-atom as literals after it, matching `[:class:]`.
        // NOTE: a negated bracket ending in an equivalence class matches
        // nothing in bash 5.2 (a bash quirk); huck negates normally — kept
        // by design (docs/bash-divergences.md).
        #[allow(clippy::collapsible_if)] // keep the explicit fall-through comment.
        if chars[k] == '[' && k + 1 < chars.len() && chars[k + 1] == '=' {
            if let Some(close) = (k + 2..chars.len().saturating_sub(1))
                .find(|&j| chars[j] == '=' && chars[j + 1] == ']')
            {
                let name: String = chars[k + 2..close].iter().collect();
                let atom = match collsym(&name) {
                    Some(c) => ClassAtom::Ch(c),
                    None => ClassAtom::Never,
                };
                return (atom, RangeEp::NotEligible, close + 2);
            }
        }
        // `\c`: a quoted member (bash-form pattern text).
        if chars[k] == '\\' && k + 1 < chars.len() {
            return (
                ClassAtom::Ch(chars[k + 1]),
                RangeEp::Eligible(Some(chars[k + 1])),
                k + 2,
            );
        }
        // Plain char.
        (
            ClassAtom::Ch(chars[k]),
            RangeEp::Eligible(Some(chars[k])),
            k + 1,
        )
    }

    let mut closed = false;
    while i < chars.len() {
        if chars[i] == ']' {
            closed = true;
            i += 1;
            break;
        }
        let (atom, lo_ep, after) = parse_atom(chars, i);
        // Range: <atom> '-' <atom>, where the '-' is not the trailing set
        // char, gated on SHAPE-eligibility of the first atom (not on its
        // value being resolved — an invalid collating endpoint still forms
        // (and fails) a range instead of leaking as literals).
        let mut consumed_as_range = false;
        if let RangeEp::Eligible(lo_val) = lo_ep
            && after < chars.len()
            && chars[after] == '-'
            && after + 1 < chars.len()
            && chars[after + 1] != ']'
        {
            let (_batom, hi_ep, after2) = parse_atom(chars, after + 1);
            let hi_val = match hi_ep {
                RangeEp::Eligible(v) => v,
                RangeEp::NotEligible => None,
            };
            match (lo_val, hi_val) {
                (Some(lo), Some(hi)) => set.push(ClassAtom::Range(lo, hi)),
                _ => set.push(ClassAtom::Never), // invalid collating endpoint(s)
            }
            i = after2;
            consumed_as_range = true;
        }
        if !consumed_as_range {
            set.push(atom);
            i = after;
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
    if ci { lc(a) == lc(b) } else { a == b }
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
        Item::Lit(c) => {
            !text.is_empty() && eqc(text[0], *c, ci) && match_here(rest, &text[1..], ci)
        }
        Item::AnyChar => !text.is_empty() && match_here(rest, &text[1..], ci),
        Item::AnyRun => (0..=text.len()).any(|k| match_here(rest, &text[k..], ci)),
        Item::Class { negated, set } => {
            !text.is_empty()
                && class_matches(set, *negated, text[0], ci)
                && match_here(rest, &text[1..], ci)
        }
        Item::Group { kind, alts } => match kind {
            GroupKind::ExactlyOne => (0..=text.len()).any(|k| {
                alt_matches_whole(alts, &text[..k], ci) && match_here(rest, &text[k..], ci)
            }),
            GroupKind::ZeroOrOne => {
                match_here(rest, text, ci)
                    || (1..=text.len()).any(|k| {
                        alt_matches_whole(alts, &text[..k], ci) && match_here(rest, &text[k..], ci)
                    })
            }
            GroupKind::ZeroOrMore => match_star(alts, rest, text, ci),
            GroupKind::OneOrMore => (1..=text.len()).any(|k| {
                alt_matches_whole(alts, &text[..k], ci) && match_star(alts, rest, &text[k..], ci)
            }),
            GroupKind::Not => (0..=text.len()).any(|k| {
                !alt_matches_whole(alts, &text[..k], ci) && match_here(rest, &text[k..], ci)
            }),
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
    let start = if absolute {
        "/".to_string()
    } else {
        String::new()
    };
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
    // ---- #717: bash's bracket rules at the chokepoint ----------------------

    fn pm(pattern: &str, text: &str) -> bool {
        pattern_matches(
            pattern,
            text,
            MatchOpts {
                extglob: true,
                case_insensitive: false,
            },
        )
    }

    /// An unmatched `[` is an ordinary character (`sm_loop.c` BRACKMATCH).
    #[test]
    fn unmatched_bracket_is_a_literal() {
        assert!(pm("[x", "[x"));
        assert!(!pm("[ab", "[b"));
        assert!(pm("[", "["));
        assert!(pm("[]", "[]"));
        assert!(pm("[*", "[anything"));
        assert!(!pm("[*", "x"));
        assert!(pm("[!a", "[!a"));
        assert!(pm("[^a", "[^a"));
        // After the literal `[`, `[:alpha:]` is an ORDINARY bracket
        // expression — the set `{: a l p h}` — not a class.
        assert!(pm("[[:alpha:]", "[a"));
        assert!(!pm("[[:alpha:]", "[x"));
        assert!(!pm("[[:alpha:]", "x"));
        assert!(pm("[[:alpha:", "[[:alpha:"));
    }

    /// A range with no end can never match — not even literally.
    #[test]
    fn dangling_range_never_matches() {
        assert!(!pm("[a-", "[a-"));
        assert!(!pm("[a-", "a"));
        assert_eq!(normalize("[a-"), None);
        assert!(pm("[a-]", "-"));
    }

    /// Matched bracket expressions are untouched by normalization.
    #[test]
    fn normalize_rewrites_only_the_unmatched() {
        assert_eq!(normalize("[x").unwrap(), "[[]x");
        assert_eq!(normalize("[]").unwrap(), "[[]]");
        assert_eq!(normalize("[^a").unwrap(), "[[]^a");
        assert_eq!(normalize("[^a]").unwrap(), "[!a]");
        assert_eq!(normalize("[]a]").unwrap(), "[]a]");
        assert_eq!(normalize("a[[:alpha:]]b").unwrap(), "a[[:alpha:]]b");
        assert!(matches!(normalize("plain*"), Some(Cow::Borrowed(_))));
    }

    /// PATSCAN: a `[` that never closes inside `@(…)` swallows the `)`, so
    /// the group is not one and the whole thing is literal.
    #[test]
    fn group_detection_runs_before_brackets() {
        assert!(!has_extglob("@([x)"));
        assert!(!has_extglob("@(a|[b)"));
        assert!(has_extglob("[@(a|b)"));
        assert!(has_extglob("@(a)[a"));
        assert!(pm("@([x)", "@([x)"));
        assert!(!pm("@([x)", "[x"));
        assert!(pm("[@(a|b)", "[a"));
    }

    /// `gm_loop.c` MATCHLEN: the fixed length `${v/…}` searches with.
    #[test]
    fn fixed_match_len_follows_matchlen() {
        assert_eq!(fixed_match_len("abc"), Some(3));
        assert_eq!(fixed_match_len("a?c"), Some(3));
        assert_eq!(fixed_match_len("a*c"), None);
        assert_eq!(fixed_match_len("[ab]c"), Some(2));
        assert_eq!(fixed_match_len("[*"), Some(2));
        assert_eq!(fixed_match_len("[*c"), Some(3));
        assert_eq!(fixed_match_len("[b*"), Some(3));
        assert_eq!(fixed_match_len("\\*x"), Some(2));
        assert_eq!(fixed_match_len("@(a)"), None);
        assert_eq!(fixed_match_len(""), Some(0));
    }

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
        assert_eq!(
            got,
            abs(&base, &["aab", "abc", "b", "cd", "dir1", "dir2", "xy"])
        );
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

    fn t(p: &str) -> String {
        translate_bracket_negation(p).into_owned()
    }

    #[test]
    fn leading_caret_becomes_bang() {
        assert_eq!(t("[^abc]"), "[!abc]");
        assert_eq!(t("[^0-9]"), "[!0-9]");
    }
    #[test]
    fn bang_unchanged() {
        assert_eq!(t("[!abc]"), "[!abc]");
    }
    #[test]
    fn plain_class_unchanged() {
        assert_eq!(t("[abc]"), "[abc]");
    }
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
        assert_eq!(t("[[:alpha:]]"), "[[:alpha:]]"); // no leading ^
        assert_eq!(t("[^[:digit:]]"), "[![:digit:]]"); // leading ^ converted
    }
    #[test]
    fn no_change_returns_borrowed() {
        assert!(matches!(
            translate_bracket_negation("[abc]"),
            Cow::Borrowed(_)
        ));
        assert!(matches!(
            translate_bracket_negation("plain"),
            Cow::Borrowed(_)
        ));
    }
}

#[cfg(test)]
mod posix_class_tests {
    use super::{
        collsym, extglob_match, has_collating_symbol, has_equivalence_class, has_posix_class,
    };

    fn m(p: &str, t: &str) -> bool {
        extglob_match(p, t, false)
    }

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
    fn ascii_class_glibc_extension() {
        // `ascii` is a glibc fnmatch extension beyond POSIX's 12 classes;
        // bash's own posixpat.tests exercises it alongside equivalence
        // classes (`[[:alpha:]][[=b=]][[:ascii:]]`).
        assert!(m("[[:ascii:]]", "A"));
        assert!(m("[[:ascii:]]", "\u{7f}")); // DEL — still ASCII (0-127)
        assert!(!m("[[:ascii:]]", "\u{80}")); // first non-ASCII byte value
        assert!(!m("[[:ascii:]]", "é"));
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
        assert!(!has_posix_class("\\[[:x")); // escaped, no close
    }
    #[test]
    fn collsym_named_and_single_and_invalid() {
        assert_eq!(collsym("hyphen"), Some('-'));
        assert_eq!(collsym("space"), Some(' '));
        assert_eq!(collsym("grave-accent"), Some('`'));
        assert_eq!(collsym("newline"), Some('\n'));
        assert_eq!(collsym("period"), Some('.'));
        assert_eq!(collsym("a"), Some('a')); // single-char passthrough (letters omitted from table)
        assert_eq!(collsym("-"), Some('-')); // single-char passthrough
        assert_eq!(collsym("Z"), Some('Z'));
        assert_eq!(collsym("zz"), None); // multi-char non-name → invalid
        assert_eq!(collsym("yyz"), None);
    }
    #[test]
    fn has_collating_symbol_detects() {
        assert!(has_collating_symbol("[[.a.]]"));
        assert!(has_collating_symbol("x[[.hyphen.]-9]y"));
        assert!(!has_collating_symbol("[[:alpha:]]")); // that's a class, not a collating symbol
        assert!(!has_collating_symbol("[abc]"));
        assert!(!has_collating_symbol("plain"));
        assert!(!has_collating_symbol("\\[.a.]")); // escaped `[` — not a bracket
    }
    #[test]
    fn collating_symbol_matching() {
        // single-char and named collating elements
        assert!(m("[[.a.]]", "a"));
        assert!(!m("[[.a.]]", "b"));
        assert!(m("[[.hyphen.]]", "-"));
        assert!(m("[[.space.]]", " "));
        assert!(!m("[[.grave-accent.]]", " ")); // ` != space  → posixpat ok 6
        // collating symbols as range endpoints
        assert!(m("[[.a.]-[.z.]]", "p")); // ok 3
        assert!(m("[[.hyphen.]-9]", "-")); // ok 2
        assert!(m("[[.-.]-9]", "4")); // ok 7
        assert!(!m("[[.a.]-[.Z.]]", "p")); // reversed range → no match → ok 11
        // invalid collating symbols (multi-char non-names)
        assert!(!m("[[.yyz.]-[.z.]]", "c")); // invalid range start → ok 8
        assert!(m("[[.yyz.][.a.]-z]", "c")); // invalid atom + valid range → ok 9
        assert!(m("[[.a.]-[.zz.]p]", "p")); // invalid range end, literal p → ok 12
        assert!(m("[[.aa.]-[.z.]p]", "p")); // invalid range start, literal p → ok 13
        // invalid collating range endpoint consumes the WHOLE `atom-atom`
        // span as a single failed range (bash 5.2.21, LC_ALL=C confirmed) —
        // it must NOT leak the '-' or the second atom as separate literals.
        assert!(!m("[[.aa.]-[.z.]p]", "z"));
        assert!(!m("[[.aa.]-[.z.]p]", "-"));
        assert!(!m("[[.yyz.]-[.z.]]", "z"));
        assert!(!m("[[.yyz.]-[.z.]]", "-"));
        // negation composes
        assert!(!m("[![.a.]]", "a"));
        assert!(m("[![.a.]]", "b"));
        // mixed with a POSIX class
        assert!(m("[[:digit:][.hyphen.]]", "-"));
        assert!(m("[[:digit:][.hyphen.]]", "5"));
        // no regression on plain ranges / literals
        assert!(m("[a-z]", "m"));
        assert!(!m("[a-z]", "M"));
        assert!(m("[a-]", "-")); // trailing '-' is literal
        assert!(m("[]a]", "]")); // ']' first is literal
    }
    #[test]
    fn has_equivalence_class_detects() {
        assert!(has_equivalence_class("[[=b=]]"));
        assert!(has_equivalence_class("x[[=b=]]y"));
        assert!(!has_equivalence_class("[[.b.]]")); // that's a collating symbol, not an equiv class
        assert!(!has_equivalence_class("[[:alpha:]]")); // that's a class, not an equiv class
        assert!(!has_equivalence_class("[abc]"));
        assert!(!has_equivalence_class("plain"));
        assert!(!has_equivalence_class("\\[=b=]")); // escaped `[` — not a bracket
    }
    #[test]
    fn equivalence_class_matching() {
        // C locale: [[=x=]] matches ONLY x (each char is its own class).
        assert!(m("[[=b=]]", "b"));
        assert!(!m("[[=b=]]", "c"));
        // composes with adjacent POSIX classes (posixpat.tests ok 1)
        assert!(m("[[:alpha:]][[=b=]][[:ascii:]]", "abc"));
        assert!(!m("[[:alpha:]][[=b=]][[:ascii:]]", "azc"));
        // Negation composes. DELIBERATE DIVERGENCE (kept by design, see
        // docs/bash-divergences.md): bash 5.2 has a quirk where a NEGATED
        // bracket whose LAST atom is an equivalence class matches NOTHING
        // (`[![=b=]]` → no match for any char); huck negates normally
        // (matches any char but `b`).
        assert!(m("[![=b=]]", "c")); // huck negates normally; bash matches nothing here
        assert!(!m("[![=b=]]", "b"));
        // When the equivalence class is NOT the last atom, bash negates
        // normally and huck agrees (byte-identical) — the quirk is specific
        // to the trailing `=]]`.
        assert!(m("[![=b=]x]", "c")); // not b, not x
        assert!(!m("[![=b=]x]", "b"));
        assert!(!m("[![=b=]x]", "x"));
        // not a range endpoint: `-` and the next atom leak as literals,
        // mirroring `[:class:]` (NotEligible), unlike `[.sym.]`.
        assert!(m("[[=a=]-z]", "-"));
        assert!(m("[[=a=]-z]", "z"));
        assert!(!m("[[=a=]-z]", "m"));
        // invalid multi-char non-name → no match
        assert!(!m("[[=zz=]]", "z"));
    }
}
