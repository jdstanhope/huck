# huck v12: Parameter-Expansion Modifiers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement 13 parameter-expansion modifiers — the default-value family (`:-`/`-`/`:=`/`=`/`:?`/`?`/`:+`/`+`), length (`${#var}`), and prefix/suffix removal (`#`/`##`/`%`/`%%`) — with recursive operand expansion and glob-based pattern matching.

**Architecture:** New `WordPart::ParamExpansion` variant carries a `ParamModifier` enum (lives in `src/lexer.rs` alongside the rest of the AST). A new `src/param_expansion.rs` module owns evaluation — looking up the variable, dispatching by modifier kind, calling back into `expand_assignment` for recursive operand expansion, and using `glob::Pattern` for prefix/suffix matching. The lexer's `read_braced_var_name` is replaced with `read_braced_param_expansion`, which uses two helpers (`scan_braced_operand`, `parse_braced_operand`) to handle nested braces, quotes, and operand re-tokenization.

**Tech Stack:** Rust 2024 edition. No new dependencies (reuses `glob`).

**Reference:** Design spec at `docs/superpowers/specs/2026-05-19-huck-parameter-expansion-modifiers-design.md`.

---

## File Map

- **Create:** `src/param_expansion.rs` — `ExpansionResult`, `expand_modifier`, `remove_prefix`, `remove_suffix`, `condition_is_null`, `expand_word_to_string`
- **Create:** `tests/param_expansion_integration.rs` — end-to-end via shell binary
- **Modify:** `src/lexer.rs` — add `WordPart::ParamExpansion`, `ParamModifier` enum, 3 new `LexError` variants, `scan_braced_operand`, `parse_braced_operand`, replace `read_braced_var_name` with `read_braced_param_expansion`
- **Modify:** `src/expand.rs` — new `ParamExpansion` arm in `expand` and `expand_assignment`
- **Modify:** `src/shell.rs` — `lex_error_message` arms for the three new `LexError` variants
- **Modify:** `src/main.rs` — register `mod param_expansion`
- **Modify:** `README.md` — v12 row, features section, test count

---

## Task 1: AST + LexError variants + lexer helpers

Add the `WordPart::ParamExpansion` variant, `ParamModifier` enum, three new `LexError` variants, and two scanning helpers (`scan_braced_operand`, `parse_braced_operand`). Update all exhaustive match sites with placeholder arms. No lexer producer or expand consumer wired yet.

**Files:**
- Modify: `src/lexer.rs`
- Modify: `src/expand.rs`
- Modify: `src/shell.rs`

- [ ] **Step 1: Write failing tests for `scan_braced_operand` and `parse_braced_operand`**

Add to `src/lexer.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn scan_braced_operand_simple() {
    let mut chars = "foo}".chars().peekable();
    assert_eq!(scan_braced_operand(&mut chars).unwrap(), "foo");
}

#[test]
fn scan_braced_operand_nested_braces() {
    let mut chars = "${Y}}".chars().peekable();
    assert_eq!(scan_braced_operand(&mut chars).unwrap(), "${Y}");
}

#[test]
fn scan_braced_operand_double_quote_protects_brace() {
    let mut chars = "\"a}b\"c}".chars().peekable();
    assert_eq!(scan_braced_operand(&mut chars).unwrap(), "\"a}b\"c");
}

#[test]
fn scan_braced_operand_single_quote_protects_brace() {
    let mut chars = "'a}b'c}".chars().peekable();
    assert_eq!(scan_braced_operand(&mut chars).unwrap(), "'a}b'c");
}

#[test]
fn scan_braced_operand_unterminated_is_error() {
    let mut chars = "foo".chars().peekable();
    assert_eq!(scan_braced_operand(&mut chars).unwrap_err(), LexError::UnterminatedBrace);
}

#[test]
fn parse_braced_operand_single_word() {
    let w = parse_braced_operand("foo").unwrap();
    assert_eq!(w.0.len(), 1);
    assert_eq!(w.0[0], WordPart::Literal { text: "foo".to_string(), quoted: false });
}

#[test]
fn parse_braced_operand_two_words_join_with_space() {
    let w = parse_braced_operand("foo bar").unwrap();
    assert_eq!(w.0.len(), 3);
    assert_eq!(w.0[0], WordPart::Literal { text: "foo".to_string(), quoted: false });
    assert_eq!(w.0[1], WordPart::Literal { text: " ".to_string(), quoted: false });
    assert_eq!(w.0[2], WordPart::Literal { text: "bar".to_string(), quoted: false });
}

#[test]
fn parse_braced_operand_top_level_pipe_is_error() {
    assert_eq!(parse_braced_operand("foo | bar").unwrap_err(), LexError::InvalidBraceOperand);
}

#[test]
fn parse_braced_operand_empty_returns_empty_word() {
    let w = parse_braced_operand("").unwrap();
    assert_eq!(w.0.len(), 0);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test scan_braced_operand_ parse_braced_operand_`
