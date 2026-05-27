# huck v33 — `${var:offset:length}` Substring Expansion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close M-16 by implementing bash's `${var:offset}` and
`${var:offset:length}` substring parameter expansion for scalar named
variables and positional parameters. Array slicing on `$@` / `$*` is
explicitly out of scope.

**Architecture:** Pure expansion-layer feature. The lexer `:` dispatch in
`parse_braced_param` gets a new fall-through arm that emits a new
`ParamModifier::Substring { offset: Word, length: Option<Word> }` AST node
when the char after `:` is not one of `-=+?` (which keep their existing
colon-modifier meanings). A new `scan_substring_operands` helper (modeled
on v32's review-fix `scan_substitution_operand`) reuses `scan_braced_operand`
for depth/quote-aware operand collection, then splits on the first
depth-zero `:`. The digit-only brace branch is extended to share the
modifier dispatch with named-var brace forms so `${1:0:3}` works. The
evaluator adds a pure `substring()` helper plus a new `ParamModifier::Substring`
arm in `expand_modifier` that expands each operand to a string, runs it
through `arith::parse` + `arith::eval`, and applies the bash 5.x edge-case
rules with char-counting (Unicode codepoints).

**Tech Stack:** Rust. Reuses existing `crate::arith` parser/evaluator from
v22. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-27-huck-substring-expansion-design.md`

**Branch:** `v33-substring-expansion` (already created and checked out).

---

### Task 1: AST + placeholder evaluator arm

**Files:**
- Modify: `src/lexer.rs` (`ParamModifier` enum at `src/lexer.rs:57-71`)
- Modify: `src/param_expansion.rs` (`expand_modifier` match block at `src/param_expansion.rs:17-84`)

**Note for implementer:** This task only adds the AST variant and a
compile-clean placeholder evaluator arm. Lex-time emission comes in Task 2;
real evaluation comes in Task 4.

- [ ] **Step 1: Add the `Substring` variant to `ParamModifier`**

In `src/lexer.rs`, add a new variant at the end of the `ParamModifier`
enum (after the existing `Substitute { ... }` variant, before the closing `}`
of the enum at line 71):

```rust
    Substring {
        offset: Word,
        length: Option<Word>,
    },
```

- [ ] **Step 2: Build to flush the exhaustiveness error**

Run: `cargo build 2>&1 | tail -30`
Expected: an `error[E0004]: non-exhaustive patterns: '&ParamModifier::Substring { .. }' not covered` in `src/param_expansion.rs::expand_modifier` around line 17. Note the file + line for Step 3.

- [ ] **Step 3: Add a temporary placeholder evaluator arm so the build passes**

In `src/param_expansion.rs::expand_modifier`, immediately before the closing
`}` of the `match modifier { ... }` block (right after the `Substitute` arm
that ends around line 83):

```rust
        ParamModifier::Substring { .. } => {
            // Filled in by Task 4.
            ExpansionResult::Value(shell.lookup_var(name).unwrap_or_default())
        }
```

- [ ] **Step 4: Re-build to verify clean**

Run: `cargo build 2>&1 | tail -5`
Expected: clean build, 0 errors. (Warnings about unused fields on the new
variant are acceptable — they go away in Task 4.)

- [ ] **Step 5: Run the full test suite to confirm no regression**

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | tail -5`
Expected: all suites pass (1230 baseline from end of v32).

- [ ] **Step 6: Commit**

```bash
git add src/lexer.rs src/param_expansion.rs
git commit -m "ast: add ParamModifier::Substring scaffold (v33 task 1)"
```

---

### Task 2: Lexer — `:` dispatch, `scan_substring_operands`, digit-name fall-through

**Files:**
- Modify: `src/lexer.rs`
  - Refactor existing modifier dispatch (currently `lexer.rs:1074-1140`) so the digit-only branch (`lexer.rs:1051-1067`) can share it.
  - Replace the `Some(':')` arm at `lexer.rs:1079-1090` with a peek-based dispatcher.
  - New helper `scan_substring_operands` near `scan_substitution_operand` (~line 1185).

