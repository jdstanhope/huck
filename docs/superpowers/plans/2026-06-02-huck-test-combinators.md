# huck v75 — Test Combinators Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task is implemented by a fresh subagent, with spec-compliance review and code-quality review between tasks.

**Goal:** Add bash-compatible AND (`-a`), OR (`-o`), and parenthesized grouping `( ... )` to the `test` / `[` builtin.

**Architecture:** Refactor the existing if-else cascade in `evaluate` into a dispatcher that keeps POSIX's 0-4-arg short-form algorithm for backward compatibility AND adds a recursive-descent parser for 5+ args. 2-4-arg calls try the short-form first; if it returns `Err`, fall through to the parser (catches `( ... )` forms that the short-form rejects). Grammar: `EXPR ::= EXPR -o ANDEXPR | ANDEXPR`; `ANDEXPR ::= ANDEXPR -a UNEXPR | UNEXPR`; `UNEXPR ::= ! UNEXPR | PRIMARY`; `PRIMARY ::= ( EXPR ) | unary word | word binop word | word`.

**Tech Stack:** Rust 1.85+; no new dependencies.

**Branch:** `v75-test-combinators` (create from `main` in Preamble P.1).

**Spec:** `docs/superpowers/specs/2026-06-02-huck-test-combinators-design.md`.

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
git checkout -b v75-test-combinators
```

Expected: `Switched to a new branch 'v75-test-combinators'`.

- [ ] **Step 3: Confirm baseline tests pass**

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "Baseline:", sum}'`
Expected: 2118 (current main).

- [ ] **Step 4: Confirm clippy is clean**

Run: `cargo clippy --all-targets 2>&1 | tail -3`
Expected: `Finished` no warnings.

---

## File-structure map

| File | Responsibility | Tasks |
|------|----------------|-------|
| `src/test_builtin.rs` | Refactor `evaluate`; add `-a` unary; add `Parser` struct + parse_* methods; add ~20 unit tests | 1, 2 |
| `tests/test_combinators_integration.rs` | 6 binary-driven combinator tests (new) | 3 |
| `tests/scripts/test_combinators_diff_check.sh` | New bash-diff harness, 8 fragments | 3 |
| `docs/bash-divergences.md` | M-25 entry update + change-log entry | 3 |
| `README.md` | v75 row | 3 |

---

## Task 1: Refactor evaluate + add `-a` unary file-exists

**Files:**
- Modify: `src/test_builtin.rs` — extract `evaluate_short_form` helper; add `-a` to is_unary_op and apply_unary

**Goal:** Pure refactor (no user-visible behavior change beyond `-a` becoming a unary file-exists alias). Lays groundwork for Task 2's parser dispatch.

### Steps

- [ ] **Step 1: Extract `evaluate_short_form` from the existing `evaluate`**

Edit `src/test_builtin.rs`. Currently `evaluate` looks like (around line 8-39):

```rust
pub fn evaluate(args: &[String]) -> Result<bool, String> {
    match args.len() {
        0 => Ok(false),
        1 => Ok(!args[0].is_empty()),
        2 => {
            if args[0] == "!" {
                negate(evaluate(&args[1..2]))
            } else if is_unary_op(&args[0]) {
                apply_unary(&args[0], &args[1])
            } else {
                Err(format!("{}: unary operator expected", args[0]))
            }
        }
        3 => {
            if is_binary_op(&args[1]) {
                apply_binary(&args[1], &args[0], &args[2])
            } else if args[0] == "!" {
                negate(evaluate(&args[1..3]))
            } else {
                Err(format!("{}: binary operator expected", args[1]))
            }
        }
        4 => {
            if args[0] == "!" {
                negate(evaluate(&args[1..4]))
            } else {
                Err("too many arguments".to_string())
            }
        }
        _ => Err("too many arguments".to_string()),
    }
}
```

Replace with:

