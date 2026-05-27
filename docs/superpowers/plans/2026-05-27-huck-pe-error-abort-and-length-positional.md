# huck v34 — Fatal PE Errors (M-58) + Length-of-Positional (M-60) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close M-58 (parameter-expansion errors `${var:?}` and substring
`< 0` should abort the simple command and exit the shell in
non-interactive mode) AND M-60 (`${#1}` / `${#@}` / `${#*}` length-of-
positional / count-of-positionals) in one iteration.

**Architecture:** Three-layer fatal-error propagation. `expand_modifier`
returns a new `ExpansionResult::Fatal { status }` from the
`ErrorIfUnset` and `Substring`-negative-computed-length arms. The three
`expand_*` functions in `src/expand.rs` see Fatal and stash the status
on a new `Shell::pending_fatal_pe_error: Option<i32>` field. The
executor's `resolve()` and `execute_sequence_body` peek the flag after
each `expand()` call and bail the current simple command / rest of the
logical command. The REPL loop in `Shell::run()` drains the flag after
each `process_line`; if drained AND a new `Shell::is_interactive` flag
is false (stdin not a TTY), exits the shell with the fatal status.
M-60 lives entirely in the lexer's `#` arm + the evaluator's `Length`
arm — no propagation needed.

**Tech Stack:** Rust. New stdlib import: `std::io::IsTerminal` (stable
since 1.70). No new external dependencies.

**Spec:** `docs/superpowers/specs/2026-05-27-huck-pe-error-abort-and-length-positional-design.md`

**Branch:** `v34-pe-abort-and-length-positional` (already created and checked out).

---

### Task 1: M-60 — `${#1}` / `${#@}` / `${#*}` lexer + Length evaluator

**Files:**
- Modify: `src/lexer.rs` (the `Some('#')` arm in `parse_braced_param` around line 1027; tests module at the bottom of the file)
- Modify: `src/param_expansion.rs` (the `Length` arm in `expand_modifier` around line 18; tests module)

**Note for implementer:** Task 1 is self-contained — it doesn't touch
fatal-error propagation. The lexer change extends the `#` arm to
accept digit-only positional names, plus `@` and `*` as special names
that signal "count of positional args". The evaluator change switches
from `shell.get` (vars-only) to `shell.lookup_var` (which resolves
positionals AND special params like `$0`/`$$`/`$!`), and adds an
`@`/`*` branch that returns the count.

- [ ] **Step 1: Write the failing lexer tests**

Append to the `tests` module in `src/lexer.rs`. Search for the existing
`brace_substring_*` group from v33 (e.g. `brace_substring_simple`) and
add these tests right after them. The `single_param_expansion` and
`word_to_literal` helpers already exist in this module.

```rust
    #[test]
    fn brace_length_positional() {
        let mut t = tokenize_words("${#1}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { name, modifier, quoted } = part {
            assert_eq!(name, "1");
            assert!(!quoted);
            assert!(matches!(modifier, ParamModifier::Length));
        } else { panic!("expected ParamExpansion, got {part:?}") }
    }

    #[test]
    fn brace_length_multi_digit_positional() {
        let mut t = tokenize_words("${#10}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { name, modifier, .. } = part {
            assert_eq!(name, "10");
            assert!(matches!(modifier, ParamModifier::Length));
        } else { panic!("expected ParamExpansion") }
    }

    #[test]
    fn brace_length_at() {
        let mut t = tokenize_words("${#@}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { name, modifier, .. } = part {
            assert_eq!(name, "@");
            assert!(matches!(modifier, ParamModifier::Length));
        } else { panic!("expected ParamExpansion") }
    }

    #[test]
    fn brace_length_star() {
        let mut t = tokenize_words("${#*}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { name, modifier, .. } = part {
            assert_eq!(name, "*");
            assert!(matches!(modifier, ParamModifier::Length));
        } else { panic!("expected ParamExpansion") }
    }

    #[test]
    fn brace_length_unchanged_for_named() {
        // Regression: `${#foo}` still parses as Length on a named var.
        let mut t = tokenize_words("${#foo}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { name, modifier, .. } = part {
            assert_eq!(name, "foo");
            assert!(matches!(modifier, ParamModifier::Length));
        } else { panic!("expected ParamExpansion") }
    }

    #[test]
    fn brace_length_bare_hash_unchanged() {
        // Regression: `${#}` still parses as Var { name: "#" }.
        let mut t = tokenize_words("${#}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::Var { name, .. } = part {
            assert_eq!(name, "#");
        } else { panic!("expected Var(#), got {part:?}") }
    }
```

- [ ] **Step 2: Run the lexer tests to verify they fail**

Run: `cargo test --lib brace_length_ 2>&1 | tail -20`
Expected: the 4 new tests for positional/digit/`@`/`*` fail. The two
"unchanged" regression tests should already pass (they test current
behavior).

- [ ] **Step 3: Extend the `#` arm in `parse_braced_param`**

In `src/lexer.rs`, find the existing `#` arm. Search for `if chars.peek() == Some(&'#')`
to locate it (around line 1027). The current block is:

```rust
    if chars.peek() == Some(&'#') {
        chars.next(); // consume '#'
        if chars.peek() == Some(&'}') {
            // ${#} — count of positional args.
            chars.next(); // consume '}'
            parts.push(WordPart::Var { name: "#".to_string(), quoted });
            return Ok(());
        }
        // ${#name} — length of $name.
        let name = read_braced_name(chars)?;
        if name.is_empty() {
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
```