**Note for implementer:** The two big edits in this task are independent —
do them in the order shown (operand scanner first, then dispatch hookup,
then digit fall-through). After each sub-edit, re-build but don't commit
until the end.

`scan_braced_operand` (defined at `src/lexer.rs:769`) returns the raw body
of a `${…}` operand up to (but not consuming) the matching `}`, with
brace-depth tracking and quote protection. The v32 review-fix
`scan_substitution_operand` (defined ~line 1185) showed the pattern:
delegate to `scan_braced_operand`, then do a second pass that splits on
the first depth-zero delimiter. Substring follows the same shape but
splits on `:` instead of `/`.

- [ ] **Step 1: Write the failing lexer tests**

Append to the `tests` module in `src/lexer.rs` (right after the existing
`brace_substitute_nested_braced_var_in_replacement` test — search for that
identifier to find the spot). The helpers `single_param_expansion` and
`word_to_literal` already exist in this module from v32.

```rust
    #[test]
    fn brace_substring_simple() {
        let mut t = tokenize_words("${name:1}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { name, modifier: ParamModifier::Substring { offset, length }, quoted } = part {
            assert_eq!(name, "name");
            assert!(!quoted);
            assert_eq!(word_to_literal(&offset), "1");
            assert!(length.is_none());
        } else { panic!("expected Substring") }
    }

    #[test]
    fn brace_substring_with_length() {
        let mut t = tokenize_words("${name:1:3}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Substring { offset, length }, .. } = part {
            assert_eq!(word_to_literal(&offset), "1");
            assert_eq!(word_to_literal(&length.expect("length")), "3");
        } else { panic!("expected Substring") }
    }

    #[test]
    fn brace_substring_negative_offset_with_space() {
        // `${name: -3}` — the space disambiguates from `:-` (UseDefault).
        let mut t = tokenize_words("${name: -3}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Substring { offset, .. }, .. } = part {
            assert_eq!(word_to_literal(&offset), " -3");
        } else { panic!("expected Substring, got {part:?}") }
    }

    #[test]
    fn brace_substring_no_space_is_use_default_regression() {
        // `${name:-3}` — no space, so this MUST remain UseDefault with default "3".
        let mut t = tokenize_words("${name:-3}").unwrap();
        let part = single_param_expansion(&mut t);
        assert!(
            matches!(part, WordPart::ParamExpansion { modifier: ParamModifier::UseDefault { colon: true, .. }, .. }),
            "expected UseDefault, got {part:?}",
        );
    }

    #[test]
    fn brace_substring_positional() {
        // `${1:0:3}` — must emit ParamExpansion (not Var) so the modifier runs.
        let mut t = tokenize_words("${1:0:3}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { name, modifier: ParamModifier::Substring { offset, length }, .. } = part {
            assert_eq!(name, "1");
            assert_eq!(word_to_literal(&offset), "0");
            assert_eq!(word_to_literal(&length.expect("length")), "3");
        } else { panic!("expected Substring on positional, got {part:?}") }
    }

    #[test]
    fn brace_substring_nested_braced_var_in_operand() {
        // The depth-aware split must not break on the inner `${start}`'s `}`.
        let mut t = tokenize_words("${name:${start}:${len}}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Substring { offset, length }, .. } = part {
            // Offset word should contain a Var part for `start`.
            let Word(off_parts) = &offset;
            assert!(
                off_parts.iter().any(|p| matches!(p, WordPart::Var { name, .. } if name == "start")),
                "expected Var(start) in offset, got {off_parts:?}",
            );
            // Length word should contain a Var part for `len`.
            let Word(len_parts) = length.as_ref().expect("length");
            assert!(
                len_parts.iter().any(|p| matches!(p, WordPart::Var { name, .. } if name == "len")),
                "expected Var(len) in length, got {len_parts:?}",
            );
        } else { panic!("expected Substring") }
    }

    #[test]
    fn brace_substring_unterminated_is_error() {
        assert!(matches!(
            tokenize_words("${name:1:3"),
            Err(LexError::UnterminatedBrace)
        ));
    }
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cargo test --lib brace_substring_ 2>&1 | tail -30`
Expected: all 7 tests fail. The current behavior is
`LexError::InvalidBraceModifier(":1")` for any `:` followed by a digit, so
most will fail with a tokenize_words `.unwrap()` panic.

