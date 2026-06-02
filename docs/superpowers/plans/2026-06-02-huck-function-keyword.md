# huck v77 — Function Keyword Form Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task is implemented by a fresh subagent, with spec-compliance review and code-quality review between tasks.

**Goal:** Add bash's `function NAME { ... }` keyword form for function definition alongside huck's existing POSIX `name() { ... }` form.

**Architecture:** Single-file change in `src/command.rs`. New `Keyword::Function` variant + parse-map entry; new `parse_function_keyword_def` helper handling optional `()` after the name; shared `is_function_body_shape` predicate used by BOTH form-parsers (also extends POSIX form to accept `[[ ]]` bodies, closing a pre-existing gap). Lexer, executor, expand, and all other modules unchanged.

**Tech Stack:** Rust 1.85+; no new dependencies.

**Branch:** `v77-function-keyword` (create from `main` in Preamble P.1).

**Spec:** `docs/superpowers/specs/2026-06-02-huck-function-keyword-design.md`.

**Commit trailer (every commit):**

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble P.1: Branch setup

- [ ] **Step 1: Verify clean tree on main**

Run: `git status && git rev-parse --abbrev-ref HEAD`
Expected: branch `main`, clean working tree.

- [ ] **Step 2: Create the iteration branch**

```bash
git checkout -b v77-function-keyword
```

Expected: `Switched to a new branch 'v77-function-keyword'`.

- [ ] **Step 3: Confirm baseline tests pass**

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "Baseline:", sum}'`
Expected: 2223 (current main).

- [ ] **Step 4: Confirm clippy is clean**

Run: `cargo clippy --all-targets 2>&1 | tail -3`
Expected: `Finished` with no warnings.

---

## File-structure map

| File | Responsibility | Tasks |
|------|----------------|-------|
| `src/command.rs` | Keyword enum gains `Function`; parse map; new `parse_function_keyword_def`; shared `is_function_body_shape`; POSIX-form parser refactored to call the predicate; ~13 unit tests | 1 |
| `tests/function_keyword_integration.rs` | NEW. 6 binary-driven integration tests | 2 |
| `tests/scripts/function_keyword_diff_check.sh` | NEW. 6 bash-diff fragments | 2 |
| `docs/bash-divergences.md` | M-09 flipped to `[fixed v77]`; new deferred entries for relaxed name chars + definition-attached redirections; change-log entry | 2 |
| `README.md` | New v77 iteration row | 2 |

---

## Task 1: Parser changes + unit tests

**Files:**
- Modify: `src/command.rs` — add `Keyword::Function`; parse-map entry; new helper; shared body-shape predicate; refactor `parse_function_def`; add unit tests at the bottom of the existing `#[cfg(test)] mod tests` block.

**Goal:** `function NAME { ... }` and `function NAME () { ... }` parse to `Command::FunctionDef`. POSIX form continues to work and additionally accepts `[[ ]]` bodies. All 2223 baseline tests continue passing.

### Steps

- [ ] **Step 1: Add `Function` to the `Keyword` enum**

Edit `src/command.rs`. Find the `pub enum Keyword` declaration (around line 5). The current variants end with `DoubleBracketClose`. Add `Function` to the enum:

```rust
pub enum Keyword {
    If,
    Then,
    Elif,
    Else,
    Fi,
    While,
    Until,
    Do,
    Done,
    For,
    In,
    Case,
    Esac,
    LBrace,
    RBrace,
    DoubleBracketOpen,   // [[
    DoubleBracketClose,  // ]]
    Function,
}
```

- [ ] **Step 2: Add `Function => "function"` to `Keyword::name()`**

In the same file, find `impl Keyword { fn name(self) -> &'static str { match self { ... } } }` (around line 24). Add the `Function` arm at the end:

```rust
            Keyword::DoubleBracketOpen => "[[",
            Keyword::DoubleBracketClose => "]]",
            Keyword::Function => "function",
        }
    }
}
```

- [ ] **Step 3: Add `"function" => Some(Keyword::Function)` to the parse map**

Find `fn keyword_of(token: &Token) -> Option<Keyword>` (around line 51). Inside the `match text.as_str()` block (around line 59), add the `"function"` arm right before the `_ => None`:

```rust
        "[[" => Some(Keyword::DoubleBracketOpen),
        "]]" => Some(Keyword::DoubleBracketClose),
        "function" => Some(Keyword::Function),
        _ => None,
    }
}
```