Expected: FAIL (helpers don't exist).

- [ ] **Step 3: Add the three new `LexError` variants**

Edit `src/lexer.rs`. Find `pub enum LexError` (top of file). Add:

```rust
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
    SubstitutionLexError(Box<LexError>),
    SubstitutionParseError(crate::command::ParseError),
}
```

- [ ] **Step 4: Add the `ParamModifier` enum**

Edit `src/lexer.rs`. Find the `WordPart` enum (around line 34). Add `ParamModifier` above it:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamModifier {
    Length,
    UseDefault    { word: Word, colon: bool },
    AssignDefault { word: Word, colon: bool },
    ErrorIfUnset  { word: Word, colon: bool },
    UseAlternate  { word: Word, colon: bool },
    RemovePrefix  { pattern: Word, longest: bool },
    RemoveSuffix  { pattern: Word, longest: bool },
}
```

- [ ] **Step 5: Add the `WordPart::ParamExpansion` variant**

In the `WordPart` enum, append:

```rust
pub enum WordPart {
    Literal { text: String, quoted: bool },
    Tilde(TildeSpec),
    Var { name: String, quoted: bool },
    LastStatus { quoted: bool },
    CommandSub { sequence: crate::command::Sequence, quoted: bool },
    Arith { expr: crate::arith::ArithExpr, quoted: bool },
    ParamExpansion { name: String, modifier: ParamModifier, quoted: bool },
}
```

Note: `Word` is also `Clone`-needed for `ParamModifier`'s `word`/`pattern` fields. Verify `pub struct Word(pub Vec<WordPart>);` derives `Clone` — if not, add it (and add `Clone` to `WordPart` and any referenced types if needed). Most existing variants should already be `Clone` since v11 added `#[derive(Debug, Clone, PartialEq, Eq)]` to `ArithExpr`. If `WordPart` doesn't derive `Clone` yet, add it now.

- [ ] **Step 6: Implement the two helpers**

Add to `src/lexer.rs` (place them near `scan_paren_substitution` or `scan_arith_body`):

```rust
/// Reads the inner text of a `${...}` operand. The opening `{` has already
/// been consumed; this function consumes through the matching `}` at depth 0.
/// Tracks paren-depth, plus `'...'` and `"..."` so a stray `}` inside a
/// quoted span doesn't close the expansion. Returns the inner text (without
/// the closing `}`).
fn scan_braced_operand(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<String, LexError> {
    let mut body = String::new();
    let mut depth: u32 = 1;
    loop {
        match chars.next() {
            None => return Err(LexError::UnterminatedBrace),
            Some('\\') => {
                body.push('\\');
                if let Some(c) = chars.next() { body.push(c); }
            }
            Some('"') => {
                body.push('"');
                loop {
                    match chars.next() {
                        None => return Err(LexError::UnterminatedBrace),
                        Some('"') => { body.push('"'); break; }
                        Some('\\') => {
                            body.push('\\');
                            if let Some(c) = chars.next() { body.push(c); }
                        }
                        Some(c) => body.push(c),
                    }
                }
            }
            Some('\'') => {
                body.push('\'');
                loop {
                    match chars.next() {
                        None => return Err(LexError::UnterminatedBrace),
                        Some('\'') => { body.push('\''); break; }
                        Some(c) => body.push(c),
                    }
                }
            }
            Some('{') => { depth += 1; body.push('{'); }
            Some('}') => {
                if depth == 1 { return Ok(body); }
                depth -= 1;
                body.push('}');
            }
            Some(c) => body.push(c),
        }
    }
}

/// Tokenizes the operand body and merges the resulting words into a single
/// `Word`, inserting a literal space between adjacent words to preserve
/// IFS-split-relevant whitespace.
fn parse_braced_operand(body: &str) -> Result<Word, LexError> {
    let tokens = tokenize(body)
        .map_err(|e| LexError::SubstitutionLexError(Box::new(e)))?;
    let mut parts: Vec<WordPart> = Vec::new();
    let mut first = true;
    for tok in tokens {
        match tok {
            Token::Word(Word(ps)) => {
                if !first {
                    parts.push(WordPart::Literal {
                        text: " ".to_string(),
                        quoted: false,
                    });
                }
                parts.extend(ps);
                first = false;
            }
            Token::Op(_) => return Err(LexError::InvalidBraceOperand),
        }
    }
    Ok(Word(parts))
}
```

- [ ] **Step 7: Add placeholder arms in the exhaustive matches that now break**

The new `ParamExpansion` variant breaks any exhaustive match on `WordPart`. Add placeholder arms:

In `src/expand.rs`, find the `match part { ... }` in `pub fn expand` and add:

```rust
WordPart::ParamExpansion { .. } => {
    unreachable!("ParamExpansion: lexer wiring lands in a later task");
}
```

Similarly in `pub fn expand_assignment`:

```rust
WordPart::ParamExpansion { .. } => {
    unreachable!("ParamExpansion in assignment context: lexer wiring lands in a later task");
}
```

In `src/shell.rs`, find `lex_error_message` and add arms for the three new `LexError` variants:

```rust
LexError::InvalidBraceModifier(c) => format!(": invalid parameter-expansion modifier: {c}"),
LexError::EmptyParamName => ": parameter expansion with empty name".to_string(),
LexError::InvalidBraceOperand => ": invalid operator in parameter-expansion operand".to_string(),
```

