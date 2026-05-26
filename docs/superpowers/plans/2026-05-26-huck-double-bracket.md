# v30: `[[ ]]` Extended Test — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement bash's `[[ ]]` extended test — pattern matching (`==`/`!=` with glob RHS), regex (`=~`), lex compare (`<`/`>`), integer compare (`-eq` et al.), file/string tests, combinators (`!`/`&&`/`||`/`( )`), no word-splitting / no pathname expansion on operands. Closes M-14.

**Architecture:** `[[` and `]]` become reserved keywords in the lexer. New `Command::DoubleBracket(Box<TestExpr>)` AST variant with its own expression tree (Unary / Binary / Regex / Not / And / Or). Parser uses Pratt-style precedence (`||` < `&&` < `!` < primary). Executor evaluates the tree to `bool` and returns `Continue(0|1|2)`. Pattern matching for `==`/`!=` reuses v21's `expand_pattern`; regex via the `regex` crate.

**Tech Stack:** Rust 1.95; existing huck modules + new `regex = "1.10"` dependency for `=~`.

**Spec:** `docs/superpowers/specs/2026-05-26-huck-double-bracket-design.md`.

**Branch:** `v30-double-bracket` (off `main` at commit `a796c3e`).

**Baseline:** 1147 tests pass, 0 clippy warnings.

---

## File structure

