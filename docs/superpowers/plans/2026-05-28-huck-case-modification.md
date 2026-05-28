# huck v37 — `${var^^}` / `${var,,}` Case Modification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close M-17 by implementing bash's case-modification parameter
expansion in all eight forms (`^^` / `^` / `,,` / `,` × bare / with
pattern). Closes the parameter-expansion-modifier cluster that v32
(substitute), v33 (substring), v34 (length-of-positional + fatal PE)
opened.

**Architecture:** Pure expansion-layer feature. New
`ParamModifier::Case { direction, all, pattern }` variant plus a new
`CaseDirection { Upper, Lower }` enum. Two new lexer arms in
`dispatch_braced_modifier` (one for `^`, one for `,`) follow the same
shape as the existing `#`/`%` arms — peek for the doubled form, then
scan an optional operand. A new `scan_optional_braced_operand` helper
reuses v32's `scan_braced_operand` + `parse_braced_operand` machinery
and returns `Option<Word>` (`None` when the body is empty). The
evaluator gets a pure-function `case_modify()` helper that applies
Rust's Unicode-aware `char::to_uppercase` / `char::to_lowercase`
iterators with optional per-character glob filtering via the existing
`glob::Pattern` crate.

**Tech Stack:** Rust. Reuses `glob` crate (already a dep) and stdlib
Unicode case mapping. No new external deps.

**Spec:** `docs/superpowers/specs/2026-05-28-huck-case-modification-design.md`

**Branch:** `v37-case-modification` (already created and checked out).

---

### Task 1: AST scaffold for `Case` + placeholder evaluator arm

**Files:**
- Modify: `src/lexer.rs` (`ParamModifier` enum at `src/lexer.rs:57-75`; new `CaseDirection` enum alongside `SubstAnchor` at `src/lexer.rs:50-54`)
- Modify: `src/param_expansion.rs` (`expand_modifier` match block; placeholder arm)

**Note for implementer:** Read `src/lexer.rs:50-75` first to mirror the existing `SubstAnchor` + `ParamModifier::Substitute` shape exactly. The `Case` variant uses the same conventions as `Substitute`: a small `Copy` enum for the discriminator (here `CaseDirection`), a `bool` for the all/first flag, and an `Option<Word>` for the optional pattern.

- [ ] **Step 1: Add the `CaseDirection` enum next to `SubstAnchor`**

In `src/lexer.rs`, immediately above the `ParamModifier` enum (around line 55 — between the closing `}` of `SubstAnchor` at line 54 and the `#[derive(...)]` of `ParamModifier` at line 56), insert:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseDirection {
    Upper,  // ^ / ^^
    Lower,  // , / ,,
}
```

- [ ] **Step 2: Add the `Case` variant to `ParamModifier`**

At the end of the existing `ParamModifier` variants (after `Substring { offset, length }` at line 71-74), add:

```rust
    Case {
        direction: CaseDirection,
        all: bool,
        pattern: Option<Word>,
    },
```

- [ ] **Step 3: Build the project to flush exhaustiveness errors**

Run: `cargo build 2>&1 | tail -30`

Expected: at least one `error[E0004]: non-exhaustive patterns: \`&ParamModifier::Case { .. }\` not covered` in `src/param_expansion.rs::expand_modifier`. Note the file + line for Step 4.

(No exhaustiveness panic should fire at runtime yet — the variant is unreachable until the lexer emits it.)

- [ ] **Step 4: Add a temporary placeholder evaluator arm so the build passes**

In `src/param_expansion.rs::expand_modifier`, immediately before the closing `}` of the `match modifier { ... }` block, add:

```rust
        ParamModifier::Case { .. } => {
            // Filled in by Task 4.
            ExpansionResult::Value(shell.lookup_var(name).unwrap_or_default())
        }
```

Re-run: `cargo build 2>&1 | tail -5`