```rust
pub fn evaluate(args: &[String]) -> Result<bool, String> {
    // POSIX § 4.62 short-form for 0-1 args: required for correctness
    // (e.g. `[ -a ]` is true — a 1-arg call returns truthiness of the
    // string, not a unary-op application).
    match args.len() {
        0 => return Ok(false),
        1 => return Ok(!args[0].is_empty()),
        _ => {}
    }
    // For 2-4 args, try the POSIX short-form first. It handles every
    // backward-compatible case (existing tests). Task 2 wires the
    // grammar parser as a fall-through for forms the short-form
    // rejects (e.g. `[ ( -n a ) ]`).
    evaluate_short_form(args)
}

fn evaluate_short_form(args: &[String]) -> Result<bool, String> {
    match args.len() {
        2 => {
            if args[0] == "!" {
                negate(evaluate(&args[1..2]))
            } else if is_unary_op(&args[0]) {
                apply_unary(&args[0], &args[1])
            } else {
                Err(format!("{}: unary operator expected", args[0]))
            }
        }
        3 => {
            if is_binary_op(&args[1]) {
                apply_binary(&args[1], &args[0], &args[2])
            } else if args[0] == "!" {
                negate(evaluate(&args[1..3]))
            } else {
                Err(format!("{}: binary operator expected", args[1]))
            }
        }
        4 => {
            if args[0] == "!" {
                negate(evaluate(&args[1..4]))
            } else {
                Err("too many arguments".to_string())
            }
        }
        _ => Err("too many arguments".to_string()),
    }
}
```

Note: the `_ => Err("too many arguments")` arm at the bottom of `evaluate_short_form` is unreachable in this task (the dispatcher in `evaluate` only calls it for 2-4 args). Task 2 will leave this arm intact since the parser is the new 5+ path; the unreachable arm acts as a defensive fallback.

- [ ] **Step 2: Add `-a` to `is_unary_op`**

Find `is_unary_op` (around line 47-52):

```rust
fn is_unary_op(s: &str) -> bool {
    matches!(
        s,
        "-e" | "-f" | "-d" | "-r" | "-w" | "-x" | "-s" | "-L" | "-z" | "-n"
    )
}
```

Replace with:

```rust
fn is_unary_op(s: &str) -> bool {
    matches!(
        s,
        // `-a` is bash's deprecated unary alias for `-e` (file exists).
        // It also serves as the binary AND combinator in operator
        // position; the grammar parser (v75) disambiguates by context.
        "-a" | "-e" | "-f" | "-d" | "-r" | "-w" | "-x" | "-s" | "-L" | "-z" | "-n"
    )
}
```

- [ ] **Step 3: Add `-a` arm to `apply_unary`**

Find `apply_unary` (around line 63-85). Add an `-a` arm that mirrors `-e`:

```rust
fn apply_unary(op: &str, operand: &str) -> Result<bool, String> {
    match op {
        "-z" => Ok(operand.is_empty()),
        "-n" => Ok(!operand.is_empty()),
        // `-a` and `-e` both test for file existence. POSIX prefers
        // `-e`; bash retains `-a` as a deprecated alias.
        "-a" | "-e" => Ok(std::fs::metadata(operand).is_ok()),
        "-f" => Ok(std::fs::metadata(operand)
            .map(|m| m.is_file())
            .unwrap_or(false)),
        // …existing -d, -s, -L, -r, -w, -x arms unchanged…
        _ => Err(format!("{op}: unknown operator")),
    }
}
```

(Only the `-e` arm changes — extend its match pattern to include `-a`. Leave all other arms exactly as they were.)

- [ ] **Step 4: Add unit tests for the `-a` unary form**

Append to `mod tests` at the bottom of `src/test_builtin.rs`:

```rust
#[test]
fn unary_dash_a_is_file_exists_alias() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("present");
    std::fs::write(&file, b"data").unwrap();
    let file_s = file.to_str().unwrap();
    let missing = dir.path().join("absent");
    let missing_s = missing.to_str().unwrap();

    // 2-arg short-form: `[ -a present ]` → true.
    assert_eq!(evaluate(&args(&["-a", file_s])), Ok(true));
    // 2-arg short-form: `[ -a absent ]` → false (file does not exist).
    assert_eq!(evaluate(&args(&["-a", missing_s])), Ok(false));
}

#[test]
fn dash_a_in_two_arg_position_is_unary_not_and() {
    // Sanity: `-a` in the unary-op position of a 2-arg call is
    // file-exists, not the AND combinator (which requires 3+ args
    // and operand on each side).
    assert_eq!(evaluate(&args(&["-a", "/"])), Ok(true));
}
```

- [ ] **Step 5: Run all tests**

Run: `cargo test 2>&1 | grep -E "test result|FAILED" | head -10`
Expected: 0 failures. Total should be 2118 + 2 (the two new `-a` tests) = 2120.

Run: `cargo clippy --all-targets 2>&1 | tail -5`
Expected: 0 warnings.

If any existing test fails, the refactor broke a path. Most likely: the 2-4-arg dispatch through `evaluate_short_form` doesn't preserve `evaluate` recursion semantics for the `!` cases. Verify that `evaluate(&args[1..2])` is still called (not `evaluate_short_form`) — the recursive `evaluate` call handles the 1-arg case via the dispatcher's early return.

