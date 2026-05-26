# huck v32 — `${var/pat/repl}` Pattern Substitution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close M-15 by implementing the six bash `${var/pat/repl}`
substitution forms in huck (first / all / anchored-prefix /
anchored-suffix, each with optional replacement).

**Architecture:** Pure expansion-layer feature. New lexer arm in
`parse_braced_param` recognizes `/` after the parameter name, splits the
operand on the first unescaped `/`, and emits a new
`ParamModifier::Substitute { pattern, replacement, anchor, all }` AST
node. New evaluator helper `substitute()` in `src/param_expansion.rs`
reuses `glob::Pattern` and the `char_indices` boundary scan used by
`remove_prefix`/`remove_suffix`. No executor changes.

**Tech Stack:** Rust, `glob` crate (already a dependency).

**Spec:** `docs/superpowers/specs/2026-05-26-huck-pattern-substitution-design.md`

**Branch:** `v32-pattern-substitution` (already created and checked out).

---

### Task 1: AST + lexer scaffold for `Substitute`

**Files:**
- Modify: `src/lexer.rs` (`ParamModifier` enum around line 50; new `SubstAnchor` enum nearby; new `Some('/')` arm in `parse_braced_param` around line 1111 between `Some('%')` and the `Some(c) => Err(...)` fallback; new `scan_substitution_operand` helper near `modifier_with_operand` around line 1139)

**Note for implementer:** Read the surrounding context first
(`src/lexer.rs:1061-1149`) so you mirror the existing `Some('#')` /
`Some('%')` arm style exactly. The boundary scan for operands uses
`scan_braced_operand` + `parse_braced_operand`; both already exist —
your new helper sits alongside them, not replacing them.

- [ ] **Step 1: Add the `SubstAnchor` enum next to `ParamModifier`**

In `src/lexer.rs`, immediately above the `ParamModifier` enum declaration
(around line 49):

```rust
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum SubstAnchor {
    None,    // ${var/pat/repl} and ${var//pat/repl}
    Prefix,  // ${var/#pat/repl}
    Suffix,  // ${var/%pat/repl}
}
```

- [ ] **Step 2: Add the `Substitute` variant to `ParamModifier`**

Add at the end of the existing `ParamModifier` variants (in `src/lexer.rs`
around line 50-65):

```rust
    Substitute {
        pattern: Word,
        replacement: Word,
        anchor: SubstAnchor,
        all: bool,
    },
```

- [ ] **Step 3: Build the project to flush exhaustiveness errors**

Run: `cargo build 2>&1 | tail -30`
Expected: at least one error like
`error[E0004]: non-exhaustive patterns: \`&ParamModifier::Substitute { .. }\` not covered`
in `src/param_expansion.rs::expand_modifier`. This is the trail head for
Task 2's todo. Note the file + line for later.

(No exhaustiveness panic should fire at runtime yet — the variant is
unreachable until the lexer emits it.)

- [ ] **Step 4: Add a temporary placeholder evaluator arm so the build passes**

In `src/param_expansion.rs::expand_modifier`, immediately before the
closing `}` of the `match modifier { ... }` block, add:

```rust
        ParamModifier::Substitute { .. } => {
            // Filled in by Task 4.
            ExpansionResult::Value(shell.get(name).unwrap_or("").to_string())
        }
```

Re-run: `cargo build 2>&1 | tail -5`
Expected: clean build, 0 errors, 0 warnings.

- [ ] **Step 5: Commit**

```bash
git add src/lexer.rs src/param_expansion.rs
git commit -m "ast: add ParamModifier::Substitute + SubstAnchor scaffold (v32 task 1)"
```

---

### Task 2: Lexer — operand scan + `/` arm

**Files:**
- Modify: `src/lexer.rs` (new helper `scan_substitution_operand` near `modifier_with_operand` around line 1139; new `Some('/')` arm in `parse_braced_param` around line 1111)