Expected: clean build, 0 errors. (A dead-code warning on `CaseDirection` and the `Case` variant's fields is acceptable — they go away in Task 4. If clippy with `-D warnings` complains, add `#[allow(dead_code)]` to the new enum and to the unused fields.)

- [ ] **Step 5: Run full clippy with the project's policy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -10`

Expected: clean. If clippy reports dead-code on `CaseDirection::Upper` / `CaseDirection::Lower` (binary crates DO get the lint on `pub` items if they're never constructed), add `#[allow(dead_code)]` ABOVE the enum:

```rust
#[allow(dead_code)]  // removed when lexer emits Case in Task 2
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseDirection {
    Upper,
    Lower,
}
```

Same treatment for the `Case` variant's fields if needed (annotate the variant or the unused fields).

- [ ] **Step 6: Run the full test suite to confirm no regression**

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | grep -E "failed: [1-9]"`

Expected: no output (no failures). Baseline ~1397 tests.

- [ ] **Step 7: Commit**

```bash
git add src/lexer.rs src/param_expansion.rs
git commit -m "$(cat <<'EOF'
ast: add ParamModifier::Case + CaseDirection scaffold (v37 task 1)

Adds the Case variant alongside Substitute / Substring in
ParamModifier, plus a new CaseDirection enum (Upper / Lower).
Placeholder arm in expand_modifier returns the raw var value;
real implementation lands in Task 4. Compile-clean baseline.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Lexer — `^`/`,` arms + `scan_optional_braced_operand` helper

**Files:**
- Modify: `src/lexer.rs` (new helper `scan_optional_braced_operand` near `modifier_with_operand`; new `Some('^')` and `Some(',')` arms in `dispatch_braced_modifier` around line 1198-1213 — slotted after the existing `Some('/')` arm)

**Note for implementer:** Read `src/lexer.rs:1184-1213` first to mirror the existing `Some('#')` and `Some('/')` arms. The new arms have the same shape as `#`: peek for the doubled form (`^^` vs `^`), then read an optional operand.

`scan_braced_operand` returns the raw string body up to (but not consuming — wait, see verify-it note below) the closing `}`. Use `scan_optional_braced_operand` which wraps it and returns `Option<Word>` based on whether the body is empty.

VERIFY at implementation time whether `scan_braced_operand` returns the body INCLUDING or EXCLUDING the closing `}`, and whether it consumes the `}` from the input iterator. The v32 `scan_substitution_operand` and v33 `scan_substring_operands` both call `scan_braced_operand` directly and don't have to do any extra `}` consumption afterward — so `scan_braced_operand` already consumes the closing `}`. Follow that pattern.

- [ ] **Step 1: Write the failing lexer tests**

Append to `src/lexer.rs` tests module (search for `brace_substring_simple` to find a nearby anchor; place the new tests after the existing `brace_substring_*` group):

```rust
    #[test]
    fn brace_case_upper_all() {
        let mut t = tokenize_words("${name^^}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { name, modifier: ParamModifier::Case { direction, all, pattern }, quoted } = part {
            assert_eq!(name, "name");
            assert!(!quoted);
            assert_eq!(direction, CaseDirection::Upper);
            assert!(all);
            assert!(pattern.is_none());
        } else { panic!("expected Case, got {part:?}") }
    }

    #[test]
    fn brace_case_upper_first() {
        let mut t = tokenize_words("${name^}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Case { direction, all, pattern }, .. } = part {
            assert_eq!(direction, CaseDirection::Upper);
            assert!(!all);
            assert!(pattern.is_none());
        } else { panic!("expected Case") }
    }

    #[test]
    fn brace_case_lower_all() {
        let mut t = tokenize_words("${name,,}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Case { direction, all, pattern }, .. } = part {
            assert_eq!(direction, CaseDirection::Lower);
            assert!(all);
            assert!(pattern.is_none());
        } else { panic!("expected Case") }
    }

    #[test]
    fn brace_case_lower_first() {
        let mut t = tokenize_words("${name,}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Case { direction, all, pattern }, .. } = part {
            assert_eq!(direction, CaseDirection::Lower);
            assert!(!all);
            assert!(pattern.is_none());
        } else { panic!("expected Case") }
    }

    #[test]
    fn brace_case_upper_all_with_pattern() {
        let mut t = tokenize_words("${name^^[aeiou]}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Case { direction, all, pattern }, .. } = part {
            assert_eq!(direction, CaseDirection::Upper);
            assert!(all);
            let p = pattern.expect("pattern");
            assert_eq!(word_to_literal(&p), "[aeiou]");
        } else { panic!("expected Case") }
    }

    #[test]
    fn brace_case_positional() {
        // `${1^^}` — emits ParamExpansion (not Var) so the modifier runs.
        let mut t = tokenize_words("${1^^}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { name, modifier: ParamModifier::Case { all, .. }, .. } = part {
            assert_eq!(name, "1");
            assert!(all);
        } else { panic!("expected Case on positional, got {part:?}") }
    }

    #[test]
    fn brace_case_unterminated_is_error() {
        assert!(matches!(
            tokenize_words("${name^^"),
            Err(LexError::UnterminatedBrace)
        ));
    }
```

The helpers `single_param_expansion` and `word_to_literal` already exist in this module from v32 (around line 1530-1540 — search for `fn single_param_expansion`).

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cargo test --lib brace_case_ 2>&1 | tail -20`

Expected: all 7 tests fail with `LexError::InvalidBraceModifier("^")` or `InvalidBraceModifier(",")` (these chars currently fall through to the `Some(c) => Err(...)` fallback at `src/lexer.rs:1214`).

- [ ] **Step 3: Implement `scan_optional_braced_operand`**

In `src/lexer.rs`, add this helper near the existing `modifier_with_operand` (search for `fn modifier_with_operand` — should be around line 1230 or so). Add it as a new function in the same area:

```rust
/// Scans a single optional operand inside a `${name<mod>OPERAND}` form.
/// Returns `None` if the operand body is empty (i.e. the modifier is
/// immediately followed by `}`), or `Some(Word)` for a non-empty body.
/// Delegates to `scan_braced_operand` (depth + quote aware) so nested
/// `${...}` constructs in the operand are handled correctly.
fn scan_optional_braced_operand(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<Option<Word>, LexError> {
    let body = scan_braced_operand(chars)?;
    if body.is_empty() {
        Ok(None)
    } else {
        Ok(Some(parse_braced_operand(&body)?))
    }
}
```

- [ ] **Step 4: Add the `Some('^')` arm to `dispatch_braced_modifier`**

In `src/lexer.rs::dispatch_braced_modifier`, find the `Some('/')` arm (around line 1198). After its closing `}` (and BEFORE the `Some(c) => Err(...)` fallback at line 1214), add:

```rust
        Some('^') => {
            let all = chars.peek() == Some(&'^');
            if all { chars.next(); }
            let pattern = scan_optional_braced_operand(chars)?;
            parts.push(WordPart::ParamExpansion {
                name,
                modifier: ParamModifier::Case { direction: CaseDirection::Upper, all, pattern },
                quoted,
            });
            Ok(())
        }
        Some(',') => {
            let all = chars.peek() == Some(&',');
            if all { chars.next(); }
            let pattern = scan_optional_braced_operand(chars)?;
            parts.push(WordPart::ParamExpansion {
                name,
                modifier: ParamModifier::Case { direction: CaseDirection::Lower, all, pattern },
                quoted,
            });
            Ok(())
        }
```

If you added `#[allow(dead_code)]` annotations in Task 1 (Step 5) to silence dead-code warnings on `CaseDirection::Upper` / `CaseDirection::Lower` and on the `Case` variant fields, REMOVE those annotations now — the variants are constructed by the lexer and the fields will be read in Task 4.

- [ ] **Step 5: Run the lexer tests to verify they pass**

Run: `cargo test --lib brace_case_ 2>&1 | tail -20`

Expected: all 7 tests pass.

- [ ] **Step 6: Run the entire lexer test suite to confirm no regression**

Run: `cargo test --lib lexer::tests:: 2>&1 | tail -5`

Expected: 0 failures.

- [ ] **Step 7: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`

Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add src/lexer.rs
git commit -m "$(cat <<'EOF'
lex: ${var^^} / ${var,,} dispatch + scan_optional_braced_operand (v37 task 2)

Two new arms in dispatch_braced_modifier mirror the existing #/% shape
(peek for doubled form, then read optional operand). New helper
scan_optional_braced_operand wraps scan_braced_operand and returns
Option<Word> — None for empty body (`${var^^}` with no pattern),
Some for non-empty (`${var^^[aeiou]}`). 7 lexer unit tests cover all
four bare forms + the with-pattern form + positional names +
unterminated brace.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Evaluator — `case_modify()` helper + unit tests

**Files:**
- Modify: `src/param_expansion.rs` (new helper `case_modify()` near `substitute()`; unit tests at the bottom of the tests module)

**Note for implementer:** Read `src/param_expansion.rs:211+` first to mirror `substitute()`'s glob compile-failure idiom (`glob::Pattern::new(...)` returns `Err` → return value unchanged). The new helper is pure (no `Shell` access) — takes `&str` + `CaseDirection` + `bool` + `Option<&str>`, returns `String`.

Also: `expand_word_to_string` lives at `src/param_expansion.rs:149` and is `pub(crate)` — Task 4 will use it to convert the optional pattern Word to a string before passing to `case_modify`.

- [ ] **Step 1: Write the failing unit tests**

Append to the `tests` module in `src/param_expansion.rs` (just before the final closing `}` of the module). Bring `CaseDirection` into scope at the top of the tests module if needed (the existing `use crate::lexer::{ParamModifier, SubstAnchor, Word};` line — extend it):

```rust
use crate::lexer::{CaseDirection, ParamModifier, SubstAnchor, Word};
```

(If the tests module has its own `use` lines, add `CaseDirection` to the import.)

Now add the unit tests:

```rust
    #[test]
    fn case_modify_upper_all_no_pattern() {
        assert_eq!(case_modify("hello", CaseDirection::Upper, true, None), "HELLO");
    }

    #[test]
    fn case_modify_upper_first_no_pattern() {
        assert_eq!(case_modify("hello", CaseDirection::Upper, false, None), "Hello");
    }

    #[test]
    fn case_modify_lower_all_no_pattern() {
        assert_eq!(case_modify("HELLO", CaseDirection::Lower, true, None), "hello");
    }

    #[test]
    fn case_modify_lower_first_no_pattern() {
        assert_eq!(case_modify("HELLO", CaseDirection::Lower, false, None), "hELLO");
    }

    #[test]
    fn case_modify_upper_all_with_pattern_filters_chars() {
        // [aeiou] — only vowels upper-cased.
        assert_eq!(case_modify("hello", CaseDirection::Upper, true, Some("[aeiou]")), "hEllO");
    }

    #[test]
    fn case_modify_upper_first_with_pattern_picks_first_match() {
        // Only the first MATCHING char (the `e`) gets upper-cased.
        assert_eq!(case_modify("hello", CaseDirection::Upper, false, Some("[aeiou]")), "hEllo");
    }

    #[test]
    fn case_modify_unicode_handles_multichar_uppercase() {
        // Rust's `'ß'.to_uppercase()` yields two chars: 'S', 'S'.
        assert_eq!(case_modify("straße", CaseDirection::Upper, true, None), "STRASSE");
    }

    #[test]
    fn case_modify_empty_value_returns_empty() {
        assert_eq!(case_modify("", CaseDirection::Upper, true, None), "");
    }

    #[test]
    fn case_modify_invalid_glob_returns_value_unchanged() {
        // `[abc` (unclosed bracket) — glob::Pattern::new returns Err.
        assert_eq!(case_modify("hello", CaseDirection::Upper, true, Some("[abc")), "hello");
    }

    #[test]
    fn case_modify_no_match_first_form_returns_unchanged() {
        // No char in "hello" matches [xyz]; all=false → return unchanged.
        assert_eq!(case_modify("hello", CaseDirection::Upper, false, Some("[xyz]")), "hello");
    }
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --lib param_expansion::tests::case_modify_ 2>&1 | tail -20`

Expected: all 10 tests fail with "cannot find function `case_modify`" or similar.

- [ ] **Step 3: Implement `case_modify()`**

Add this helper to `src/param_expansion.rs` between the existing `substitute()` function (ends around line 280-290) and the `#[cfg(test)] mod tests { ... }` block. Don't forget to bring `CaseDirection` into the file's main `use` line (search for `use crate::lexer::{ParamModifier, SubstAnchor, Word};` — extend it):

```rust
use crate::lexer::{CaseDirection, ParamModifier, SubstAnchor, Word};
```

Then add the function:

```rust
/// Applies bash-style case modification to `value`. The `direction`
/// (Upper/Lower) and `all` flag together determine whether every char
/// or only the first matching char gets converted. `pattern` filters
/// per-character — if `None`, every char matches; if `Some(p)`, only
/// chars matching the glob `p` get converted. Glob compile errors
/// return `value` unchanged (silent fallthrough, matches v32's
/// `substitute`). Unicode-aware via Rust's `char::to_uppercase` /
/// `char::to_lowercase` iterators — handles multi-char expansions
/// like `'ß'.to_uppercase()` → "SS".
fn case_modify(
    value: &str,
    direction: CaseDirection,
    all: bool,
    pattern: Option<&str>,
) -> String {
    // Compile the pattern, if any. On compile failure, return value
    // unchanged (matches v32 substitute's silent-no-op convention).
    let compiled = match pattern {
        Some(p) => match glob::Pattern::new(p) {
            Ok(pat) => Some(pat),
            Err(_) => return value.to_string(),
        },
        None => None,
    };
    let opts = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: false,
        require_literal_leading_dot: false,
    };

    let should_modify = |c: char| -> bool {
        match &compiled {
            None => true,
            Some(pat) => pat.matches_with(&c.to_string(), opts),
        }
    };

    let apply = |c: char| -> String {
        match direction {
            CaseDirection::Upper => c.to_uppercase().collect(),
            CaseDirection::Lower => c.to_lowercase().collect(),
        }
    };

    let mut out = String::with_capacity(value.len());
    if all {
        for c in value.chars() {
            if should_modify(c) {
                out.push_str(&apply(c));
            } else {
                out.push(c);
            }
        }
    } else {
        let mut done = false;
        for c in value.chars() {
            if !done && should_modify(c) {
                out.push_str(&apply(c));
                done = true;
            } else {
                out.push(c);
            }
        }
    }
    out
}
```

- [ ] **Step 4: Run the unit tests to verify they pass**

Run: `cargo test --lib param_expansion::tests::case_modify_ 2>&1 | tail -20`

Expected: all 10 tests pass.

- [ ] **Step 5: Run the entire param_expansion test module to confirm no regression**

Run: `cargo test --lib param_expansion 2>&1 | tail -5`

Expected: 0 failures.

- [ ] **Step 6: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`

Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/param_expansion.rs
git commit -m "$(cat <<'EOF'
param: case_modify() helper for ${var^^} / ${var,,} (v37 task 3)

Pure-function helper takes &str + CaseDirection + all flag + optional
glob pattern; returns modified String. Unicode-aware via Rust's
char::to_uppercase / char::to_lowercase iterators (handles `ß`→`SS`).
Glob compile failure returns value unchanged (silent fallthrough,
matches v32 substitute). First-form (`all=false`) skips chars until
the first match per the pattern, then leaves rest unchanged. 10 unit
tests.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Evaluator — wire `Case` arm into `expand_modifier`

**Files:**
- Modify: `src/param_expansion.rs` (replace the placeholder `Case { .. }` arm from Task 1 with the real call into `case_modify()`; unit tests at the bottom of the tests module)

**Note for implementer:** The arm pattern mirrors v32's `Substitute` arm (around line 78-83 in `expand_modifier`): pull the var via `lookup_var`, expand the pattern Word to a string (if present), call the helper.

- [ ] **Step 1: Write the failing through-the-arm tests**

Append to the `tests` module in `src/param_expansion.rs` (after the existing case_modify_* tests from Task 3):

```rust
    #[test]
    fn expand_modifier_case_upper_all_named_var() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_CASE1", "hello".to_string());
        let m = ParamModifier::Case {
            direction: CaseDirection::Upper,
            all: true,
            pattern: None,
        };
        let r = expand_modifier("HUCK_TEST_PE_CASE1", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("HELLO".to_string()));
    }

    #[test]
    fn expand_modifier_case_upper_positional_lookup() {
        // Verifies the arm uses lookup_var (so digit names resolve).
        let mut shell = Shell::new();
        shell.positional_args = vec!["hello".to_string()];
        let m = ParamModifier::Case {
            direction: CaseDirection::Upper,
            all: true,
            pattern: None,
        };
        let r = expand_modifier("1", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("HELLO".to_string()));
    }

    #[test]
    fn expand_modifier_case_unset_var_returns_empty() {
        let mut shell = Shell::new();
        let m = ParamModifier::Case {
            direction: CaseDirection::Upper,
            all: true,
            pattern: None,
        };
        let r = expand_modifier("HUCK_TEST_PE_CASE_UNSET", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("".to_string()));
    }
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --lib expand_modifier_case_ 2>&1 | tail -10`

Expected: the 3 tests fail because the placeholder arm just returns the raw var value without case-modifying.

- [ ] **Step 3: Replace the placeholder `Case` arm with the real impl**

In `src/param_expansion.rs::expand_modifier`, find the placeholder added in Task 1:

```rust
        ParamModifier::Case { .. } => {
            // Filled in by Task 4.
            ExpansionResult::Value(shell.lookup_var(name).unwrap_or_default())
        }