- [ ] **Step 6: Commit**

```bash
git add src/test_builtin.rs
git commit -m "$(cat <<'EOF'
test_builtin: extract evaluate_short_form + add -a unary (v75 task 1)

Refactor: split the existing 2-4-arg arms of `evaluate` into a private
`evaluate_short_form` helper. The dispatcher in `evaluate` keeps the
POSIX 0-1-arg shortcuts inline and routes 2-4-arg calls to the
short-form helper. No behavior change for existing call sites.

Lays groundwork for Task 2's recursive-descent parser, which will
fall through from the short-form to handle nested-paren and 5+ arg
combinator forms.

Also adds `-a` as a unary file-exists alias for `-e` (matches bash's
deprecated alias). `[ -a /tmp ]` now returns true via the 2-arg
short-form. The dual role of `-a` (file-exists in unary position,
AND combinator in operator position) is disambiguated by structural
position; the grammar parser in Task 2 handles the AND side.

2 new unit tests pin the `-a` unary semantics.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Recursive-descent parser + dispatch

**Files:**
- Modify: `src/test_builtin.rs` — add `Parser` struct + parse_* methods; wire dispatcher; add ~18 grammar-focused unit tests

**Goal:** End-to-end: `[ -n a -a -n b ]`, `[ -z "" -o -n x ]`, `[ ( ... ) ]`, and nested forms all work. Falls through from the short-form for 2-4-arg cases that need the grammar (e.g., `[ ( -n a ) ]`).

### Steps

- [ ] **Step 1: Add `Parser` struct and parse_* methods**

Append to `src/test_builtin.rs` just before the `#[cfg(test)] mod tests` block. The parser owns a slice and a position; each method advances `pos` and returns `Result<bool, String>`.