- [ ] **Step 4: Confirm baseline tests still pass after the enum / map additions**

Run: `cargo build 2>&1 | tail -5`
Expected: clean build.

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print sum}'`
Expected: 2223 (no behavioral change yet — `Keyword::Function` is recognized but not dispatched anywhere, so it falls through to the `Some(other) => Err(ParseError::UnexpectedKeyword(other.name().to_string()))` arm in `parse_command`. This means `function foo { :; }` will now error with `unexpected keyword: function` instead of trying to parse it as a pipeline. Don't worry — Step 6 adds the dispatch arm.)

- [ ] **Step 5: Extract `is_function_body_shape` predicate**

In `src/command.rs`, find `fn parse_function_def` (around line 744). Look for the `if !matches!(body, ...)` body-shape check (around line 761). Replace it with a call to a new predicate. First add the predicate as a sibling private function (place it directly above `parse_function_def`):

```rust
/// True if `body` is one of the compound-command shapes that's
/// allowed as a function body in both POSIX `name() body` form and
/// the bash `function NAME body` form.
fn is_function_body_shape(body: &Command) -> bool {
    matches!(
        body,
        Command::If(_)
            | Command::While(_)
            | Command::For(_)
            | Command::Case(_)
            | Command::BraceGroup(_)
            | Command::Subshell { .. }
            | Command::DoubleBracket(_)
    )
}
```

Note `Command::DoubleBracket(_)` is INCLUDED here, even though the existing POSIX `parse_function_def` does NOT accept it today. This is the pre-existing gap the spec closes.

Then update `parse_function_def` to call the predicate. Replace the body-shape check (lines 761-768 or thereabouts):

```rust
    let body = parse_command(iter)?;
    if !is_function_body_shape(&body) {
        return Err(ParseError::FunctionBody);
    }
    Ok(Command::FunctionDef { name, body: Box::new(body) })
}
```

- [ ] **Step 6: Add the `Keyword::Function` dispatch arm in `parse_command`**

Find the match in `parse_command` (around line 651). The match arms currently end with `Some(Keyword::DoubleBracketOpen) => parse_double_bracket(iter)` then `Some(other) => Err(ParseError::UnexpectedKeyword(...))` then `None => { ... }`.

Add a new arm BEFORE the `Some(other)` catch-all:

```rust
        Some(Keyword::DoubleBracketOpen) => parse_double_bracket(iter),
        Some(Keyword::Function) => parse_function_keyword_def(iter),
        Some(other) => Err(ParseError::UnexpectedKeyword(other.name().to_string())),
```

- [ ] **Step 7: Add the `parse_function_keyword_def` helper**

In `src/command.rs`, place `parse_function_keyword_def` immediately after the existing `parse_function_def` (around line 770):

```rust
/// Parses `function NAME [()] compound-command`. The caller has
/// verified the next token is the `function` keyword (still in the
/// iterator). Consumes the keyword, the name, optional `()`, and
/// the compound body.
fn parse_function_keyword_def<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Command, ParseError> {
    // Consume `function` keyword.
    iter.next();

    // Read the name. Must be a Word that's a valid POSIX identifier
    // and not a reserved keyword.
    let name_word = match iter.next() {
        Some(Token::Word(w)) => w,
        _ => return Err(ParseError::FunctionName),
    };
    let name = valid_identifier_text(&name_word).ok_or(ParseError::FunctionName)?;

    // Optionally consume `()`.
    if matches!(iter.peek(), Some(Token::Op(Operator::LParen))) {
        iter.next(); // consume `(`
        match iter.next() {
            Some(Token::Op(Operator::RParen)) => {}
            _ => return Err(ParseError::FunctionBody),
        }
    }

    // Allow newlines between name (or `()`) and the body.
    skip_newlines(iter);
    if iter.peek().is_none() {
        return Err(ParseError::UnterminatedFunction);
    }

    let body = parse_command(iter)?;
    if !is_function_body_shape(&body) {
        return Err(ParseError::FunctionBody);
    }
    Ok(Command::FunctionDef { name, body: Box::new(body) })
}
```

- [ ] **Step 8: Build and run the existing suite**

Run: `cargo build 2>&1 | tail -5`
Expected: clean build.

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print sum}'`
Expected: 2223 (no test changes yet; the new dispatch is in place but no test exercises it yet).

If a test fails, investigate. The most likely cause is a test that asserted the OLD POSIX-form behavior of rejecting `[[ ]]` bodies — that's a real bash divergence we're closing as part of v77; locate and update that test if it exists. If no test fails, proceed.

- [ ] **Step 9: Add the 13 new unit tests**

The existing test module at the bottom of `src/command.rs` parses input using the idiomatic pattern:

```rust
let tokens = crate::lexer::tokenize("...").unwrap();
let parsed = parse(tokens).unwrap().expect("non-empty parse");
// parsed is a Sequence; parsed.first is a Command directly (no AndOr wrapper).
```

`parse()` returns `Result<Option<Sequence>, ParseError>` — the `.unwrap()` checks Ok, the `.expect("non-empty parse")` unwraps `Some`. For the single-command-per-input tests below, `parsed.first` is the Command we want.

`Sequence` shape (per `src/command.rs:488`): `{ first: Command, rest: Vec<(Connector, Command)>, background: bool }`. `parsed.rest` is empty for single-command inputs.

`Command::DoubleBracket` is a **struct variant** `{ expr, inline_assignments }`, not a tuple variant. Match as `Command::DoubleBracket { .. }`.

Append the 13 tests below to the existing `#[cfg(test)] mod tests` block in `src/command.rs`:

```rust
    #[test]
    fn function_keyword_form_brace_body() {
        let tokens = crate::lexer::tokenize("function f { echo hi; }").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        match parsed.first {
            Command::FunctionDef { name, body } => {
                assert_eq!(name, "f");
                assert!(matches!(*body, Command::BraceGroup(_)),
                        "expected brace body, got {body:?}");
            }
            other => panic!("expected FunctionDef, got {other:?}"),
        }
    }

    #[test]
    fn function_keyword_form_with_parens() {
        let tokens = crate::lexer::tokenize("function f() { :; }").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        assert!(matches!(parsed.first, Command::FunctionDef { ref name, .. } if name == "f"),
                "got {:?}", parsed.first);
    }

    #[test]
    fn function_keyword_form_with_parens_and_spaces() {
        let tokens = crate::lexer::tokenize("function f  (  )  { :; }").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        assert!(matches!(parsed.first, Command::FunctionDef { ref name, .. } if name == "f"),
                "got {:?}", parsed.first);
    }

    #[test]
    fn function_keyword_form_subshell_body() {
        let tokens = crate::lexer::tokenize("function f() ( echo nested )").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        match parsed.first {
            Command::FunctionDef { name, body } => {
                assert_eq!(name, "f");
                assert!(matches!(*body, Command::Subshell { .. }),
                        "expected subshell body, got {body:?}");
            }
            other => panic!("expected FunctionDef, got {other:?}"),
        }
    }

    #[test]
    fn function_keyword_form_compound_body_no_braces() {
        // `function f if true; then :; fi` — no braces; if-statement body.
        let tokens = crate::lexer::tokenize("function f if true; then :; fi").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        match parsed.first {
            Command::FunctionDef { name, body } => {
                assert_eq!(name, "f");
                assert!(matches!(*body, Command::If(_)),
                        "expected if body, got {body:?}");
            }
            other => panic!("expected FunctionDef, got {other:?}"),
        }
    }

    #[test]
    fn function_keyword_form_newline_before_body() {
        let tokens = crate::lexer::tokenize("function f\n{\n:;\n}").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        assert!(matches!(parsed.first, Command::FunctionDef { ref name, .. } if name == "f"),
                "got {:?}", parsed.first);
    }

    #[test]
    fn function_keyword_no_name_errors() {
        let tokens = crate::lexer::tokenize("function { :; }").unwrap();
        let err = parse(tokens).expect_err("should error");
        assert!(matches!(err, ParseError::FunctionName), "got {err:?}");
    }

    #[test]
    fn function_keyword_keyword_name_errors() {
        // Names that are themselves reserved keywords are rejected.
        let tokens = crate::lexer::tokenize("function if { :; }").unwrap();
        let err = parse(tokens).expect_err("should error");
        assert!(matches!(err, ParseError::FunctionName), "got {err:?}");
    }

    #[test]
    fn function_keyword_missing_body_errors() {
        let tokens = crate::lexer::tokenize("function f").unwrap();
        let err = parse(tokens).expect_err("should error");
        assert!(matches!(err, ParseError::UnterminatedFunction), "got {err:?}");
    }

    #[test]
    fn function_keyword_bad_body_errors() {
        // `function f echo hi` — body is a pipeline, not a compound.
        let tokens = crate::lexer::tokenize("function f echo hi").unwrap();
        let err = parse(tokens).expect_err("should error");
        assert!(matches!(err, ParseError::FunctionBody), "got {err:?}");
    }

    #[test]
    fn function_keyword_unbalanced_parens_errors() {
        let tokens = crate::lexer::tokenize("function f ( { :; }").unwrap();
        let err = parse(tokens).expect_err("should error");
        assert!(matches!(err, ParseError::FunctionBody), "got {err:?}");
    }

    #[test]
    fn function_posix_form_double_bracket_body() {
        // Regression: POSIX form should ALSO accept [[ ]] body
        // (closes a pre-existing gap; was rejected pre-v77).
        let tokens = crate::lexer::tokenize("f() [[ -e /dev/null ]]").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        match parsed.first {
            Command::FunctionDef { name, body } => {
                assert_eq!(name, "f");
                assert!(matches!(*body, Command::DoubleBracket { .. }),
                        "expected DoubleBracket body, got {body:?}");
            }
            other => panic!("expected FunctionDef, got {other:?}"),
        }
    }

    #[test]
    fn function_keyword_form_double_bracket_body() {
        // Wrapped in a brace group: the body IS BraceGroup; inside the
        // brace group is a DoubleBracket. Verify the function parses.
        let tokens = crate::lexer::tokenize("function f { [[ -e /dev/null ]]; }").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        assert!(matches!(parsed.first, Command::FunctionDef { ref name, .. } if name == "f"),
                "got {:?}", parsed.first);
    }
```

- [ ] **Step 10: Run the new tests**

Run: `cargo test --quiet function_keyword 2>&1 | tail -15`
Expected: 13 new tests pass.

If `function_posix_form_double_bracket_body` fails because the AST doesn't include `Command::DoubleBracket`, search for the actual variant name with `grep -n "DoubleBracket" src/command.rs | head -5`. Update the predicate's variant to match. (Likely candidates: `Command::DoubleBracket`, `Command::DoubleBracketCmd`, or a different shape.)

- [ ] **Step 11: Add the regression tests for variable-vs-keyword disambiguation**

Append these two tests to the same test module:

```rust
    #[test]
    fn function_as_assignment_var_still_works() {
        // `function=value` must still parse as a normal assignment,
        // NOT trigger the function-keyword path. The lexer's
        // assignment-prefix detection fires before keyword
        // classification, so the token reaches the parser as a
        // Word with an AssignPrefix part.
        let tokens = crate::lexer::tokenize("function=value").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        // At minimum it must NOT parse as a FunctionDef.
        assert!(
            !matches!(parsed.first, Command::FunctionDef { .. }),
            "function=value must not parse as a FunctionDef: {:?}",
            parsed.first,
        );
        // Specifically, it should parse as a Pipeline → Simple →
        // Assign with target Bare("function").
        let Command::Pipeline(p) = &parsed.first else {
            panic!("expected Pipeline, got {:?}", parsed.first);
        };
        let Command::Simple(SimpleCommand::Assign(assigns)) = &p.commands[0] else {
            panic!("expected Simple/Assign, got {:?}", p.commands[0]);
        };
        assert_eq!(assigns.len(), 1);
        match &assigns[0].target {
            AssignTarget::Bare(name) => assert_eq!(name, "function"),
            other => panic!("expected Bare target, got {other:?}"),
        }
    }

    #[test]
    fn function_in_arg_position_still_works() {
        // `echo function` — `function` is in argument position, not
        // command position, so it must NOT trigger the keyword arm.
        let tokens = crate::lexer::tokenize("echo function").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        assert!(
            !matches!(parsed.first, Command::FunctionDef { .. }),
            "echo function must not parse as a FunctionDef: {:?}",
            parsed.first,
        );
    }
```

Run: `cargo test --quiet function_keyword 2>&1 | tail -5 && cargo test --quiet function_as_assignment 2>&1 | tail -5 && cargo test --quiet function_in_arg 2>&1 | tail -5`
Expected: 13 keyword-form tests + 2 disambiguation tests + 2 double-bracket regressions = 15 new tests total, all pass.

- [ ] **Step 12: Full suite + clippy**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "After Task 1:", sum}'
cargo clippy --all-targets 2>&1 | tail -5
```
Expected: 2238 tests pass (2223 + 15 new). Clippy clean.

If any pre-existing test failed because of the POSIX-form `[[ ]]` body change, locate it. If it was asserting `parse_function_def` REJECTS `[[ ]]` body, update it to expect ACCEPT. Document in the commit message.

- [ ] **Step 13: Smoke-test from the binary**

```bash
echo 'function greet { echo "hello $1"; }
greet world
function inner() { echo nested; }
inner' | cargo run --quiet
```

Expected output:
```
hello world
nested
```

- [ ] **Step 14: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v77 task 1: parser for `function NAME { body }` keyword form

New Keyword::Function variant + parse-map entry in src/command.rs;
new parse_function_keyword_def helper handling optional `()` after
the name; shared is_function_body_shape predicate factored out so
BOTH the POSIX `name() body` form and the new keyword form go through
the same body-shape check.

The shared predicate also closes a pre-existing gap: huck's POSIX
form was rejecting `[[ ]]` bodies (`f() [[ -e /dev/null ]]`).
Bash accepts both forms; v77 brings both into parity. Verified by
regression test `function_posix_form_double_bracket_body`.

Lexer, executor, expand, and all other modules unchanged. function=value
continues to parse as an assignment because the lexer's assignment-prefix
detection (src/lexer.rs:497) fires before keyword classification.

15 new unit tests cover brace/parens/subshell/compound/newline bodies,
all 5 error paths, and the disambiguation cases (function as var name,
function in arg position).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Integration tests + bash-diff harness + docs

**Files:**
- Create: `tests/function_keyword_integration.rs` — 6 binary-driven integration tests.
- Create: `tests/scripts/function_keyword_diff_check.sh` — bash-diff harness, 6 fragments.
- Modify: `docs/bash-divergences.md` — flip M-09 to `[fixed v77]`; add two new `[deferred]` entries (relaxed name chars; definition-attached redirections); change-log entry.
- Modify: `README.md` — v77 iteration row.

**Goal:** End-to-end behavioral validation. The bash-diff harness asserts byte-identical behavior with bash 5.2 for the new keyword-form fragments.

### Steps

- [ ] **Step 1: Create the integration test file**

Create `tests/function_keyword_integration.rs`:

```rust
//! Integration tests for v77 `function NAME { ... }` keyword form.
//! Drives the `huck` binary via stdin and asserts on stdout/exit code.