- [ ] **Step 3: Implement `scan_substring_operands`**

Add this helper to `src/lexer.rs` immediately AFTER `split_substitution_body`
(which ends ~line 1258 — search for `fn split_substitution_body`). Use the
exact same shape: delegate to `scan_braced_operand`, then split with
depth/quote tracking.

```rust
/// Walks the chars iterator from just after the leading `:` of a substring
/// operand. Delegates to `scan_braced_operand` to collect the raw body
/// (which depth-tracks nested `${...}` and protects `}` inside quoted
/// spans), then splits on the first unescaped `:` at brace-depth zero
/// outside any quoted span. Returns `(offset_word, Some(length_word))` if a
/// delimiter was found, or `(offset_word, None)` if no `:` appeared in the
/// body. Escapes follow the same rules as `split_substitution_body`:
/// `\:` is reserved for a literal colon, `\\` is a literal backslash,
/// any other `\x` passes through unchanged.
fn scan_substring_operands(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<(Word, Option<Word>), LexError> {
    let body = scan_braced_operand(chars)?;
    let (offset_src, length_src) = split_substring_body(&body);
    let offset = parse_braced_operand(&offset_src)?;
    let length = match length_src {
        Some(s) => Some(parse_braced_operand(&s)?),
        None => None,
    };
    Ok((offset, length))
}

/// Splits a substring-operand body (as returned by `scan_braced_operand`)
/// on the first unescaped `:` that sits at brace-depth zero outside any
/// quoted span. Returns `(offset_src, Some(length_src))` if a delimiter
/// was found, or `(offset_src, None)` otherwise (the no-length form).
fn split_substring_body(body: &str) -> (String, Option<String>) {
    let mut offset = String::new();
    let mut length = String::new();
    let mut delim_seen = false;
    let mut depth: u32 = 0;
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let lit = match chars.peek().copied() {
                    Some(':') => { chars.next(); ':' }
                    Some('\\') => { chars.next(); '\\' }
                    _ => '\\',
                };
                if delim_seen { length.push(lit); } else { offset.push(lit); }
            }
            '"' => {
                let dst = if delim_seen { &mut length } else { &mut offset };
                dst.push('"');
                while let Some(qc) = chars.next() {
                    dst.push(qc);
                    if qc == '\\' {
                        if let Some(nc) = chars.next() { dst.push(nc); }
                    } else if qc == '"' {
                        break;
                    }
                }
            }
            '\'' => {
                let dst = if delim_seen { &mut length } else { &mut offset };
                dst.push('\'');
                for qc in chars.by_ref() {
                    dst.push(qc);
                    if qc == '\'' { break; }
                }
            }
            '{' => {
                depth += 1;
                if delim_seen { length.push('{'); } else { offset.push('{'); }
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if delim_seen { length.push('}'); } else { offset.push('}'); }
            }
            ':' if depth == 0 && !delim_seen => { delim_seen = true; }
            _ => {
                if delim_seen { length.push(c); } else { offset.push(c); }
            }
        }
    }
    if delim_seen {
        (offset, Some(length))
    } else {
        (offset, None)
    }
}
```

- [ ] **Step 4: Refactor modifier dispatch into a helper so the digit branch can share it**

The current named-var branch in `parse_braced_param` reads the name then
dispatches modifiers based on the next char (currently `src/lexer.rs:1074-1140`).
The digit-only branch (currently `src/lexer.rs:1051-1067`) reads the digit
name then immediately requires `}`. To support `${1:0:3}`, the digit
branch must dispatch modifiers too.