```rust
/// Recursive-descent parser for the `test` grammar:
///
/// ```text
/// EXPR    ::= EXPR -o ANDEXPR | ANDEXPR
/// ANDEXPR ::= ANDEXPR -a UNEXPR | UNEXPR
/// UNEXPR  ::= ! UNEXPR | PRIMARY
/// PRIMARY ::= ( EXPR ) | <unary> <word> | <word> <binop> <word> | <word>
/// ```
///
/// Used for 5+ argument calls and for 2-4 argument calls that the
/// POSIX short-form algorithm rejects (e.g. `[ ( -n a ) ]`).
struct Parser<'a> {
    args: &'a [String],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&str> {
        self.args.get(self.pos).map(String::as_str)
    }

    fn take(&mut self) -> Option<&str> {
        let s = self.args.get(self.pos).map(String::as_str);
        if s.is_some() {
            self.pos += 1;
        }
        s
    }

    /// EXPR ::= ANDEXPR ( -o ANDEXPR )*
    fn parse_expr(&mut self) -> Result<bool, String> {
        let mut result = self.parse_and()?;
        while self.peek() == Some("-o") {
            self.pos += 1; // consume -o
            let rhs = self.parse_and()?;
            result = result || rhs;
        }
        Ok(result)
    }

    /// ANDEXPR ::= UNEXPR ( -a UNEXPR )*
    fn parse_and(&mut self) -> Result<bool, String> {
        let mut result = self.parse_unary()?;
        while self.peek() == Some("-a") {
            self.pos += 1; // consume -a
            let rhs = self.parse_unary()?;
            result = result && rhs;
        }
        Ok(result)
    }

    /// UNEXPR ::= ! UNEXPR | PRIMARY
    fn parse_unary(&mut self) -> Result<bool, String> {
        if self.peek() == Some("!") {
            self.pos += 1;
            let inner = self.parse_unary()?;
            Ok(!inner)
        } else {
            self.parse_primary()
        }
    }

    /// PRIMARY ::= ( EXPR ) | <unary> <word> | <word> <binop> <word> | <word>
    fn parse_primary(&mut self) -> Result<bool, String> {
        // Empty input where a primary is expected.
        if self.peek().is_none() {
            return Err("expression expected".to_string());
        }
        // Closing paren / combinator where a primary is expected.
        match self.peek() {
            Some(")") | Some("-a") | Some("-o") => {
                return Err("expression expected".to_string());
            }
            _ => {}
        }
        // Parenthesized group.
        if self.peek() == Some("(") {
            self.pos += 1; // consume (
            let inner = self.parse_expr()?;
            match self.peek() {
                Some(")") => {
                    self.pos += 1;
                    return Ok(inner);
                }
                _ => return Err("missing ')'".to_string()),
            }
        }
        // <unary> <word> — recognize a known unary op followed by a word.
        // Look at the next two tokens; if first is a unary op AND the
        // second exists, consume both.
        if let (Some(op), Some(_operand)) = (
            self.args.get(self.pos).map(String::as_str),
            self.args.get(self.pos + 1).map(String::as_str),
        ) && is_unary_op(op)
        {
            // But wait — `-a` is both a unary op AND the AND combinator.
            // In primary position with a following non-combinator token,
            // it's the unary file-exists. If the token AFTER would be
            // `-a`/`-o`/`)`/end-of-args, that means we'd be consuming
            // a "primary <combinator>" pair which is normal (operand is
            // a filename like "a"). The disambiguation is positional:
            // we're in parse_primary, so consume as unary. The grammar
            // ensures `-a`/`-o` at this position would have already been
            // rejected by the early `)` / combinator check above.
            let op = op.to_string();
            let operand = self.args[self.pos + 1].clone();
            self.pos += 2;
            return apply_unary(&op, &operand);
        }
        // <word> <binop> <word> — three-token binary form.
        if let (Some(_lhs), Some(op), Some(_rhs)) = (
            self.args.get(self.pos).map(String::as_str),
            self.args.get(self.pos + 1).map(String::as_str),
            self.args.get(self.pos + 2).map(String::as_str),
        ) && is_binary_op(op)
        {
            let lhs = self.args[self.pos].clone();
            let op = op.to_string();
            let rhs = self.args[self.pos + 2].clone();
            self.pos += 3;
            return apply_binary(&op, &lhs, &rhs);
        }
        // Bare word — truthiness of the string.
        let word = self.take().unwrap_or("");
        Ok(!word.is_empty())
    }
}
```

- [ ] **Step 2: Wire the parser into `evaluate`**

Currently `evaluate` (after Task 1) returns `evaluate_short_form(args)` for any args.len() >= 2. Update to:

```rust
pub fn evaluate(args: &[String]) -> Result<bool, String> {
    match args.len() {
        0 => return Ok(false),
        1 => return Ok(!args[0].is_empty()),
        _ => {}
    }
    // For 2-4 args, try the POSIX short-form first; if it returns Err,
    // fall through to the grammar parser (handles nested-paren forms).
    if args.len() <= 4
        && let Ok(b) = evaluate_short_form(args)
    {
        return Ok(b);
    }
    let mut p = Parser { args, pos: 0 };
    let result = p.parse_expr()?;
    if p.pos != args.len() {
        return Err(format!("{}: unexpected argument", args[p.pos]));
    }
    Ok(result)
}
```

- [ ] **Step 3: Add ~18 grammar unit tests**

Append to `mod tests` at the bottom of `src/test_builtin.rs`. Each test uses the existing `args(&[...])` helper:

```rust
#[test]
fn combinator_and_both_true() {
    // [ -n a -a -n b ] → true
    assert_eq!(evaluate(&args(&["-n", "a", "-a", "-n", "b"])), Ok(true));
}

#[test]
fn combinator_and_one_false() {
    // [ -z a -a -n b ] → false (left is false)
    assert_eq!(evaluate(&args(&["-z", "a", "-a", "-n", "b"])), Ok(false));
}

#[test]
fn combinator_or_first_true() {
    // [ -n a -o -z b ] → true
    assert_eq!(evaluate(&args(&["-n", "a", "-o", "-z", "b"])), Ok(true));
}

#[test]
fn combinator_or_both_false() {
    // [ -z a -o -z b ] → false
    assert_eq!(evaluate(&args(&["-z", "a", "-o", "-z", "b"])), Ok(false));
}

#[test]
fn parens_simple_wrapping() {
    // [ ( -n a ) ] → true (falls through from 3-arg short-form)
    assert_eq!(evaluate(&args(&["(", "-n", "a", ")"])), Ok(true));
}

#[test]
fn nested_parens() {
    // [ ( ( -n a ) ) ] → true
    assert_eq!(
        evaluate(&args(&["(", "(", "-n", "a", ")", ")"])),
        Ok(true)
    );
}

#[test]
fn parens_group_changes_precedence() {
    // Without parens: [ -z a -o -n b -a -n c ] is `-z a -o (-n b -a -n c)`
    // = false OR (true AND true) = true.
    // With parens around the OR: [ ( -z a -o -n b ) -a -n c ]
    // = (false OR true) AND true = true.
    assert_eq!(
        evaluate(&args(&["(", "-z", "a", "-o", "-n", "b", ")", "-a", "-n", "c"])),
        Ok(true)
    );
}