**Note for implementer:** `scan_braced_operand` returns the raw string
body up to (but not consuming) the closing `}`. You need a different
scanner here because you must split on the first **unescaped** `/`. The
escape rules: `\/` → literal `/`, `\\` → literal `\`, any other `\x`
passes through as `\x` (so the existing `parse_braced_operand` can
handle `\$` etc. unchanged).

- [ ] **Step 1: Write the failing test for the operand scanner**

Add to `src/lexer.rs` tests module (after the existing
`brace_remove_suffix_*` tests around line 2714):

```rust
#[test]
fn brace_substitute_first_match() {
    let mut t = tokenize_words("\"${name/foo/bar}\"").unwrap();
    let part = single_param_expansion(&mut t);
    match part {
        WordPart::ParamExpansion { name, modifier, quoted } => {
            assert_eq!(name, "name");
            assert!(quoted);
            match modifier {
                ParamModifier::Substitute { pattern, replacement, anchor, all } => {
                    assert_eq!(word_to_literal(&pattern), "foo");
                    assert_eq!(word_to_literal(&replacement), "bar");
                    assert_eq!(anchor, SubstAnchor::None);
                    assert!(!all);
                }
                _ => panic!("expected Substitute"),
            }
        }
        _ => panic!("expected ParamExpansion"),
    }
}

#[test]
fn brace_substitute_all_matches() {
    let mut t = tokenize_words("${name//foo/bar}").unwrap();
    let part = single_param_expansion(&mut t);
    if let WordPart::ParamExpansion { modifier: ParamModifier::Substitute { all, anchor, .. }, .. } = part {
        assert!(all);
        assert_eq!(anchor, SubstAnchor::None);
    } else { panic!("expected Substitute") }
}

#[test]
fn brace_substitute_anchored_prefix() {
    let mut t = tokenize_words("${name/#foo/bar}").unwrap();
    let part = single_param_expansion(&mut t);
    if let WordPart::ParamExpansion { modifier: ParamModifier::Substitute { anchor, all, .. }, .. } = part {
        assert_eq!(anchor, SubstAnchor::Prefix);
        assert!(!all);
    } else { panic!("expected Substitute") }
}

#[test]
fn brace_substitute_anchored_suffix() {
    let mut t = tokenize_words("${name/%foo/bar}").unwrap();
    let part = single_param_expansion(&mut t);
    if let WordPart::ParamExpansion { modifier: ParamModifier::Substitute { anchor, all, .. }, .. } = part {
        assert_eq!(anchor, SubstAnchor::Suffix);
        assert!(!all);
    } else { panic!("expected Substitute") }
}

#[test]
fn brace_substitute_missing_replacement_is_empty_word() {
    let mut t = tokenize_words("${name/foo}").unwrap();
    let part = single_param_expansion(&mut t);
    if let WordPart::ParamExpansion { modifier: ParamModifier::Substitute { pattern, replacement, .. }, .. } = part {
        assert_eq!(word_to_literal(&pattern), "foo");
        assert_eq!(word_to_literal(&replacement), "");
    } else { panic!("expected Substitute") }
}

#[test]
fn brace_substitute_escaped_slash_in_pattern() {
    let mut t = tokenize_words("${path//\\//-}").unwrap();
    let part = single_param_expansion(&mut t);
    if let WordPart::ParamExpansion { modifier: ParamModifier::Substitute { pattern, replacement, all, .. }, .. } = part {
        assert_eq!(word_to_literal(&pattern), "/");
        assert_eq!(word_to_literal(&replacement), "-");
        assert!(all);
    } else { panic!("expected Substitute") }
}

