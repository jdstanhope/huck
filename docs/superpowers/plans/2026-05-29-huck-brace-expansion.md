# huck v46 — Brace Expansion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add bash brace expansion (`{a,b,c}`, `{1..5}`, `{a..e}`,
`{01..10}`, `{1..10..2}`, prefix/suffix, Cartesian, nested) to
huck's lexer.

**Architecture:** New `src/brace_expand.rs` module containing the
recursive expansion algorithm. Lexer integration in `src/lexer.rs`
routes every `Token::Word` emission through a new
`emit_word_with_braces` helper that detects unquoted braces and
emits one Word per expansion. New `LexError::BraceExpansionLimit`
variant for the safety cap (65,536 expansions).

**Tech Stack:** Rust. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-29-huck-brace-expansion-design.md`

**Branch:** `v46-brace-expansion` (created in preamble step P.1).

**Commit trailer convention**:

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble: Create the working branch

- [ ] **Step P.1: Create branch from main and check it out**

```bash
git checkout main
git pull --ff-only
git checkout -b v46-brace-expansion
```

Expected: `Switched to a new branch 'v46-brace-expansion'`.

The spec + this plan are committed as the first commit on this
branch (handled by the controller before Task 1 begins).

---

## Task 1: Module + lexer integration + 17 unit tests

**Files:**
- Create: `src/brace_expand.rs` — `expand` + `parse_body` +
  `BraceError` + 12 unit tests.
- Modify: `src/main.rs` — add `mod brace_expand;`.
- Modify: `src/lexer.rs` — add `LexError::BraceExpansionLimit`
  variant; add `word_contains_unquoted_brace`,
  `build_concat_with_sentinels`, `split_on_sentinels`,
  `emit_word_with_braces` helpers; replace 9 sites of
  `tokens.push(Token::Word(Word(parts)))` with the helper; append
  5 lexer unit tests.
- Modify: `src/shell.rs` — add `LexError::BraceExpansionLimit` arm
  in `lex_error_message`.

### Step 1.1: Create `src/brace_expand.rs` with `expand` + helpers

Create `src/brace_expand.rs` with this content:

```rust
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
    if let Some(items) = split_top_level_commas(body) {
        if items.len() >= 2 {
            return Some(items);
        }
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

    // Try char range.
    let left_chars: Vec<char> = left.chars().collect();
    let right_chars: Vec<char> = right.chars().collect();
    if left_chars.len() == 1 && right_chars.len() == 1 {
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
```

- [ ] **Step 1.1: Create the module file**

### Step 1.2: Wire the module into the crate

In `src/main.rs:1-16`, find the existing module declarations
(`mod arith;`, `mod builtins;`, etc.). Add `mod brace_expand;`
between `mod arith;` and `mod builtins;` (alphabetical order):

```rust
mod arith;
mod brace_expand;
mod builtins;
mod command;
// ... rest unchanged
```

- [ ] **Step 1.2: Add `mod brace_expand;`**

### Step 1.3: Build to confirm the module compiles

Run: `cargo build`
Expected: clean. New module is unused at this point.

- [ ] **Step 1.3: Build clean**

### Step 1.4: Run the 12 brace_expand unit tests

Run: `cargo test --bin huck brace_expand:: -- --nocapture`
Expected: 12 tests pass.

If any test fails: the algorithm has a bug. Most likely culprits:
- `parse_range` edge cases for zero-pad width or step direction.
- `find_matching_rbrace` missing the increment on nested `{`.
- Cartesian/nested recursion order.

Iterate until all 12 pass before moving on.

- [ ] **Step 1.4: 12 brace_expand tests pass**

### Step 1.5: Add `LexError::BraceExpansionLimit` variant

In `src/lexer.rs:1-16`, find the `LexError` enum (the current
shape is the v39 form ending with `AnsiCInvalidCodepoint(u32)`).
Add `BraceExpansionLimit` as the last variant:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum LexError {
    UnterminatedQuote,
    InvalidVarName,
    UnterminatedBrace,
    UnterminatedSubstitution,
    UnterminatedArith,
    ArithParse(String),
    InvalidBraceModifier(String),
    EmptyParamName,
    InvalidBraceOperand,
    Substitution(Box<LexError>),
    SubstitutionParseError(crate::command::ParseError),
    UnterminatedHeredoc,
    AnsiCInvalidCodepoint(u32),
    BraceExpansionLimit,
}
```

- [ ] **Step 1.5: Add the variant**

### Step 1.6: Add `lex_error_message` arm in `src/shell.rs`

In `src/shell.rs`, find `fn lex_error_message` (around line 305).
Find the `LexError::AnsiCInvalidCodepoint(v) => ...` arm (added
in v39) and add a new arm IMMEDIATELY after it (still before the
closing `}` of the match):

```rust
        LexError::BraceExpansionLimit => ": brace expansion: too many elements".to_string(),
```

- [ ] **Step 1.6: Add the arm**

### Step 1.7: Build

Run: `cargo build`
Expected: clean — the new `LexError` variant is matched
exhaustively in `lex_error_message`.

- [ ] **Step 1.7: Build clean**

### Step 1.8: Add lexer helpers in `src/lexer.rs`

In `src/lexer.rs`, find the `flush_literal` function (around
line 462 — search for `fn flush_literal`). **Immediately above**
`fn flush_literal`, insert these four new helpers:

```rust
/// Returns true if any unquoted Literal part in `parts` contains
/// an unquoted `{`. The fast-path check for brace expansion.
fn word_contains_unquoted_brace(parts: &[WordPart]) -> bool {
    parts.iter().any(|p| {
        matches!(p, WordPart::Literal { text, quoted: false } if text.contains('{'))
    })
}

/// Builds a concat string for brace expansion. Unquoted Literal
/// text is appended verbatim. Other parts (quoted Literals, Var,
/// Arith, CommandSub, Tilde, etc.) get a sentinel block
/// `\u{0001}<idx>\u{0002}` and are recorded in `placeholders`.
fn build_concat_with_sentinels(parts: &[WordPart]) -> (String, Vec<WordPart>) {
    let mut concat = String::new();
    let mut placeholders: Vec<WordPart> = Vec::new();
    for p in parts {
        match p {
            WordPart::Literal { text, quoted: false } => {
                concat.push_str(text);
            }
            other => {
                let idx = placeholders.len();
                placeholders.push(other.clone());
                concat.push('\u{0001}');
                concat.push_str(&idx.to_string());
                concat.push('\u{0002}');
            }
        }
    }
    (concat, placeholders)
}

/// Walks an expanded brace-expansion string and reconstructs a
/// `Vec<WordPart>`. Literal runs (no sentinels) become Literals
/// with `quoted: false`. Each sentinel block `\u{0001}<idx>\u{0002}`
/// is replaced by `placeholders[idx].clone()`.
fn split_on_sentinels(s: &str, placeholders: &[WordPart]) -> Vec<WordPart> {
    let mut out: Vec<WordPart> = Vec::new();
    let mut buf = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{0001}' {
            if !buf.is_empty() {
                out.push(WordPart::Literal { text: std::mem::take(&mut buf), quoted: false });
            }
            let mut idx_str = String::new();
            while let Some(&nc) = chars.peek() {
                if nc == '\u{0002}' {
                    chars.next();
                    break;
                }
                idx_str.push(nc);
                chars.next();
            }
            if let Ok(idx) = idx_str.parse::<usize>() {
                if let Some(p) = placeholders.get(idx) {
                    out.push(p.clone());
                }
            }
        } else {
            buf.push(c);
        }
    }
    if !buf.is_empty() {
        out.push(WordPart::Literal { text: buf, quoted: false });
    }
    out
}

/// Emits a Word into `tokens`. If the parts contain an unquoted
/// `{`, runs brace expansion and emits one Word per expansion.
fn emit_word_with_braces(
    tokens: &mut Vec<Token>,
    parts: Vec<WordPart>,
) -> Result<(), LexError> {
    if !word_contains_unquoted_brace(&parts) {
        tokens.push(Token::Word(Word(parts)));
        return Ok(());
    }
    let (concat, placeholders) = build_concat_with_sentinels(&parts);
    let expansions = crate::brace_expand::expand(&concat)
        .map_err(|_| LexError::BraceExpansionLimit)?;
    for s in expansions {
        let new_parts = split_on_sentinels(&s, &placeholders);
        tokens.push(Token::Word(Word(new_parts)));
    }
    Ok(())
}
```

- [ ] **Step 1.8: Insert the four helpers**

### Step 1.9: Replace all 9 `tokens.push(Token::Word(...))` sites

In `src/lexer.rs`, there are exactly 9 sites where
`tokens.push(Token::Word(Word(...)))` is called. Find each and
replace with `emit_word_with_braces(&mut tokens, ...)?;`.

The 9 sites are at lines 146, 280, 294, 316, 339, 348, 357, 397,
451 (run `grep -n "tokens.push(Token::Word" src/lexer.rs` to
verify the current line numbers if any have shifted).

Each site looks like one of two forms:

Form A (uses `std::mem::take`):

```rust
tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
```

Form B (final emission at end of function):

```rust
tokens.push(Token::Word(Word(parts)));
```

For Form A, replace with:

```rust
emit_word_with_braces(&mut tokens, std::mem::take(&mut parts))?;
```

For Form B (at the end of the function, only at line 451), replace
with:

```rust
emit_word_with_braces(&mut tokens, parts)?;
```

After replacement, run `grep -n "tokens.push(Token::Word" src/lexer.rs`.
Expected: zero matches.

- [ ] **Step 1.9: Replace all 9 sites**

### Step 1.10: Build

Run: `cargo build`
Expected: clean. The `?` in `emit_word_with_braces` returns
`LexError`, which `tokenize` already returns.

- [ ] **Step 1.10: Build clean**

### Step 1.11: Run brace_expand tests + full lexer tests

Run: `cargo test --bin huck brace_expand:: -- --nocapture`
Expected: 12 brace_expand tests still pass.

Run: `cargo test --bin huck lexer::tests`
Expected: all existing lexer tests pass.

- [ ] **Step 1.11: Lexer tests still green**

### Step 1.12: Add 5 lexer-level unit tests

In `src/lexer.rs`, find `#[cfg(test)] mod tests` (near the bottom).
Append these 5 tests inside the mod block, before its closing `}`:

```rust
    #[test]
    fn tokenize_brace_emits_multiple_words() {
        let toks = tokenize("echo {a,b,c}").expect("lex");
        // Should produce 4 Word tokens: echo, a, b, c (plus any
        // separators we don't care about).
        let word_texts: Vec<String> = toks
            .iter()
            .filter_map(|t| match t {
                Token::Word(Word(parts)) => {
                    let s: String = parts
                        .iter()
                        .filter_map(|p| match p {
                            WordPart::Literal { text, .. } => Some(text.clone()),
                            _ => None,
                        })
                        .collect();
                    Some(s)
                }
                _ => None,
            })
            .collect();
        assert_eq!(word_texts, vec!["echo", "a", "b", "c"]);
    }

    #[test]
    fn tokenize_brace_preserves_var() {
        let toks = tokenize("echo $x{a,b}").expect("lex");
        // First word is `echo`. Then two more Words, each with
        // a Var part followed by a Literal part.
        let word_tokens: Vec<&Vec<WordPart>> = toks
            .iter()
            .filter_map(|t| match t {
                Token::Word(Word(parts)) => Some(parts),
                _ => None,
            })
            .collect();
        assert_eq!(word_tokens.len(), 3);
        // word_tokens[0] is `echo` (one Literal part).
        // word_tokens[1] and [2] are Var+Literal pairs.
        for w in &word_tokens[1..] {
            assert!(matches!(w[0], WordPart::Var { .. }));
            assert!(matches!(w[1], WordPart::Literal { quoted: false, .. }));
        }
    }

    #[test]
    fn tokenize_quoted_brace_not_expanded() {
        let toks = tokenize("echo \"{a,b}\"").expect("lex");
        let word_count = toks.iter().filter(|t| matches!(t, Token::Word(_))).count();
        assert_eq!(word_count, 2, "expected 2 Words (echo + the quoted literal), got {word_count}");
    }

    #[test]
    fn tokenize_single_quoted_brace_not_expanded() {
        let toks = tokenize("echo '{a,b}'").expect("lex");
        let word_count = toks.iter().filter(|t| matches!(t, Token::Word(_))).count();
        assert_eq!(word_count, 2, "expected 2 Words, got {word_count}");
    }

    #[test]
    fn tokenize_backslash_brace_not_expanded() {
        // The lexer's `\X` arm pushes each escaped char as a
        // one-char QUOTED Literal (quoted: true). Brace expansion
        // only fires on UNQUOTED Literals, so `\{a,b\}` survives
        // as a single Word.
        let toks = tokenize("echo \\{a,b\\}").expect("lex");
        let word_count = toks.iter().filter(|t| matches!(t, Token::Word(_))).count();
        assert_eq!(word_count, 2, "expected 2 Words, got {word_count}");
    }
```

- [ ] **Step 1.12: Append the 5 lexer tests**

### Step 1.13: Run the new lexer tests

Run: `cargo test --bin huck tokenize_brace tokenize_quoted_brace tokenize_single_quoted_brace tokenize_backslash_brace -- --nocapture`
Expected: all 5 pass.

If `tokenize_backslash_brace_not_expanded` fails: the lexer's
backslash arm may behave differently than expected. Inspect
`src/lexer.rs:223-243` to confirm — but per the v39-era code, the
`\X` arm pushes `WordPart::Literal { text: ch.to_string(), quoted: true }`
which is exactly what brace expansion ignores. If the test fails,
report as DONE_WITH_CONCERNS describing the actual behavior so we
can document it as an L-* divergence.

- [ ] **Step 1.13: New lexer tests pass**

### Step 1.14: Run full unit suite

Run: `cargo test --bin huck`
Expected: all unit tests pass. If any existing test fails because
brace expansion is firing in unexpected contexts (e.g. tests that
used `{a,b}` as literal input expecting one Word): inspect each
failure and either update the test to use quoted input
(`'{a,b}'`) or document the new behavior.

- [ ] **Step 1.14: Full unit suite passes**

### Step 1.15: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 1.15: Clippy clean**

### Step 1.16: Commit

```bash
git add src/brace_expand.rs src/main.rs src/lexer.rs src/shell.rs
git commit -m "$(cat <<'EOF'
lex: brace expansion {a,b,c} / {1..5} / etc. (v46 task 1)

New src/brace_expand.rs module containing:
- pub fn expand(input: &str) -> Result<Vec<String>, BraceError>
- BraceError enum with TooManyElements variant
- Recursive descent algorithm supporting comma lists, integer
  ranges (with step + descending + zero-pad), char ranges, nested
  braces, Cartesian product across consecutive braces, and
  invalid-brace-as-literal fallthrough.

Lexer integration in src/lexer.rs:
- New helpers word_contains_unquoted_brace, build_concat_with_sentinels,
  split_on_sentinels, emit_word_with_braces.
- All 9 sites that previously did tokens.push(Token::Word(...))
  now route through emit_word_with_braces. The helper fast-paths
  Words without unquoted braces (no perf cost for non-brace input)
  and otherwise builds a sentinel-bearing concat string, calls
  brace_expand::expand, and emits one Word per expansion.
- Sentinel chars \u{0001}..\u{0002} mark positions occupied by
  non-Literal WordParts (Var, Arith, CommandSub, Tilde) and quoted
  Literals; these are preserved verbatim through expansion and
  re-stitched into the new WordPart sequences.

LexError gains a BraceExpansionLimit variant for the safety cap
(65,536 expansions per word); rendered in src/shell.rs's
lex_error_message as ": brace expansion: too many elements".

Quoted braces are NOT expanded (the lexer flags quoted Literals
and the helper places them behind sentinels). Backslash-escaped
braces are also NOT expanded because the lexer's \X arm pushes
each escaped char as a one-char quoted Literal — bash-compatible.

12 unit tests in src/brace_expand.rs cover comma lists,
asc/desc/step integer ranges, char ranges, zero-padded ranges,
nesting, Cartesian, two invalid-as-literal forms, and the
TooManyElements cap. 5 lexer-level tests verify token emission,
Var preservation across expansion, and quote/single-quote/backslash
suppression.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 1.16: Commit Task 1**

---

## Task 2: Integration tests

**Files:**
- Create: `tests/brace_expansion_integration.rs`

Three binary-driven tests verifying brace expansion through the
running huck binary.

### Step 2.1: Create the integration test file

Create `tests/brace_expansion_integration.rs` with this content:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn brace_list_in_echo() {
    let (out, _) = run_capture("echo {a,b,c}\nexit\n");
    assert!(
        out.lines().any(|l| l == "a b c"),
        "expected `a b c` line in: {:?}",
        out
    );
}

#[test]
fn brace_range_in_for_loop() {
    let (out, _) = run_capture("for i in {1..3}; do echo \"i=$i\"; done\nexit\n");
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.contains(&"i=1"), "missing i=1 in: {:?}", out);
    assert!(lines.contains(&"i=2"), "missing i=2 in: {:?}", out);
    assert!(lines.contains(&"i=3"), "missing i=3 in: {:?}", out);
}

#[test]
fn brace_cartesian() {
    let (out, _) = run_capture(
        "for d in /tmp/{a,b}/{x,y}; do echo $d; done\nexit\n",
    );
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.contains(&"/tmp/a/x"), "missing /tmp/a/x in: {:?}", out);
    assert!(lines.contains(&"/tmp/a/y"), "missing /tmp/a/y in: {:?}", out);
    assert!(lines.contains(&"/tmp/b/x"), "missing /tmp/b/x in: {:?}", out);
    assert!(lines.contains(&"/tmp/b/y"), "missing /tmp/b/y in: {:?}", out);
}
```

- [ ] **Step 2.1: Create the file**

### Step 2.2: Run the integration suite

Run: `cargo test --test brace_expansion_integration -- --nocapture`
Expected: all 3 tests pass.

If a test fails because Task 1's expansion missed some detail:
inspect actual stdout. Do NOT relax the assertions — fix Task 1
instead (report BLOCKED if a real bug surfaces).

- [ ] **Step 2.2: Tests pass**

### Step 2.3: Full integration suite

Run: `cargo test --tests`
Expected: all integration tests pass. Known PTY flake tolerated.

- [ ] **Step 2.3: Full integration suite green**

### Step 2.4: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 2.4: Clippy clean**

### Step 2.5: Commit

```bash
git add tests/brace_expansion_integration.rs
git commit -m "$(cat <<'EOF'
test: brace expansion integration coverage (v46 task 2)

Three binary-driven tests verifying that brace expansion fires
end-to-end through the running huck binary. brace_list_in_echo
verifies `echo {a,b,c}` produces the line `a b c`.
brace_range_in_for_loop verifies a `for i in {1..3}` loop
iterates over 1, 2, 3. brace_cartesian verifies that
`/tmp/{a,b}/{x,y}` produces 4 paths.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2.5: Commit Task 2**

---

## Task 3: Docs

**Files:**
- Modify: `docs/bash-divergences.md` — add new M-61 entry,
  change-log entry.
- Modify: `README.md` — v46 row + remove brace expansion from
  "Not yet implemented" stanza.

### Step 3.1: Add the M-61 entry in `docs/bash-divergences.md`

Brace expansion does not currently have a tracked M-* entry in
`docs/bash-divergences.md` (the existing M-* numbers go up to
M-60). We're adding M-61 as a NEW entry for this iteration.

Find the appropriate section to add it. The "Word expansion" or
"Pathname / glob" subsection inside Tier 2 is the natural home —
brace expansion is a word-expansion stage. Search the file for
section headings to find a good fit; if none of the existing
subsections fits cleanly, add it under a new short subsection
heading `### Word expansion` near the other Tier 2 entries.

Add this entry:

```markdown
- **M-61: Brace expansion (`{a,b,c}` / `{1..5}` / etc.)** — `[fixed v46]` medium. Comma lists, integer ranges (asc/desc, optional step, zero-padded), character ranges, prefix/suffix, nested braces, and Cartesian product across consecutive braces. Runs at the lexer stage before parameter / command / arith expansion. Quoted braces (`"{a,b}"`, `'{a,b}'`, `\{a,b\}`) are NOT expanded. Safety cap at 65,536 expansions per word; exceeding errors with `huck: syntax error: brace expansion: too many elements`.
```

- [ ] **Step 3.1: Add M-61 entry**

### Step 3.2: Add v46 change-log entry

In `docs/bash-divergences.md`, find `## Change log` and the most
recent `**2026-05-29**` entry (v45, M-45). Add IMMEDIATELY after
it:

```markdown
- **2026-05-29**: M-61 (brace expansion) shipped as v46. New `src/brace_expand.rs` module with recursive `expand` algorithm covering comma lists, integer ranges (asc/desc/step/zero-pad), char ranges, prefix/suffix, nested, and Cartesian product. Lexer integration in `src/lexer.rs` routes every Word emission through `emit_word_with_braces`; sentinel-bearing concat preserves Var/Arith/CommandSub/Tilde and quoted Literals across expansion. New `LexError::BraceExpansionLimit` variant for the 65,536 safety cap. No new L-* divergences.
```

- [ ] **Step 3.2: Add change-log entry**

### Step 3.3: Add v46 row to README

In `README.md`, find the version table. After the v45 row (search
for `| v45       |`), add IMMEDIATELY after it:

```markdown
| v46       | Brace expansion `{a,b,c}` / `{1..5}` (M-61)                    |
```

Match column padding to v45 (count actual trailing spaces in the
file).

- [ ] **Step 3.3: Add README v46 row**

### Step 3.4: Trim `brace expansion` from "Not yet implemented"

In `README.md`, find the block around lines ~233-238:

```markdown
**Not yet implemented:**
brace expansion (`{a,b,c}`), extended job specs
(`%cmd`/`%?cmd`), backgrounded multi-pipeline sequences
(`cmd1 && cmd2 &`), aliases.
```

Replace with:

```markdown
**Not yet implemented:**
extended job specs (`%cmd`/`%?cmd`), backgrounded multi-pipeline
sequences (`cmd1 && cmd2 &`), aliases.
```

Removed: `brace expansion (\`{a,b,c}\`)` (shipped this iteration).

- [ ] **Step 3.4: Trim README stanza**

### Step 3.5: Full suite

Run: `cargo test --all-targets`
Expected: all tests pass (modulo known PTY flake).

- [ ] **Step 3.5: Full suite green**

### Step 3.6: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 3.6: Clippy clean**

### Step 3.7: Commit

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: add M-61 (brace expansion) fixed v46; trim stale entry

New M-61 entry in docs/bash-divergences.md tracks brace expansion
as [fixed v46]. Covers all bash forms (comma lists, integer +
char ranges with asc/desc/step/zero-pad, prefix/suffix, nested,
Cartesian) plus the lexer-stage placement and the 65,536 safety
cap.

Change log: 2026-05-29 v46 entry summarizing the new
brace_expand module, lexer integration via emit_word_with_braces,
and the BraceExpansionLimit error variant.

README: v46 row added to the version table; "Not yet implemented"
stanza trimmed to remove brace expansion (shipped this
iteration).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3.7: Commit Task 3**

---

## Final verification (controller, not a task)

After the three task commits land:

1. Run `cargo test --all-targets` once more.
2. Run `cargo clippy --all-targets -- -D warnings`.
3. Confirm the branch has exactly four commits ahead of `main`:
   docs preamble (spec + plan), task 1, task 2, task 3.
4. Dispatch a final cross-task code-reviewer subagent over the
   full diff (`main..v46-brace-expansion`).
5. Merge to `main` with `--no-ff`, push, delete the branch, update
   the `huck iterations` memory with v46.