#[test]
fn precedence_and_higher_than_or() {
    // [ -z a -o -n b -a -n c ] = false OR (true AND true) = true.
    assert_eq!(
        evaluate(&args(&["-z", "a", "-o", "-n", "b", "-a", "-n", "c"])),
        Ok(true)
    );
}

#[test]
fn negation_of_combinator_lhs() {
    // [ ! -n a -a -n b ] = (NOT (-n a)) AND (-n b) = false AND true = false.
    assert_eq!(evaluate(&args(&["!", "-n", "a", "-a", "-n", "b"])), Ok(false));
}

#[test]
fn double_negation() {
    // [ ! ! -n a ] = NOT NOT true = true.
    assert_eq!(evaluate(&args(&["!", "!", "-n", "a"])), Ok(true));
}

#[test]
fn empty_parens_error() {
    let r = evaluate(&args(&["(", ")"]));
    assert!(r.is_err(), "expected error, got {r:?}");
    assert!(
        r.unwrap_err().contains("expression"),
        "expected 'expression' in error"
    );
}

#[test]
fn unbalanced_open_paren_error() {
    let r = evaluate(&args(&["(", "-n", "a"]));
    assert!(r.is_err());
    assert!(r.unwrap_err().contains(")"), "expected ')' in error");
}

#[test]
fn unbalanced_close_paren_error() {
    // [ -n a ) ] — `)` at position 2 is an unexpected token in primary
    // position; the parser will reject it. The 3-arg short-form sees
    // no binary op in position 1 and returns Err, then we fall through.
    let r = evaluate(&args(&["-n", "a", ")"]));
    assert!(r.is_err());
}

#[test]
fn dangling_combinator_at_end_error() {
    // [ -n a -a ] — falls through short-form (4 args), parser consumes
    // `-n a -a`, then `parse_unary` runs out of input.
    let r = evaluate(&args(&["-n", "a", "-a"]));
    assert!(r.is_err());
    assert!(
        r.unwrap_err().contains("expression"),
        "expected 'expression' in error"
    );
}

#[test]
fn combinator_with_binary_operands() {
    // [ a = a -a 1 -lt 2 ] = (a == a) AND (1 < 2) = true.
    assert_eq!(
        evaluate(&args(&["a", "=", "a", "-a", "1", "-lt", "2"])),
        Ok(true)
    );
}

#[test]
fn mixed_unary_and_binary() {
    // [ -e /tmp -a 1 -lt 2 ] = true AND true = true.
    assert_eq!(
        evaluate(&args(&["-e", "/tmp", "-a", "1", "-lt", "2"])),
        Ok(true)
    );
}

#[test]
fn long_and_chain_left_associative() {
    // [ -n a -a -n b -a -n c -a -n d ] = all true.
    assert_eq!(
        evaluate(&args(&[
            "-n", "a", "-a", "-n", "b", "-a", "-n", "c", "-a", "-n", "d"
        ])),
        Ok(true)
    );
}

#[test]
fn or_chain_left_associative() {
    // [ -z a -o -z b -o -n c ] = false OR false OR true = true.
    assert_eq!(
        evaluate(&args(&["-z", "a", "-o", "-z", "b", "-o", "-n", "c"])),
        Ok(true)
    );
}

#[test]
fn parens_three_arg_form() {
    // [ ( a ) ] (3 args) — 3-arg short-form has no binary op in
    // position 1 ('a'), returns Err. Falls through to grammar:
    // parens around bare word `a` → truthiness of `a` → true.
    assert_eq!(evaluate(&args(&["(", "a", ")"])), Ok(true));
}
```

- [ ] **Step 4: Run all tests**

Run: `cargo test --bin huck test_builtin 2>&1 | tail -25`
Expected: all tests in `mod tests` pass. The 18 new grammar tests should all pass.

Run: `cargo test 2>&1 | grep -E "test result|FAILED" | head -10`
Expected: 0 failures. Total should be 2120 + 18 = 2138.

Run: `cargo clippy --all-targets 2>&1 | tail -5`
Expected: 0 warnings.

Common debug paths if a test fails:
- `combinator_and_one_false`: verify `parse_and`'s short-circuit logic — `result && rhs` evaluates both sides (Rust's `&&` is short-circuit, but here we already consumed RHS, so this is by-design). The boolean result is still correct.
- `parens_simple_wrapping`: this is a 4-arg call. Verify the short-form returns Err for `( -n a )` (4-arg arm without leading `!`) so the parser fall-through fires.
- `unbalanced_close_paren_error`: 3 args, short-form arm 1 (`a` is not binary-op), returns Err. Parser runs, consumes `-n a`, sees `)`, errors via the "unexpected argument" check at end of `evaluate`.

- [ ] **Step 5: Commit**

```bash
git add src/test_builtin.rs
git commit -m "$(cat <<'EOF'
test_builtin: recursive-descent parser for -a/-o/parens (v75 task 2)