```

Replace with:

```rust
        ParamModifier::Case { direction, all, pattern } => {
            let v = shell.lookup_var(name).unwrap_or_default();
            let pat_string = pattern.as_ref().map(|w| expand_word_to_string(w, shell));
            ExpansionResult::Value(case_modify(&v, *direction, *all, pat_string.as_deref()))
        }
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test --lib expand_modifier_case_ 2>&1 | tail -10`

Expected: all 3 tests pass.

- [ ] **Step 5: Run full lib test suite for regression check**

Run: `cargo test --lib 2>&1 | tail -5`

Expected: 0 failures.

- [ ] **Step 6: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`

Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/param_expansion.rs
git commit -m "$(cat <<'EOF'
param: wire Case arm into expand_modifier (v37 task 4)

Case arm uses shell.lookup_var (so digit names resolve through
positional_args) and expand_word_to_string to materialize the optional
pattern Word to a string before calling case_modify. 3 through-the-arm
tests cover named-var, positional, and unset paths.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Integration tests via the binary

**Files:**
- Create: `tests/param_case_integration.rs`

**Note for implementer:** Same harness pattern as `tests/param_substring_integration.rs` (v33) and `tests/param_substitution_integration.rs` (v32). Spawn `huck` via `Command::new(huck_binary())` with stdin piped. Each test gets a fresh process — state never leaks.