use std::io::Write;
use std::process::{Command, Stdio};

fn run_huck(script: &str) -> (String, String, i32) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_huck"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn keyword_form_define_and_call() {
    let (out, _, code) = run_huck("function greet { echo hello; }\ngreet\n");
    assert_eq!(code, 0);
    assert_eq!(out, "hello\n");
}

#[test]
fn keyword_form_with_optional_parens() {
    let (out, _, code) = run_huck("function greet() { echo hi; }\ngreet\n");
    assert_eq!(code, 0);
    assert_eq!(out, "hi\n");
}

#[test]
fn keyword_form_positional_args_propagate() {
    let script = r#"function f { echo "$1-$2"; }
f alpha beta
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "alpha-beta\n");
}

#[test]
fn keyword_form_and_posix_form_are_equivalent() {
    let script = r#"function kf { echo "via $1"; }
pf() { echo "via $1"; }
kf keyword
pf posix
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "via keyword\nvia posix\n");
}

#[test]
fn keyword_form_redefine_via_posix_latest_wins() {
    let script = r#"function f { echo first; }
f() { echo second; }
f
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "second\n");
}

#[test]
fn keyword_form_subshell_body() {
    let script = r#"function f() ( echo subshell-body )
f
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "subshell-body\n");
}
```

- [ ] **Step 2: Run integration tests**

Run: `cargo test --test function_keyword_integration --quiet 2>&1 | tail -5`
Expected: 6 tests pass.

If any fail, investigate. The most likely cause is a parser bug uncovered by end-to-end execution (function body builds correctly at parse time but executes incorrectly).

- [ ] **Step 3: Create the bash-diff harness**

Create `tests/scripts/function_keyword_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash↔huck diff harness for the `function NAME { ... }`
# keyword form. Each fragment runs through `bash` and `huck` via stdin
# (huck has no -c flag); outputs must be byte-identical.

