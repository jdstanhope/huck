# Multi-line `[[ ]]` + Missing Test Operators Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `[[ … ]]` conditions that span multiple physical lines gather continuation input correctly, and add the four test operators M-14 omitted — `-v` (variable-is-set), `-nt`/`-ot`/`-ef` (file age/identity) — to both `[[ … ]]` and the `test`/`[` builtin.

**Architecture:** (1) huck's continuation reader is parse-error-driven, so the `[[` parser is refined to return `ParseError::UnterminatedDoubleBracket` when input is exhausted *before* `]]` is consumed (vs. today's misleading `TestExprMissingOperand`), and `continuation::classify` maps that error to a new `Incomplete` reason with a space joiner. (2) The four operators are added to `test_builtin` (shared by both constructs); `-nt`/`-ot`/`-ef` are pure filesystem, while `-v` needs var-set knowledge supplied via a `var_is_set` predicate (keeping `test_builtin` decoupled from `Shell`) and a new `Shell::is_set`.

**Tech Stack:** Rust; huck's `ParseError`/`TestExpr`/`TestUnaryOp`/`TestBinaryOp`/`Shell` types; `std::fs` + `std::os::unix::fs::MetadataExt`.

**Spec:** `docs/superpowers/specs/2026-06-04-dbracket-multiline-design.md`

**Refinement vs spec (read this):** The spec proposed threading `&Shell` through `test_builtin::evaluate`. This plan instead injects a `var_is_set: &dyn Fn(&str) -> bool` predicate, because `test_builtin::evaluate` has **85 existing call sites** (almost all unit tests) and the module currently has zero `Shell` coupling. The predicate keeps all 85 calls working through a thin `evaluate(args)` wrapper and keeps the module decoupled. Behavior is identical. `[[`'s `-v` is evaluated directly in `eval_test_expr` (which has `&Shell`), so `eval_unary` needs no change.