- [ ] **Step 1: Create the test file with end-to-end coverage**

Create `tests/param_case_integration.rs` with:

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
fn case_upper_all_basic() {
    let (out, _) = run("s=hello\necho ${s^^}\nexit\n");
    assert!(out.lines().any(|l| l == "HELLO"), "stdout: {out}");
}

#[test]
fn case_upper_first_basic() {
    let (out, _) = run("s=hello\necho ${s^}\nexit\n");
    assert!(out.lines().any(|l| l == "Hello"), "stdout: {out}");
}

#[test]
fn case_lower_all_basic() {
    let (out, _) = run("s=HELLO\necho ${s,,}\nexit\n");
    assert!(out.lines().any(|l| l == "hello"), "stdout: {out}");
}

#[test]
fn case_lower_first_basic() {
    let (out, _) = run("s=HELLO\necho ${s,}\nexit\n");
    assert!(out.lines().any(|l| l == "hELLO"), "stdout: {out}");
}

#[test]
fn case_upper_with_pattern_filters() {
    // Only vowels get upper-cased.
    let (out, _) = run("s=hello\necho ${s^^[aeiou]}\nexit\n");
    assert!(out.lines().any(|l| l == "hEllO"), "stdout: {out}");
}

#[test]
fn case_upper_unicode() {
    // Unicode-aware: é → É.
    let (out, _) = run("s=café\necho ${s^^}\nexit\n");
    assert!(out.lines().any(|l| l == "CAFÉ"), "stdout: {out}");
}