Extract the body of the named-var modifier dispatch (everything inside the
`match chars.next() { ... }` block at `src/lexer.rs:1074-1140`) into a new
helper function. Right after `read_braced_name` (around line 1164), add:

```rust
/// Dispatches a `${name<modifier>...}` form once `name` has been read. The
/// next char to read from `chars` is whatever follows the name (typically
/// `}`, `:`, `-`, `=`, `?`, `+`, `#`, `%`, or `/`). Pushes a single
/// `WordPart` (`Var` or `ParamExpansion`) onto `parts`.
fn dispatch_braced_modifier(
    name: String,
    quoted: bool,
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    parts: &mut Vec<WordPart>,
) -> Result<(), LexError> {
    match chars.next() {
        Some('}') => {
            parts.push(WordPart::Var { name, quoted });
            Ok(())
        }
        Some(':') => {
            match chars.peek().copied() {
                Some('-') => {
                    chars.next();
                    let modifier = modifier_with_operand(chars, |w| ParamModifier::UseDefault { word: w, colon: true })?;
                    parts.push(WordPart::ParamExpansion { name, modifier, quoted });
                    Ok(())
                }
                Some('=') => {
                    chars.next();
                    let modifier = modifier_with_operand(chars, |w| ParamModifier::AssignDefault { word: w, colon: true })?;
                    parts.push(WordPart::ParamExpansion { name, modifier, quoted });
                    Ok(())
                }
                Some('?') => {
                    chars.next();
                    let modifier = modifier_with_operand(chars, |w| ParamModifier::ErrorIfUnset { word: w, colon: true })?;
                    parts.push(WordPart::ParamExpansion { name, modifier, quoted });
                    Ok(())
                }
                Some('+') => {
                    chars.next();
                    let modifier = modifier_with_operand(chars, |w| ParamModifier::UseAlternate { word: w, colon: true })?;
                    parts.push(WordPart::ParamExpansion { name, modifier, quoted });
                    Ok(())
                }
                Some(_) => {
                    let (offset, length) = scan_substring_operands(chars)?;
                    parts.push(WordPart::ParamExpansion {
                        name,
                        modifier: ParamModifier::Substring { offset, length },
                        quoted,
                    });
                    Ok(())
                }
                None => Err(LexError::UnterminatedBrace),
            }
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
        Some('/') => {
            let all = chars.peek() == Some(&'/');
            if all { chars.next(); }
            let anchor = match chars.peek().copied() {
                Some('#') if !all => { chars.next(); SubstAnchor::Prefix }
                Some('%') if !all => { chars.next(); SubstAnchor::Suffix }
                _ => SubstAnchor::None,
            };
            let (pattern, replacement) = scan_substitution_operand(chars)?;
            parts.push(WordPart::ParamExpansion {
                name,
                modifier: ParamModifier::Substitute { pattern, replacement, anchor, all },
                quoted,
            });
            Ok(())
        }
        Some(c) => Err(LexError::InvalidBraceModifier(c.to_string())),
        None => Err(LexError::UnterminatedBrace),
    }
}
```

- [ ] **Step 5: Replace the named-var modifier dispatch with a call to the helper**

In `parse_braced_param` at `src/lexer.rs:1069-1143`, replace the block:

```rust
    let name = read_braced_name(chars)?;
    if name.is_empty() {
        return Err(LexError::EmptyParamName);
    }

    match chars.next() {
        Some('}') => { ... }
        Some(':') => { ... }
        ...
        Some(c) => Err(LexError::InvalidBraceModifier(c.to_string())),
        None => Err(LexError::UnterminatedBrace),
    }
```

with:

```rust
    let name = read_braced_name(chars)?;
    if name.is_empty() {
        return Err(LexError::EmptyParamName);
    }
    dispatch_braced_modifier(name, quoted, chars, parts)
```

- [ ] **Step 6: Replace the digit-only branch with a call to the helper**

In `parse_braced_param` at `src/lexer.rs:1051-1067`, replace:

```rust
    if matches!(chars.peek().copied(), Some(c) if c.is_ascii_digit()) {
        let mut name = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() {
                name.push(c);
                chars.next();
            } else {
                break;
            }
        }
        if chars.next() != Some('}') {
            return Err(LexError::UnterminatedBrace);
        }
        parts.push(WordPart::Var { name, quoted });
        return Ok(());
    }