If `command.rs` or `executor.rs` have any exhaustive `WordPart` matches (verify by running `cargo build`), add `_ => false` wildcards or explicit `ParamExpansion` arms there too.

- [ ] **Step 8: Run tests**

Run: `cargo test`
Expected: all existing tests pass. The 9 new helper tests pass. Dead-code warnings expected on `ParamExpansion`, `ParamModifier` variants, `InvalidBraceModifier`/`EmptyParamName`/`InvalidBraceOperand` — they'll be resolved in later tasks.

- [ ] **Step 9: Commit**

```bash
git add src/lexer.rs src/expand.rs src/shell.rs
git commit -m "v12 task 1: AST + LexError variants + scan/parse_braced_operand helpers"
```

---

## Task 2: param_expansion module skeleton + Length modifier

Create `src/param_expansion.rs` with `ExpansionResult`, `condition_is_null`, `expand_word_to_string`, and the `expand_modifier` dispatch function with only the `Length` arm implemented. Other arms use `unreachable!` for now.

**Files:**
- Create: `src/param_expansion.rs`
- Modify: `src/main.rs` (register module)

- [ ] **Step 1: Write failing tests for Length and helpers**

Create `src/param_expansion.rs` with this content (tests fail initially because of the unreachable!() in other arms):

```rust
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
        // "é" is 2 bytes in UTF-8 but 1 character.
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
```

- [ ] **Step 2: Register the module**

Edit `src/main.rs`. Find the `mod` declarations and add (alphabetically):

```rust
mod param_expansion;
```

- [ ] **Step 3: Run tests**

Run: `cargo test param_expansion::`
Expected: 5 tests pass.

- [ ] **Step 4: Run full suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/param_expansion.rs src/main.rs
git commit -m "v12 task 2: param_expansion module skeleton with Length modifier"
```

---

## Task 3: Default-value family modifiers

Implement the four colon/no-colon modifier families: `UseDefault`, `AssignDefault`, `ErrorIfUnset`, `UseAlternate`. Each has tests for set / unset / empty value across colon and no-colon variants.

**Files:**
- Modify: `src/param_expansion.rs`
- Modify: `src/shell_state.rs` (verify `Shell::set` is the right API; if it's `pub fn`, no change needed)

- [ ] **Step 1: Write failing tests for each modifier**

Append to `src/param_expansion.rs` test module:

```rust
use crate::lexer::{Word, WordPart};

fn lit(s: &str) -> Word {
    Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }])
}

// UseDefault: ${var:-w} / ${var-w}

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

// AssignDefault: ${var:=w} / ${var=w}

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

// ErrorIfUnset: ${var:?w} / ${var?w}

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

// UseAlternate: ${var:+w} / ${var+w}

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
    // Without `:`, only unset (not empty) is null.
    let mut shell = Shell::new();
    shell.export_set("HUCK_TEST_PE_UA4", "".to_string());
    let m = ParamModifier::UseAlternate { word: lit("alt"), colon: false };
    let r = expand_modifier("HUCK_TEST_PE_UA4", &m, &mut shell);
    assert_eq!(r, ExpansionResult::Value("alt".to_string()));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test param_expansion::tests::use_default_ param_expansion::tests::assign_default_ param_expansion::tests::error_if_unset_ param_expansion::tests::use_alternate_`
Expected: PANIC with "default-value family lands in Task 3" (current Task 2 placeholder).

- [ ] **Step 3: Implement the four arms**

Replace the `unreachable!("default-value family lands in Task 3")` block in `expand_modifier` with:

```rust
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
            let default = if *colon { "parameter null or not set" } else { "parameter not set" };
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
```

- [ ] **Step 4: Run tests**

Run: `cargo test param_expansion::`
Expected: all 16 tests pass (5 from Task 2 + 11 new).

- [ ] **Step 5: Run full suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/param_expansion.rs
git commit -m "v12 task 3: default-value family modifiers (\${var:-/:=/:?/:+w})"
```

---

## Task 4: Prefix/suffix removal modifiers

Add `remove_prefix` and `remove_suffix` helpers, plus the `RemovePrefix` and `RemoveSuffix` arms in `expand_modifier`. Patterns use the `glob` crate with `require_literal_separator: false`.

**Files:**
- Modify: `src/param_expansion.rs`

- [ ] **Step 1: Write failing tests for the helpers**

Append to `src/param_expansion.rs` test module:

```rust
// remove_prefix / remove_suffix helpers

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
fn remove_prefix_empty_pattern_returns_value_unchanged() {
    // Empty pattern matches the empty prefix (which removes nothing).
    assert_eq!(remove_prefix("hello", "", false), "hello");
}

#[test]
fn remove_prefix_invalid_glob_returns_value_unchanged() {
    // `[` with no closing `]` is an invalid pattern.
    assert_eq!(remove_prefix("hello", "[abc", false), "hello");
}

#[test]
fn remove_prefix_literal_match() {
    assert_eq!(remove_prefix("hello world", "hello ", false), "world");
}

#[test]
fn remove_prefix_glob_crosses_slash() {
    // require_literal_separator: false — `*` matches across `/`.
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
    // "café.txt" — `é` is 2 bytes. Removing `.txt` should yield "café".
    assert_eq!(remove_suffix("café.txt", ".txt", false), "café");
}

// expand_modifier arms

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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test param_expansion::tests::remove_prefix_ param_expansion::tests::remove_suffix_ param_expansion::tests::expand_modifier_remove_`
Expected: FAIL — `remove_prefix` and `remove_suffix` don't exist; arms panic with the Task 2 placeholder.