#[test]
fn case_pattern_uses_other_var() {
    // The pattern is expanded — $p resolves to [ae] before glob compile.
    let (out, _) = run("s=hello\np=[ae]\necho ${s^^$p}\nexit\n");
    assert!(out.lines().any(|l| l == "hEllo"), "stdout: {out}");
}

#[test]
fn case_in_function_with_positional() {
    let (out, _) = run("f() { echo \"${1^^}\"; }\nf hello\nexit\n");
    assert!(out.lines().any(|l| l == "HELLO"), "stdout: {out}");
}

#[test]
fn case_in_pipeline_stage() {
    let (out, _) = run("s=hello\necho ${s^^} | cat\nexit\n");
    assert!(out.lines().any(|l| l == "HELLO"), "stdout: {out}");
}
```

- [ ] **Step 2: Run the new integration tests**

Run: `cargo test --test param_case_integration 2>&1 | tail -10`

Expected: all 9 tests pass.

- [ ] **Step 3: Commit**

```bash
git add tests/param_case_integration.rs
git commit -m "$(cat <<'EOF'
test: ${var^^} / ${var,,} integration coverage (v37 task 5)

9 binary-driven tests: 4 bare-form basics (upper-all, upper-first,
lower-all, lower-first), 1 with-pattern filter, 1 Unicode (café→CAFÉ),
1 pattern-from-other-var, 1 in-function-with-positional, 1
in-pipeline-stage.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Docs + version blurb + full-suite verify

