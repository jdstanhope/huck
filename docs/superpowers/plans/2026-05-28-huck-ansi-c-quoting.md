# huck v39 — `$'…'` ANSI-C Quoting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close bash divergence M-28 by adding `$'…'` ANSI-C quoting to huck's
lexer, decoding all 16 bash escape sequences to a single quoted literal.

**Architecture:** Pure lexical change. One new arm in
`read_dollar_expansion` (src/lexer.rs) for `Some('\'')`. Two new private
helpers — `read_ansi_c_quoted` and `decode_ansi_c_escape` — handle scanning
and decoding. One new `LexError` variant (`AnsiCInvalidCodepoint(u32)`) for
out-of-range numeric escapes. No parser, AST, executor, or expansion
changes. The emitted shape (`WordPart::Literal { quoted: true }`) is the
same shape that `'…'` already produces, so word splitting and globbing are
already suppressed downstream.

**Tech Stack:** Rust. No new dependencies. Uses `char::from_u32` for
codepoint validation and Rust's `char::to_ascii_uppercase` for `\cX` decoding.

**Spec:** `docs/superpowers/specs/2026-05-28-huck-ansi-c-quoting-design.md`

**Branch:** `v39-ansi-c-quoting` (must be created from `main` before Task 1).

**Commit trailer convention** (every commit in this iteration):

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble: Create the working branch

- [ ] **Step P.1: Create branch from main and check it out**

```bash
git checkout main
git pull --ff-only
git checkout -b v39-ansi-c-quoting
```

Expected: `Switched to a new branch 'v39-ansi-c-quoting'`.

---

## Task 1: Lexer recognizer + decoder + unit tests

**Files:**
- Modify: `src/lexer.rs` (add LexError variant, helpers, dispatch arm, ~12 unit tests)
- Modify: `src/shell.rs:300-319` (add `lex_error_message` arm for the new variant)

### Step 1.1: Add the new `LexError` variant

- [ ] **Step 1.1: Add `AnsiCInvalidCodepoint(u32)` to `LexError`**

In `src/lexer.rs:2-15`, change the enum from:

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
}
```

to:

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
}
```

- [ ] **Step 1.2: Build to confirm the variant compiles**