Adds a Parser struct with parse_expr/parse_and/parse_unary/parse_primary
implementing the bash/POSIX test grammar:

  EXPR    ::= EXPR -o ANDEXPR | ANDEXPR
  ANDEXPR ::= ANDEXPR -a UNEXPR | UNEXPR
  UNEXPR  ::= ! UNEXPR | PRIMARY
  PRIMARY ::= ( EXPR ) | <unary> <word> | <word> <binop> <word> | <word>

Precedence: -o < -a < ! < parens < unary/binary primaries. Each AND/OR
chain is left-associative.

`evaluate` now dispatches: 0/1 args via the inline shortcut; 2-4 args
try the POSIX short-form first and fall through to the grammar parser
on Err (catches `[ ( -n a ) ]` and similar); 5+ args go straight to
the grammar.

18 new grammar unit tests cover: AND/OR truth tables, precedence,
parens grouping, nested parens, negation chains, error paths (empty
parens, unbalanced parens, dangling combinator). Existing tests
unchanged — all still pass via the short-form path.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Integration tests + bash-diff harness + docs

**Files:**
- Create: `tests/test_combinators_integration.rs`
- Create: `tests/scripts/test_combinators_diff_check.sh`
- Modify: `docs/bash-divergences.md` — M-25 entry + change-log entry
- Modify: `README.md` — v75 row

### Steps