set -u

HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
if [[ ! -x "$HUCK_BIN" ]]; then
    echo "huck binary not found at $HUCK_BIN — run cargo build first" >&2
    exit 1
fi

PASS=0
FAIL=0

check() {
    local label="$1"
    local fragment="$2"
    local bash_out huck_out

    bash_out=$(printf '%s\n' "$fragment" | bash 2>&1; echo "EXIT:$?")
    huck_out=$(printf '%s\n' "$fragment" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")

    if [[ "$bash_out" == "$huck_out" ]]; then
        printf "PASS: %s\n" "$label"
        PASS=$((PASS + 1))
    else
        printf "FAIL: %s\n" "$label"
        diff <(echo "$bash_out") <(echo "$huck_out") | sed 's/^/    /'
        FAIL=$((FAIL + 1))
    fi
}

# 1. Basic keyword-form definition + call.
check "function-keyword brace body" \
      'function greet { echo hello; }; greet'

# 2. Keyword form with optional parens.
check "function-keyword with parens" \
      'function greet() { echo hi; }; greet'

# 3. Keyword form with subshell body.
check "function-keyword subshell body" \
      'function f () ( echo nested ); f'

# 4. Keyword form with positional args.
check "function-keyword positional args" \
      'function f { echo "$1-$2"; }; f alpha beta'

# 5. Keyword form with if body (no braces).
check "function-keyword if body" \
      'function f if true; then echo cond; fi; f'

# 6. Keyword and POSIX forms produce identical behavior.
check "function-keyword vs POSIX equivalence" \
      'function kf { echo via-$1; }; pf() { echo via-$1; }; kf x; pf y'

echo ""
echo "Total: $((PASS + FAIL)), Pass: $PASS, Fail: $FAIL"
exit $((FAIL > 0 ? 1 : 0))
```

Make it executable:

```bash
chmod +x tests/scripts/function_keyword_diff_check.sh
```

- [ ] **Step 4: Build the debug binary and run the harness**

```bash
cargo build --quiet
tests/scripts/function_keyword_diff_check.sh
```

Expected: `Total: 6, Pass: 6, Fail: 0`.

If any fragment fails, the diff is shown. Investigate. Common causes:
- Function body printing whitespace differently than bash (unlikely — fragments use simple `echo`).
- A trailing newline mismatch.
- The fragment relying on a huck-specific divergence.

Document any intentional divergences with `# DIVERGES: <why>` comments and exclude the fragment from the run.

- [ ] **Step 5: Update `docs/bash-divergences.md` — flip M-09**

Edit `docs/bash-divergences.md`. Find the M-09 line (around line 135):

```
- **M-09: `function name { … }` keyword form** — `[deferred]` medium. huck: only the POSIX `name() …` form. bash: also accepts the `function` keyword form.
```

Replace with:

```
- **M-09: `function name { … }` keyword form** — `[fixed v77]` medium. The `function` keyword is now reserved at command position; `function NAME { body }` and `function NAME () { body }` (optional parens) both parse to the same `Command::FunctionDef` AST as the POSIX `NAME() body` form. Names follow the POSIX identifier rule (`[A-Za-z_][A-Za-z0-9_]*`, no reserved keywords); both forms accept compound bodies (`{ }` / `( )` / `if` / `while` / `for` / `case` / `[[ ]]`). v77 also closes a pre-existing gap where the POSIX form rejected `[[ ]]` bodies — both forms now share a `is_function_body_shape` predicate. `function=value` continues to work as a variable assignment because the lexer's assignment-prefix detection fires before keyword classification. **Deferred** (new follow-on entries below): relaxed name characters (`.`/`-`/`+`/`:`) per bash 5; definition-attached redirections (`function NAME { body } > file` and `NAME() { body } > file` — neither form supports this in huck).
```

- [ ] **Step 6: Add the two new deferred entries**

Still in `docs/bash-divergences.md`, find the appropriate tier section for syntax/parser-level deferrals (look for where similar parser-related entries live; M-09 was in the medium-priority bucket). Add these two new entries:

```
- **M-09a: Relaxed function-name characters** — `[deferred]` low. huck restricts function names to POSIX identifiers (`[A-Za-z_][A-Za-z0-9_]*`) in BOTH the `name() body` and `function name body` forms. Bash 5 accepts `.`, `-`, `+`, `:` and other non-POSIX-identifier characters when the function is defined via the keyword form (`function foo.bar { :; }`). Rarely used in practice.
- **M-09b: Definition-attached redirections** — `[deferred]` low. Both function-definition forms (`name() body > file` and `function name body > file`) currently reject trailing redirections. Bash allows attaching redirections to the function definition itself, taking effect at every call. Affects both forms equally.
```

- [ ] **Step 7: Add the change-log entry**

Still in `docs/bash-divergences.md`, find the change-log section at the bottom of the file. Add a new dated entry:

```
- **2026-06-02**: M-09 (`function NAME { ... }` keyword form) shipped as v77. New `Keyword::Function` variant + parse-map entry in `src/command.rs`. New `parse_function_keyword_def` helper in `parse_command`'s dispatch. Shared `is_function_body_shape` predicate factored out and used by both form-parsers; closes a pre-existing gap where the POSIX `name() body` form rejected `[[ ]]` bodies. Lexer, executor, expand unchanged. `function=value` continues to work as an assignment (lexer assignment-prefix detection precedes keyword classification). 15 unit tests + 6 integration tests + 6 bash-diff fragments byte-identical to bash 5.2. Two new `[deferred]` entries added (M-09a relaxed name chars, M-09b definition-attached redirections — both affect both function-definition forms equally).
```

- [ ] **Step 8: Update `README.md`**

Edit `README.md`. Find the iteration table (search for `| v76`). Add a v77 row immediately below v76, matching the column structure:

```
| v77 | 2026-06-02 | Function keyword form (`function NAME { ... }`) | M-09 |
```

If a test-count line exists elsewhere in the README, update it from 2223 to the new total. (`cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print sum}'` will report the current total — expected to be 2244 after Task 2.)

- [ ] **Step 9: Run the full suite, clippy, and harness one more time**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "After Task 2:", sum}'
cargo clippy --all-targets 2>&1 | tail -5
cargo build --quiet && tests/scripts/function_keyword_diff_check.sh
```

Expected:
- 2244 tests pass (2238 + 6 integration).
- Clippy clean.
- Bash-diff: 6/6 byte-identical.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v77 task 2: integration tests + bash-diff harness + docs

* tests/function_keyword_integration.rs (new): 6 binary-driven tests
  covering keyword-form define+call, optional-parens form, positional
  args, equivalence with POSIX form, redefine-latest-wins, subshell body.
* tests/scripts/function_keyword_diff_check.sh (new): huck's 5th
  bash-diff harness; 6 fragments byte-identical to bash 5.2.
* docs/bash-divergences.md: M-09 flipped from [deferred] to [fixed v77]
  with full surface description. Two new [deferred] entries: M-09a
  (relaxed function-name characters) and M-09b (definition-attached
  redirections — affects both forms). Change-log entry.
* README.md: v77 iteration row + test count bump.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final review checklist

Before merging the branch, the controller dispatches a final code-reviewer over the whole branch diff via `superpowers:requesting-code-review`. Specific things to verify:

- [ ] **All 2244 tests pass on the branch.**
- [ ] **Clippy clean (`cargo clippy --all-targets`).**
- [ ] **The bash-diff harness reports 6/6.**
- [ ] **`function=value` still parses as an assignment** (regression test `function_as_assignment_var_still_works`).
- [ ] **`echo function` still echoes the literal** (regression test `function_in_arg_position_still_works`).
- [ ] **POSIX `f() [[ ... ]]` now parses** (regression test `function_posix_form_double_bracket_body`).
- [ ] **`function f` (no body) reports `UnterminatedFunction`, not a generic syntax error.**
- [ ] **`function if { :; }` reports `FunctionName`, not `UnterminatedFunction` or `UnexpectedKeyword`.**
- [ ] **Both function-definition forms produce identical `Command::FunctionDef` AST** — no executor-level differentiation introduced.
- [ ] **No lexer, executor, expand, builtin, or shell-state changes.** `git diff main..HEAD -- 'src/*.rs' ':!src/command.rs'` should be empty.

## Merge

After review fixes land, merge with `--no-ff`:

```bash
git checkout main
git merge --no-ff v77-function-keyword -m "Merge v77: function keyword form (M-09)"
git push origin main
git branch -d v77-function-keyword
```

Then update the long-running memory files (`huck_iterations.md` + `MEMORY.md`) per the iteration workflow.