Run: `cargo build`
Expected: compiles cleanly (the `lex_error_message` match in `src/shell.rs`
is non-exhaustive against the new variant; the compiler may emit a warning
but Rust's match exhaustiveness check only errors when warnings are
deny'd — we fix it in step 1.3 before any tests run).

If you instead see `error[E0004]: non-exhaustive patterns` blocking the
build, proceed directly to step 1.3.

### Step 1.3: Add `lex_error_message` arm

- [ ] **Step 1.3: Render the new variant**

In `src/shell.rs:300-319`, change the `match error { ... }` to add an arm
**before** the existing `LexError::UnterminatedHeredoc` arm:

```rust
        LexError::AnsiCInvalidCodepoint(v) => {
            format!(": invalid Unicode codepoint in $'...' escape: U+{:04X}", v)
        }
```

So the full block becomes:

```rust
fn lex_error_message(error: LexError) -> String {
    match error {
        LexError::UnterminatedQuote => ": unterminated quote".to_string(),
        LexError::InvalidVarName => ": invalid variable name in '${...}'".to_string(),
        LexError::UnterminatedBrace => ": unterminated '${...}'".to_string(),
        LexError::UnterminatedSubstitution => ": unterminated command substitution".to_string(),
        LexError::UnterminatedArith => ": unterminated arithmetic expansion".to_string(),
        LexError::ArithParse(msg) => format!(": arithmetic expansion: {msg}"),
        LexError::InvalidBraceModifier(c) => format!(": invalid parameter-expansion modifier: {c}"),
        LexError::EmptyParamName => ": parameter expansion with empty name".to_string(),
        LexError::InvalidBraceOperand => ": invalid operator in parameter-expansion operand".to_string(),
        LexError::Substitution(inner) => {
            format!(" in command substitution{}", lex_error_message(*inner))
        }
        LexError::SubstitutionParseError(inner) => {
            format!(" in command substitution: {}", parse_error_message(inner))
        }
        LexError::UnterminatedHeredoc => ": unterminated here-document".to_string(),
        LexError::AnsiCInvalidCodepoint(v) => {
            format!(": invalid Unicode codepoint in $'...' escape: U+{:04X}", v)
        }
    }
}
```

- [ ] **Step 1.4: Build to confirm**

Run: `cargo build`
Expected: clean build, zero warnings about the new variant.

### Step 1.5–1.12: TDD the recognizer and helpers

For each test in turn, add the test, run it to confirm it fails, then add
just enough code to make it pass.

- [ ] **Step 1.5: Write the first failing test (`\n` → newline)**

Add to the existing `#[cfg(test)] mod tests { ... }` block in
`src/lexer.rs` (near the bottom of the file — find the `mod tests` and
append within it):

```rust
    #[test]
    fn ansi_c_quote_newline_escape() {
        let toks = tokenize("$'a\\nb'").expect("lex");
        // Single Word token with one quoted Literal containing "a\nb"
        match &toks[0] {
            Token::Word(parts) => {
                assert_eq!(parts.len(), 1);
                match &parts[0] {
                    WordPart::Literal { text, quoted } => {
                        assert_eq!(text, "a\nb");
                        assert!(*quoted, "expected quoted Literal");
                    }
                    other => panic!("expected Literal, got {:?}", other),
                }
            }
            other => panic!("expected Word token, got {:?}", other),
        }
    }
```

- [ ] **Step 1.6: Run the test; confirm it fails**

Run: `cargo test --lib ansi_c_quote_newline_escape -- --nocapture`
Expected: FAIL — current behavior tokenizes `$` separately, so `parts[0]`
won't match the expected shape. The failure mode is either a panic message
from `assert_eq!` showing `"a\\nb"` instead of `"a\nb"`, or `parts.len()`
not being 1.

- [ ] **Step 1.7: Implement the recognizer + helpers**

In `src/lexer.rs`, locate `fn read_dollar_expansion` (around line 683).
Inside its `match chars.peek().copied() { ... }`, add a new arm
immediately **after** the `Some('{') => { ... }` block and **before** the
`Some('?')` arm:

```rust
        Some('\'') => {
            chars.next();
            let text = read_ansi_c_quoted(chars)?;
            parts.push(WordPart::Literal { text, quoted: true });
        }
```

The arm should be inserted so the full match looks like:

```rust
    match chars.peek().copied() {
        Some('(') => { ... existing ... }
        Some('{') => {
            chars.next();
            read_braced_param_expansion(chars, parts, quoted)?;
        }
        Some('\'') => {
            chars.next();
            let text = read_ansi_c_quoted(chars)?;
            parts.push(WordPart::Literal { text, quoted: true });
        }
        Some('?') => { ... existing ... }
        // ... rest unchanged
    }
```

Then add the two helper functions **immediately below**
`read_dollar_expansion` (before `scan_arith_body`):

```rust
/// Reads the body of a `$'...'` ANSI-C quoted string. The opening `$'` has
/// already been consumed; this scans forward, processing C-style backslash
/// escapes, until the matching unescaped `'` is consumed. Returns the
/// decoded string.
fn read_ansi_c_quoted(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<String, LexError> {
    let mut out = String::new();
    loop {
        match chars.next() {
            None => return Err(LexError::UnterminatedQuote),
            Some('\'') => return Ok(out),
            Some('\\') => decode_ansi_c_escape(chars, &mut out)?,
            Some(c) => out.push(c),
        }
    }
}

/// Decodes a single backslash escape inside `$'...'` and appends the
/// result to `out`. The leading `\` has already been consumed.
fn decode_ansi_c_escape(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    out: &mut String,
) -> Result<(), LexError> {
    match chars.next() {
        None => return Err(LexError::UnterminatedQuote),
        Some('a') => out.push('\x07'),
        Some('b') => out.push('\x08'),
        Some('e') | Some('E') => out.push('\x1B'),
        Some('f') => out.push('\x0C'),
        Some('n') => out.push('\n'),
        Some('r') => out.push('\r'),
        Some('t') => out.push('\t'),
        Some('v') => out.push('\x0B'),
        Some('\\') => out.push('\\'),
        Some('\'') => out.push('\''),
        Some('"') => out.push('"'),
        Some('?') => out.push('?'),
        Some(c @ '0'..='7') => {
            let mut v: u32 = c.to_digit(8).unwrap();
            for _ in 0..2 {
                match chars.peek().copied() {
                    Some(d @ '0'..='7') => {
                        chars.next();
                        v = v * 8 + d.to_digit(8).unwrap();
                    }
                    _ => break,
                }
            }
            push_codepoint(out, v)?;
        }
        Some('x') => {
            if chars.peek().copied().is_some_and(|c| c.is_ascii_hexdigit()) {
                let v = scan_hex_digits(chars, 2);
                push_codepoint(out, v)?;
            } else {
                out.push('\\');
                out.push('x');
            }
        }
        Some('u') => {
            if chars.peek().copied().is_some_and(|c| c.is_ascii_hexdigit()) {
                let v = scan_hex_digits(chars, 4);
                push_codepoint(out, v)?;
            } else {
                out.push('\\');
                out.push('u');
            }
        }
        Some('U') => {
            if chars.peek().copied().is_some_and(|c| c.is_ascii_hexdigit()) {
                let v = scan_hex_digits(chars, 8);
                push_codepoint(out, v)?;
            } else {
                out.push('\\');
                out.push('U');
            }
        }
        Some('c') => match chars.next() {
            None => {
                out.push('\\');
                out.push('c');
            }
            Some('?') => out.push('\x7F'),
            Some('@') => out.push('\0'),
            Some(c) => {
                let v = (c.to_ascii_uppercase() as u32) & 0x1F;
                push_codepoint(out, v)?;
            }
        },
        Some(other) => {
            out.push('\\');
            out.push(other);
        }
    }
    Ok(())
}

/// Reads up to `max` hex digits (greedy, stops at first non-hex char) and
/// returns their value. Caller has already confirmed at least one hex
/// digit is available.
fn scan_hex_digits(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    max: u32,
) -> u32 {
    let mut v: u32 = 0;
    for _ in 0..max {
        match chars.peek().copied() {
            Some(d) if d.is_ascii_hexdigit() => {
                chars.next();
                v = v.wrapping_mul(16) + d.to_digit(16).unwrap();
            }
            _ => break,
        }
    }
    v
}

/// Appends a codepoint to `out`, or errors if the value is not a valid
/// Unicode scalar (surrogate range or > U+10FFFF).
fn push_codepoint(out: &mut String, v: u32) -> Result<(), LexError> {
    match char::from_u32(v) {
        Some(c) => {
            out.push(c);
            Ok(())
        }
        None => Err(LexError::AnsiCInvalidCodepoint(v)),
    }
}
```

- [ ] **Step 1.8: Run the first test; confirm it passes**

Run: `cargo test --lib ansi_c_quote_newline_escape -- --nocapture`
Expected: PASS.

- [ ] **Step 1.9: Add the remaining 11 lexer unit tests**

Append these tests to the same `mod tests` block in `src/lexer.rs`:

```rust
    #[test]
    fn ansi_c_quote_tab_escape() {
        let toks = tokenize("$'a\\tb'").expect("lex");
        let Token::Word(parts) = &toks[0] else { panic!("expected Word") };
        assert_eq!(parts.len(), 1);
        let WordPart::Literal { text, quoted } = &parts[0] else { panic!("expected Literal") };
        assert_eq!(text, "a\tb");
        assert!(*quoted);
    }

    #[test]
    fn ansi_c_quote_backslash_and_quote() {
        // $'\\\'' → literal backslash + literal quote (two chars)
        let toks = tokenize("$'\\\\\\''").expect("lex");
        let Token::Word(parts) = &toks[0] else { panic!("expected Word") };
        assert_eq!(parts.len(), 1);
        let WordPart::Literal { text, .. } = &parts[0] else { panic!("expected Literal") };
        assert_eq!(text, "\\'");
    }

    #[test]
    fn ansi_c_quote_hex_escapes() {
        // \x48\x69 → "Hi"
        let toks = tokenize("$'\\x48\\x69'").expect("lex");
        let Token::Word(parts) = &toks[0] else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = &parts[0] else { panic!("expected Literal") };
        assert_eq!(text, "Hi");
    }

    #[test]
    fn ansi_c_quote_octal_escapes() {
        // \110\151 → "Hi"  (0o110=72='H', 0o151=105='i')
        let toks = tokenize("$'\\110\\151'").expect("lex");
        let Token::Word(parts) = &toks[0] else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = &parts[0] else { panic!("expected Literal") };
        assert_eq!(text, "Hi");
    }

    #[test]
    fn ansi_c_quote_octal_greedy_stops_at_non_octal() {
        // \18 → \1 followed by literal '8' → "\x01" + "8"
        let toks = tokenize("$'\\18'").expect("lex");
        let Token::Word(parts) = &toks[0] else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = &parts[0] else { panic!("expected Literal") };
        assert_eq!(text, "\x018");
    }

    #[test]
    fn ansi_c_quote_unicode_4digit() {
        // é → é (U+00E9, "LATIN SMALL LETTER E WITH ACUTE")
        let toks = tokenize("$'\\u00e9'").expect("lex");
        let Token::Word(parts) = &toks[0] else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = &parts[0] else { panic!("expected Literal") };
        assert_eq!(text, "é");
    }

    #[test]
    fn ansi_c_quote_unicode_8digit() {
        // \U0001F600 → 😀 (grinning face)
        let toks = tokenize("$'\\U0001F600'").expect("lex");
        let Token::Word(parts) = &toks[0] else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = &parts[0] else { panic!("expected Literal") };
        assert_eq!(text, "\u{1F600}");
    }

    #[test]
    fn ansi_c_quote_control_chars() {
        // \cA → \x01, \cZ → \x1A
        let toks = tokenize("$'\\cA\\cZ'").expect("lex");
        let Token::Word(parts) = &toks[0] else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = &parts[0] else { panic!("expected Literal") };
        assert_eq!(text, "\x01\x1a");
    }

    #[test]
    fn ansi_c_quote_unknown_escape_preserves_both() {
        // \q → literal "\q" (two chars)
        let toks = tokenize("$'\\q'").expect("lex");
        let Token::Word(parts) = &toks[0] else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = &parts[0] else { panic!("expected Literal") };
        assert_eq!(text, "\\q");
    }

    #[test]
    fn ansi_c_quote_empty() {
        let toks = tokenize("$''").expect("lex");
        let Token::Word(parts) = &toks[0] else { panic!("expected Word") };
        assert_eq!(parts.len(), 1);
        let WordPart::Literal { text, quoted } = &parts[0] else { panic!("expected Literal") };
        assert_eq!(text, "");
        assert!(*quoted);
    }

    #[test]
    fn ansi_c_quote_unterminated_is_error() {
        let err = tokenize("$'foo").unwrap_err();
        assert_eq!(err, LexError::UnterminatedQuote);
    }

    #[test]
    fn ansi_c_quote_invalid_codepoint_is_error() {
        // \uD800 is a surrogate, not a valid Unicode scalar
        let err = tokenize("$'\\uD800'").unwrap_err();
        assert_eq!(err, LexError::AnsiCInvalidCodepoint(0xD800));
    }

    #[test]
    fn ansi_c_quote_concatenates_with_adjacent_word() {
        // $'a\nb'foo → single Word with two Literal parts
        let toks = tokenize("$'a\\nb'foo").expect("lex");
        let Token::Word(parts) = &toks[0] else { panic!("expected Word") };
        assert_eq!(parts.len(), 2);
        let WordPart::Literal { text, quoted } = &parts[0] else { panic!("expected Literal at [0]") };
        assert_eq!(text, "a\nb");
        assert!(*quoted);
        let WordPart::Literal { text, quoted } = &parts[1] else { panic!("expected Literal at [1]") };
        assert_eq!(text, "foo");
        assert!(!*quoted);
    }
```

- [ ] **Step 1.10: Run the full new test set**

Run: `cargo test --lib ansi_c_quote -- --nocapture`
Expected: 13 tests pass (the original `ansi_c_quote_newline_escape` + the
12 above). All `ansi_c_quote_*` show PASS.

- [ ] **Step 1.11: Run the whole lexer test module to confirm no regression**

Run: `cargo test --lib lexer::tests`
Expected: all existing lexer tests pass alongside the new ones.

- [ ] **Step 1.12: Run clippy on the touched files**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

### Step 1.13: Commit

- [ ] **Step 1.13: Commit Task 1**

```bash
git add src/lexer.rs src/shell.rs
git commit -m "$(cat <<'EOF'
lex: $'…' ANSI-C quoting (v39 task 1)

Add a new arm in read_dollar_expansion that recognizes $' and dispatches
to a new read_ansi_c_quoted helper. The helper scans up to the matching
unescaped ' and decodes the full bash escape set: \a \b \e \E \f \n \r \t
\v \\ \' \" \? plus octal (\nnn 1-3 digits), hex (\xHH 1-2 digits),
Unicode (\uXXXX 1-4 hex, \UXXXXXXXX 1-8 hex), and control (\cX).
Numeric escapes are interpreted as Unicode codepoints, not raw bytes —
documented as L-11. Unknown escapes (\q) preserve both the backslash and
the following character literally, matching bash. The result is emitted
as a single quoted WordPart::Literal, which already suppresses word
splitting and globbing downstream.

A new LexError variant AnsiCInvalidCodepoint(u32) signals \u/\U/\xHH/\nnn
that resolve to surrogates or values > U+10FFFF; rendered by
lex_error_message in src/shell.rs.

13 lexer unit tests cover each escape category plus greedy digit stops,
unterminated body, invalid codepoint, empty body, and concatenation with
an adjacent unquoted word.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Expected: commit succeeds, hook (if any) passes.

---

## Task 2: Integration tests

**Files:**
- Create: `tests/ansi_c_quoting_integration.rs`

Six binary-driven tests covering end-to-end behavior through the running
`huck` binary. Pattern is identical to
`tests/arith_completion_integration.rs`.

- [ ] **Step 2.1: Create the integration test file**

Create `tests/ansi_c_quoting_integration.rs` with this content:

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
fn ansi_c_tab_in_echo() {
    // $'a\tb' should print "a<TAB>b\n"
    let (out, _) = run("echo $'a\\tb'\nexit\n");
    assert!(out.lines().any(|l| l == "a\tb"), "stdout: {:?}", out);
}

#[test]
fn ansi_c_unicode_letter() {
    // $'é' should print "é\n"
    let (out, _) = run("echo $'\\u00e9'\nexit\n");
    assert!(out.lines().any(|l| l == "é"), "stdout: {:?}", out);
}

#[test]
fn ansi_c_hex_escapes_form_string() {
    // printf '%s' $'\x48\x69' → "Hi" (no trailing newline)
    let (out, _) = run("printf '%s' $'\\x48\\x69'\nexit\n");
    assert_eq!(out, "Hi", "stdout: {:?}", out);
}

#[test]
fn ansi_c_in_assignment_then_double_quoted_expansion() {
    // x=$'\n'; echo "[$x]" → "[<NL>]" i.e. "[\n]\n"
    let (out, _) = run("x=$'\\n'\necho \"[$x]\"\nexit\n");
    // Expect a line "[" then a line "]"
    let lines: Vec<&str> = out.lines().collect();
    assert!(
        lines.iter().any(|&l| l == "[") && lines.iter().any(|&l| l == "]"),
        "stdout: {:?}",
        out
    );
}

#[test]
fn ansi_c_case_pattern_matches_decoded() {
    // case $'\t' in $'\t') echo yes ;; *) echo no ;; esac → "yes"
    let script = "case $'\\t' in\n  $'\\t') echo yes ;;\n  *) echo no ;;\nesac\nexit\n";
    let (out, _) = run(script);
    assert!(out.lines().any(|l| l == "yes"), "stdout: {:?}", out);
}

#[test]
fn ansi_c_concatenation_with_unquoted_suffix() {
    // echo $'a\tb'cd → "a<TAB>bcd"
    let (out, _) = run("echo $'a\\tb'cd\nexit\n");
    assert!(out.lines().any(|l| l == "a\tbcd"), "stdout: {:?}", out);
}
```

- [ ] **Step 2.2: Run the new integration suite**

Run: `cargo test --test ansi_c_quoting_integration`
Expected: 6 tests pass.

- [ ] **Step 2.3: Run the full integration suite to confirm no cross-test regressions**

Run: `cargo test --tests`
Expected: all integration tests pass. The known PTY flake
`pty_compound_stage_pipeline_stops_and_resumes` may fail intermittently
under load — re-run it in isolation
(`cargo test --test pty_interactive pty_compound_stage_pipeline_stops_and_resumes`)
to confirm it's the unrelated v29-era flake, not a new break.

- [ ] **Step 2.4: Commit Task 2**

```bash
git add tests/ansi_c_quoting_integration.rs
git commit -m "$(cat <<'EOF'
test: $'…' ANSI-C quoting integration coverage (v39 task 2)

Six binary-driven tests exercising $'...' in the contexts where shell
scripts most commonly use it: as an echo / printf argument, with hex /
unicode / control / tab escapes, in a variable assignment subsequently
re-expanded inside double quotes, in a case pattern, and concatenated
with an adjacent unquoted suffix.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Expected: commit succeeds.

---

## Task 3: Docs + full-suite verify

**Files:**
- Modify: `docs/bash-divergences.md` (M-28 entry, new L-11 entry, Change log row, summary count)
- Modify: `README.md` (add v39 row to the version table)

- [ ] **Step 3.1: Flip M-28 status in the Quoting section**

In `docs/bash-divergences.md`, find the M-28 entry inside the
`### Quoting` subsection (currently around line 169). Replace:

```markdown
- **M-28: `$'…'` ANSI-C quoting** — `[deferred]` high. huck: parses `$'\n'` as `$` + literal `\n` text. bash: processes C escapes.
```

with:

```markdown
- **M-28: `$'…'` ANSI-C quoting** — `[fixed v39]` high. All 16 bash escapes: `\a`, `\b`, `\e`/`\E`, `\f`, `\n`, `\r`, `\t`, `\v`, `\\`, `\'`, `\"`, `\?`, `\nnn` (1-3 octal), `\xHH` (1-2 hex), `\uXXXX` (1-4 hex), `\UXXXXXXXX` (1-8 hex), `\cX` (control). Numeric escapes are interpreted as Unicode codepoints rather than raw bytes — see L-11. Unknown escapes preserve both the backslash and the following character (`$'\q'` → literal `\q`). Result emitted as a quoted Literal: no further expansion, no word splitting, no globbing. Implemented purely in the lexer (`read_dollar_expansion` + `read_ansi_c_quoted` + `decode_ansi_c_escape`).
```

- [ ] **Step 3.2: Add the new L-11 divergence entry**

In `docs/bash-divergences.md`, find the Tier 4 list around lines 296-304
(the block that starts with `- **L-01**` and ends with `- **L-07**`).
After the `- **L-07**` line and before the `### L-08:` header, the file
currently has the short L-01..L-07 list. The longer L-08, L-09, L-10
entries follow under `###` subheaders. Add a new L-11 subheader entry
after the L-10 block (which lives around line 322 — find it by searching
for `### L-10:` and inserting after that subsection's body, before the
next `## Change log` heading).

Append this new subsection:

```markdown
### L-11: `$'\xHH'` and `$'\nnn'` produce Unicode codepoints, not raw bytes

Bash inserts the raw byte value (0x00–0xFF) directly into the output
string. huck, whose strings are Rust `String` (UTF-8), interprets the
numeric value as a Unicode codepoint via `char::from_u32`. For ASCII-range
values (< 0x80) the two encodings are bit-identical. For high-bit values
the divergence is visible: bash's `$'\xFF'` is a single byte (`0xFF`),
huck's `$'\xFF'` is the two-byte UTF-8 encoding of U+00FF
(`0xC3 0xBF`).

This aligns with L-04 (huck's Unicode-by-default convention for parameter
expansion). Scripts that depend on injecting raw high bytes via
`$'\xHH'` — rare in practice — will see different output sizes.
Surrogate-range escapes (`\uD800`..`\uDFFF`) and codepoints above
U+10FFFF are rejected with a `LexError::AnsiCInvalidCodepoint` rather
than silently producing invalid UTF-8.
```

- [ ] **Step 3.3: Add the v39 row to the Change log**

In `docs/bash-divergences.md`, find the `## Change log` heading
(around line 332) and the most recent entry — the `**2026-05-28**` line
about v38. Add a new dated bullet immediately after it (still under the
same `## Change log` heading):

```markdown
- **2026-05-28**: M-28 (`$'…'` ANSI-C quoting) shipped as v39. New arm in `read_dollar_expansion` dispatches to `read_ansi_c_quoted` + `decode_ansi_c_escape`. All 16 bash escape forms supported. Numeric escapes resolve to Unicode codepoints (new L-11 divergence for `\xHH`/`\nnn` > 0x7F). Unknown escapes preserve `\` + following char. New `LexError::AnsiCInvalidCodepoint(u32)` for surrogates / out-of-range values. Pure lexer change — no parser, AST, executor, or expansion changes.
```

- [ ] **Step 3.4: Bump the Tier 4 summary count**

In `docs/bash-divergences.md` line 27, find the row:

```markdown
| Low-impact (Tier 4) | 10 | Edge cases, cosmetic (L-08 added v29: redirect source-order divergence; L-09 added v30: regex-engine divergence; L-10 added v33: split-scanner limitation) |
```

Replace it with:

```markdown
| Low-impact (Tier 4) | 11 | Edge cases, cosmetic (L-08 added v29: redirect source-order divergence; L-09 added v30: regex-engine divergence; L-10 added v33: split-scanner limitation; L-11 added v39: `$'\xHH'` Unicode-vs-byte) |
```

- [ ] **Step 3.5: Add the v39 row to the README version table**

In `README.md`, find the version table (around lines 40-48). After the
existing `| v38 | Arithmetic completion ...` row, add:

```markdown
| v39       | ANSI-C quoting `$'…'` (M-28)                                   |
```

So the final block reads:

```markdown
| v37       | Case modification `${var^^}` / `${var,,}` (M-17)               |
| v38       | Arithmetic completion (M-55 + M-56 + M-57 + `**`)              |
| v39       | ANSI-C quoting `$'…'` (M-28)                                   |
```

- [ ] **Step 3.6: Run the full suite**

Run: `cargo test --all-targets`
Expected: all tests pass (modulo the known PTY flake — see Step 2.3).

- [ ] **Step 3.7: Run clippy on everything**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 3.8: Commit Task 3**

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: mark M-28 fixed; v39 in README; new L-11 divergence

Quoting section: M-28 flipped from [deferred] to [fixed v39] with the
full feature description (all 16 escape forms + Unicode codepoint
semantics + unknown-escape rule).

Tier 4: new L-11 subsection documenting that $'\xHH' and $'\nnn' with
values > 0x7F insert a UTF-8 codepoint rather than the raw byte (a
consequence of huck's String-based representation, aligned with L-04).
Summary table count bumped from 10 to 11.

Change log: 2026-05-28 v39 entry summarizing the lexer-only
implementation footprint.

README: v39 row added to the version table.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Expected: commit succeeds.

---

## Final verification (controller, not a task)

After all three task commits land, the controller should:

1. Run `cargo test --all-targets` once more from a clean checkout to confirm
   no regressions.
2. Run `cargo clippy --all-targets -- -D warnings`.
3. Confirm the branch has exactly three commits ahead of `main`:
   `v39 task 1`, `v39 task 2`, `v39 task 3`.
4. Dispatch the final code-reviewer subagent over the full diff
   (`main..v39-ansi-c-quoting`).
5. Merge to `main` with `--no-ff`, push, delete the branch, and update the
   `huck iterations` memory entry to include v39.