- [ ] **Step 3: Add the helpers and replace the placeholder arms**

Add to `src/param_expansion.rs` (above the test module):

```rust
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
```

Replace the `unreachable!("prefix/suffix removal lands in Task 4")` arms in `expand_modifier`:

```rust
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
```

- [ ] **Step 4: Run tests**

Run: `cargo test param_expansion::`
Expected: all tests pass (~31).

- [ ] **Step 5: Run full suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/param_expansion.rs
git commit -m "v12 task 4: prefix/suffix removal modifiers (\${var#/##/%/%%pat})"
```

---

## Task 5: Wire `expand_modifier` into `expand` and `expand_assignment`

Replace the Task 1 placeholder `unreachable!` arms in `expand.rs` with real calls to `expand_modifier`. The lexer still doesn't produce the variant, so this stays dead at runtime until Task 6. Direct tests via constructed `WordPart::ParamExpansion` values verify the pipeline.

**Files:**
- Modify: `src/expand.rs`

- [ ] **Step 1: Write failing tests that construct `WordPart::ParamExpansion` manually**

Add to `src/expand.rs` test module:

```rust
#[test]
fn expand_param_expansion_use_default_unquoted_unset() {
    use crate::lexer::ParamModifier;
    let mut shell = Shell::new();
    let word = Word(vec![WordPart::ParamExpansion {
        name: "HUCK_TEST_PE_E1".to_string(),
        modifier: ParamModifier::UseDefault {
            word: Word(vec![WordPart::Literal { text: "fallback".to_string(), quoted: false }]),
            colon: true,
        },
        quoted: false,
    }]);
    let fields = expand(&word, &mut shell);
    let strings: Vec<String> = fields.into_iter().map(|f| f.chars).collect();
    assert_eq!(strings, vec!["fallback".to_string()]);
}

#[test]
fn expand_param_expansion_quoted_value_with_space_stays_one_field() {
    use crate::lexer::ParamModifier;
    let mut shell = Shell::new();
    let word = Word(vec![WordPart::ParamExpansion {
        name: "HUCK_TEST_PE_E2".to_string(),
        modifier: ParamModifier::UseDefault {
            word: Word(vec![WordPart::Literal { text: "a b c".to_string(), quoted: false }]),
            colon: true,
        },
        quoted: true,
    }]);
    let fields = expand(&word, &mut shell);
    let strings: Vec<String> = fields.into_iter().map(|f| f.chars).collect();
    assert_eq!(strings, vec!["a b c".to_string()]);
}