```

with:

```rust
    if matches!(chars.peek().copied(), Some(c) if c.is_ascii_digit()) {
        let mut name = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() {
                name.push(c);
                chars.next();
            } else {
                break;
            }
        }
        return dispatch_braced_modifier(name, quoted, chars, parts);
    }
```

- [ ] **Step 7: Build and run the new tests**

Run: `cargo build 2>&1 | tail -10`
Expected: clean build.

Run: `cargo test --lib brace_substring_ 2>&1 | tail -20`
Expected: all 7 new tests pass.

- [ ] **Step 8: Run the entire lexer test suite to confirm no regression**

Run: `cargo test --lib lexer::tests:: 2>&1 | tail -5`
Expected: 0 failures. (The lexer module has ~190 tests; all must still
pass. Special attention: `tokenize_braced_positional`, all
`brace_substitute_*`, all `brace_use_default_*` — these exercise the
refactor.)

- [ ] **Step 9: Commit**

```bash
git add src/lexer.rs
git commit -m "lex: ${var:off:len} dispatch + scan_substring_operands + digit-name modifier fall-through (v33 task 2)"
```

---

### Task 3: Evaluator — `substring()` helper + unit tests

**Files:**
- Modify: `src/param_expansion.rs` (new helper `substring()` near the existing `substitute()`; unit tests at the bottom of the tests module)

**Note for implementer:** This task is a pure-function helper. It does not
touch the `Shell` or any modifier dispatch — only takes a `&str`, two
`i64`s (one optional), and returns `Result<String, &'static str>`. The
algorithm is in the spec; the table below restates it for convenience.

Algorithm (char-counting throughout):
1. `chars: Vec<char> = value.chars().collect()`; `strlen = chars.len() as i64`.
2. Effective offset: if `offset >= 0` → `min(offset, strlen)`; if `offset < 0` → `max(strlen + offset, 0)`.
3. Effective length:
   - `None` → `strlen - eff_off`.
   - `Some(n), n >= 0` → `min(n, strlen - eff_off)`.
   - `Some(n), n < 0` → `strlen + n - eff_off`. If result < 0 → `Err("substring expression < 0")`.
4. Slice `chars[eff_off .. eff_off + eff_len]`, collect into `String`.

- [ ] **Step 1: Write the failing unit tests**

Append to the `tests` module in `src/param_expansion.rs` (just before the
final closing `}` of the module). The exhaustive table from the spec:

```rust
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
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --lib param_expansion::tests::substring_ 2>&1 | tail -20`
Expected: all 14 tests fail with "cannot find function `substring`".

- [ ] **Step 3: Implement `substring()`**

In `src/param_expansion.rs`, add this helper just below the existing
`substitute` function and above the `#[cfg(test)] mod tests` block:

```rust
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
```

- [ ] **Step 4: Run the unit tests to verify pass**

Run: `cargo test --lib param_expansion::tests::substring_ 2>&1 | tail -20`
Expected: all 14 tests pass.

- [ ] **Step 5: Run the entire param_expansion test module to confirm no regression**

Run: `cargo test --lib param_expansion 2>&1 | tail -5`
Expected: 0 failures.

- [ ] **Step 6: Commit**

```bash
git add src/param_expansion.rs
git commit -m "param: substring() helper for \${var:off:len} (v33 task 3)"
```

---

### Task 4: Evaluator — wire `Substring` arm into `expand_modifier`

**Files:**
- Modify: `src/param_expansion.rs` (replace the placeholder `Substring { .. }` arm added in Task 1 with the real call into `substring()`; add an `eval_arith_word` helper)