Replace the entire `if chars.peek() == Some(&'#') { ... }` block with:

```rust
    if chars.peek() == Some(&'#') {
        chars.next(); // consume '#'
        let next = chars.peek().copied();
        if next == Some('}') {
            // ${#} — count of positional args.
            chars.next();
            parts.push(WordPart::Var { name: "#".to_string(), quoted });
            return Ok(());
        }
        // ${#name}: name may be a regular identifier, a digit-only
        // positional name (${#1}, ${#10}), or a special name @/* that
        // means "count of positional args" (same as ${#}).
        let name = match next {
            Some(c) if c.is_ascii_digit() => {
                let mut s = String::new();
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_digit() { s.push(d); chars.next(); } else { break; }
                }
                s
            }
            Some('@') => { chars.next(); "@".to_string() }
            Some('*') => { chars.next(); "*".to_string() }
            _ => read_braced_name(chars)?,
        };
        if name.is_empty() {
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
```

- [ ] **Step 4: Run the lexer tests to verify they pass**

Run: `cargo test --lib brace_length_ 2>&1 | tail -10`
Expected: all 6 tests pass.

- [ ] **Step 5: Run the entire lexer test suite to confirm no regression**

Run: `cargo test --lib lexer::tests:: 2>&1 | tail -5`
Expected: 0 failures.

- [ ] **Step 6: Write the failing evaluator tests**

Append to the `tests` module in `src/param_expansion.rs`. Search for
existing `expand_modifier_length_*` tests (they should exist for the
named-var case from earlier iterations) and add these right after:

```rust
    #[test]
    fn expand_modifier_length_at_returns_positional_count() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "bb".to_string(), "ccc".to_string()];
        let m = ParamModifier::Length;
        let r = expand_modifier("@", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("3".to_string()));
    }

    #[test]
    fn expand_modifier_length_star_returns_positional_count() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "bb".to_string()];
        let m = ParamModifier::Length;
        let r = expand_modifier("*", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("2".to_string()));
    }

    #[test]
    fn expand_modifier_length_positional_returns_char_count() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["hello".to_string()];
        let m = ParamModifier::Length;
        let r = expand_modifier("1", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("5".to_string()));
    }

    #[test]
    fn expand_modifier_length_unset_positional_returns_zero() {
        let mut shell = Shell::new();
        // positional_args is empty by default; ${#5} → 0.
        let m = ParamModifier::Length;
        let r = expand_modifier("5", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("0".to_string()));
    }
```

- [ ] **Step 7: Run the new evaluator tests to verify they fail**

Run: `cargo test --lib expand_modifier_length_ 2>&1 | tail -20`
Expected: the 4 new tests fail. Most likely with the wrong value
(`"0"` for the named-var path because `shell.get("@")` returns None,
so `unwrap_or("")` gives empty string, and `chars().count()` is 0).

- [ ] **Step 8: Update the `Length` arm in `expand_modifier`**

In `src/param_expansion.rs`, the current `Length` arm (around line 18)
is:

```rust
        ParamModifier::Length => {
            let v = shell.get(name).unwrap_or("");
            ExpansionResult::Value(v.chars().count().to_string())
        }
```

Replace with:

```rust
        ParamModifier::Length => {
            let n = match name.as_str() {
                "@" | "*" => shell.positional_args.len(),
                _ => shell.lookup_var(name).unwrap_or_default().chars().count(),
            };
            ExpansionResult::Value(n.to_string())
        }
```

- [ ] **Step 9: Run the evaluator tests to verify they pass**

Run: `cargo test --lib expand_modifier_length_ 2>&1 | tail -10`
Expected: all the `expand_modifier_length_*` tests pass (the 4 new
ones plus the pre-existing named-var ones).

- [ ] **Step 10: Run the full param_expansion test module**

Run: `cargo test --lib param_expansion 2>&1 | tail -5`
Expected: 0 failures.

- [ ] **Step 11: Commit**

```bash
git add src/lexer.rs src/param_expansion.rs
git commit -m "$(cat <<'EOF'
ast+eval: \${#1} / \${#@} / \${#*} length-of-positional (v34 task 1, M-60)

Lexer # arm accepts digit-only positional names plus the special @/*
names that count positional args (same as \${#}). Length evaluator
switches to lookup_var (so digit names resolve through positional_args)
and adds an @/* branch returning the count.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: `ExpansionResult::Fatal` + `Shell` fields scaffold

**Files:**
- Modify: `src/param_expansion.rs` (extend `ExpansionResult` enum at lines 6-10)
- Modify: `src/shell_state.rs` (add two `pub` fields to `Shell` struct; initialize in `Shell::new()`; add `take_pending_fatal_pe_error()` accessor; add `use std::io::IsTerminal;` import)

**Note for implementer:** This task only adds the AST/state scaffold —
no behavior change yet. The `Fatal` variant is unreachable (no arm
produces it), and the new Shell fields are never read. Build passes
because the existing match sites use `match` not exhaustive pattern
matching on `ExpansionResult` — they all `match { Value(v) => ..., Empty => ... }`
which will warn but not error. We'll explicitly handle Fatal in Task 4.

Actually — wait. The three `expand_*` functions in `src/expand.rs` use
exhaustive matches on `ExpansionResult`. Adding `Fatal` will cause
non-exhaustive-pattern errors at three sites. We'll handle that by
adding a placeholder arm in each of those three sites that does the
right thing in Task 4's wiring; for now (Task 2) we add a no-op
placeholder `Fatal { .. } => {}` arm to keep the build clean.

- [ ] **Step 1: Add the `Fatal` variant to `ExpansionResult`**

In `src/param_expansion.rs`, replace lines 6-10:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum ExpansionResult {
    Value(String),
    Empty,
}
```

with:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum ExpansionResult {
    Value(String),
    Empty,
    /// Fatal parameter-expansion error: the caller must abort the
    /// surrounding simple command and (in non-interactive mode) exit
    /// the shell. The message has already been printed by the arm that
    /// produced this; `status` is the exit code.
    Fatal { status: i32 },
}
```

- [ ] **Step 2: Build to flush exhaustiveness errors**

Run: `cargo build 2>&1 | tail -30`
Expected: `error[E0004]: non-exhaustive patterns: '&ExpansionResult::Fatal { .. }' not covered` in `src/expand.rs` at 3 sites (one each in `expand`, `expand_assignment`, `expand_pattern`). Note the file + lines for the next step.

- [ ] **Step 3: Add placeholder arms in the three `expand_*` functions**

In `src/expand.rs`, locate each `ExpansionResult` match site (each is
inside a `WordPart::ParamExpansion { name, modifier, quoted } => { ... }`
arm of the outer word-parts match). Each currently looks something
like:

```rust
match crate::param_expansion::expand_modifier(name, modifier, shell) {
    crate::param_expansion::ExpansionResult::Value(v) => { ... }
    crate::param_expansion::ExpansionResult::Empty => { ... }
}
```

Add a placeholder `Fatal` arm immediately before the closing `}` of
each match block:

```rust
    crate::param_expansion::ExpansionResult::Fatal { .. } => {
        // Wired in Task 4.
    }
```

Do this in all three functions: `expand`, `expand_assignment`,
`expand_pattern`.

- [ ] **Step 4: Re-build to verify clean**

Run: `cargo build 2>&1 | tail -5`
Expected: clean build (warnings about the unused `Fatal` field
`status` and the unused `pending_fatal_pe_error` are acceptable —
they go away in Task 3/4).

- [ ] **Step 5: Add `use std::io::IsTerminal;` to `src/shell_state.rs`**

At the top of `src/shell_state.rs`, alongside the existing `use`
statements, add:

```rust
use std::io::IsTerminal;
```

- [ ] **Step 6: Add the two fields to `Shell`**

Find the `pub struct Shell { ... }` declaration in `src/shell_state.rs`.
Add at the end (after the existing fields, before the closing `}`):

```rust
    /// `Some(status)` after a fatal parameter-expansion error fires
    /// inside an `expand_*` call. The executor peeks this to bail the
    /// current simple command; the REPL loop drains it via
    /// `take_pending_fatal_pe_error` to decide whether to exit (in
    /// non-interactive mode) or return to prompt (interactive).
    pub pending_fatal_pe_error: Option<i32>,
    /// True if stdin was a TTY at startup. Determines whether fatal PE
    /// errors exit the shell or just return to the prompt.
    pub is_interactive: bool,
```

- [ ] **Step 7: Initialize the fields in `Shell::new()`**

Find the body of `Shell::new()` in `src/shell_state.rs` (the
`pub fn new() -> Self { Self { ... } }` block, around line 30-60).
Inside the struct literal, add the two new fields at the end (before
the closing `}` of the struct literal):

```rust
            pending_fatal_pe_error: None,
            is_interactive: std::io::stdin().is_terminal(),
```

- [ ] **Step 8: Add the `take_pending_fatal_pe_error` accessor**

In the `impl Shell { ... }` block in `src/shell_state.rs`, alongside
other accessors like `last_status` / `set_last_status` / `lookup_var`
(search for `pub fn last_status`), add:

```rust
    /// Returns and clears the pending fatal-PE-error flag.
    pub fn take_pending_fatal_pe_error(&mut self) -> Option<i32> {
        self.pending_fatal_pe_error.take()
    }
```

- [ ] **Step 9: Re-build to verify clean**

Run: `cargo build 2>&1 | tail -5`
Expected: clean build, 0 errors.

- [ ] **Step 10: Run the full test suite to confirm no regression**

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | grep -E "failed: [1-9]"`
Expected: no output (no failures). Test count should now be ~1286 (1276
baseline + ~10 new from Task 1).

- [ ] **Step 11: Commit**