#[test]
fn expand_param_expansion_unquoted_value_with_space_splits() {
    use crate::lexer::ParamModifier;
    let mut shell = Shell::new();
    shell.export_set("HUCK_TEST_PE_E3", "a b c".to_string());
    let word = Word(vec![WordPart::ParamExpansion {
        name: "HUCK_TEST_PE_E3".to_string(),
        modifier: ParamModifier::UseDefault {
            word: Word(vec![]),
            colon: true,
        },
        quoted: false,
    }]);
    let fields = expand(&word, &mut shell);
    let strings: Vec<String> = fields.into_iter().map(|f| f.chars).collect();
    assert_eq!(strings, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
}

#[test]
fn expand_assignment_param_expansion_no_split() {
    use crate::lexer::ParamModifier;
    let mut shell = Shell::new();
    let word = Word(vec![WordPart::ParamExpansion {
        name: "HUCK_TEST_PE_E4".to_string(),
        modifier: ParamModifier::UseDefault {
            word: Word(vec![WordPart::Literal { text: "a b c".to_string(), quoted: false }]),
            colon: true,
        },
        quoted: false,
    }]);
    let value = expand_assignment(&word, &mut shell);
    assert_eq!(value, "a b c");
}

#[test]
fn expand_param_expansion_error_yields_empty_field_sets_status() {
    use crate::lexer::ParamModifier;
    let mut shell = Shell::new();
    let word = Word(vec![WordPart::ParamExpansion {
        name: "HUCK_TEST_PE_E5".to_string(),
        modifier: ParamModifier::ErrorIfUnset {
            word: Word(vec![WordPart::Literal { text: "missing".to_string(), quoted: false }]),
            colon: true,
        },
        quoted: false,
    }]);
    let fields = expand(&word, &mut shell);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].chars, "");
    assert_eq!(shell.last_status(), 1);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test expand_param_expansion_ expand_assignment_param_expansion_`
Expected: PANIC at `unreachable!` from Task 1.

- [ ] **Step 3: Replace the placeholder arm in `expand`**

In `src/expand.rs`, find the `unreachable!("ParamExpansion: lexer wiring lands in a later task")` arm in `pub fn expand`. Replace with:

```rust
WordPart::ParamExpansion { name, modifier, quoted } => {
    match crate::param_expansion::expand_modifier(name, modifier, shell) {
        crate::param_expansion::ExpansionResult::Value(v) => {
            if *quoted {
                current.push_str(&v, true);
                has_emitted = true;
            } else {
                emit_split_fields(&v, &mut current, &mut result, &mut has_emitted);
            }
        }
        crate::param_expansion::ExpansionResult::Empty => {
            has_emitted = true;
        }
    }
}
```

- [ ] **Step 4: Replace the placeholder arm in `expand_assignment`**

```rust
WordPart::ParamExpansion { name, modifier, .. } => {
    match crate::param_expansion::expand_modifier(name, modifier, shell) {
        crate::param_expansion::ExpansionResult::Value(v) => result.push_str(&v),
        crate::param_expansion::ExpansionResult::Empty => {}
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test expand_param_expansion_ expand_assignment_param_expansion_`
Expected: PASS (5 new tests).

- [ ] **Step 6: Run full suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/expand.rs
git commit -m "v12 task 5: wire expand_modifier into expand and expand_assignment"
```

---

## Task 6: Lexer integration

Replace `read_braced_var_name` with `read_braced_param_expansion`. Recognize all 13 modifier syntaxes and produce `WordPart::ParamExpansion`. Plain `${var}` (no modifier) still produces `WordPart::Var` for backward compatibility.

**Files:**
- Modify: `src/lexer.rs`

- [ ] **Step 1: Write failing lexer tests for each modifier syntax**

Add to `src/lexer.rs` test module:

```rust
#[test]
fn tokenize_brace_var_no_modifier_still_emits_var() {
    let tokens = tokenize("${foo}").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0], WordPart::Var { name: "foo".to_string(), quoted: false });
}

#[test]
fn tokenize_length_modifier() {
    let tokens = tokenize("${#foo}").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    assert_eq!(parts.len(), 1);
    let WordPart::ParamExpansion { name, modifier, quoted } = &parts[0] else {
        panic!("expected ParamExpansion, got {:?}", parts[0]);
    };
    assert_eq!(name, "foo");
    assert_eq!(*quoted, false);
    assert!(matches!(modifier, ParamModifier::Length));
}

#[test]
fn tokenize_use_default_colon_dash() {
    let tokens = tokenize("${X:-w}").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    let WordPart::ParamExpansion { name, modifier, .. } = &parts[0] else { panic!() };
    assert_eq!(name, "X");
    match modifier {
        ParamModifier::UseDefault { word, colon } => {
            assert_eq!(*colon, true);
            assert_eq!(word.0, vec![WordPart::Literal { text: "w".to_string(), quoted: false }]);
        }
        other => panic!("expected UseDefault, got {:?}", other),
    }
}

#[test]
fn tokenize_use_default_no_colon() {
    let tokens = tokenize("${X-w}").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
    assert!(matches!(modifier, ParamModifier::UseDefault { colon: false, .. }));
}

#[test]
fn tokenize_assign_default_colon_equals() {
    let tokens = tokenize("${X:=w}").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
    assert!(matches!(modifier, ParamModifier::AssignDefault { colon: true, .. }));
}

#[test]
fn tokenize_error_if_unset_colon_question() {
    let tokens = tokenize("${X:?msg}").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
    assert!(matches!(modifier, ParamModifier::ErrorIfUnset { colon: true, .. }));
}

#[test]
fn tokenize_use_alternate_colon_plus() {
    let tokens = tokenize("${X:+w}").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
    assert!(matches!(modifier, ParamModifier::UseAlternate { colon: true, .. }));
}

#[test]
fn tokenize_remove_prefix_short() {
    let tokens = tokenize("${X#pat}").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
    assert!(matches!(modifier, ParamModifier::RemovePrefix { longest: false, .. }));
}

#[test]
fn tokenize_remove_prefix_long() {
    let tokens = tokenize("${X##pat}").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
    assert!(matches!(modifier, ParamModifier::RemovePrefix { longest: true, .. }));
}

#[test]
fn tokenize_remove_suffix_short() {
    let tokens = tokenize("${X%pat}").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
    assert!(matches!(modifier, ParamModifier::RemoveSuffix { longest: false, .. }));
}

#[test]
fn tokenize_remove_suffix_long() {
    let tokens = tokenize("${X%%pat}").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
    assert!(matches!(modifier, ParamModifier::RemoveSuffix { longest: true, .. }));
}

#[test]
fn tokenize_nested_param_expansion_in_operand() {
    // ${X:-${Y}} — operand is itself a parameter expansion.
    let tokens = tokenize("${X:-${Y}}").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
    if let ParamModifier::UseDefault { word, .. } = modifier {
        assert_eq!(word.0.len(), 1);
        assert!(matches!(word.0[0], WordPart::Var { .. }));
    } else {
        panic!("expected UseDefault");
    }
}

#[test]
fn tokenize_quoted_operand_preserves_spaces() {
    let tokens = tokenize("${X:-\"a b\"}").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
    if let ParamModifier::UseDefault { word, .. } = modifier {
        // Quoted "a b" produces one Literal with quoted: true.
        assert_eq!(word.0.len(), 1);
        assert_eq!(word.0[0], WordPart::Literal { text: "a b".to_string(), quoted: true });
    } else {
        panic!();
    }
}

#[test]
fn tokenize_quoted_outer_param_expansion() {
    let tokens = tokenize("\"${X:-w}\"").unwrap();
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    let WordPart::ParamExpansion { quoted, .. } = &parts[0] else { panic!() };
    assert_eq!(*quoted, true);
}

#[test]
fn tokenize_invalid_modifier_errors() {
    let err = tokenize("${X:&Y}").unwrap_err();
    assert!(matches!(err, LexError::InvalidBraceModifier(_)));
}

#[test]
fn tokenize_empty_param_name_errors() {
    let err = tokenize("${:-foo}").unwrap_err();
    assert_eq!(err, LexError::EmptyParamName);
}

#[test]
fn tokenize_unterminated_brace_modifier_errors() {
    let err = tokenize("${X:-foo").unwrap_err();
    assert_eq!(err, LexError::UnterminatedBrace);
}

#[test]
fn tokenize_pipe_in_operand_errors() {
    let err = tokenize("${X:-foo | bar}").unwrap_err();
    assert_eq!(err, LexError::InvalidBraceOperand);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test tokenize_brace_var_no_modifier tokenize_length_modifier tokenize_use_default_ tokenize_assign_default_ tokenize_error_if_unset_ tokenize_use_alternate_ tokenize_remove_prefix_ tokenize_remove_suffix_ tokenize_nested_param_ tokenize_quoted_operand_ tokenize_quoted_outer_ tokenize_invalid_modifier_ tokenize_empty_param_name_ tokenize_unterminated_brace_modifier_ tokenize_pipe_in_operand_`
Expected: FAIL — the current `read_braced_var_name` doesn't handle modifiers.

- [ ] **Step 3: Replace `read_braced_var_name` with `read_braced_param_expansion`**

Find the existing `read_braced_var_name` function in `src/lexer.rs` (around line 288). Replace it with this new function:

```rust
/// Reads a `${...}` parameter expansion. The opening `$` and `{` have
/// already been consumed (the caller is in `read_dollar_expansion` after
/// matching `'{'`). Reads the variable name, optional modifier, and
/// optional operand, and pushes either a `WordPart::Var` (plain `${name}`)
/// or a `WordPart::ParamExpansion` (any modifier).
fn read_braced_param_expansion(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    parts: &mut Vec<WordPart>,
    quoted: bool,
) -> Result<(), LexError> {
    // Length form: ${#name}
    if chars.peek() == Some(&'#') {
        chars.next();
        let name = read_braced_name(chars)?;
        if name.is_empty() {
            // ${#} would be the special parameter (out of scope for v12).
            return Err(LexError::EmptyParamName);
        }
        if chars.next() != Some('}') {
            return Err(LexError::UnterminatedBrace);
        }
        parts.push(WordPart::ParamExpansion {
            name,
            modifier: ParamModifier::Length,
            quoted,
        });
        return Ok(());
    }

    let name = read_braced_name(chars)?;
    if name.is_empty() {
        return Err(LexError::EmptyParamName);
    }

    match chars.next() {
        Some('}') => {
            parts.push(WordPart::Var { name, quoted });
            Ok(())
        }
        Some(':') => {
            let next = chars.next();
            let modifier = match next {
                Some('-') => modifier_with_operand(chars, |w| ParamModifier::UseDefault { word: w, colon: true })?,
                Some('=') => modifier_with_operand(chars, |w| ParamModifier::AssignDefault { word: w, colon: true })?,
                Some('?') => modifier_with_operand(chars, |w| ParamModifier::ErrorIfUnset { word: w, colon: true })?,
                Some('+') => modifier_with_operand(chars, |w| ParamModifier::UseAlternate { word: w, colon: true })?,
                Some(c) => return Err(LexError::InvalidBraceModifier(format!(":{c}"))),
                None => return Err(LexError::UnterminatedBrace),
            };
            parts.push(WordPart::ParamExpansion { name, modifier, quoted });
            Ok(())
        }
        Some('-') => {
            let modifier = modifier_with_operand(chars, |w| ParamModifier::UseDefault { word: w, colon: false })?;
            parts.push(WordPart::ParamExpansion { name, modifier, quoted });
            Ok(())
        }
        Some('=') => {
            let modifier = modifier_with_operand(chars, |w| ParamModifier::AssignDefault { word: w, colon: false })?;
            parts.push(WordPart::ParamExpansion { name, modifier, quoted });
            Ok(())
        }
        Some('?') => {
            let modifier = modifier_with_operand(chars, |w| ParamModifier::ErrorIfUnset { word: w, colon: false })?;
            parts.push(WordPart::ParamExpansion { name, modifier, quoted });
            Ok(())
        }
        Some('+') => {
            let modifier = modifier_with_operand(chars, |w| ParamModifier::UseAlternate { word: w, colon: false })?;
            parts.push(WordPart::ParamExpansion { name, modifier, quoted });
            Ok(())
        }
        Some('#') => {
            let longest = chars.peek() == Some(&'#');
            if longest { chars.next(); }
            let modifier = modifier_with_operand(chars, |w| ParamModifier::RemovePrefix { pattern: w, longest })?;
            parts.push(WordPart::ParamExpansion { name, modifier, quoted });
            Ok(())
        }
        Some('%') => {
            let longest = chars.peek() == Some(&'%');
            if longest { chars.next(); }
            let modifier = modifier_with_operand(chars, |w| ParamModifier::RemoveSuffix { pattern: w, longest })?;
            parts.push(WordPart::ParamExpansion { name, modifier, quoted });
            Ok(())
        }
        Some(c) => Err(LexError::InvalidBraceModifier(c.to_string())),
        None => Err(LexError::UnterminatedBrace),
    }
}

/// Reads identifier chars (the parameter name) inside a `${...}` until
/// it hits a non-identifier char. Does NOT consume that terminator.
fn read_braced_name(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<String, LexError> {
    let mut name = String::new();
    while let Some(&c) = chars.peek() {
        if c == '_' || c.is_ascii_alphanumeric() {
            if name.is_empty() && c.is_ascii_digit() {
                // Starts with digit — not a valid identifier (special
                // parameters like $1 are out of scope for v12).
                return Err(LexError::InvalidVarName);
            }
            name.push(c);
            chars.next();
        } else {
            break;
        }
    }
    Ok(name)
}

/// Scans the operand text until the matching `}` and parses it as a
/// single `Word`. Builds the `ParamModifier` via the caller's closure.
fn modifier_with_operand<F>(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    build: F,
) -> Result<ParamModifier, LexError>
where
    F: FnOnce(Word) -> ParamModifier,
{
    let body = scan_braced_operand(chars)?;
    let word = parse_braced_operand(&body)?;
    Ok(build(word))
}
```

- [ ] **Step 4: Wire it into the existing `read_dollar_expansion` `'{'` branch**

Find the existing `Some('{')` arm in `read_dollar_expansion` (was around line 286 in the old code). Replace its body so it calls the new function:

```rust
Some('{') => {
    chars.next();
    read_braced_param_expansion(chars, parts, quoted)?;
}
```

(The old code was reading just the name and matching `}` itself; the new helper handles everything.)

- [ ] **Step 5: Run the new tests**

Run: `cargo test tokenize_brace_var_no_modifier tokenize_length_modifier tokenize_use_default_ tokenize_assign_default_ tokenize_error_if_unset_ tokenize_use_alternate_ tokenize_remove_prefix_ tokenize_remove_suffix_ tokenize_nested_param_ tokenize_quoted_operand_ tokenize_quoted_outer_ tokenize_invalid_modifier_ tokenize_empty_param_name_ tokenize_unterminated_brace_modifier_ tokenize_pipe_in_operand_`
Expected: PASS (18 new tests).

- [ ] **Step 6: Run the full suite**

Run: `cargo test`
Expected: all tests pass. The previously dead `ParamExpansion` variant and the three new `LexError` variants now have producers, so their dead-code warnings should disappear.

- [ ] **Step 7: Manual smoke test**

```bash
cargo build --release
~/projects/shuck/target/release/huck <<'EOF'
echo ${X:-default}
X=value
echo ${X:-default}
echo ${X:=default}
echo ${UNSET:?missing arg}
f=/path/to/file.txt
echo ${f##*/}
echo ${f%.*}
s=hello
echo ${#s}
echo "${X:-$(echo nested)}"
exit
EOF
```

Expected stdout to contain (order with prompt lines):
```
default
value
value
huck: UNSET: missing arg
file.txt
/path/to/file
5
value
```

(`huck: UNSET: missing arg` goes to stderr.)

- [ ] **Step 8: Commit**

```bash
git add src/lexer.rs
git commit -m "v12 task 6: lexer recognizes all 13 parameter-expansion modifier shapes"
```

---

## Task 7: End-to-end integration tests

End-to-end tests spawn the built binary, feed stdin, assert stdout/stderr/exit status.

**Files:**
- Create: `tests/param_expansion_integration.rs`

- [ ] **Step 1: Create the test file**

Create `tests/param_expansion_integration.rs`:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn use_default_unset_uses_default() {
    let (out, _) = run("echo ${X:-default}\nexit\n");
    assert!(out.lines().any(|l| l == "default"), "stdout: {out}");
}

#[test]
fn use_default_set_uses_value() {
    let (out, _) = run("X=value\necho ${X:-default}\nexit\n");
    assert!(out.lines().any(|l| l == "value"), "stdout: {out}");
}

#[test]
fn assign_default_mutates_shell() {
    let (out, _) = run("echo ${X:=default}\necho $X\nexit\n");
    let lines: Vec<&str> = out.lines().filter(|l| *l == "default").collect();
    assert!(lines.len() >= 2, "expected 'default' twice, stdout: {out}");
}

#[test]
fn error_if_unset_writes_to_stderr() {
    let (_, err) = run("echo ${UNSET:?missing}\nexit\n");
    assert!(err.contains("UNSET: missing"), "stderr: {err}");
}

#[test]
fn use_alternate_set_uses_alternate() {
    let (out, _) = run("X=anything\necho ${X:+set}\nexit\n");
    assert!(out.lines().any(|l| l == "set"), "stdout: {out}");
}

#[test]
fn use_alternate_unset_yields_empty() {
    let (out, _) = run("echo ${X:+set}\nexit\n");
    // Empty argument to echo prints a blank line.
    assert!(out.lines().any(|l| l.is_empty()), "stdout: {out}");
}

#[test]
fn remove_prefix_longest_strips_path() {
    let (out, _) = run("f=/path/to/file.txt\necho ${f##*/}\nexit\n");
    assert!(out.lines().any(|l| l == "file.txt"), "stdout: {out}");
}

#[test]
fn remove_suffix_strips_extension() {
    let (out, _) = run("f=/path/to/file.txt\necho ${f%.*}\nexit\n");
    assert!(out.lines().any(|l| l == "/path/to/file"), "stdout: {out}");
}

#[test]
fn length_of_set_string() {
    let (out, _) = run("s=hello\necho ${#s}\nexit\n");
    assert!(out.lines().any(|l| l == "5"), "stdout: {out}");
}

#[test]
fn nested_command_sub_in_default() {
    let (out, _) = run("echo \"${X:-$(echo nested)}\"\nexit\n");
    assert!(out.lines().any(|l| l == "nested"), "stdout: {out}");
}

#[test]
fn quoted_default_with_spaces_stays_one_arg() {
    // If the result word-split, echo would print "a  b  c" (three args
    // with multiple spaces collapsed to single). With proper quoting
    // we get "a b c" verbatim.
    let (out, _) = run("echo \"${X:-a b c}\"\nexit\n");
    assert!(out.lines().any(|l| l == "a b c"), "stdout: {out}");
}
```

- [ ] **Step 2: Run integration tests**

Run: `cargo test --test param_expansion_integration`
Expected: 11 tests pass.

- [ ] **Step 3: Run full suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add tests/param_expansion_integration.rs
git commit -m "v12 task 7: end-to-end parameter-expansion integration tests"
```

---

## Task 8: README update

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add v12 row to status table**

Append after the v11 row:

```
| v12       | Parameter-expansion modifiers (`${var:-w}`, `${var#pat}`, etc.) |
```

- [ ] **Step 2: Add Parameter-expansion subsection in Features**

After the existing **Arithmetic expansion (v11):** block:

```markdown
**Parameter-expansion modifiers (v12):**
Default-value family: `${var:-w}` (use `w` if null), `${var:=w}`
(also assign), `${var:?w}` (stderr error if null), `${var:+w}` (use
`w` if set). The non-`:` variants (`-`/`=`/`?`/`+`) treat only unset
as null. Length: `${#var}` returns the Unicode character count.
Prefix/suffix removal: `${var#pat}`/`${var##pat}` strip the shortest
or longest matching prefix; `${var%pat}`/`${var%%pat}` strip the
suffix. Patterns use glob syntax (`*`, `?`, `[abc]`) and `*` can
cross `/`. The operand `w` (or `pat`) is recursively expanded —
variables, arithmetic, command sub, and tilde all work inside.
Pattern substitution `${var/pat/repl}`, substring `${var:off:len}`,
and case modification are not yet implemented.
```

- [ ] **Step 3: Remove the v12 reference from Not-yet-implemented**

Find the bullet ``parameter-expansion modifiers (`${var:-x}`/`${var/pat/repl}`/etc.)`` and either remove it (since most modifiers are now done) or shorten to ``pattern-substitution and substring parameter-expansion (`${var/pat/repl}`, `${var:off:len}`)``. The shorter version more accurately reflects what's still missing — go with that.

- [ ] **Step 4: Update the Syntax line**

Find:
```
`cd ~-`, `PATH=~/bin:~/lib`, `ls *.txt`, `echo [ab].rs`, `echo $((2+3))`.
```
Replace with:
```
`cd ~-`, `PATH=~/bin:~/lib`, `ls *.txt`, `echo [ab].rs`, `echo $((2+3))`, `echo ${X:-default}`, `echo ${f##*/}`.
```

- [ ] **Step 5: Update test count**

Run: `cargo test 2>&1 | grep 'test result'`
Sum the passed counts across all binaries and update `cargo test               # full test suite (NNN tests)` to the new total. Expect roughly 394 baseline + ~55 new = ~449.

- [ ] **Step 6: Commit**

```bash
git add README.md
git commit -m "v12 task 8: README — add v12 row and parameter-expansion section"
```

---

## Final review checkpoint

After Task 8:

- [ ] `cargo test` shows the expected total passing, 0 failing
- [ ] `cargo clippy -- -D warnings` is clean (or any new warnings are intentional)
- [ ] Manual REPL smoke session covering each modifier family: `${X:-w}`, `${X:=w}`, `${X:?w}`, `${X:+w}`, `${#X}`, `${X#pat}`, `${X##pat}`, `${X%pat}`, `${X%%pat}`, plus nested forms (`${X:-${Y}}`, `${X:-$(date)}`).
- [ ] Final review the whole branch as a single diff before merging to main