- `Cargo.toml` — add `regex = "1.10"` dependency.
- `src/lexer.rs` — `Keyword::DoubleBracketOpen` (`[[`) and `Keyword::DoubleBracketClose` (`]]`); keyword recognition at word-start position.
- `src/command.rs` — `Command::DoubleBracket(Box<TestExpr>)` variant; new `TestExpr`/`TestUnaryOp`/`TestBinaryOp` types (inline or in a new `src/test_expr.rs` module — implementer's choice); parser `parse_double_bracket` with Pratt-style precedence.
- `src/executor.rs` — `run_double_bracket` + `eval_test_expr` (recursive evaluator); shared file-test logic factored from `test_builtin` or duplicated minimally.
- `tests/double_bracket_integration.rs` (new) — end-to-end coverage.
- `docs/bash-divergences.md` — M-14 fixed; doc the regex-engine and locale divergences.
- `README.md` — v30 row.

---

## Task 1: Lexer + AST + parser (front-end) + regex dep

After this task, `[[ ... ]]` parses into the AST. Executor `unreachable!`s on the new `DoubleBracket` variant.

**Files:** `Cargo.toml`, `src/lexer.rs`, `src/command.rs` (or `src/test_expr.rs`), `src/executor.rs` (placeholder).

- [ ] **Step 1: Snapshot baseline**

```bash
cd /home/john/projects/shuck
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: `1147 0` and `0`.

- [ ] **Step 2: Add `regex` crate dependency**

In `Cargo.toml`, add to `[dependencies]`:
```toml
regex = "1.10"
```

Run `cargo build` to fetch + verify it compiles.

- [ ] **Step 3: Add Keyword variants**

In `src/lexer.rs`, extend the `Keyword` enum (or however reserved words are represented):
```rust
pub enum Keyword {
    // existing...
    DoubleBracketOpen,    // [[
    DoubleBracketClose,   // ]]
}
```

Update the lexer's keyword-recognition path to emit these when a word at command-start position is exactly `[[` or `]]`. Look at how `if`/`while`/`{` are recognized; mirror that pattern.

- [ ] **Step 4: Failing lexer tests**

In `src/lexer.rs::tests`:

```rust
#[test]
fn tokenize_double_bracket_open_at_word_start() {
    let tokens = tokenize("[[").unwrap();
    // Adjust to whatever Token variant the implementer uses for keywords —
    // either Token::Op(Operator::DoubleBracketOpen) or Token::Keyword(Keyword::DoubleBracketOpen)
    // or similar. Use the existing keyword-token shape.
    assert!(matches!(tokens[0], Token::Op(Operator::DoubleBracketOpen) | Token::Word(_)));
    // The above is lenient because the exact token shape depends on
    // huck's existing keyword representation. Replace with the actual
    // expected variant once you've inspected the lexer.
}

#[test]
fn tokenize_double_bracket_close() {
    let tokens = tokenize("]]").unwrap();
    // Same shape consideration as above.
}

#[test]
fn tokenize_double_bracket_not_at_word_start_is_literal() {
    let tokens = tokenize("cmd[[foo]]").unwrap();
    // The [[ is mid-word; should NOT trigger keyword recognition.
    assert!(matches!(&tokens[0], Token::Word(w) if /* contains "cmd[[foo]]" as literal */ true));
}
```

(The implementer should adjust assertions to match huck's actual keyword-token shape. Run these to confirm they fail before implementing.)

- [ ] **Step 5: Implement lexer keyword recognition**

In `src/lexer.rs::tokenize` (or the keyword-detection helper it calls), add `[[` and `]]` to the reserved-words check, alongside `if`/`while`/`then`/`else`/`fi`/`{`/`}`/etc. The check fires at WORD-START position (when `has_token == false` AND the next chars form the keyword exactly).

Verify the 3 lexer tests pass.

- [ ] **Step 6: Add AST types**

In `src/command.rs` (or new `src/test_expr.rs` module):

```rust
pub enum TestExpr {
    Unary { op: TestUnaryOp, operand: Word },
    Binary { op: TestBinaryOp, lhs: Word, rhs: Word },
    Regex { lhs: Word, pattern: Word },
    Not(Box<TestExpr>),
    And(Box<TestExpr>, Box<TestExpr>),
    Or(Box<TestExpr>, Box<TestExpr>),
}

pub enum TestUnaryOp {
    FileExists,       // -e
    IsRegFile,        // -f
    IsDir,            // -d
    IsReadable,       // -r
    IsWritable,       // -w
    IsExecutable,     // -x
    IsNonEmpty,       // -s
    IsSymlink,        // -L
    StringNonEmpty,   // -n
    StringEmpty,      // -z
}

pub enum TestBinaryOp {
    StringEq,    // == or = (bash alias)
    StringNe,    // !=
    StringLt,    // < (lex)
    StringGt,    // > (lex)
    IntEq,       // -eq
    IntNe,       // -ne
    IntLt,       // -lt
    IntGt,       // -gt
    IntLe,       // -le
    IntGe,       // -ge
}
```

And the Command variant:
```rust
pub enum Command {
    // existing...
    DoubleBracket(Box<TestExpr>),    // NEW
}
```

- [ ] **Step 7: Failing parser tests**

In `src/command.rs::tests`:

```rust
#[test]
fn parse_dbracket_string_eq_literal() {
    let tokens = tokenize("[[ a == b ]]").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::DoubleBracket(expr) = parsed.first else {
        panic!("expected DoubleBracket, got {:?}", parsed.first)
    };
    let TestExpr::Binary { op, .. } = &*expr else { panic!() };
    assert!(matches!(op, TestBinaryOp::StringEq));
}

#[test]
fn parse_dbracket_string_eq_single_equals() {
    // [[ a = b ]] is bash alias for [[ a == b ]].
    let tokens = tokenize("[[ a = b ]]").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty");
    let Command::DoubleBracket(expr) = parsed.first else { panic!() };
    let TestExpr::Binary { op, .. } = &*expr else { panic!() };
    assert!(matches!(op, TestBinaryOp::StringEq));
}

#[test]
fn parse_dbracket_regex() {
    let tokens = tokenize("[[ s =~ ^foo$ ]]").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty");
    let Command::DoubleBracket(expr) = parsed.first else { panic!() };
    assert!(matches!(&*expr, TestExpr::Regex { .. }));
}

#[test]
fn parse_dbracket_integer_compare() {
    let tokens = tokenize("[[ 5 -eq 5 ]]").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty");
    let Command::DoubleBracket(expr) = parsed.first else { panic!() };
    let TestExpr::Binary { op, .. } = &*expr else { panic!() };
    assert!(matches!(op, TestBinaryOp::IntEq));
}

#[test]
fn parse_dbracket_unary_file() {
    let tokens = tokenize("[[ -f /tmp ]]").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty");
    let Command::DoubleBracket(expr) = parsed.first else { panic!() };
    let TestExpr::Unary { op, .. } = &*expr else { panic!() };
    assert!(matches!(op, TestUnaryOp::IsRegFile));
}

#[test]
fn parse_dbracket_unary_string_empty() {
    let tokens = tokenize("[[ -z foo ]]").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty");
    let Command::DoubleBracket(expr) = parsed.first else { panic!() };
    let TestExpr::Unary { op, .. } = &*expr else { panic!() };
    assert!(matches!(op, TestUnaryOp::StringEmpty));
}

#[test]
fn parse_dbracket_not() {
    let tokens = tokenize("[[ ! -f /tmp/x ]]").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty");
    let Command::DoubleBracket(expr) = parsed.first else { panic!() };
    let TestExpr::Not(inner) = &*expr else { panic!() };
    assert!(matches!(&**inner, TestExpr::Unary { .. }));
}

#[test]
fn parse_dbracket_and() {
    let tokens = tokenize("[[ -f a && -r a ]]").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty");
    let Command::DoubleBracket(expr) = parsed.first else { panic!() };
    assert!(matches!(&*expr, TestExpr::And(_, _)));
}

#[test]
fn parse_dbracket_or() {
    let tokens = tokenize("[[ x == a || x == b ]]").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty");
    let Command::DoubleBracket(expr) = parsed.first else { panic!() };
    assert!(matches!(&*expr, TestExpr::Or(_, _)));
}

#[test]
fn parse_dbracket_grouped() {
    let tokens = tokenize("[[ ( a == a || b == c ) && d == d ]]").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty");
    let Command::DoubleBracket(expr) = parsed.first else { panic!() };
    // Top-level should be And with the first operand being the grouped Or.
    let TestExpr::And(lhs, _) = &*expr else { panic!() };
    assert!(matches!(&**lhs, TestExpr::Or(_, _)));
}

#[test]
fn parse_dbracket_empty_errors() {
    let tokens = tokenize("[[ ]]").unwrap();
    assert!(parse(tokens).is_err());
}

#[test]
fn parse_dbracket_unterminated_errors() {
    let tokens = tokenize("[[ x == y").unwrap();
    assert!(parse(tokens).is_err());
}
```

Run: expect failures (parser doesn't have DoubleBracket dispatch yet).

- [ ] **Step 8: Implement Pratt-style parser**

In `src/command.rs`, add `parse_double_bracket(iter) -> Result<Command, ParseError>`. The dispatch from `parse_command` recognizes the `DoubleBracketOpen` keyword and routes to this.

Pratt levels (low → high):
```rust
fn parse_test_or(iter) -> Result<TestExpr, ParseError> {
    let lhs = parse_test_and(iter)?;
    while peek is `||` { iter.next(); let rhs = parse_test_and(iter)?; lhs = TestExpr::Or(Box::new(lhs), Box::new(rhs)); }
    Ok(lhs)
}
fn parse_test_and(iter) -> Result<TestExpr, ParseError> { /* similar with && */ }
fn parse_test_not(iter) -> Result<TestExpr, ParseError> {
    if peek is `!` { iter.next(); let inner = parse_test_not(iter)?; Ok(TestExpr::Not(Box::new(inner))) }
    else { parse_test_primary(iter) }
}
fn parse_test_primary(iter) -> Result<TestExpr, ParseError> {
    if peek is `(` {
        iter.next();
        let inner = parse_test_or(iter)?;
        expect `)`;
        Ok(inner)
    } else {
        // Read first operand Word; peek for binary op; possibly read rhs.
        // Recognise unary ops (-f, -z, etc.) when the first Word starts with `-`
        // and matches a known unary op name.
        parse_test_atom(iter)
    }
}
fn parse_test_atom(iter) -> Result<TestExpr, ParseError> {
    let first = iter.next();   // peek and consume; could be Word(-f) or just a Word
    if first is unary-op Word (starts with - and matches -f/-d/etc.):
        let operand = next_word(iter)?;
        Ok(TestExpr::Unary { op, operand })
    else:
        let lhs = first as Word;
        let op_token = next_token(iter)?;   // expect binary op
        let rhs = next_word(iter)?;
        match op_token:
            "==" | "=" => Ok(Binary { StringEq, lhs, rhs }),
            "!=" => Ok(Binary { StringNe, lhs, rhs }),
            "=~" => Ok(Regex { lhs, pattern: rhs }),
            "<" => Ok(Binary { StringLt, lhs, rhs }),
            ">" => Ok(Binary { StringGt, lhs, rhs }),
            "-eq" => Ok(Binary { IntEq, lhs, rhs }),
            ... etc.
            _ => Err(ParseError::TestExprBadOperator(...))
}
```

**Operator-token recognition**: inside `[[ ]]`, tokens like `==`, `!=`, `=~`, `<`, `>` arrive as ordinary Word tokens (because the lexer doesn't have them as separate operators). Recognize them by inspecting the Word's single-Literal text. `-eq`/`-ne`/etc. are similarly Word tokens. `&&`/`||`/`!`/`(`/`)` may or may not be existing Operator tokens — handle either case.

Operand reading: at this level, `next_word(iter)` is "consume one Word token", returning ParseError on missing.

End-of-expression detection: `parse_test_or` (and helpers) stop when they see `Keyword::DoubleBracketClose` or a closing `)` (when called from inside grouping).

Implementer note: huck doesn't currently have a `!` operator token (per the v30 spec, may need to add). Look at how `&&`/`||` are tokenized today; `!` may already be lexed as part of a Word, in which case parse_test_not recognizes it via word inspection.

After `parse_test_or` returns, `parse_double_bracket` consumes `Keyword::DoubleBracketClose` and returns `Command::DoubleBracket(Box::new(expr))`.

Empty body (`[[ ]]`): `parse_test_or` immediately fails because `parse_test_atom` finds the close-bracket where a primary was expected. Return `ParseError::EmptyDoubleBracket` (or whatever variant fits — could reuse a generic `UnexpectedToken`).

Unterminated (`[[ x == y` with no `]]`): `parse_double_bracket` exhausts iter looking for `]]`. Return `ParseError::UnterminatedDoubleBracket`.

- [ ] **Step 9: Wire into `parse_command` dispatcher**

Where `parse_command` dispatches on keywords (`if` → `parse_if`, `while` → `parse_while`, `{` → `parse_brace_group`, etc.), add an arm for `Keyword::DoubleBracketOpen` → `parse_double_bracket`. Place it consistently with the existing keyword arms.

- [ ] **Step 10: Add executor `unreachable!` placeholder**

In `src/executor.rs::run_command`, add an arm:
```rust
Command::DoubleBracket(_) => {
    unreachable!("Command::DoubleBracket execution lands in Task 2; \
                  parser produces this now but the executor doesn't route it yet")
}
```

Similarly anywhere `Command` is exhaustively matched.

- [ ] **Step 11: Verify**

```bash
cargo build 2>&1 | tail -3
cargo test --bin huck parse_dbracket tokenize_double_bracket 2>&1 | tail -20
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: ~15 parser tests + 3 lexer tests pass; full suite ~1165, 0 fails, 0 warnings.

- [ ] **Step 12: Commit**

```bash
git add -A
git commit -m "ast+lex+parse: \`[[ ]]\` extended test syntax + regex dep

Keyword::DoubleBracketOpen/Close at command-start; Command::DoubleBracket
variant wrapping a TestExpr tree (Unary/Binary/Regex/Not/And/Or).
Pratt-style precedence parser: || < && < ! < primary. RHS operand
recognition by Word inspection (==, !=, =~, <, >, -eq, etc.).
Cargo.toml gains regex = \"1.10\" for =~ runtime (used in Task 2).
Executor stubs unreachable!() until Task 2 wires evaluation."
```

---

## Task 2: Executor — `eval_test_expr` + `run_double_bracket`

After this task, `[[ ... ]]` actually evaluates and returns the right exit status.

**Files:** `src/executor.rs`, possibly `src/test_builtin.rs` (if factoring shared file-test helpers).

- [ ] **Step 1: Failing smoke test (integration)**

Create `tests/double_bracket_integration.rs`:
```rust
//! End-to-end tests for v30 [[ ]] extended test.

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
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn dbracket_string_eq_true() {
    let (out, _) = run("[[ hello == hello ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}
```

Run: expect failure (executor `unreachable!`s).

- [ ] **Step 2: Add `run_double_bracket` + `eval_test_expr`**

In `src/executor.rs`:

```rust
fn run_double_bracket(expr: &TestExpr, shell: &mut Shell) -> ExecOutcome {
    match eval_test_expr(expr, shell) {
        Ok(true) => ExecOutcome::Continue(0),
        Ok(false) => ExecOutcome::Continue(1),
        Err(msg) => {
            eprintln!("huck: [[: {msg}");
            ExecOutcome::Continue(2)
        }
    }
}

fn eval_test_expr(expr: &TestExpr, shell: &mut Shell) -> Result<bool, String> {
    match expr {
        TestExpr::Unary { op, operand } => {
            let s = expand_assignment(operand, shell);
            Ok(eval_unary(*op, &s))
        }
        TestExpr::Binary { op, lhs, rhs } => {
            let l = expand_assignment(lhs, shell);
            eval_binary(*op, &l, rhs, shell)
        }
        TestExpr::Regex { lhs, pattern } => {
            let l = expand_assignment(lhs, shell);
            let p = expand_assignment(pattern, shell);
            let re = regex::Regex::new(&p).map_err(|e| format!("regex error: {e}"))?;
            Ok(re.is_match(&l))
        }
        TestExpr::Not(inner) => eval_test_expr(inner, shell).map(|b| !b),
        TestExpr::And(a, b) => {
            if eval_test_expr(a, shell)? { eval_test_expr(b, shell) } else { Ok(false) }
        }
        TestExpr::Or(a, b) => {
            if eval_test_expr(a, shell)? { Ok(true) } else { eval_test_expr(b, shell) }
        }
    }
}
```

`eval_unary` and `eval_binary` are helpers. For `eval_unary`:
```rust
fn eval_unary(op: TestUnaryOp, s: &str) -> bool {
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    match op {
        TestUnaryOp::FileExists => fs::symlink_metadata(s).is_ok(),
        TestUnaryOp::IsRegFile => fs::metadata(s).is_ok_and(|m| m.is_file()),
        TestUnaryOp::IsDir => fs::metadata(s).is_ok_and(|m| m.is_dir()),
        TestUnaryOp::IsReadable => /* unix mode check */,
        // ... etc, mirror src/test_builtin.rs's existing file-test helpers ...
        TestUnaryOp::StringNonEmpty => !s.is_empty(),
        TestUnaryOp::StringEmpty => s.is_empty(),
    }
}
```

If `test_builtin` already has these as standalone helpers (not tied to its arg-parser), extract them or call directly. Otherwise duplicate the small file-test logic — keep it minimal.

For `eval_binary`:
```rust
fn eval_binary(op: TestBinaryOp, lhs: &str, rhs_word: &Word, shell: &mut Shell)
    -> Result<bool, String>
{
    match op {
        TestBinaryOp::StringEq | TestBinaryOp::StringNe => {
            let pattern_str = expand_pattern(rhs_word, shell);
            let pat = glob::Pattern::new(&pattern_str)
                .map_err(|e| format!("bad pattern: {e}"))?;
            let matched = pat.matches(lhs);
            Ok(if matches!(op, TestBinaryOp::StringEq) { matched } else { !matched })
        }
        TestBinaryOp::StringLt | TestBinaryOp::StringGt => {
            let rhs = expand_assignment(rhs_word, shell);
            Ok(match op {
                TestBinaryOp::StringLt => lhs < rhs.as_str(),
                TestBinaryOp::StringGt => lhs > rhs.as_str(),
                _ => unreachable!(),
            })
        }
        TestBinaryOp::IntEq | TestBinaryOp::IntNe | TestBinaryOp::IntLt
        | TestBinaryOp::IntGt | TestBinaryOp::IntLe | TestBinaryOp::IntGe => {
            let rhs = expand_assignment(rhs_word, shell);
            let l: i64 = lhs.parse().map_err(|_| format!("bad integer: {lhs}"))?;
            let r: i64 = rhs.parse().map_err(|_| format!("bad integer: {rhs}"))?;
            Ok(match op {
                TestBinaryOp::IntEq => l == r,
                TestBinaryOp::IntNe => l != r,
                TestBinaryOp::IntLt => l < r,
                TestBinaryOp::IntGt => l > r,
                TestBinaryOp::IntLe => l <= r,
                TestBinaryOp::IntGe => l >= r,
                _ => unreachable!(),
            })
        }
    }
}
```

- [ ] **Step 3: Wire into `run_command`**

Replace the Task 1 `unreachable!` arm:
```rust
Command::DoubleBracket(expr) => run_double_bracket(expr, shell),
```

- [ ] **Step 4: Verify smoke test passes + full suite**

```bash
cargo test --test double_bracket_integration dbracket_string_eq_true 2>&1 | tail -10
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: smoke test passes; full suite ~1166, 0 fails, 0 warnings.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "exec: eval_test_expr + run_double_bracket for \`[[ ]]\`

Recursive evaluator. Unary ops use file-system metadata or string
emptiness. Binary string ops via expand_pattern (matches v21 case-pattern
semantics — RHS quoted → literal compare; unquoted → glob pattern).
Lex compare \`<\`/\`>\` is byte-order (locale not honored — documented).
Integer compare parses i64 + numeric ordering. Regex via Rust regex
crate (RE2-style; no lookbehind/lookahead — documented). Short-circuit
&&/|| / negation /. Exit codes: 0 true / 1 false / 2 syntax-or-eval-error."
```

---

## Task 3: Full integration test suite

Cover every spec test-table row.

**Files:** `tests/double_bracket_integration.rs`.

- [ ] **Step 1: Add remaining integration tests**

Append to `tests/double_bracket_integration.rs` (smoke test from Task 2 already covers `dbracket_string_eq_true`):

```rust
#[test]
fn dbracket_string_eq_false_sets_status() {
    let (out, _) = run("[[ hello == world ]]; echo $?\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "1"), "got: {out}");
}

#[test]
fn dbracket_pattern_match_glob() {
    let (out, _) = run("[[ hello.txt == *.txt ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_quoted_rhs_is_literal() {
    let (out, _) = run("[[ hello.txt == \"*.txt\" ]] || echo no\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "no"), "got: {out}");
}

#[test]
fn dbracket_regex_match() {
    let (out, _) = run("[[ hello42 =~ ^[a-z]+[0-9]+$ ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_regex_invalid_errors() {
    let (out, err) = run("[[ x =~ \"[\" ]]; echo $?\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "2"), "got out: {out} err: {err}");
}

#[test]
fn dbracket_int_eq() {
    let (out, _) = run("[[ 5 -eq 5 ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_int_gt() {
    let (out, _) = run("[[ 10 -gt 3 ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_int_bad() {
    let (out, _) = run("[[ abc -eq 5 ]]; echo $?\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "2"), "got: {out}");
}

#[test]
fn dbracket_file_test_existing() {
    let (out, _) = run("[[ -f /etc/hostname ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_file_test_missing() {
    let (out, _) = run("[[ ! -f /definitely/not/here ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_string_empty_z() {
    let (out, _) = run("[[ -z \"\" ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_string_nonempty_n() {
    let (out, _) = run("[[ -n hello ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_and_short_circuit_avoids_error() {
    // Second test would error if reached, but && short-circuits on false.
    let (out, _) = run("[[ -f /no/such && -r /no/such ]]; echo $?\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "1"), "got: {out}");
}

#[test]
fn dbracket_or_short_circuit() {
    let (out, _) = run("[[ hello == hello || -f /no/such ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_grouped_precedence() {
    let (out, _) = run("[[ ( a == a || b == c ) && d == d ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_no_word_splitting() {
    let (out, _) = run("FOO=\"a b\"\n[[ $FOO == \"a b\" ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_in_if() {
    let (out, _) = run("if [[ -f /etc/hostname ]]; then echo ok; fi\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_chained_with_and() {
    let (out, _) = run("[[ a == a ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_with_inline_assignment() {
    let (out, _) = run("FOO=hi [[ $FOO == hi ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_in_subshell() {
    let (out, _) = run("([[ a == a ]]) && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}
```

- [ ] **Step 2: Verify**

```bash
cargo test --test double_bracket_integration 2>&1 | tail -25
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: ~20 integration tests pass; full suite ~1186, 0 fails, 0 warnings.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "test: full v30 \`[[ ]]\` integration coverage

20 tests covering string compare (literal + pattern + quoted-literal),
regex (match + invalid), integer compare (eq/gt + bad), file/string
tests, short-circuit && / ||, grouping/precedence, no-word-splitting,
composition with if / inline-assignment / subshell."
```

---

## Task 4: Docs

Mark M-14 fixed; document the regex/locale divergences; add v30 row.

**Files:** `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Update `docs/bash-divergences.md`**

Find M-14. Replace its body to:
```markdown
- **M-14: `[[ … ]]` extended test** — `[fixed (2026-05-26)]` high. Now supported: pattern `==`/`!=` (RHS glob; quoted → literal), regex `=~` (via Rust `regex` crate — RE2-style; no lookbehind/lookahead), lexicographic `<`/`>` (byte-order; no LC_COLLATE), integer `-eq`/`-ne`/`-lt`/`-gt`/`-le`/`-ge`, file tests (`-f`/`-d`/`-r`/`-w`/`-x`/`-e`/`-s`/`-L`), string tests (`-n`/`-z`), combinators (`!`, `&&`, `||`, grouping `()`). No word-splitting or pathname expansion on operands per bash. Out of scope: `-v var` (var-set), `-nt`/`-ot`/`-ef` (file age/identity), bash arrays.
```

Update Tier 2 count (drops by 1).

Optional: add a Tier 4 entry for the regex-engine divergence if it warrants explicit tracking:
```markdown
### L-09: Regex `=~` is RE2-style, not POSIX ERE
- **Status**: intentional (v30)
- **Severity**: low
- **huck**: `[[ $s =~ regex ]]` uses the Rust `regex` crate (RE2-based). No lookbehind / lookahead (`(?<=...)`, `(?=...)`); minor syntax differences from POSIX ERE for some edge cases (e.g., `(?:...)` non-capturing groups are supported in both, but bash's POSIX-mode is stricter).
- **bash**: POSIX ERE. Has its own quirks.
- **Why intentional**: `regex` is a mature, fast, well-maintained Rust crate. Implementing POSIX-ERE-faithful regex isn't worth the cost for the rare divergences. Most real-world shell-regex usage works identically.
- **Workaround**: if a script relies on POSIX-ERE-specific features, fall back to `grep -E "pattern"` (which uses libc's POSIX ERE).
```

(Use whatever ID is next in the L-/I- sequence.)

Add change-log entry:
```markdown
- **2026-05-26**: M-14 (`[[ ]]` extended test) shipped as v30. Regex engine is `regex` crate (RE2-style; L-09 documents the divergence from POSIX ERE).
```

- [ ] **Step 2: Update `README.md`**

Add v30 row to the status table:
```
| v30       | `[[ ]]` extended test (pattern/regex/int/file/combinators) |
```

- [ ] **Step 3: Verify**

```bash
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: same counts as Task 3 (no code changes); 0 fails, 0 warnings.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "docs: M-14 fixed; L-09 regex-engine divergence; v30 in README"
```

---

## Final verification (no separate task)

```bash
cargo build 2>&1 | tail -3
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print "Pass: " p ", Fail: " (f+0)}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```

Acceptance: 0 failures, 0 warnings, clean build. Then dispatch the final cross-cutting opus review. After approval:

```bash
git -C /home/john/projects/shuck checkout main
git -C /home/john/projects/shuck merge --ff-only v30-double-bracket
git -C /home/john/projects/shuck branch -d v30-double-bracket
```

---

## Self-review checklist

1. **Spec coverage**: every spec section maps to a task.
   - Lexer + AST + Parser → Task 1.
   - Executor → Task 2.
   - Edge cases + Tests → Task 3.
   - Doc updates → Task 4.

2. **Placeholders**: parser implementation in Task 1 Step 8 is sketched in pseudocode; the implementer reads the existing `parse_if`/`parse_while` for style cues. Recognition of `==`/`!=`/`=~`/etc. inside `[[ ]]` is documented as "by Word inspection" — the implementer should pick a clean helper.

3. **Type consistency**: `Command::DoubleBracket(Box<TestExpr>)`; `TestExpr` is a recursive tree with `TestUnaryOp` and `TestBinaryOp` enums. Used consistently across parser (build), executor (eval), tests (destructure).

4. **Order dependencies**:
   - Task 1 must precede everything.
   - Task 2 depends on Task 1.
   - Task 3 depends on Task 2 (integration tests need execution).
   - Task 4 is independent of code.

5. **Backward-compat callouts**: zero breaking changes. `[[` was a parse error pre-v30; now a valid command. Existing tests don't exercise it.