```bash
git add src/param_expansion.rs src/expand.rs src/shell_state.rs
git commit -m "$(cat <<'EOF'
scaffold: ExpansionResult::Fatal + Shell::pending_fatal_pe_error / is_interactive (v34 task 2)

ExpansionResult gains a Fatal variant; Shell gains pending_fatal_pe_error
(set when a fatal modifier fires, drained by the REPL) and is_interactive
(set from std::io::IsTerminal on stdin at construction). Placeholder
Fatal arms in the three expand_* functions keep the build clean; real
wiring lands in Task 4.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: `expand_modifier` entry-guard + ErrorIfUnset fatal arm + Substring fatal arm

**Files:**
- Modify: `src/param_expansion.rs` (top of `expand_modifier` for entry guard; `ErrorIfUnset` arm at line 40; `Substring` arm at line 84; tests module)

**Note for implementer:** This task changes the modifier-layer behavior:
two arms now return `Fatal` instead of Empty-after-eprintln. The entry
guard short-circuits all further modifier calls once a fatal is pending
(no further work; no further eprintlns). Unit tests pin the new
behavior.

- [ ] **Step 1: Write the failing evaluator tests**

Append to the `tests` module in `src/param_expansion.rs` (right after
the `expand_modifier_length_*` tests from Task 1):

```rust
    #[test]
    fn expand_modifier_error_if_unset_returns_fatal() {
        let mut shell = Shell::new();
        let m = ParamModifier::ErrorIfUnset {
            word: lit("missing"),
            colon: true,
        };
        let r = expand_modifier("HUCK_TEST_PE_FATAL1", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Fatal { status: 1 });
    }

    #[test]
    fn expand_modifier_error_if_unset_with_message_returns_fatal_and_prints() {
        let mut shell = Shell::new();
        let m = ParamModifier::ErrorIfUnset {
            word: lit("custom message"),
            colon: false,
        };
        let r = expand_modifier("HUCK_TEST_PE_FATAL2", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Fatal { status: 1 });
        // We can't easily capture stderr here — the integration tests
        // in Task 5 verify the printed message. The unit test confirms
        // only the return shape.
    }

    #[test]
    fn expand_modifier_error_if_unset_when_set_returns_value() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_FATAL3", "set".to_string());
        let m = ParamModifier::ErrorIfUnset {
            word: lit("missing"),
            colon: true,
        };
        let r = expand_modifier("HUCK_TEST_PE_FATAL3", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Value("set".to_string()));
    }

    #[test]
    fn expand_modifier_substring_negative_computed_length_returns_fatal() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_FATAL4", "abc".to_string());
        let m = ParamModifier::Substring {
            offset: lit("0"),
            length: Some(lit("-4")),
        };
        let r = expand_modifier("HUCK_TEST_PE_FATAL4", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Fatal { status: 1 });
    }

    #[test]
    fn expand_modifier_substring_bad_arith_stays_empty_not_fatal() {
        // Regression guard: bad arith in offset stays non-fatal (matches
        // bash: arithmetic errors inside ${var:off:len} operands don't
        // exit the script).
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_PE_FATAL5", "hello".to_string());
        let m = ParamModifier::Substring {
            offset: lit("@@@"),
            length: None,
        };
        let r = expand_modifier("HUCK_TEST_PE_FATAL5", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Empty);
        assert_eq!(shell.last_status(), 1);
        assert_eq!(shell.pending_fatal_pe_error, None);
    }

    #[test]
    fn expand_modifier_short_circuits_when_pending_fatal_is_set() {
        // Entry guard: if a previous expansion already set the fatal
        // flag, expand_modifier returns Empty immediately without doing
        // work — no eprintln, no side-effects.
        let mut shell = Shell::new();
        shell.pending_fatal_pe_error = Some(1);
        shell.export_set("HUCK_TEST_PE_FATAL6", "abc".to_string());
        let m = ParamModifier::Substring {
            offset: lit("0"),
            length: Some(lit("-4")), // would normally be fatal
        };
        let r = expand_modifier("HUCK_TEST_PE_FATAL6", &m, &mut shell);
        assert_eq!(r, ExpansionResult::Empty);
        // The flag must remain set (not cleared by the guard).
        assert_eq!(shell.pending_fatal_pe_error, Some(1));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib expand_modifier_error_if_unset_returns_fatal expand_modifier_substring_negative_computed_length_returns_fatal expand_modifier_short_circuits 2>&1 | tail -20`
Expected: at least 3 tests fail (the ones asserting `Fatal`); the regression-guard tests should pass against current behavior.

- [ ] **Step 3: Add the entry guard at the top of `expand_modifier`**

In `src/param_expansion.rs::expand_modifier`, the function currently
opens (around line 12-17):

```rust
pub fn expand_modifier(
    name: &str,
    modifier: &ParamModifier,
    shell: &mut Shell,
) -> ExpansionResult {
    match modifier {
        ParamModifier::Length => {
```

Insert an entry guard between the function open-brace and the `match`:

```rust
pub fn expand_modifier(
    name: &str,
    modifier: &ParamModifier,
    shell: &mut Shell,
) -> ExpansionResult {
    if shell.pending_fatal_pe_error.is_some() {
        return ExpansionResult::Empty;
    }
    match modifier {
        ParamModifier::Length => {
```

- [ ] **Step 4: Change the `ErrorIfUnset` arm to return Fatal**

In `src/param_expansion.rs::expand_modifier`, the current `ErrorIfUnset`
arm (around lines 40-59) ends with:

```rust
                shell.set_last_status(1);
                ExpansionResult::Empty
            } else {
                ExpansionResult::Value(raw.unwrap_or_default())
            }
        }
```

Replace the two lines `shell.set_last_status(1); ExpansionResult::Empty`
with the Fatal return — there's no need to set last_status here
because the REPL drain will set it from the fatal status:

```rust
                ExpansionResult::Fatal { status: 1 }
            } else {
                ExpansionResult::Value(raw.unwrap_or_default())
            }
        }
```

The eprintln above this remains; only the post-eprintln lines change.

- [ ] **Step 5: Change the `Substring` arm's negative-computed-length branch to return Fatal**

In `src/param_expansion.rs::expand_modifier`, the current `Substring` arm
(around lines 84-105) has a match block at the end:

```rust
            match substring(&value, off_n, len_n) {
                Ok(s) => ExpansionResult::Value(s),
                Err(msg) => {
                    eprintln!("huck: {}: {}", name, msg);
                    shell.set_last_status(1);
                    ExpansionResult::Empty
                }
            }
```

Replace the `Err(msg) =>` arm to return Fatal:

```rust
            match substring(&value, off_n, len_n) {
                Ok(s) => ExpansionResult::Value(s),
                Err(msg) => {
                    eprintln!("huck: {}: {}", name, msg);
                    ExpansionResult::Fatal { status: 1 }
                }
            }
```

- [ ] **Step 6: Run the new evaluator tests to verify they pass**

Run: `cargo test --lib expand_modifier_error_if_unset_returns_fatal expand_modifier_substring_negative_computed_length_returns_fatal expand_modifier_short_circuits 2>&1 | tail -20`
Expected: all 6 new tests pass.

- [ ] **Step 7: Update pre-existing tests that asserted old behavior**

Old behavior was that `${X:?msg}` set `$? = 1` and returned Empty. Any
pre-existing test asserting `ExpansionResult::Empty` AND
`shell.last_status() == 1` for ErrorIfUnset needs updating to expect
`ExpansionResult::Fatal { status: 1 }` instead.

Run: `cargo test --lib expand_modifier 2>&1 | tail -30`
Look for any failures. The likely candidates are tests with names like
`expand_modifier_error_if_unset_*`. For each failure, update the
assertion from:

```rust
assert_eq!(r, ExpansionResult::Empty);
assert_eq!(shell.last_status(), 1);
```

to:

```rust
assert_eq!(r, ExpansionResult::Fatal { status: 1 });
```

And add a comment: `// v34: ErrorIfUnset now returns Fatal instead of Empty + $?=1.`

If a similar failing test exists for the substring negative-length path
(e.g. `expand_modifier_substring_negative_length_below_zero_errors_and_empty`),
update it the same way.

Re-run `cargo test --lib expand_modifier 2>&1 | tail -10`. Expect 0 failures.

- [ ] **Step 8: Run full lib test suite for regression check**

Run: `cargo test --lib 2>&1 | tail -5`
Expected: 0 failures.

- [ ] **Step 9: Commit**

```bash
git add src/param_expansion.rs
git commit -m "$(cat <<'EOF'
eval: ErrorIfUnset + Substring negative-length now return Fatal (v34 task 3)

expand_modifier gains an entry-guard that short-circuits to Empty if a
previous fatal is already pending. ErrorIfUnset's eprintln-then-Empty
path becomes eprintln-then-Fatal{status:1}. Substring's negative-
computed-length error path becomes Fatal{status:1}. Bad-arith in
offset/length stays Empty (non-fatal, matches bash).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: `expand_*` propagation + executor wiring

**Files:**
- Modify: `src/expand.rs` (replace the 3 placeholder `Fatal { .. }` arms in `expand` / `expand_assignment` / `expand_pattern` with real propagation)
- Modify: `src/executor.rs` (`resolve()` at line 1118; `execute_sequence_body` at line 55)

**Note for implementer:** This task connects the modifier-layer Fatal
return to the executor: the three `expand_*` functions stash the
status on `Shell`, the executor's `resolve()` returns Err on the
flag, and `execute_sequence_body` bails the rest of the logical
command on the flag. No tests added in this task — Task 5 covers the
end-to-end behavior via integration tests. The behavior is already
testable via existing PE tests if we add a mid-pipeline assertion,
but for cohesion all integration tests live in Task 5.

- [ ] **Step 1: Replace the `Fatal` placeholder arms in `src/expand.rs`**

Three sites. Each looks like:

```rust
    crate::param_expansion::ExpansionResult::Fatal { .. } => {
        // Wired in Task 4.
    }
```

Replace each with (note: `status` is bound by-value since the match is
on the owned `ExpansionResult` return value — no deref):

```rust
    crate::param_expansion::ExpansionResult::Fatal { status } => {
        shell.pending_fatal_pe_error = Some(status);
        return result;
    }
```

Apply this in all three sites. If the local accumulator variable name
differs (each function builds something different — `expand` builds a
`Vec<Field>`, `expand_assignment` and `expand_pattern` build `String`),
use whatever the actual builder variable is named in that function. The
returned partial value is irrelevant — every caller will check the
flag and bail.

- [ ] **Step 2: Build to verify**

Run: `cargo build 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 3: Add the peek-check in `resolve()`**

In `src/executor.rs::resolve()` (search for `fn resolve(cmd: &ExecCommand`,
around line 1118), the current function opens:

```rust
fn resolve(cmd: &ExecCommand, shell: &mut Shell) -> Result<ResolvedCommand, i32> {
    let prog_fields = glob_expand_fields(expand(&cmd.program, shell));
    if prog_fields.is_empty() {
        eprintln!("huck: command not found:");
        return Err(127);
    }
    let mut iter = prog_fields.into_iter();
    let program = iter.next().unwrap();
    let mut args: Vec<String> = iter.collect();
    for word in &cmd.args {
        args.extend(glob_expand_fields(expand(word, shell)));
    }
```

Insert peek-checks AFTER the program expand AND AFTER each args expand:

```rust
fn resolve(cmd: &ExecCommand, shell: &mut Shell) -> Result<ResolvedCommand, i32> {
    let prog_fields = glob_expand_fields(expand(&cmd.program, shell));
    if let Some(status) = shell.pending_fatal_pe_error {
        return Err(status);
    }
    if prog_fields.is_empty() {
        eprintln!("huck: command not found:");
        return Err(127);
    }
    let mut iter = prog_fields.into_iter();
    let program = iter.next().unwrap();
    let mut args: Vec<String> = iter.collect();
    for word in &cmd.args {
        args.extend(glob_expand_fields(expand(word, shell)));
        if let Some(status) = shell.pending_fatal_pe_error {
            return Err(status);
        }
    }
```

The flag is peeked (`.pending_fatal_pe_error` accesses the field directly
without taking) — the REPL drains it later.

- [ ] **Step 4: Add the peek-check in `execute_sequence_body`**

In `src/executor.rs::execute_sequence_body` (line 55), there are TWO
`set_last_status(c)` call sites — one at line 69 (after the first
command) and one at line 87 (in the rest-of-sequence loop).

After EACH `shell.set_last_status(c)` call, add a peek-check that
bails the sequence:

```rust
    if let ExecOutcome::Continue(c) = status {
        shell.set_last_status(c);
        if shell.pending_fatal_pe_error.is_some() {
            return ExecOutcome::Continue(c);
        }
    }
```

The block currently is:

```rust
    if let ExecOutcome::Continue(c) = status {
        shell.set_last_status(c);
    }
```

Same replacement in BOTH sites (the line-69 site and the line-87 site).

- [ ] **Step 5: Re-build to verify**

Run: `cargo build 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 6: Run the full test suite to confirm no regression**

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | grep -E "failed: [1-9]"`
Expected: no output. (Some existing integration tests in
`tests/param_substring_integration.rs` may now fail because the v33
behavior changed — the `substring_negative_computed_length_errors` test
script reaches `echo "[${s:0:-4}]"` and the echo no longer runs because
the command aborts. We'll update those tests in Task 5 alongside the
new integration tests.)

If `tests/param_substring_integration.rs` shows failures, run JUST
that file: `cargo test --test param_substring_integration 2>&1 | tail -20`.
The expected failure is `substring_negative_computed_length_errors`
because the assertion `out.lines().any(|l| l == "[]")` now fails (no
output line is produced — the command aborted before echo could run).
That's expected; Task 5 updates the assertion.

If any OTHER tests fail unexpectedly, STOP and investigate before proceeding.

- [ ] **Step 7: Commit**

```bash
git add src/expand.rs src/executor.rs
git commit -m "$(cat <<'EOF'
exec: propagate Fatal PE errors through expand_* + resolve + sequence (v34 task 4)

The three expand_* functions stash status on shell.pending_fatal_pe_error
and bail their field-construction loop. resolve() peek-checks the flag
after each expand() and returns Err(status) without spawning a process.
execute_sequence_body peek-checks after each pipeline's Continue(c) and
bails the rest of the ;/&&/|| chain.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: REPL drain + v33 test updates + integration tests

**Files:**
- Modify: `src/shell.rs` (REPL `Continue(status)` branch around line 75)
- Modify: `tests/param_substring_integration.rs` (update 2 v33 tests)
- Create: `tests/pe_error_abort_integration.rs` (new integration test file)

**Note for implementer:** This is the top-of-stack wiring. After
`process_line` returns, the REPL drains the flag. If drained AND
non-interactive, exit the shell with the drained status. Then the
integration tests verify end-to-end behavior.

- [ ] **Step 1: Update the REPL `Continue` branch in `src/shell.rs::run()`**

In `src/shell.rs::run()`, around line 75, the current branch is:

```rust
                    ExecOutcome::Continue(status) => shell.set_last_status(status),
```

Replace with:

```rust
                    ExecOutcome::Continue(status) => {
                        shell.set_last_status(status);
                        if let Some(fatal_status) = shell.take_pending_fatal_pe_error() {
                            if !shell.is_interactive {
                                shell.history.save();
                                return fatal_status;
                            }
                            // Interactive: $? already set above; fall through
                            // to next prompt iteration.
                        }
                    }
```

- [ ] **Step 2: Update the v33 substring tests**

In `tests/param_substring_integration.rs`, find `substring_negative_computed_length_errors`
(around line 68). Current shape:

```rust
#[test]
fn substring_negative_computed_length_errors() {
    // The error path returns Empty (so the field is empty) and the eprintln
    // fires. We don't assert $?==1 here because of huck's M-58 divergence:
    // parameter-expansion errors set $? but do NOT abort the simple command,
    // ... [long comment block]
    let (out, err) = run("s=abc\necho \"[${s:0:-4}]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[]"), "stdout: {out}");
    assert!(err.contains("substring expression < 0"), "stderr: {err}");
}
```

Replace the entire function with:

```rust
#[test]
fn substring_negative_computed_length_aborts_command() {
    // v34 (M-58 fix): negative computed length aborts the surrounding
    // simple command. The `echo "[...]"` never runs, so no `[]` line in
    // stdout. The script then runs `echo after`, which DOES run because
    // huck is in non-interactive mode (piped stdin) and the abort would
    // exit the shell. So in this test we verify the non-interactive
    // exit-on-fatal behavior: huck exits before reaching `echo after`.
    let (out, err) = run("s=abc\necho \"[${s:0:-4}]\"\necho after\nexit\n");
    assert!(!out.lines().any(|l| l == "[]"), "stdout should NOT have []: {out}");
    assert!(!out.lines().any(|l| l == "after"), "stdout should NOT have 'after': {out}");
    assert!(err.contains("substring expression < 0"), "stderr: {err}");
}
```

Then find `substring_bad_arith_returns_empty_sets_status` (around line 119).
The comment block currently cross-references the M-58 divergence. Update it:

Current:
```rust
#[test]
fn substring_bad_arith_returns_empty_sets_status() {
    // See substring_negative_computed_length_errors for why $? is not asserted
    // here (M-58 family divergence: PE errors set $? then `echo` resets it).
    let (out, err) = run("s=hello\necho \"[${s:@@@}]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[]"), "stdout: {out}");
    assert!(err.contains("arithmetic"), "stderr: {err}");
}
```

Replace with:

```rust
#[test]
fn substring_bad_arith_returns_empty_sets_status() {
    // Bad arith in substring offset/length stays NON-fatal in v34: prints
    // the arithmetic error, sets $?=1, but does not abort the surrounding
    // command (matches bash, which only treats `:?` and substring-<0 as
    // exit-the-script errors). The echo still runs and prints `[]`.
    let (out, err) = run("s=hello\necho \"[${s:@@@}]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[]"), "stdout: {out}");
    assert!(err.contains("arithmetic"), "stderr: {err}");
}
```

- [ ] **Step 3: Run the updated v33 tests to confirm they pass**

Run: `cargo test --test param_substring_integration 2>&1 | tail -10`
Expected: all 16 tests pass (the two updated tests now reflect the v34
behavior; the other 14 are unchanged).

- [ ] **Step 4: Create `tests/pe_error_abort_integration.rs`**

Create the file with:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run(script: &str) -> (String, String, std::process::ExitStatus) {
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
        out.status,
    )
}

#[test]
fn error_if_unset_aborts_rest_of_sequence() {
    // `${X:?msg}; echo continued` — bash exits before echo runs.
    let (out, err, _) = run("${X:?missing}\necho continued\nexit\n");
    assert!(!out.lines().any(|l| l == "continued"), "stdout: {out}");
    assert!(err.contains("X: missing"), "stderr: {err}");
}

#[test]
fn error_if_unset_non_interactive_exits_shell() {
    // The script is `${X:?missing}\necho after\n` — huck should exit
    // with status 1 BEFORE reaching `echo after` (no `after` in stdout).
    let (out, err, status) = run("${X:?missing}\necho after\n");
    assert!(!out.lines().any(|l| l == "after"), "stdout should not have 'after': {out}");
    assert!(err.contains("X: missing"), "stderr: {err}");
    assert_eq!(status.code(), Some(1), "exit status should be 1, got {status:?}");
}

#[test]
fn error_if_unset_colon_treats_empty_as_unset() {
    // X is set to empty; `:?` treats empty as unset so it should fire.
    let (out, _err, status) = run("X=\"\"\n${X:?empty}\necho after\n");
    assert!(!out.lines().any(|l| l == "after"), "stdout: {out}");
    assert_eq!(status.code(), Some(1));
}

#[test]
fn error_if_unset_without_colon_only_aborts_when_unset() {
    // X is set to empty; `?` (no colon) treats empty as set, so it
    // should NOT fire.
    let (out, _err, _) = run("X=\"\"\n: ${X?empty}\necho after\nexit\n");
    assert!(out.lines().any(|l| l == "after"), "stdout: {out}");
}

#[test]
fn error_if_unset_when_set_passes_through() {
    let (out, _err, _) = run("X=hello\necho \"${X:?missing}\"\nexit\n");
    assert!(out.lines().any(|l| l == "hello"), "stdout: {out}");
}

#[test]
fn substring_negative_computed_length_aborts_and_exits() {
    let (out, err, status) = run("s=abc\necho \"[${s:0:-4}]\"\necho after\n");
    assert!(!out.lines().any(|l| l == "[]"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "after"), "stdout: {out}");
    assert!(err.contains("substring expression < 0"), "stderr: {err}");
    assert_eq!(status.code(), Some(1));
}

#[test]
fn bad_arith_in_substring_stays_non_fatal() {
    // Regression: arithmetic errors in offset/length do NOT abort the
    // command — only `:?` and substring-<0 do.
    let (out, err, _) = run("s=hello\necho \"[${s:@@@}]\"\necho after\nexit\n");
    assert!(out.lines().any(|l| l == "[]"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "after"), "stdout: {out}");
    assert!(err.contains("arithmetic"), "stderr: {err}");
}

#[test]
fn length_positional_in_function() {
    let (out, _err, _) = run("f() { echo ${#1}; }\nf hello\nexit\n");
    assert!(out.lines().any(|l| l == "5"), "stdout: {out}");
}

#[test]
fn length_at_star_match_hash_in_function() {
    let (out, _err, _) = run("f() { echo \"${#@},${#*},${#}\"; }\nf x y z\nexit\n");
    assert!(out.lines().any(|l| l == "3,3,3"), "stdout: {out}");
}

#[test]
fn error_if_unset_inside_subshell_does_not_kill_parent() {
    // The subshell's fatal flag stays in the cloned Shell; the parent's
    // flag is untouched. After the subshell exits non-zero, `echo after`
    // runs in the parent.
    let (out, _err, _) = run("(${X:?missing})\necho after\nexit\n");
    assert!(out.lines().any(|l| l == "after"), "stdout: {out}");
}
```