- [ ] **Step 1: Write integration tests at `tests/test_combinators_integration.rs`**

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String, i32) {
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
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn if_with_and_combinator() {
    let (out, _, _) = run_capture("if [ -n \"a\" -a -n \"b\" ]; then echo Y; fi\nexit\n");
    assert!(out.lines().any(|l| l == "Y"), "got: {out:?}");
}

#[test]
fn if_with_or_combinator() {
    let (out, _, _) = run_capture("if [ -z \"\" -o -n \"x\" ]; then echo Y; fi\nexit\n");
    assert!(out.lines().any(|l| l == "Y"), "got: {out:?}");
}

#[test]
fn nested_parens_in_if() {
    let (out, _, _) = run_capture(
        "if [ \\( -n a -o -n b \\) -a -n c ]; then echo Y; fi\nexit\n"
    );
    assert!(out.lines().any(|l| l == "Y"), "got: {out:?}");
}

#[test]
fn negated_combinator_in_if() {
    let (out, _, _) = run_capture(
        "if [ ! \\( -z a -o -z b \\) ]; then echo Y; fi\nexit\n"
    );
    assert!(out.lines().any(|l| l == "Y"), "got: {out:?}");
}

#[test]
fn bracket_form_with_combinator() {
    let (out, _, _) = run_capture("[ -n a -a -n b ] && echo Y\nexit\n");
    assert!(out.lines().any(|l| l == "Y"), "got: {out:?}");
}

#[test]
fn unbalanced_paren_produces_non_zero_exit() {
    let (_out, _err, _) = run_capture(
        "[ \\( -n a ]\necho rc=$?\nexit\n"
    );
    // No assertion on output; just ensure huck doesn't crash and
    // emits a non-zero rc. We check by capturing the `echo rc=$?` line.
    // If the test command failed (Err result), $? should be non-zero.
    let (out, _, _) = run_capture(
        "[ \\( -n a ]\necho rc=$?\nexit\n"
    );
    let rc_line = out.lines().find(|l| l.starts_with("rc=")).unwrap_or("rc=?");
    assert_ne!(rc_line, "rc=0", "expected non-zero rc, got: {rc_line}");
}
```

Run: `cargo test --test test_combinators_integration 2>&1 | tail -10`
Expected: 6 tests pass.

- [ ] **Step 2: Write the bash-diff harness `tests/scripts/test_combinators_diff_check.sh`**

```bash
#!/usr/bin/env bash
# Manual sanity check: run the same test-combinator fragments through
# bash and huck, diff outputs.
set -u

HUCK="$(dirname "$0")/../../target/debug/huck"
if [ ! -x "$HUCK" ]; then
    echo "build huck first: cargo build" >&2
    exit 1
fi

if ! command -v bash >/dev/null 2>&1; then
    echo "bash not found on PATH; this differential harness requires bash" >&2
    exit 1
fi

fragments=(
    '[ -n a -a -n b ]; echo $?'
    '[ -z a -o -n b ]; echo $?'
    '[ \( -n a -o -n b \) -a -n c ]; echo $?'
    '[ ! -n a ]; echo $?'
    '[ ! -n a -a -n b ]; echo $?'
    '[ \( -z "" -a -n x \) -o -n y ]; echo $?'
    '[ -a /tmp ]; echo $?'
    '[ -n a -a -n b -a -n c -a -n d ]; echo $?'
)

fail=0
for f in "${fragments[@]}"; do
    b_out=$(bash -c "$f" 2>&1)
    h_out=$(echo "$f" | "$HUCK" 2>&1)
    if [ "$b_out" != "$h_out" ]; then
        echo "DIFF on: $f"
        diff <(printf '%s\n' "$b_out") <(printf '%s\n' "$h_out") || true
        echo "---"
        fail=1
    fi
done

if [ "$fail" -eq 0 ]; then
    echo "all test-combinator fragments produce identical output to bash"
fi
exit "$fail"
```

Then:

```bash
chmod +x tests/scripts/test_combinators_diff_check.sh
cargo build
bash tests/scripts/test_combinators_diff_check.sh
```

Expected: "all test-combinator fragments produce identical output to bash". If ANY DIFF prints, debug huck (not the harness).

- [ ] **Step 3: Update M-25 entry in `docs/bash-divergences.md`**

Find M-25 (around line 195):

```markdown
- **M-25: `test -a`/`-o`/`( )` combinators** — `[deferred]` high. huck: only POSIX 1-3 arg + `!`. bash: full chained expressions.
```

Replace with:

```markdown
- **M-25: `test` combinators (`-a`/`-o`/`( )`)** — `[fixed v75]` high. The `test` / `[` builtin now supports the full bash combinator grammar via a recursive-descent parser in `src/test_builtin.rs`: `EXPR ::= EXPR -o ANDEXPR | ANDEXPR`; `ANDEXPR ::= ANDEXPR -a UNEXPR | UNEXPR`; `UNEXPR ::= ! UNEXPR | PRIMARY`; `PRIMARY ::= ( EXPR ) | <unary> <word> | <word> <binop> <word> | <word>`. Precedence (low→high): `-o` < `-a` < `!` < parens < unary/binary primaries. Both `-a` and `-o` chains are left-associative. The POSIX 0-4-arg short-form algorithm is preserved for backward compatibility (every existing 2-3-arg form behaves identically); 5+ args go straight to the grammar, and 2-4-arg calls that the short-form rejects (e.g. `[ ( -n a ) ]`) fall through to the parser. `-a` retains its bash-deprecated unary alias meaning ("file exists", same as `-e`) in primary position; in operator position the parser consumes it as AND. Error messages: `expression expected` (empty group, dangling combinator); `missing ')'` (unbalanced parens). ~20 unit tests in `mod tests` + 6 binary-driven integration tests in `tests/test_combinators_integration.rs` + 8 bash-diff fragments in `tests/scripts/test_combinators_diff_check.sh` (byte-identical to bash 5.2.21).
```

- [ ] **Step 4: Add change-log entry**

At the end of `docs/bash-divergences.md`, append:

```markdown
- **2026-06-02**: M-25 (`test` combinators) shipped as v75. Adds bash-compatible `-a` (AND), `-o` (OR), and parenthesized grouping `( ... )` to the `test` / `[` builtin via a recursive-descent parser in `src/test_builtin.rs`. The existing POSIX § 4.62 short-form algorithm (0-4-arg special cases) is preserved as the primary dispatch for backward compatibility; 5+ args go to the grammar, and 2-4-arg calls that the short-form rejects fall through. Grammar: `EXPR -o ANDEXPR | ANDEXPR`; `ANDEXPR -a UNEXPR | UNEXPR`; `! UNEXPR | PRIMARY`; `( EXPR ) | unary word | word binop word | word`. Precedence: `-o` < `-a` < `!` < parens. Left-associative chains. `-a` is also a unary file-exists alias for `-e` (matches bash's deprecated alias) — disambiguated by structural position. 20 unit tests in `mod tests` + 6 binary-driven integration tests in `tests/test_combinators_integration.rs` + 8 bash-diff fragments in `tests/scripts/test_combinators_diff_check.sh` (byte-identical to bash 5.2.21). Existing 2118 tests all still pass under the short-form path, confirming the refactor preserves prior behavior. Closes the second-to-last high-priority Tier-2 deferral; only M-36 (programmable completion) remains.
```

- [ ] **Step 5: Add v75 row to `README.md`**

Find the v74 row (around line 83):

```markdown
| v74       | configurable IFS (M-05)                                        |
```

Append:

```markdown
| v74       | configurable IFS (M-05)                                        |
| v75       | test combinators (M-25)                                        |
```

- [ ] **Step 6: Final verification**

```bash
cargo build 2>&1 | tail -5
cargo test 2>&1 | grep -E "test result|FAILED" | tail -10
cargo clippy --all-targets 2>&1 | tail -5
bash tests/scripts/test_combinators_diff_check.sh
bash tests/scripts/ifs_diff_check.sh
bash tests/scripts/arrays_diff_check.sh
```

All six should pass. Total test count should be ~2144 (2138 + 6 integration tests).

- [ ] **Step 7: Commit**

```bash
git add tests/test_combinators_integration.rs tests/scripts/test_combinators_diff_check.sh docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs+tests: test combinators shipped v75 (M-25)

6 binary-driven integration tests covering if-with-AND, if-with-OR,
nested parens, negated combinator, bracket-form AND chain, and
unbalanced-paren error exit.

New M-25 entry in bash-divergences.md (was [deferred] high; now
[fixed v75]). Change-log entry. README v75 row.

New tests/scripts/test_combinators_diff_check.sh bash-diff harness —
8 fragments verified byte-identical to bash 5.2.21. The existing
arrays_diff_check.sh and ifs_diff_check.sh remain green, confirming
no regression in v71/v72/v73/v74 work.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final verification & merge prep

- [ ] **Step 1: Full test pass**

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "Total:", sum}'`
Expected: ~2144 (2118 + 20 unit + 6 integration).

- [ ] **Step 2: Clippy clean**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: no warnings.

- [ ] **Step 3: All three bash-diff harnesses pass**

```bash
bash tests/scripts/arrays_diff_check.sh
bash tests/scripts/ifs_diff_check.sh
bash tests/scripts/test_combinators_diff_check.sh
```

Expected: all three print "all ... identical to bash".

- [ ] **Step 4: M-25 entry well-formed**

Run: `grep -nE "M-25.*fixed v75" docs/bash-divergences.md | head -3`
Expected: at least one match (Tier-2 entry + change-log entry).

- [ ] **Step 5: Confirm v75 row in README**

Run: `grep "v75" README.md`
Expected: one row with `test combinators (M-25)`.

- [ ] **Step 6: Ask user for merge confirmation via AskUserQuestion**

Per the v52-v74 workflow.

- [ ] **Step 7: On approval, merge to main**

```bash
git checkout main
git merge --no-ff v75-test-combinators -m "Merge v75: test combinators (M-25)"
git push origin main
git branch -d v75-test-combinators
```

- [ ] **Step 8: Post-merge memory update**

Update `/home/john/.claude/projects/-home-john-projects-shuck/memory/MEMORY.md` and `project_huck_iterations.md` with the v75 entry.

---

## Notes for the implementer

1. **Subagent isolation**: each task is implemented by a fresh subagent. The plan body is their only context.

2. **TDD discipline**: write tests first when behavior is new. The Task 2 grammar tests can be added BEFORE the parser exists (they'll fail) and verified to pass after the parser lands.

3. **The short-form fall-through**: this is the critical correctness invariant. For 2-4 args, try `evaluate_short_form` first; if Ok, return; if Err, run the grammar parser. This preserves every existing test while extending the surface.

4. **`-a` dual role**: in 2-arg position via short-form → unary file-exists. In operator position via grammar → AND combinator. The parser's `parse_and` looks for `-a` AFTER a primary; `parse_primary` consumes `-a` followed by a word as unary. The positional disambiguation falls out of the grammar naturally.

5. **Error-message exactness**: the test `empty_parens_error` asserts the error contains `"expression"`. The test `unbalanced_open_paren_error` asserts it contains `")"`. If you choose different wording, update the tests accordingly — but the suggested wording matches bash's diagnostic style.

6. **Code-quality reviewer notes** (anticipate):
   - "Why not a single match-based parser?" — recursive descent is clearer at this scale; the grammar levels map 1:1 to functions.
   - "Why short-form fall-through instead of unifying?" — preserves existing test coverage with zero behavioral risk; the grammar handles everything the short-form rejects.
   - "`-a` in `is_unary_op` could be confusing" — yes, it's the bash compatibility cost. Comment explains.