**Note for implementer:** The `eval_arith_word` helper needs to:
1. Expand the `Word` to a string (no field-splitting — use `expand_assignment`).
2. Parse the resulting string as arithmetic via `crate::arith::parse`.
3. Evaluate the parsed expression via `crate::arith::eval`.
4. On either error, print `huck: arithmetic: <msg>` and set `$? = 1`, return `Err(())`.

`crate::arith::parse(input: &str) -> Result<ArithExpr, ArithError>` lives
in `src/arith.rs:169`. `crate::arith::eval(expr: &ArithExpr, shell: &Shell)
-> Result<i64, ArithError>` lives in `src/arith.rs:278`. Both return
`ArithError` which has a `Display` impl (used by the existing
`WordPart::Arith` handling in `src/expand.rs:187-200`).

Reuse `shell.lookup_var(name).unwrap_or_default()` (not `shell.get(name)`)
to fetch the value — `lookup_var` handles positional names (`"1"`, `"2"`,
…) which `shell.get` does not.

- [ ] **Step 1: Write failing unit tests that go through `expand_modifier`**

Append to the `tests` module in `src/param_expansion.rs` (after the
existing `substring_*` unit tests from Task 3). A helper `lit` already
exists in this module from v32 — it builds a literal `Word`. If not, the
existing `expand_modifier_substitute_*` tests show how to construct
`Word`s with literal text.

```rust
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
        assert_eq!(r, ExpansionResult::Empty);
        assert_eq!(shell.last_status(), 1);
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
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --lib expand_modifier_substring_ 2>&1 | tail -30`
Expected: all 7 tests fail. Most will fail because the placeholder arm
from Task 1 just returns the raw var value without applying the substring.

- [ ] **Step 3: Add the `eval_arith_word` helper**

In `src/param_expansion.rs`, add this private helper near the top of the
file (just below the existing `expand_word_to_string` helper, or near the
other private helpers used by `expand_modifier`):

```rust
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
```

- [ ] **Step 4: Replace the placeholder `Substring` arm with the real impl**

In `src/param_expansion.rs::expand_modifier`, replace the placeholder arm
added in Task 1:

```rust
        ParamModifier::Substring { .. } => {
            // Filled in by Task 4.
            ExpansionResult::Value(shell.lookup_var(name).unwrap_or_default())
        }
```

with:

```rust
        ParamModifier::Substring { offset, length } => {
            let value = shell.lookup_var(name).unwrap_or_default();
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
                    shell.set_last_status(1);
                    ExpansionResult::Empty
                }
            }
        }
```

- [ ] **Step 5: Run tests to verify pass**

Run: `cargo test --lib expand_modifier_substring_ 2>&1 | tail -20`
Expected: all 7 tests pass.

- [ ] **Step 6: Run full lib test suite for regression check**

Run: `cargo test --lib 2>&1 | tail -10`
Expected: 0 failures. Baseline was 962 lib tests at end of v32; this task
adds ~7 (Task 3 added ~14), so the new baseline is ~983.

- [ ] **Step 7: Commit**

```bash
git add src/param_expansion.rs
git commit -m "param: wire Substring arm + eval_arith_word into expand_modifier (v33 task 4)"
```

---

### Task 5: Integration tests via the binary

**Files:**
- Create: `tests/param_substring_integration.rs`

**Note for implementer:** These are end-to-end tests that spawn the `huck`
binary with a script on stdin and assert on the captured stdout. Use the
exact same `run()` helper shape as `tests/param_substitution_integration.rs`.

Since `set --` is M-08-deferred, positional params are tested via function
calls (`f() { echo "${1:0:3}"; }; f hello`).

- [ ] **Step 1: Create the test file with end-to-end coverage**

Create `tests/param_substring_integration.rs` with:

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
fn substring_basic_offset_only() {
    let (out, _) = run("s=hello\necho ${s:1}\nexit\n");
    assert!(out.lines().any(|l| l == "ello"), "stdout: {out}");
}

#[test]
fn substring_offset_and_length() {
    let (out, _) = run("s=hello\necho ${s:1:3}\nexit\n");
    assert!(out.lines().any(|l| l == "ell"), "stdout: {out}");
}