#[test]
fn brace_substitute_unterminated_is_error() {
    assert!(matches!(
        tokenize_words("${name/foo/bar"),
        Err(LexError::UnterminatedBrace)
    ));
}
```

The helpers `single_param_expansion` and `word_to_literal` may not exist
yet. Check the bottom of the existing tests module for similar helpers
(`fn single_param_modifier(...)`, etc., around line 2640). If you don't
find suitable helpers, define them at the top of the tests module:

```rust
fn single_param_expansion(tokens: &mut Vec<Token>) -> WordPart {
    let word = match tokens.remove(0) {
        Token::Word(w) => w,
        other => panic!("expected Word, got {:?}", other),
    };
    word.0.into_iter().next().expect("non-empty word")
}

fn word_to_literal(w: &Word) -> String {
    let mut s = String::new();
    for p in &w.0 {
        if let WordPart::Literal { text, .. } = p {
            s.push_str(text);
        }
    }
    s
}
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cargo test --lib brace_substitute_ 2>&1 | tail -20`
Expected: all 7 new tests fail (most with "expected ParamExpansion" or
the lexer returning `Err(LexError::InvalidBraceModifier("/"))`).

- [ ] **Step 3: Implement `scan_substitution_operand`**

Add this helper to `src/lexer.rs` just below `modifier_with_operand`
(around line 1149):

```rust
/// Walks the chars iterator from just after the leading `/` of a
/// substitution operand. Splits pattern from replacement on the first
/// unescaped `/`. `\/` becomes a literal `/` in the pattern half; `\\`
/// becomes a literal `\`; any other `\x` passes through unchanged so the
/// existing operand parser sees it. Consumes through the closing `}`.
fn scan_substitution_operand(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<(Word, Word), LexError> {
    let mut pattern_src = String::new();
    let mut replacement_src = String::new();
    let mut in_replacement = false;
    let mut found_close = false;
    while let Some(c) = chars.next() {
        match c {
            '}' => { found_close = true; break; }
            '\\' => {
                match chars.peek().copied() {
                    Some('/') => {
                        chars.next();
                        let dst = if in_replacement { &mut replacement_src } else { &mut pattern_src };
                        dst.push('/');
                    }
                    Some('\\') => {
                        chars.next();
                        let dst = if in_replacement { &mut replacement_src } else { &mut pattern_src };
                        dst.push('\\');
                    }
                    _ => {
                        let dst = if in_replacement { &mut replacement_src } else { &mut pattern_src };
                        dst.push('\\');
                    }
                }
            }
            '/' if !in_replacement => {
                in_replacement = true;
            }
            _ => {
                let dst = if in_replacement { &mut replacement_src } else { &mut pattern_src };
                dst.push(c);
            }
        }
    }
    if !found_close {
        return Err(LexError::UnterminatedBrace);
    }
    let pattern = parse_braced_operand(&pattern_src)?;
    let replacement = parse_braced_operand(&replacement_src)?;
    Ok((pattern, replacement))
}
```

- [ ] **Step 4: Add the `Some('/')` arm to `parse_braced_param`**

In `src/lexer.rs::parse_braced_param`, immediately above the
`Some(c) => Err(LexError::InvalidBraceModifier(c.to_string()))` fallback
(around line 1112):

```rust
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
```

- [ ] **Step 5: Run the lexer tests to verify they pass**

Run: `cargo test --lib brace_substitute_ 2>&1 | tail -20`
Expected: all 7 tests pass.

- [ ] **Step 6: Run the entire lexer test suite to confirm no regression**

Run: `cargo test --lib lexer::tests:: 2>&1 | tail -5`
Expected: 0 failures.

- [ ] **Step 7: Commit**

```bash
git add src/lexer.rs
git commit -m "lex: ${var/pat/repl} operand scan + parse_braced_param arm (v32 task 2)"
```

---

### Task 3: Evaluator — `substitute()` helper + unit tests

**Files:**
- Modify: `src/param_expansion.rs` (new helper `substitute()` near `remove_prefix`/`remove_suffix`; unit tests at the bottom of the tests module)

**Note for implementer:** Read `src/param_expansion.rs:93-149` first to
mirror the `remove_prefix` / `remove_suffix` boundary-scan idiom exactly.
You will not use `glob::Pattern::matches` (which expects the whole
string) — only `matches_with(&value[start..end], opts)` against slices,
same as the existing helpers.

- [ ] **Step 1: Write the failing unit tests**

Append to the `tests` module in `src/param_expansion.rs` (just before the
final closing `}`):

```rust
#[test]
fn substitute_first_match_unanchored() {
    assert_eq!(substitute("foobar", "o", "X", SubstAnchor::None, false), "fXobar");
}