- [ ] **Step 5: Run the new integration tests**

Run: `cargo test --test pe_error_abort_integration 2>&1 | tail -20`
Expected: all 10 tests pass.

If `error_if_unset_inside_subshell_does_not_kill_parent` fails, check
whether subshell forking actually clones the Shell (it should per v25 /
v28 — `fork_and_run_in_subshell` in `src/executor.rs`). If the flag is
shared, the test will fail because the parent inherits the subshell's
pending fatal. Fix would be to ensure the cloned Shell starts with
`pending_fatal_pe_error: None` after subshell setup. If you have to
fix this, add a one-line `cloned.pending_fatal_pe_error = None;` in
the subshell-fork helper, and note it in the commit message.

- [ ] **Step 6: Run the full test suite**

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | grep -E "failed: [1-9]"`
Expected: no output (no failures).

- [ ] **Step 7: Commit**

```bash
git add src/shell.rs tests/param_substring_integration.rs tests/pe_error_abort_integration.rs
git commit -m "$(cat <<'EOF'
exec+test: REPL drain on fatal PE + integration coverage (v34 task 5)

Shell::run drains pending_fatal_pe_error after process_line; in
non-interactive mode (stdin not a TTY) exits with the fatal status.
Updates the two v33 substring integration tests for the new abort
behavior. New tests/pe_error_abort_integration.rs covers all the
M-58 fatal scenarios + M-60 length-of-positional via functions.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Docs + full-suite verification