**Files:**
- Modify: `docs/bash-divergences.md` (M-17 → `[fixed v37]`; amend L-04; add changelog row)
- Modify: `README.md` (v37 row)

- [ ] **Step 1: Mark M-17 fixed in `docs/bash-divergences.md`**

Find the M-17 entry at `docs/bash-divergences.md:151`:

```markdown
- **M-17: `${var^^}` / `${var,,}` case modification** — `[deferred]` medium. huck: `InvalidBraceModifier("^")`. bash: upper/lower case.
```

Replace with:

```markdown
- **M-17: `${var^^}` / `${var,,}` case modification** — `[fixed v37]` medium. All eight forms: `^^`/`^`/`,,`/`,` × bare/with-pattern. Pattern operand uses bash glob semantics (per-character match) via the existing `glob::Pattern` engine. Unicode-aware case mapping via Rust's `char::to_uppercase` / `char::to_lowercase` iterators — handles multi-char expansions like `ß`→`SS` correctly. Closes the parameter-expansion-modifier cluster started by v32 (substitute) / v33 (substring) / v34 (length + fatal PE).
```

- [ ] **Step 2: Amend L-04 with a case-modification sub-clause**

Find the L-04 entry at `docs/bash-divergences.md:301`:

```markdown
- **L-04**: `${#var}` counts Unicode chars; bash counts bytes (matches in UTF-8 locale). v33 extends the same char-counting convention to `${var:off:len}` — offset/length units are codepoints, never byte indices. Slices never split a multi-byte UTF-8 codepoint.
```

Replace with:

```markdown
- **L-04**: `${#var}` counts Unicode chars; bash counts bytes (matches in UTF-8 locale). v33 extends the same char-counting convention to `${var:off:len}` — offset/length units are codepoints, never byte indices. Slices never split a multi-byte UTF-8 codepoint. v37 `${var^^}` / `${var,,}` uses Rust's `char::to_uppercase` / `char::to_lowercase` Unicode iterators — locale-independent (matches bash with UTF-8 locale; may differ in non-UTF-8 locales).
```

- [ ] **Step 3: Add a changelog row at the bottom of `docs/bash-divergences.md`**

Find the `## Change log` section (around line 332). Append AT THE END:

```markdown
- **2026-05-28**: M-17 (`${var^^}` / `${var,,}` case modification) shipped as v37. All eight forms (`^^`/`^`/`,,`/`,` × bare/with-pattern). Reuses glob::Pattern for the per-character pattern filter; Rust's `char::to_uppercase` / `char::to_lowercase` iterators for the Unicode-aware case mapping. Closes the parameter-expansion-modifier cluster started by v32/v33/v34.
```

- [ ] **Step 4: Update the README version table**

Find the v36 row in `README.md`'s version table. Add a new row AFTER it:

```markdown
| v37       | Case modification `${var^^}` / `${var,,}` (M-17)               |
```

Match column alignment with the surrounding rows.

- [ ] **Step 5: Run the full test suite**

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | tail -30`

Expected: all suites pass. New baseline ~1426 (1397 from v36 + ~29 new across Tasks 2, 3, 4, 5).

If the PTY suite shows its v29-era flake (`pty_compound_stage_pipeline_stops_and_resumes`), re-run it in isolation: `cargo test --test pty_interactive pty_compound_stage_pipeline_stops_and_resumes 2>&1 | tail -5`. If it passes in isolation, the under-load flake is the same v29-era issue — not a v37 regression. Note in the report but don't block.

- [ ] **Step 6: Run clippy with `-D warnings`**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -10`

Expected: 0 warnings.

- [ ] **Step 7: Confirm working tree is clean**

Run: `git status`

Expected: `nothing to commit, working tree clean` on branch `v37-case-modification`. No untracked files.

- [ ] **Step 8: Commit the docs**

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: mark M-17 fixed v37; v37 in README; L-04 sub-clause

Closes the parameter-expansion-modifier cluster.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

**Steps 5-7 are verification-only — no commit between Step 4 and Step 8 above.** Hand back to the parent session for the final code-reviewer dispatch + merge to main.