#[test]
fn substitute_all_unanchored() {
    assert_eq!(substitute("foobar", "o", "X", SubstAnchor::None, true), "fXXbar");
}

#[test]
fn substitute_first_unanchored_no_match_returns_value() {
    assert_eq!(substitute("foobar", "z", "X", SubstAnchor::None, false), "foobar");
}

#[test]
fn substitute_all_with_empty_replacement_removes() {
    assert_eq!(substitute("aaa", "a", "", SubstAnchor::None, true), "");
}

#[test]
fn substitute_anchored_prefix_hit() {
    assert_eq!(substitute("hello", "he", "HI", SubstAnchor::Prefix, false), "HIllo");
}

#[test]
fn substitute_anchored_prefix_miss() {
    assert_eq!(substitute("hello", "xo", "HI", SubstAnchor::Prefix, false), "hello");
}

#[test]
fn substitute_anchored_suffix_hit() {
    assert_eq!(substitute("hello", "lo", "LO", SubstAnchor::Suffix, false), "helLO");
}

#[test]
fn substitute_anchored_suffix_miss() {
    assert_eq!(substitute("hello", "xo", "LO", SubstAnchor::Suffix, false), "hello");
}

#[test]
fn substitute_glob_star_longest_match() {
    // `*` matches the whole tail at i=0; with all=true, the second pass
    // starts past the replacement and finds nothing more.
    assert_eq!(substitute("xyz", "*", "Q", SubstAnchor::None, true), "Q");
}

#[test]
fn substitute_glob_question_mark() {
    assert_eq!(substitute("abc", "?", "X", SubstAnchor::None, false), "Xbc");
    assert_eq!(substitute("abc", "?", "X", SubstAnchor::None, true), "XXX");
}

#[test]
fn substitute_unicode_boundaries() {
    assert_eq!(substitute("café", "é", "E", SubstAnchor::None, false), "cafE");
}

#[test]
fn substitute_invalid_glob_returns_value_unchanged() {
    assert_eq!(substitute("hello", "[abc", "X", SubstAnchor::None, false), "hello");
}

#[test]
fn substitute_empty_value_returns_empty() {
    assert_eq!(substitute("", "foo", "bar", SubstAnchor::None, true), "");
}

#[test]
fn substitute_empty_pattern_matches_empty_at_start_only_with_first() {
    // Empty pattern matches the empty prefix at i=0; with all=false we
    // replace once at i=0 then stop.
    assert_eq!(substitute("abc", "", "X", SubstAnchor::None, false), "Xabc");
}

#[test]
fn substitute_empty_pattern_all_uses_guard_to_avoid_infinite_loop() {
    // Empty pattern matches empty at every position; the guard advances
    // one char each time. Result: replacement inserted before each char
    // and once at the end.
    assert_eq!(substitute("abc", "", "X", SubstAnchor::None, true), "XaXbXcX");
}
```

Note: `SubstAnchor` is in the parent `crate::lexer` module; either bring
it into scope at the top of the tests module (`use crate::lexer::SubstAnchor;`)
or qualify each use. Pick whichever matches the file's existing
import style.

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --lib param_expansion::tests::substitute_ 2>&1 | tail -20`
Expected: all 15 tests fail with "cannot find function `substitute`" or
similar.

- [ ] **Step 3: Implement `substitute()`**

Add this helper to `src/param_expansion.rs` between `remove_suffix` (ends
~line 149) and the `#[cfg(test)] mod tests { ... }` block. Don't forget
to bring `SubstAnchor` into scope at the top of the file:

```rust
use crate::lexer::{ParamModifier, SubstAnchor, Word};
```

(Update the existing `use crate::lexer::{ParamModifier, Word};` line.)

```rust
fn substitute(
    value: &str,
    pattern: &str,
    replacement: &str,
    anchor: SubstAnchor,
    all: bool,
) -> String {
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

    // Longest match at `start`: largest `end` (from boundaries) > start
    // such that value[start..end] matches. Returns None if no end works.
    // For empty-pattern callers this can return Some(start) (empty match).
    let longest_match_at = |start: usize| -> Option<usize> {
        for &end in boundaries.iter().rev() {
            if end < start { continue; }
            if pat.matches_with(&value[start..end], opts) {
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
                if pat.matches_with(&value[start..], opts) {
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
                    // Copy gap from cursor to start verbatim.
                    out.push_str(&value[cursor..start]);
                    out.push_str(replacement);
                    if end == start {
                        // Empty match: advance one char to avoid loops.
                        // Push the char at `start` (if any) so it isn't lost.
                        if start < value.len() {
                            // Find the next boundary after `start`.
                            let next = boundaries.iter().copied().find(|&b| b > start).unwrap_or(value.len());
                            out.push_str(&value[start..next]);
                            cursor = next;
                        } else {
                            cursor = start;
                        }
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
```

- [ ] **Step 4: Run the unit tests to verify pass**

Run: `cargo test --lib param_expansion::tests::substitute_ 2>&1 | tail -20`
Expected: all 15 tests pass.

- [ ] **Step 5: Run the entire param_expansion test module to confirm no regression**

Run: `cargo test --lib param_expansion 2>&1 | tail -5`
Expected: 0 failures.

- [ ] **Step 6: Commit**

```bash
git add src/param_expansion.rs
git commit -m "param: substitute() helper for ${var/pat/repl} (v32 task 3)"
```

---

### Task 4: Evaluator — wire `Substitute` arm into `expand_modifier`

**Files:**
- Modify: `src/param_expansion.rs` (replace the placeholder `Substitute { .. }` arm added in Task 1 with the real call into `substitute()`)

- [ ] **Step 1: Write a failing unit test that goes through `expand_modifier`**

Append to the tests module in `src/param_expansion.rs`:

```rust
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
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --lib expand_modifier_substitute_ 2>&1 | tail -20`
Expected: all 4 tests fail — the placeholder arm from Task 1 returns the
raw value unchanged, so all assertions fail.

- [ ] **Step 3: Replace the placeholder with the real arm**

In `src/param_expansion.rs::expand_modifier`, find the placeholder arm
added in Task 1 step 4 and replace it with:

```rust
        ParamModifier::Substitute { pattern, replacement, anchor, all } => {
            let v = shell.get(name).unwrap_or("").to_string();
            let pat = expand_word_to_string(pattern, shell);
            let rep = expand_word_to_string(replacement, shell);
            ExpansionResult::Value(substitute(&v, &pat, &rep, *anchor, *all))
        }
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --lib expand_modifier_substitute_ 2>&1 | tail -10`
Expected: all 4 tests pass.

- [ ] **Step 5: Run full lib test suite for regression check**

Run: `cargo test --lib 2>&1 | grep -E "test result:" | tail -5`
Expected: no failures.

- [ ] **Step 6: Commit**

```bash
git add src/param_expansion.rs
git commit -m "param: wire Substitute arm into expand_modifier (v32 task 4)"
```

---

### Task 5: Integration tests via the binary

**Files:**
- Create: `tests/param_substitution_integration.rs`

**Note for implementer:** Mirror the `run` / `run_with_status` helper
shape used in `tests/multiline_integration.rs:1-34`. Pipe scripts on
stdin and assert on stdout lines.

- [ ] **Step 1: Create the test file with end-to-end coverage**