#[test]
fn substring_offset_equals_strlen_is_empty() {
    let (out, _) = run("s=abc\necho \"[${s:3}]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[]"), "stdout: {out}");
}

#[test]
fn substring_offset_beyond_strlen_is_empty() {
    let (out, _) = run("s=abc\necho \"[${s:5}]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[]"), "stdout: {out}");
}

#[test]
fn substring_negative_offset_with_space() {
    // The space disambiguates from :- (UseDefault).
    let (out, _) = run("s=hello\necho ${s: -2}\nexit\n");
    assert!(out.lines().any(|l| l == "lo"), "stdout: {out}");
}

#[test]
fn substring_no_space_remains_use_default_regression() {
    // ${s:-default} must still mean UseDefault, not substring with offset=-default.
    let (out, _) = run("unset MAYBE 2>/dev/null\necho ${MAYBE:-fallback}\nexit\n");
    assert!(out.lines().any(|l| l == "fallback"), "stdout: {out}");
}

#[test]
fn substring_negative_length_counts_from_end() {
    // eff_len = 5 + -1 - 1 = 3.
    let (out, _) = run("s=hello\necho ${s:1:-1}\nexit\n");
    assert!(out.lines().any(|l| l == "ell"), "stdout: {out}");
}

#[test]
fn substring_negative_computed_length_errors() {
    let (out, err) = run("s=abc\necho \"[${s:0:-4}]\"\necho status=$?\nexit\n");
    // The error path returns Empty (so the field is empty) and sets $? to 1.
    assert!(out.lines().any(|l| l == "[]"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "status=1"), "stdout: {out}");
    assert!(err.contains("substring expression < 0"), "stderr: {err}");
}

#[test]
fn substring_unset_var_is_empty() {
    let (out, _) = run("echo \"[${MISSING:0:3}]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[]"), "stdout: {out}");
}

#[test]
fn substring_var_ref_in_offset() {
    let (out, _) = run("s=hello\nn=2\necho ${s:$n}\nexit\n");
    assert!(out.lines().any(|l| l == "llo"), "stdout: {out}");
}

#[test]
fn substring_arith_in_length() {
    let (out, _) = run("s=hello\nn=1\necho ${s:1:$((n+1))}\nexit\n");
    assert!(out.lines().any(|l| l == "el"), "stdout: {out}");
}

#[test]
fn substring_unicode() {
    let (out, _) = run("s=café\necho ${s:1:2}\nexit\n");
    assert!(out.lines().any(|l| l == "af"), "stdout: {out}");
}

#[test]
fn substring_inside_quotes_single_field() {
    // "${s:1:3}" with internal whitespace stays as one field (no IFS-split).
    let (out, _) = run("s=\"hi world\"\necho \"[${s:1:5}]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[i wor]"), "stdout: {out}");
}

#[test]
fn substring_in_pipeline_stage() {
    let (out, _) = run("s=hello\necho ${s:1:3} | cat\nexit\n");
    assert!(out.lines().any(|l| l == "ell"), "stdout: {out}");
}

#[test]
fn substring_positional_in_function() {
    let (out, _) = run("f() { echo \"${1:0:3}\"; }\nf hello\nexit\n");
    assert!(out.lines().any(|l| l == "hel"), "stdout: {out}");
}