**Files:**
- Modify: `docs/bash-divergences.md` (M-58, M-60, M-16; changelog)
- Modify: `README.md` (v34 row)

- [ ] **Step 1: Mark M-58 fixed in `docs/bash-divergences.md`**

Find the M-58 entry (search for `**M-58:`). Current line:

```markdown
- **M-58: `${var:?w}` doesn't abort non-interactive scripts** — `[open]` medium. huck: prints error, sets `$?` = 1, continues. bash: exits the script.
```

Replace with:

```markdown
- **M-58: `${var:?w}` doesn't abort non-interactive scripts** — `[fixed v34]` medium. `${var:?}` and substring-expression-<0 errors now return `ExpansionResult::Fatal { status: 1 }`, which the three `expand_*` functions stash on `Shell::pending_fatal_pe_error`. The executor's `resolve()` and `execute_sequence_body` peek-check the flag after each `expand()` and bail; the REPL drains via `take_pending_fatal_pe_error()` and (when `!is_interactive`) exits the shell. Bad-arith in substring offset/length stays non-fatal (matches bash).
```

- [ ] **Step 2: Mark M-60 fixed**

Find the M-60 entry. Current:

```markdown
- **M-60: `${#1}` (length of positional)** — `[open]` low. huck: `${#name}` requires non-digit name; rejects `${#1}` as `InvalidVarName`. bash: returns the length of `$1`.
```

Replace with:

```markdown
- **M-60: `${#1}` (length of positional)** — `[fixed v34]` low. Lexer `#` arm now accepts digit-only names (`${#1}`, `${#10}`) and the special `@`/`*` names that mean "count of positional args" (same as `${#}`). Length evaluator switched from `shell.get` to `shell.lookup_var` so digit names resolve through `positional_args`.
```

- [ ] **Step 3: Amend M-16 to remove the M-58 inheritance note**

Find the M-16 entry. The current text contains:

```
**Inherits M-58 divergence**: a substring expansion error prints the message and sets `$?=1` but does NOT abort the surrounding simple command (so `echo "[${s:0:-4}]"; echo $?` prints `[]` then `0`, not `1`).
```

Remove that entire sentence (the `**Inherits M-58 divergence**` clause).
The rest of the M-16 entry stays unchanged. The new M-16 entry should
end with just the "Out of scope" sentence about array slicing.

- [ ] **Step 4: Add a changelog row at the bottom of `docs/bash-divergences.md`**

Find the `## Change log` section. Append:

```markdown
- **2026-05-27**: M-58 (fatal PE error abort) + M-60 (length-of-positional / count-of-positionals) shipped as v34. New `ExpansionResult::Fatal` variant carries the abort signal from `expand_modifier`; the three `expand_*` functions stash it on `Shell::pending_fatal_pe_error`; the executor's `resolve()` + `execute_sequence_body` peek-check it; the REPL drains and (in non-interactive mode) exits the shell. M-16's M-58-inheritance note removed.
```

- [ ] **Step 5: Update README.md version table**

Find the table row for v33 (look for `| v33 ` in `README.md`). Add a new row AFTER it:

```markdown
| v34       | Fatal PE errors (M-58) + `${#1}`/`${#@}`/`${#*}` length (M-60)   |
```

Match the column width / pipe alignment of the surrounding rows.

- [ ] **Step 6: Commit the docs**

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: mark M-58 + M-60 fixed; v34 in README; amend M-16

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 7: Run the entire test suite**

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | tail -30`
Expected: all suites pass. New baseline ~1301 (1276 from v33 + ~25
new tests across Tasks 1, 3, 5).

If the PTY suite has its known flake (`pty_compound_stage_pipeline_stops_and_resumes`),
re-run it in isolation: `cargo test --test pty_interactive
pty_compound_stage_pipeline_stops_and_resumes 2>&1 | tail -5`. If it
passes in isolation, the under-load flake is the same v29-era issue
already documented and not a v34 regression. Note it in the report
but don't block.

- [ ] **Step 8: Run clippy with `-D warnings`**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -10`
Expected: 0 warnings.

If clippy reports anything (commonly: dead code on the new `Fatal`
variant's `status` field if a code path doesn't use it; unused-import
warning on the `IsTerminal` trait), fix inline and add a separate
review-fix commit.

- [ ] **Step 9: Confirm working tree is clean**

Run: `git status`
Expected: `nothing to commit, working tree clean` on branch
`v34-pe-abort-and-length-positional`. No untracked files.

**No commit for Step 7-9** — they're verification only. Hand back to
the parent session for the final code-reviewer dispatch + merge to main.