Create `tests/param_substitution_integration.rs` with:

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
fn subst_first_match() {
    let (out, _) = run("name=foobar\necho ${name/o/X}\nexit\n");
    assert!(out.lines().any(|l| l == "fXobar"), "stdout: {out}");
}

#[test]
fn subst_all_matches() {
    let (out, _) = run("name=foobar\necho ${name//o/X}\nexit\n");
    assert!(out.lines().any(|l| l == "fXXbar"), "stdout: {out}");
}

#[test]
fn subst_missing_replacement_removes() {
    let (out, _) = run("name=foobar\necho ${name/o}\nexit\n");
    assert!(out.lines().any(|l| l == "fobar"), "stdout: {out}");
}

#[test]
fn subst_all_with_empty_replacement_removes_all() {
    let (out, _) = run("name=aaa\necho \"[${name//a}]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[]"), "stdout: {out}");
}

#[test]
fn subst_anchored_prefix_hit() {
    let (out, _) = run("name=hello\necho ${name/#he/HI}\nexit\n");
    assert!(out.lines().any(|l| l == "HIllo"), "stdout: {out}");
}

#[test]
fn subst_anchored_prefix_miss_leaves_value() {
    let (out, _) = run("name=hello\necho ${name/#xo/HI}\nexit\n");
    assert!(out.lines().any(|l| l == "hello"), "stdout: {out}");
}

#[test]
fn subst_anchored_suffix_hit() {
    let (out, _) = run("name=hello\necho ${name/%lo/LO}\nexit\n");
    assert!(out.lines().any(|l| l == "helLO"), "stdout: {out}");
}

#[test]
fn subst_escaped_slash_in_pattern() {
    let (out, _) = run("path=a/b/c\necho ${path//\\//-}\nexit\n");
    assert!(out.lines().any(|l| l == "a-b-c"), "stdout: {out}");
}

#[test]
fn subst_inside_double_quotes_single_field() {
    let (out, _) = run("name=\"foo bar\"\necho \"[${name/ /_}]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[foo_bar]"), "stdout: {out}");
}

#[test]
fn subst_glob_star_replaces_longest_match() {
    let (out, _) = run("name=xyz\necho ${name//*/Q}\nexit\n");
    // `*` matches the entire string at i=0; replacement runs once.
    assert!(out.lines().any(|l| l == "Q"), "stdout: {out}");
}

#[test]
fn subst_unset_var_is_empty() {
    let (out, _) = run("echo \"[${MISSING/foo/bar}]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[]"), "stdout: {out}");
}

#[test]
fn subst_pattern_expansion_uses_other_var() {
    let (out, _) = run("name=foobar\np=o\necho ${name//$p/X}\nexit\n");
    assert!(out.lines().any(|l| l == "fXXbar"), "stdout: {out}");
}

#[test]
fn subst_unicode_safe() {
    let (out, _) = run("name=café\necho ${name/é/E}\nexit\n");
    assert!(out.lines().any(|l| l == "cafE"), "stdout: {out}");
}