#[test]
fn substring_bad_arith_returns_empty_sets_status() {
    let (out, err) = run("s=hello\necho \"[${s:@@@}]\"\necho status=$?\nexit\n");
    assert!(out.lines().any(|l| l == "[]"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "status=1"), "stdout: {out}");
    assert!(err.contains("arithmetic"), "stderr: {err}");
}
```

- [ ] **Step 2: Run the new integration tests**

Run: `cargo test --test param_substring_integration 2>&1 | tail -10`
Expected: all 16 tests pass.

- [ ] **Step 3: Commit**

```bash
git add tests/param_substring_integration.rs
git commit -m "test: \${var:off:len} integration coverage (v33 task 5)"
```

---

### Task 6: Docs + version blurb

**Files:**
- Modify: `docs/bash-divergences.md` (flip M-16 to fixed; amend L-04; add changelog row)
- Modify: `README.md` (new v33 row in the status table)

- [ ] **Step 1: Mark M-16 fixed in `docs/bash-divergences.md`**

Find the M-16 entry (search for `**M-16:`). Replace:

```markdown
- **M-16: `${var:off:len}` substring** — `[deferred]` high. huck: `InvalidBraceModifier(":N")`. bash: substring extraction.
```

with:

```markdown
- **M-16: `${var:off:len}` substring** — `[fixed v33]` high. `${var:offset}` and `${var:offset:length}` for scalar vars and positional params (`${1:0:3}`). Offset/length are full arithmetic expressions via `arith::parse` + `arith::eval` (variable refs, `+`/`-`/`*`/`/`/`%`, parentheses). Char-counting (codepoints), bash 5.x edge-case semantics: negative offset counts from end, negative length counts from end, negative computed length errors. **Out of scope (still open)**: `${@:off:len}` and `${*:off:len}` array slicing on positional params.
```

- [ ] **Step 2: Amend L-04 with a substring sub-bullet**

Find the L-04 entry (search for `**L-04**:`). Currently:

```markdown
- **L-04**: `${#var}` counts Unicode chars; bash counts bytes (matches in UTF-8 locale).
```

Replace with:

```markdown
- **L-04**: `${#var}` counts Unicode chars; bash counts bytes (matches in UTF-8 locale). v33 extends the same char-counting convention to `${var:off:len}` — offset/length units are codepoints, never byte indices. Slices never split a multi-byte UTF-8 codepoint.
```

- [ ] **Step 3: Add a changelog row at the bottom of `docs/bash-divergences.md`**

Find the `## Change log` section and append a new row in chronological order (the last row currently is the v32 entry from 2026-05-27):

```markdown
- **2026-05-27**: M-16 (`${var:off:len}` substring expansion) shipped as v33. Scalar vars + positional params; full arith in offset/length via reuse of v22's `arith::parse` + `arith::eval`; bash 5.x edge-case semantics with char-counting per L-04. Array slicing on `$@`/`$*` deferred (would require routing `@`/`*` through the brace lexer and emitting multiple fields).
```

- [ ] **Step 4: Update the README version blurb**

Find the v32 row in `README.md` (search for `v32`). Add a v33 row above it (the table is in reverse-chronological order — newest at top — based on the v31/v32 entries):

```markdown
| v33 | `${var:off:len}` substring (M-16) | 2026-05-27 |
```

(Match the exact column count and format of the existing v32 row — if the
table has extra columns like "Tests" or "Branch", fill them analogously.)

If the README has a paragraph-style status section instead of a table,
append a paragraph in the same style as the v32 paragraph.

- [ ] **Step 5: Commit**

```bash
git add docs/bash-divergences.md README.md
git commit -m "docs: mark M-16 fixed; v33 in README"
```

---

### Task 7: Full-suite verification (no separate commit)

**Files:** none — verification only.

- [ ] **Step 1: Run the entire test suite**

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | tail -30`
Expected: all suites pass. New baseline ~1256 (1230 from v32 + ~26 new
tests across Tasks 2/3/4/5).

If anything fails outside of PTY flakiness, STOP and investigate before
declaring task 7 done.

- [ ] **Step 2: Run clippy with the project's `-D warnings` policy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -10`
Expected: 0 warnings.

If clippy reports anything, fix it inline and re-run. Do not commit
clippy fixes as part of task 7 — fold them into the most-recent task's
commit via a separate review-fix commit if needed.

- [ ] **Step 3: Confirm working tree is clean**

Run: `git status`
Expected: `nothing to commit, working tree clean` on branch
`v33-substring-expansion`. No untracked files.

If untracked files exist (e.g., a stray `rust_out`), investigate before
proceeding — they may indicate a missed step.

**No commit for this task** — it's verification only. Hand back to the
parent session for the final code-reviewer dispatch + merge to main.