**Conventions:**
- huck is a **binary crate**: unit tests `cargo test --bin huck <filter>`; integration tests `cargo test --test <name>`; full suite `cargo test`.
- Commit trailer (exact): `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
- Baseline before this plan: **2423** tests pass, clippy clean. Every task keeps `cargo clippy --all-targets` clean and the suite green.
- `command::parse(tokens) -> Result<Option<Sequence>, ParseError>` (`src/command.rs:562`); lex via `crate::lexer::tokenize(src)`.

---

## File Structure

| File | Responsibility | Task |
|------|----------------|------|
| `src/command.rs` | `[[` parser: EOF-inside-`[[` → `UnterminatedDoubleBracket` (T1); `TestUnaryOp::VarSet` + `-v` parse (T2); `TestBinaryOp::{NewerThan,OlderThan,SameFile}` + `-nt`/`-ot`/`-ef` parse (T3) | 1,2,3 |
| `src/continuation.rs` | `ContinuationReason::DoubleBracket` + mapping + space joiner (T1) | 1 |
| `src/shell_state.rs` | `Shell::is_set(name)` (T2) | 2 |
| `src/executor.rs` | `eval_test_expr` `VarSet` intercept (T2); `eval_binary` `-nt`/`-ot`/`-ef` delegation (T3) | 2,3 |
| `src/test_builtin.rs` | `evaluate_with(args, var_is_set)` + `evaluate` wrapper + `-v` (T2); `compare_files` + `-nt`/`-ot`/`-ef` (T3) | 2,3 |
| `src/builtins.rs` | `builtin_test(name, args, shell)` + dispatch (T2) | 2 |
| `tests/dbracket_multiline_integration.rs` | NEW — integration tests | 1,2,3 |
| `tests/scripts/dbracket_multiline_diff_check.sh` | NEW — huck's 14th bash-diff harness | 4 |
| `docs/bash-divergences.md`, `README.md` | M-14 update + M-14a/M-14b + changelog + README row | 4 |

---

### Task 1: Multi-line `[[ ]]` continuation

**Files:**
- Modify: `src/command.rs` — `next_test_word` (~1865), `parse_test_atom` (~1947), `parse_test_primary` grouping (~1928-1936)
- Modify: `src/continuation.rs` — `ContinuationReason` enum (~10), `classify` step-4 match (~63-76), `joiner_for` (~91-106)
- Create: `tests/dbracket_multiline_integration.rs`
- Test: `src/continuation.rs` (`#[cfg(test)]`), `src/command.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Write failing continuation unit tests**

Add to the `#[cfg(test)] mod tests` in `src/continuation.rs`:

```rust
#[test]
fn classify_unclosed_double_bracket_is_incomplete() {
    assert_eq!(
        classify("[[ -f /etc/passwd"),
        Completeness::Incomplete(ContinuationReason::DoubleBracket)
    );
}

#[test]
fn classify_double_bracket_trailing_and_is_incomplete() {
    assert_eq!(
        classify("[[ -f /a &&"),
        Completeness::Incomplete(ContinuationReason::DoubleBracket)
    );
}

#[test]
fn classify_closed_double_bracket_is_complete() {
    assert_eq!(classify("[[ a == b ]]"), Completeness::Complete);
}

#[test]
fn classify_double_bracket_missing_operand_is_error() {
    // `]]` present, operand absent → genuine error; must NOT request continuation.
    assert_eq!(classify("[[ a == ]]"), Completeness::Error);
}

#[test]
fn classify_bare_double_bracket_token_is_complete() {
    // `echo [[` — `[[` is an ordinary argument, not a conditional opener.
    assert_eq!(classify("echo [["), Completeness::Complete);
}

#[test]
fn joiner_for_double_bracket_is_space() {
    assert_eq!(joiner_for(ContinuationReason::DoubleBracket, "[[ -f a &&"), " ");
}
```

- [ ] **Step 2: Run them, verify they fail**

Run: `cargo test --bin huck --lib continuation 2>&1 | tail -20` (or `cargo test --bin huck classify_unclosed_double 2>&1 | tail`).
Expected: the unclosed/trailing-`&&` cases FAIL (currently classify returns `Error`), and `ContinuationReason::DoubleBracket` doesn't compile yet.

- [ ] **Step 3: Add the `DoubleBracket` continuation reason + mapping + joiner**

In `src/continuation.rs`, add the variant to the enum (after `Subshell`):

```rust
pub enum ContinuationReason {
    Backslash,
    OpenQuote,
    Operator,
    Compound,
    Heredoc,
    Subshell,
    DoubleBracket,
}
```

In `classify`'s step-4 match, add an arm before the `Err(_) => Completeness::Error` catch-all:

```rust
        Err(ParseError::UnterminatedDoubleBracket) => {
            Completeness::Incomplete(ContinuationReason::DoubleBracket)
        }
```

In `joiner_for`, add the arm:

```rust
        ContinuationReason::DoubleBracket => " ",
```

- [ ] **Step 4: Refine the `[[` parser so EOF-inside-`[[` is `Unterminated`**

In `src/command.rs`:

(a) `next_test_word` — change ONLY the `None` arm (keep the stop-token arm as `TestExprMissingOperand`):

```rust
    match iter.peek() {
        None => return Err(ParseError::UnterminatedDoubleBracket),
        Some(tok) => {
            if keyword_of(tok) == Some(Keyword::DoubleBracketClose)
                || matches!(tok, Token::Op(_))
            {
                return Err(ParseError::TestExprMissingOperand);
            }
        }
    }
```

(b) `parse_test_atom` — replace the opening empty-check so EOF (`None`) is unterminated while a present `]]`/`)` is still `EmptyDoubleBracket`:

```rust
    // End of input mid-expression → unterminated (request continuation).
    if iter.peek().is_none() {
        return Err(ParseError::UnterminatedDoubleBracket);
    }
    // A present terminator (`]]` / `)`) with nothing before it → empty body.
    if is_test_expr_stop(iter) {
        return Err(ParseError::EmptyDoubleBracket);
    }
```

(c) `parse_test_primary` — the grouping `)`-expectation: add a `None` arm:

```rust
        match iter.next() {
            Some(Token::Op(Operator::RParen)) => {}
            None => return Err(ParseError::UnterminatedDoubleBracket),
            _ => return Err(ParseError::TestExprMissingOperand),
        }
```

(The binary-operator path already returns `UnterminatedDoubleBracket` on `op_token == None` and on a consumed `]]`; the top-level `parse_double_bracket_with_assigns` already handles bare `[[`/`[[ ]]`. No other parser changes needed.)

- [ ] **Step 5: Add parser unit tests (verify EOF→Unterminated, terminator→missing-operand)**

Add to the `#[cfg(test)] mod tests` in `src/command.rs` (uses `crate::lexer::tokenize` + the module's `parse`):

```rust
#[test]
fn dbracket_eof_after_open_is_unterminated() {
    let toks = crate::lexer::tokenize("[[ -f a").unwrap();
    assert!(matches!(parse(toks), Err(ParseError::UnterminatedDoubleBracket)));
}

#[test]
fn dbracket_eof_after_and_is_unterminated() {
    let toks = crate::lexer::tokenize("[[ -f a &&").unwrap();
    assert!(matches!(parse(toks), Err(ParseError::UnterminatedDoubleBracket)));
}

#[test]
fn dbracket_eof_after_binop_is_unterminated() {
    let toks = crate::lexer::tokenize("[[ a ==").unwrap();
    assert!(matches!(parse(toks), Err(ParseError::UnterminatedDoubleBracket)));
}

#[test]
fn dbracket_missing_operand_with_close_is_error() {
    let toks = crate::lexer::tokenize("[[ a == ]]").unwrap();
    assert!(matches!(parse(toks), Err(ParseError::TestExprMissingOperand)));
}
```

- [ ] **Step 6: Create integration tests for multi-line `[[`**

Create `tests/dbracket_multiline_integration.rs` (model the harness on `tests/bang_negation_integration.rs`):

```rust
//! Integration tests for v87 multi-line [[ ]] continuation + test operators (M-14a).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Runs `script` through huck on stdin; returns (stdout, exit_code).
fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn multiline_break_before_close() {
    // `]]` on the next line.
    assert_eq!(run("[[ -f /etc/passwd\n]] && echo yes\n").0, "yes\n");
}

#[test]
fn multiline_break_after_and() {
    assert_eq!(run("[[ -f /etc/passwd &&\n   -f /etc/hosts ]] && echo both\n").0, "both\n");
}

#[test]
fn multiline_break_after_open() {
    assert_eq!(run("[[\n  -f /etc/passwd ]] && echo opened\n").0, "opened\n");
}

#[test]
fn multiline_break_after_operand() {
    assert_eq!(run("[[ abc\n== abc ]] && echo eq\n").0, "eq\n");
}

#[test]
fn singleline_still_works() {
    assert_eq!(run("[[ -f /etc/passwd ]] && echo ok\n").0, "ok\n");
}

#[test]
fn bare_double_bracket_token_is_literal_arg() {
    // `echo [[` must NOT hang waiting for `]]`; prints the literal.
    assert_eq!(run("echo [[\n").0, "[[\n");
}
```

- [ ] **Step 7: Run all tests, verify pass**

Run: `cargo test --bin huck dbracket 2>&1 | tail; cargo test --bin huck continuation 2>&1 | tail`
Run: `cargo test --test dbracket_multiline_integration 2>&1 | tail -12` → all pass.
Bash-parity spot check (multi-line on stdin to BOTH):
`diff <(printf '[[ -f /etc/passwd &&\n -f /etc/hosts ]] && echo both\n' | bash) <(printf '[[ -f /etc/passwd &&\n -f /etc/hosts ]] && echo both\n' | ./target/debug/huck)` → empty.
Run full suite: `cargo test 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'` → FAIL=0.
Run: `cargo clippy --all-targets 2>&1 | tail -3` → clean.

- [ ] **Step 8: Commit**

```bash
git add src/command.rs src/continuation.rs tests/dbracket_multiline_integration.rs
git commit -m "v87 task 1: multi-line [[ ]] continuation

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `-v` (variable-is-set) in `[[ ]]` and `test`/`[`

**Files:**
- Modify: `src/shell_state.rs` — add `Shell::is_set`
- Modify: `src/command.rs` — `try_unary_op` (+`-v`), `TestUnaryOp` enum (+`VarSet`)
- Modify: `src/executor.rs` — `eval_test_expr` `Unary` arm (intercept `VarSet`)
- Modify: `src/test_builtin.rs` — `evaluate_with` + `evaluate` wrapper + `is_unary_op` + `apply_unary` + `Parser`
- Modify: `src/builtins.rs` — `builtin_test(name, args, shell)` + dispatch
- Test: unit tests in `src/shell_state.rs`, `src/test_builtin.rs`; integration in `tests/dbracket_multiline_integration.rs`

- [ ] **Step 1: Write failing `Shell::is_set` unit tests**

Add to a test module in `src/shell_state.rs`:

```rust
#[test]
fn is_set_true_for_set_var_even_when_empty() {
    let mut sh = Shell::new();
    sh.set("X", String::new());   // Shell::set(&mut self, name: &str, value: String)
    assert!(sh.is_set("X"));
}

#[test]
fn is_set_false_for_unset() {
    let sh = Shell::new();
    assert!(!sh.is_set("DEFINITELY_UNSET_VAR_XYZ"));
}

#[test]
fn is_set_positional_params() {
    let mut sh = Shell::new();
    sh.positional_args = vec!["a".into(), "b".into()];
    assert!(sh.is_set("1"));
    assert!(sh.is_set("2"));
    assert!(!sh.is_set("3"));
}

#[test]
fn is_set_special_zero_is_true() {
    let sh = Shell::new();
    assert!(sh.is_set("0"));
}
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test --bin huck is_set 2>&1 | tail` → fails (`is_set` undefined).

- [ ] **Step 3: Implement `Shell::is_set`**

Add to an `impl Shell` block in `src/shell_state.rs` (near `lookup_var`):

```rust
    /// True if the named variable/parameter is currently **set** (a
    /// set-but-empty variable counts as set; unset is false). Backs
    /// `[[ -v NAME ]]` and `test -v NAME`. Supports scalar names and
    /// positional parameters; array-element forms (`arr[i]`) are out of
    /// scope (M-14b) and fall through to a plain-name lookup (→ false).
    pub fn is_set(&self, name: &str) -> bool {
        // Always-defined special parameters.
        match name {
            "0" | "$" | "#" | "-" | "?" => return true,
            "!" => return self.last_bg_pid.is_some(),
            _ => {}
        }
        // Positional parameter: `1`, `2`, …
        if !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) {
            return name
                .parse::<usize>()
                .map(|n| n >= 1 && n <= self.positional_args.len())
                .unwrap_or(false);
        }
        self.vars.contains_key(name)
    }
```

> `self.vars` is the private `HashMap<String, Variable>` field; `is_set` is a method on `Shell` in the same module, so direct access is fine (matches the existing `self.vars.contains_key(name)` use elsewhere in the file).

- [ ] **Step 4: Run, verify `is_set` tests pass**

Run: `cargo test --bin huck is_set 2>&1 | tail` → pass.

- [ ] **Step 5: Add `-v` to the `[[` parser + AST + eval**

In `src/command.rs`, extend the `TestUnaryOp` enum:

```rust
pub enum TestUnaryOp {
    FileExists, IsRegFile, IsDir, IsReadable, IsWritable, IsExecutable,
    IsNonEmpty, IsSymlink, StringNonEmpty, StringEmpty,
    VarSet,          // -v  (variable is set)
}
```

In `try_unary_op`, add a mapping arm:

```rust
        "-v" => Some(TestUnaryOp::VarSet),
```

In `src/executor.rs` `eval_test_expr`, change the `Unary` arm to intercept `VarSet` (which needs `&Shell`) before `eval_unary`:

```rust
        TestExpr::Unary { op, operand } => {
            let s = expand_assignment(operand, shell);
            if matches!(op, TestUnaryOp::VarSet) {
                return Ok(shell.is_set(&s));
            }
            Ok(eval_unary(*op, &s))
        }
```

> `eval_unary` is unchanged — it never receives `VarSet`. If the `match` in `eval_unary` is non-exhaustive after adding the enum variant, add `TestUnaryOp::VarSet => unreachable!("VarSet handled in eval_test_expr")` to satisfy the compiler.

- [ ] **Step 6: Add `-v` to `test_builtin` via a `var_is_set` predicate**

In `src/test_builtin.rs`:

(a) Add `-v` to `is_unary_op`:

```rust
fn is_unary_op(s: &str) -> bool {
    matches!(
        s,
        "-a" | "-e" | "-f" | "-d" | "-r" | "-w" | "-x" | "-s" | "-L" | "-z" | "-n" | "-v"
    )
}
```

(b) Introduce the predicate-carrying entry. Rename the current `evaluate` body to `evaluate_with`, add the predicate param, and make `evaluate` a thin wrapper:

```rust
/// Evaluates a `test` expression. `var_is_set(name)` answers `-v NAME`.
pub fn evaluate_with(args: &[String], var_is_set: &dyn Fn(&str) -> bool) -> Result<bool, String> {
    match args.len() {
        0 => return Ok(false),
        1 => return Ok(!args[0].is_empty()),
        _ => {}
    }
    if args.len() <= 4
        && let Ok(b) = evaluate_short_form(args, var_is_set)
    {
        return Ok(b);
    }
    let mut p = Parser { args, pos: 0, dry_run: false, var_is_set };
    let result = p.parse_expr()?;
    if p.pos != args.len() {
        return Err(format!("{}: unexpected argument", args[p.pos]));
    }
    Ok(result)
}

/// Back-compat entry: no variables are considered set (`-v` → false).
/// Used by every caller that doesn't evaluate `-v` (all existing unit
/// tests and `[[`'s file-test delegation).
pub fn evaluate(args: &[String]) -> Result<bool, String> {
    evaluate_with(args, &|_| false)
}
```

(c) Thread `var_is_set` into `evaluate_short_form`, `apply_unary`, and `Parser`. Change their signatures and the recursive `evaluate(` calls inside `evaluate_short_form` to `evaluate_with(..., var_is_set)`:

```rust
fn evaluate_short_form(args: &[String], var_is_set: &dyn Fn(&str) -> bool) -> Result<bool, String> {
    match args.len() {
        2 => {
            if args[0] == "!" {
                negate(evaluate_with(&args[1..2], var_is_set))
            } else if is_unary_op(&args[0]) {
                apply_unary(&args[0], &args[1], var_is_set)
            } else {
                Err(format!("{}: unary operator expected", args[0]))
            }
        }
        3 => {
            if is_binary_op(&args[1]) {
                apply_binary(&args[1], &args[0], &args[2])
            } else if args[0] == "!" {
                negate(evaluate_with(&args[1..3], var_is_set))
            } else {
                Err(format!("{}: binary operator expected", args[1]))
            }
        }
        4 => {
            if args[0] == "!" {
                negate(evaluate_with(&args[1..4], var_is_set))
            } else {
                Err("too many arguments".to_string())
            }
        }
        _ => Err("too many arguments".to_string()),
    }
}
```

Add the `-v` arm to `apply_unary` (new signature):

```rust
fn apply_unary(op: &str, operand: &str, var_is_set: &dyn Fn(&str) -> bool) -> Result<bool, String> {
    match op {
        "-v" => Ok(var_is_set(operand)),
        // ... all existing arms unchanged ...
    }
}
```

Add the field to `Parser` and thread it where `Parser` calls `apply_unary` (in `parse_unary`/`parse_primary` — wherever `apply_unary(op, word)` is invoked, pass `self.var_is_set`):

```rust
struct Parser<'a> {
    args: &'a [String],
    pos: usize,
    dry_run: bool,
    var_is_set: &'a dyn Fn(&str) -> bool,
}
```

> Find every `apply_unary(` / `apply_binary(` call inside `Parser` and the short-form (7 call sites total per grep) and update to the new signatures. `apply_binary` does NOT take the predicate (file/int/string ops only).

- [ ] **Step 7: Thread `&Shell` into `builtin_test` for the `test`/`[` path**

In `src/builtins.rs`, change `builtin_test` to accept `shell` and call `evaluate_with` with a closure over `shell.is_set`:

```rust
fn builtin_test(name: &str, args: &[String], shell: &Shell) -> ExecOutcome {
    let eval_args: &[String] = if name == "[" {
        match args.last() {
            Some(last) if last == "]" => &args[..args.len() - 1],
            _ => {
                eprintln!("huck: [: missing ']'");
                return ExecOutcome::Continue(2);
            }
        }
    } else {
        args
    };
    match crate::test_builtin::evaluate_with(eval_args, &|n| shell.is_set(n)) {
        Ok(true) => ExecOutcome::Continue(0),
        Ok(false) => ExecOutcome::Continue(1),
        Err(msg) => {
            eprintln!("huck: {name}: {msg}");
            ExecOutcome::Continue(2)
        }
    }
}
```

Update the dispatch (`src/builtins.rs:119`):

```rust
        "test" | "[" => builtin_test(name, args, shell),
```

> The builtin dispatch `match` already has `shell` in scope (other arms like `"set" => builtin_set(args, out, shell)` use it). If `builtin_test`'s arm currently passes only `(name, args)`, add `shell`. `&Shell` (immutable) is sufficient.

- [ ] **Step 8: Write `-v` tests (unit + integration)**

Add to `src/test_builtin.rs` tests:

```rust
#[test]
fn test_v_uses_predicate() {
    let setvars = ["HOME", "EMPTYVAR"];
    let pred = |n: &str| setvars.contains(&n);
    assert_eq!(evaluate_with(&["-v".into(), "HOME".into()], &pred), Ok(true));
    assert_eq!(evaluate_with(&["-v".into(), "NOPE".into()], &pred), Ok(false));
}
```

Append to `tests/dbracket_multiline_integration.rs`:

```rust
#[test]
fn dbracket_v_set_and_unset() {
    assert_eq!(run("x=1\n[[ -v x ]] && echo set || echo unset\n").0, "set\n");
    assert_eq!(run("y=\"\"\n[[ -v y ]] && echo set || echo unset\n").0, "set\n"); // set-but-empty
    assert_eq!(run("unset z\n[[ -v z ]] && echo set || echo unset\n").0, "unset\n");
}

#[test]
fn test_builtin_v_set_and_unset() {
    assert_eq!(run("x=1\n[ -v x ] && echo set || echo unset\n").0, "set\n");
    assert_eq!(run("unset z\n[ -v z ] && echo set || echo unset\n").0, "unset\n");
}
```

- [ ] **Step 9: Run all tests + bash parity**

Run: `cargo test --bin huck is_set 2>&1 | tail; cargo test --bin huck test_v 2>&1 | tail`
Run: `cargo test --test dbracket_multiline_integration 2>&1 | tail`
Bash parity: for `x=1; [[ -v x ]] && echo S || echo U`, `unset z; [[ -v z ]]...`, and the `[ -v ... ]` forms — compare bash vs huck stdout (must match: `S`/`U`).
Full suite: `cargo test 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'` → FAIL=0.
Clippy: `cargo clippy --all-targets 2>&1 | tail -3` → clean.

- [ ] **Step 10: Commit**

```bash
git add src/shell_state.rs src/command.rs src/executor.rs src/test_builtin.rs src/builtins.rs tests/dbracket_multiline_integration.rs
git commit -m "v87 task 2: -v (variable-is-set) in [[ ]] and test/[

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: `-nt` / `-ot` / `-ef` (file age/identity) in `[[ ]]` and `test`/`[`

**Files:**
- Modify: `src/test_builtin.rs` — `is_binary_op`, `apply_binary`, new `compare_files`
- Modify: `src/command.rs` — `TestBinaryOp` enum (+3), binary-op parse arms
- Modify: `src/executor.rs` — `eval_binary` (+3 delegating arms)
- Test: `src/test_builtin.rs` unit; `tests/dbracket_multiline_integration.rs`

- [ ] **Step 1: Write failing `compare_files` unit tests**

Add to `src/test_builtin.rs` tests (uses a temp dir with controlled mtimes):

```rust
#[test]
fn compare_files_nt_ot_ef() {
    use std::io::Write;
    let dir = std::env::temp_dir().join(format!("huck_ntot_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let old = dir.join("old"); let new = dir.join("new");
    std::fs::File::create(&old).unwrap().write_all(b"x").unwrap();
    // Force `new` to be strictly newer.
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::File::create(&new).unwrap().write_all(b"y").unwrap();
    let (o, n) = (old.to_str().unwrap(), new.to_str().unwrap());
    let missing = dir.join("missing"); let m = missing.to_str().unwrap();

    assert!(compare_files("-nt", n, o));          // new newer than old
    assert!(!compare_files("-nt", o, n));
    assert!(compare_files("-ot", o, n));          // old older than new
    assert!(compare_files("-nt", n, m));          // exists -nt missing → true
    assert!(!compare_files("-nt", m, n));         // missing -nt exists → false
    assert!(compare_files("-ot", m, n));          // missing -ot exists → true
    assert!(!compare_files("-nt", m, m));         // both missing → false
    assert!(compare_files("-ef", o, o));          // same path → same inode
    assert!(!compare_files("-ef", o, n));
    assert!(!compare_files("-ef", m, m));         // missing -ef → false
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test --bin huck compare_files 2>&1 | tail` → fails (`compare_files` undefined).

- [ ] **Step 3: Implement `compare_files` + wire into `test_builtin`**

In `src/test_builtin.rs`, add the helper:

```rust
/// `-nt`/`-ot`/`-ef` for the `test`/`[` and `[[ ]]` constructs. A missing
/// file is treated as the oldest possible mtime; `-ef` requires both files
/// to exist (bash 5.2 semantics).
pub(crate) fn compare_files(op: &str, lhs: &str, rhs: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    let lm = std::fs::metadata(lhs);
    let rm = std::fs::metadata(rhs);
    let nanos = |m: &std::fs::Metadata| (m.mtime() as i128) * 1_000_000_000 + (m.mtime_nsec() as i128);
    match op {
        "-nt" => match (&lm, &rm) {
            (Ok(a), Ok(b)) => nanos(a) > nanos(b),
            (Ok(_), Err(_)) => true,   // lhs exists, rhs missing
            _ => false,                // lhs missing
        },
        "-ot" => match (&lm, &rm) {
            (Ok(a), Ok(b)) => nanos(a) < nanos(b),
            (Err(_), Ok(_)) => true,   // lhs missing, rhs exists
            _ => false,
        },
        "-ef" => match (&lm, &rm) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        },
        _ => false,
    }
}
```

Add the three ops to `is_binary_op`:

```rust
fn is_binary_op(s: &str) -> bool {
    matches!(
        s,
        "=" | "==" | "!=" | "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge"
            | "-nt" | "-ot" | "-ef"
    )
}
```

Add the arm to `apply_binary` (before the final `_ =>`):

```rust
        "-nt" | "-ot" | "-ef" => Ok(compare_files(op, lhs, rhs)),
```

- [ ] **Step 4: Run unit tests, verify pass**

Run: `cargo test --bin huck compare_files 2>&1 | tail` → pass.
Run: `cargo test --bin huck --lib test_builtin 2>&1 | tail` → existing test_builtin tests still pass.

- [ ] **Step 5: Add the operators to the `[[` parser + AST + eval**

In `src/command.rs`, extend `TestBinaryOp`:

```rust
pub enum TestBinaryOp {
    StringEq, StringNe, StringLt, StringGt,
    IntEq, IntNe, IntLt, IntGt, IntLe, IntGe,
    NewerThan,   // -nt
    OlderThan,   // -ot
    SameFile,    // -ef
}
```

In `parse_test_atom`'s operator `match op_text.as_str()`, add three arms (next to `-eq` etc.):

```rust
                "-nt" => { let rhs = next_test_word(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::NewerThan, lhs, rhs }) }
                "-ot" => { let rhs = next_test_word(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::OlderThan, lhs, rhs }) }
                "-ef" => { let rhs = next_test_word(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::SameFile, lhs, rhs }) }
```

In `src/executor.rs` `eval_binary`, add an arm (the function already has `op`, `lhs: &str`, `rhs_word`, `shell`):

```rust
        TestBinaryOp::NewerThan | TestBinaryOp::OlderThan | TestBinaryOp::SameFile => {
            let rhs = expand_assignment(rhs_word, shell);
            let op_str = match op {
                TestBinaryOp::NewerThan => "-nt",
                TestBinaryOp::OlderThan => "-ot",
                TestBinaryOp::SameFile => "-ef",
                _ => unreachable!(),
            };
            Ok(crate::test_builtin::compare_files(op_str, lhs, &rhs))
        }
```

> If `eval_binary`'s `match` is otherwise exhaustive, this new arm makes it cover the three new variants. Ensure `use` / path access to `test_builtin::compare_files` (it is `pub(crate)`).

- [ ] **Step 6: Integration tests for the file operators in both constructs**

Append to `tests/dbracket_multiline_integration.rs` (reuse a temp-dir fixture; create files with a sleep so mtimes differ, plus a hard link for `-ef`):

```rust
use std::fs;

fn run_in_dir(setup: &dyn Fn(&std::path::Path), script: &str) -> (String, i32) {
    let dir = std::env::temp_dir().join(format!("huck_v87_{}_{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    setup(&dir);
    let mut child = Command::new(huck_bin())
        .current_dir(&dir)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    let _ = fs::remove_dir_all(&dir);
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

fn make_old_new_link(dir: &std::path::Path) {
    fs::write(dir.join("old"), b"o").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    fs::write(dir.join("new"), b"n").unwrap();
    fs::hard_link(dir.join("new"), dir.join("link")).unwrap();
}

#[test]
fn dbracket_file_ops() {
    assert_eq!(run_in_dir(&make_old_new_link, "[[ new -nt old ]] && echo nt\n").0, "nt\n");
    assert_eq!(run_in_dir(&make_old_new_link, "[[ old -ot new ]] && echo ot\n").0, "ot\n");
    assert_eq!(run_in_dir(&make_old_new_link, "[[ new -ef link ]] && echo ef\n").0, "ef\n");
    assert_eq!(run_in_dir(&make_old_new_link, "[[ old -ef new ]] && echo ef || echo no\n").0, "no\n");
}

#[test]
fn test_builtin_file_ops() {
    assert_eq!(run_in_dir(&make_old_new_link, "[ new -nt old ] && echo nt\n").0, "nt\n");
    assert_eq!(run_in_dir(&make_old_new_link, "[ new -ef link ] && echo ef\n").0, "ef\n");
}
```

- [ ] **Step 7: Run all + bash parity**

Run: `cargo test --test dbracket_multiline_integration 2>&1 | tail -15` → all pass.
Bash parity (in a temp dir with `touch -d` two files + a hard link): compare `[[ new -nt old ]] && echo nt`, `[[ x -ef y ]]`, and `[ ... ]` forms — bash vs huck stdout must match.
Full suite: `cargo test 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'` → FAIL=0.
Clippy: `cargo clippy --all-targets 2>&1 | tail -3` → clean.

- [ ] **Step 8: Commit**

```bash
git add src/test_builtin.rs src/command.rs src/executor.rs tests/dbracket_multiline_integration.rs
git commit -m "v87 task 3: -nt/-ot/-ef in [[ ]] and test/[

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: bash-diff harness + docs

**Files:**
- Create: `tests/scripts/dbracket_multiline_diff_check.sh` (huck's 14th harness)
- Modify: `docs/bash-divergences.md`, `README.md`

- [ ] **Step 1: Write the harness**

Create `tests/scripts/dbracket_multiline_diff_check.sh`, modeled on `tests/scripts/bang_negation_diff_check.sh` (`set -u`, `HUCK_BIN` resolution + `[[ -x ]]` guard, `check()` comparing bash vs `$HUCK_BIN` with `EXIT:$?` appended, totals, `exit $((FAIL>0?1:0))`). `chmod +x`. Use a `mktemp -d` fixture for the file operators. Fragments are multi-line (real newlines) so both shells exercise their continuation readers.

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v87: multi-line [[ ]] + test ops (M-14a).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
FIX="$(mktemp -d)"; trap 'rm -rf "$FIX"' EXIT
: > "$FIX/old"; sleep 1; : > "$FIX/new"; ln "$FIX/new" "$FIX/link"

# Multi-line continuation (real newlines inside the fragment).
check "break before ]]"   $'[[ -f /etc/passwd\n]] && echo yes'
check "break after &&"    $'[[ -f /etc/passwd &&\n -f /etc/hosts ]] && echo both'
check "break after [["    $'[[\n -f /etc/passwd ]] && echo opened'
check "break after operand" $'[[ abc\n== abc ]] && echo eq'
check "single-line still"  '[[ -f /etc/passwd ]] && echo ok'
check "echo [[ literal"    'echo [['
# -v
check "v set"              'x=1; [[ -v x ]] && echo S || echo U'
check "v empty"            'y=; [[ -v y ]] && echo S || echo U'
check "v unset"            'unset z; [[ -v z ]] && echo S || echo U'
check "test -v set"        'x=1; [ -v x ] && echo S || echo U'
check "test -v unset"      'unset z; [ -v z ] && echo S || echo U'
# -nt/-ot/-ef
check "nt"                 "[[ '$FIX/new' -nt '$FIX/old' ]] && echo nt || echo no"
check "ot"                 "[[ '$FIX/old' -ot '$FIX/new' ]] && echo ot || echo no"
check "ef hardlink"        "[[ '$FIX/new' -ef '$FIX/link' ]] && echo ef || echo no"
check "ef different"       "[[ '$FIX/old' -ef '$FIX/new' ]] && echo ef || echo no"
check "nt missing rhs"     "[[ '$FIX/new' -nt '$FIX/missing' ]] && echo nt || echo no"
check "test nt"            "[ '$FIX/new' -nt '$FIX/old' ] && echo nt || echo no"
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Run the harness, confirm all PASS**

```bash
cd /home/john/projects/shuck
cargo build 2>&1 | tail -1
chmod +x tests/scripts/dbracket_multiline_diff_check.sh
bash tests/scripts/dbracket_multiline_diff_check.sh; echo "rc=$?"
```
Expected: `Fail: 0`, `rc=0`. If any fragment FAILs only on a `huck:` vs `bash: line N:` error-prefix difference, relocate it to an integration test (assert rc only) with a NOTE comment — do NOT weaken `check()`. (The chosen fragments avoid error-text comparison: all evaluate to clean stdout.)

- [ ] **Step 3: Update `docs/bash-divergences.md`**

Read the M-14 entry (~line 181) and the Tier-2 count line (~25) first.

1. In the M-14 entry, change the trailing "Out of scope:" sentence — `-v var`, `-nt`/`-ot`/`-ef` are no longer out of scope. Reword to: "Out of scope: bash arrays as operands. (`-v`, `-nt`/`-ot`/`-ef` shipped in v87 — see M-14a.)"
2. Add a new sub-entry after M-14:
```markdown
- **M-14a: multi-line `[[ ]]` + `-v`/`-nt`/`-ot`/`-ef`** — `[fixed v87]` medium.
  A `[[ … ]]` whose `]]` is on a later line, or whose expression is line-broken
  after `[[`, an operand, or `&&`/`||`, now gathers continuation input: the `[[`
  parser returns `UnterminatedDoubleBracket` on end-of-input-before-`]]` (vs the
  old misleading `TestExprMissingOperand`), and `continuation::classify` maps it
  to a new `DoubleBracket` reason with a space joiner. `[[ x == ]]` (terminator
  present, operand missing) stays a genuine error; `echo [[` stays a literal arg.
  Added the four operators M-14 omitted, in BOTH `[[ ]]` and `test`/`[`: `-v NAME`
  (variable-is-set; set-but-empty ⇒ true; new `Shell::is_set`), and
  `-nt`/`-ot`/`-ef` (file newer/older/same-inode; missing file = oldest;
  shared `test_builtin::compare_files`). Discovered loading a Debian `~/.bashrc`'s
  bash-completion (line-broken `[[` conditions + `-v`). huck's 14th bash-diff harness.
- **M-14b: `[[ -v arr[i] ]]` array-element form** — `[deferred]` low. `-v` supports
  scalar names and positional parameters; the array-element subscript form
  (`-v arr[1]`) is not parsed — the name falls through to a plain-name lookup
  (→ false). bash checks the specific element. Rarely used; bash-completion's
  `-v` uses are plain names.
```
3. Bump the Tier-2 count line (~25) by 1 and append `; M-14a fixed by v87, with M-14b added as a new low-priority deferred follow-on`.
4. Update the "Last updated" stamp (line 3) to `2026-06-04 (after v87 multi-line [[ ]] + test operators; M-14a fixed)`.
5. Add a changelog entry at the END (match the v86 entry's format), dated 2026-06-04, summarizing the parser refinement, the classifier mapping, `Shell::is_set`, the `var_is_set` predicate in `test_builtin`, `compare_files`, and the 14th harness.

- [ ] **Step 4: Update `README.md`**

Read the iteration table + the v86 row first. Add a v87 row in the same column format:
```markdown
| v87 | multi-line `[[ ]]` + test ops (M-14a) | Line-broken `[[ … ]]` conditions now continue across lines; added `-v`/`-nt`/`-ot`/`-ef` to both `[[ ]]` and `test`/`[` |
```
(Match the exact columns/escaping of existing rows — escape literal `|` as `\|`.)

- [ ] **Step 5: Verify whole branch**

```bash
cargo test 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'   # FAIL=0
cargo clippy --all-targets 2>&1 | tail -3                                                       # clean
for f in tests/scripts/*_diff_check.sh; do printf '%s: ' "$f"; bash "$f" >/dev/null 2>&1 && echo OK || echo FAIL; done  # all 14 OK
```

- [ ] **Step 6: Commit**

```bash
git add tests/scripts/dbracket_multiline_diff_check.sh docs/bash-divergences.md README.md
git commit -m "v87 task 4: multi-line [[ ]] bash-diff harness + docs (M-14a)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the implementer

- **Binary crate:** `cargo test --bin huck <filter>` for unit tests; `cargo test --test dbracket_multiline_integration` for integration; `cargo test` for everything.
- **The continuation fix is parse-error-driven, not depth-counting** — do not add `[[`/`]]` token counting to `classify` (it would misclassify `echo [[`). The whole mechanism is: parser returns `UnterminatedDoubleBracket` on EOF-before-`]]`; `classify` maps that one error.
- **`evaluate` stays a zero-arg-predicate wrapper** so the ~85 existing `test_builtin` call sites and `[[`'s file-test delegation (`eval_unary`) keep compiling unchanged. Only `builtin_test` and new `-v` tests call `evaluate_with`.
- **`-v` is intercepted in `eval_test_expr`** for `[[` (it has `&Shell`); `eval_unary` never sees `VarSet`.
- **Filesystem timing in tests:** create the "newer" file after a ≥10 ms sleep (unit) / `sleep 1` (harness) so mtimes differ deterministically; `compare_files` compares nanosecond mtime.
- **Don't weaken the harness:** if a fragment can't be byte-identical due to an error-text prefix, relocate it to an rc-only integration test with a NOTE.