#[test]
fn subst_in_pipeline_stage() {
    let (out, _) = run("name=foo.txt\necho ${name/.txt/.md} | cat\nexit\n");
    assert!(out.lines().any(|l| l == "foo.md"), "stdout: {out}");
}
```

- [ ] **Step 2: Run the new integration tests**

Run: `cargo test --test param_substitution_integration 2>&1 | tail -25`
Expected: all 14 tests pass. (If any fail, debug — most likely culprits:
operand-escape rules in `scan_substitution_operand`, or a quoting
mismatch between the test script and the huck lexer's double-quote
handling.)

- [ ] **Step 3: Commit**

```bash
git add tests/param_substitution_integration.rs
git commit -m "test: ${var/pat/repl} integration coverage (v32 task 5)"
```

---

### Task 6: Docs + version blurb

**Files:**
- Modify: `docs/bash-divergences.md` (mark M-15 fixed; add changelog row)
- Modify: `README.md` (bump current-version blurb to v32 if it lists one)

- [ ] **Step 1: Mark M-15 fixed in `docs/bash-divergences.md`**

Find the M-15 entry (around line 149 in the Tier 2 list). Replace:

```
- **M-15: `${var/pat/repl}` and `${var//pat/repl}`** — `[deferred]` high. huck: `InvalidBraceModifier("/")`. bash: substitution.
```

with:

```
- **M-15: `${var/pat/repl}` and `${var//pat/repl}`** — `[fixed v32]` high. All six forms: `/`, `//`, `/#`, `/%`, plus empty-repl shortcut. Glob pattern engine; `\/` escapes literal slash in pattern.
```

- [ ] **Step 2: Append changelog row in `docs/bash-divergences.md`**

At the bottom of the Changelog section (end of file), append:

```
- **2026-05-26**: M-15 (`${var/pat/repl}` pattern substitution) shipped as v32. All six bash forms supported: first-match `/`, all-matches `//`, anchored-prefix `/#`, anchored-suffix `/%`, plus empty-replacement shortcut. Glob pattern engine (same as `${var#pat}`); `\/` escapes literal slash inside the pattern half of the operand.
```

- [ ] **Step 3: Update the README version blurb (if one exists)**

Open `README.md`. If there is a "current version" / "latest iteration"
blurb mentioning v30 or v31, bump it to v32 and add a one-line
description ("v32: `${var/pat/repl}` parameter substitution"). If the
README's status table has a row for parameter-expansion forms, update
the cell for substitution from "missing" → "v32".

If no such blurb exists, skip this step (and note it in your handoff).

- [ ] **Step 4: Commit**

```bash
git add docs/bash-divergences.md README.md
git commit -m "docs: mark M-15 fixed; v32 in README"
```

---

### Task 7: Full-suite verification (no separate commit)

This is a verification gate, not a code change. The controller's
final-review pass will catch anything missed.

- [ ] **Step 1: Run the entire test suite**

Run: `cargo test 2>&1 | grep -E "test result:|FAILED" | tail -30`
Expected: every line shows `ok`, no `FAILED`. Total test count should be
1195 (baseline after v31) + ~36 new tests (7 lexer + 15 unit + 4
expand_modifier + 14 integration) = ~1231 tests, all passing.

- [ ] **Step 2: Run clippy with the project's `-D warnings` policy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -10`
Expected: clean, no errors, no warnings.

- [ ] **Step 3: If either fails, report BLOCKED with the failure output**

If everything passes, report DONE.

---

## Final review handoff

After Task 7 reports DONE, the controller will dispatch the
`feature-dev:code-reviewer` (opus) agent for a cross-cutting review of
the full v32 branch diff against `main`. The reviewer looks for:

- AST exhaustiveness in every `match modifier` site in the codebase
  (grep for `ParamModifier::` to enumerate; common miss-sites:
  serialization, debug formatters, history-redisplay code).
- Empty-match guard correctness (especially `${var//}` and pattern-of-`*`
  combinations).
- UTF-8 boundary safety on suffix-anchored matches with multi-byte tails.
- Iterator-drain risk in `scan_substitution_operand` (the
  `chars: &mut Peekable<Chars>` is shared with the outer
  `parse_braced_param` — confirm we always consume through `}` on the
  success path).
- Lexer test coverage parity with the existing `${var#pat}` /
  `${var%pat}` modifiers.

If the reviewer flags real issues, address them in a follow-up commit
(do NOT rewind / amend prior commits) and re-run the verification gate
before merging.

## Merge

After final review passes:

```bash
git checkout main
git merge --ff-only v32-pattern-substitution
git branch -d v32-pattern-substitution
```

Update memory at
`/home/john/.claude/projects/-home-john-projects-shuck/memory/project_huck_iterations.md`
with a new "Done at v32" stanza mirroring the v31 entry's structure.
