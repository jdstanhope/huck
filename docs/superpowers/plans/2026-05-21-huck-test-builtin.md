# huck v16: The `test` / `[` Builtin Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the POSIX `test` builtin and its `[` alias — file tests, string tests, integer comparisons, and `!` negation — with the POSIX argument-count evaluation algorithm.

**Architecture:** A new `src/test_builtin.rs` module owns `evaluate(args) -> Result<bool, String>` (the arg-count recursion plus operator helpers). `builtins.rs` gains `"test"`/`"["` in `BUILTIN_NAMES`, a dispatch arm, and a thin `builtin_test` wrapper that handles the `[`-form's trailing `]` and maps the result to an exit status (0 true / 1 false / 2 usage error). No lexer/parser/AST/executor change.

**Tech Stack:** Rust 2024 edition. `libc` (already a dependency) for `access`. No new dependencies.

**Reference:** Design spec at `docs/superpowers/specs/2026-05-21-huck-test-builtin-design.md`.

**Note on error prefixes:** The spec sketched error messages prefixed `huck: test:`. This plan uses `huck: {name}:` so a `[`-invoked error reads `huck: [: ...` and a `test`-invoked one reads `huck: test: ...` — matching bash and consistent with the spec's own `huck: [: missing ']'`.

---

## File Map

- **New:** `src/test_builtin.rs` — `evaluate`, the arg-count recursion, operator helpers, unit tests
- **New:** `tests/test_builtin_integration.rs` — end-to-end via the shell binary
- **Modify:** `src/builtins.rs` — `BUILTIN_NAMES` += `"test"`, `"["`; dispatch arm; `builtin_test` wrapper
- **Modify:** `src/main.rs` — register `mod test_builtin`
- **Modify:** `README.md` — v16 row, builtins list, features note, test count

---

## Task 1: `test_builtin` module — arg-count recursion

Create `src/test_builtin.rs` with the complete `evaluate` argument-count recursion, the `is_unary_op`/`is_binary_op` predicates, and `apply_unary`/`apply_binary` as stubs (filled in Tasks 2-3).

**Files:**
- Create: `src/test_builtin.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Create `src/test_builtin.rs`**

```rust
//! The `test` / `[` builtin: conditional expression evaluation.
//!
//! `evaluate` implements the POSIX argument-count algorithm. Operator
//! application lives in `apply_unary` / `apply_binary`.

/// Evaluates a `test` expression. `Ok(true)` / `Ok(false)` are the
/// result; `Err(message)` is a usage error.
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

/// Negates a result; a usage error stays a usage error.
fn negate(r: Result<bool, String>) -> Result<bool, String> {
    r.map(|b| !b)
}

/// True if `s` is a recognized unary operator.
fn is_unary_op(s: &str) -> bool {
    matches!(
        s,
        "-e" | "-f" | "-d" | "-r" | "-w" | "-x" | "-s" | "-L" | "-z" | "-n"
    )
}

/// True if `s` is a recognized binary operator.
fn is_binary_op(s: &str) -> bool {
    matches!(
        s,
        "=" | "==" | "!=" | "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge"
    )
}

/// Applies a unary operator. Implemented in Task 2.
fn apply_unary(op: &str, _operand: &str) -> Result<bool, String> {
    Err(format!("{op}: operator not yet implemented"))
}

/// Applies a binary operator. Implemented in Task 3.
fn apply_binary(op: &str, _lhs: &str, _rhs: &str) -> Result<bool, String> {
    Err(format!("{op}: operator not yet implemented"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn zero_args_is_false() {
        assert_eq!(evaluate(&[]), Ok(false));
    }

    #[test]
    fn one_arg_nonempty_is_true() {
        assert_eq!(evaluate(&args(&["x"])), Ok(true));
    }

    #[test]
    fn one_arg_empty_is_false() {
        assert_eq!(evaluate(&args(&[""])), Ok(false));
    }

    #[test]
    fn one_arg_operatorlike_is_just_truthiness() {
        // With one argument, `-f` is only a non-empty string.
        assert_eq!(evaluate(&args(&["-f"])), Ok(true));
        assert_eq!(evaluate(&args(&["!"])), Ok(true));
    }

    #[test]
    fn bang_negates_one_arg_truthiness() {
        assert_eq!(evaluate(&args(&["!", "x"])), Ok(false));
        assert_eq!(evaluate(&args(&["!", ""])), Ok(true));
    }

    #[test]
    fn two_args_unknown_operator_is_usage_error() {
        assert!(evaluate(&args(&["foo", "bar"])).is_err());
    }

    #[test]
    fn three_args_no_operator_no_bang_is_usage_error() {
        assert!(evaluate(&args(&["a", "b", "c"])).is_err());
    }

    #[test]
    fn five_or_more_args_is_usage_error() {
        assert!(evaluate(&args(&["a", "b", "c", "d", "e"])).is_err());
    }

    #[test]
    fn four_args_without_leading_bang_is_usage_error() {
        assert!(evaluate(&args(&["a", "b", "c", "d"])).is_err());
    }
}
```

- [ ] **Step 2: Register the module**

Edit `src/main.rs`. Add `mod test_builtin;` alphabetically with the other `mod` declarations.

- [ ] **Step 3: Run tests**

Run: `cargo test test_builtin::`
Expected: 9 tests pass. (`apply_unary`/`apply_binary` stubs are intentionally unexercised here — the recursion bottoms out at 0/1-arg cases.)

- [ ] **Step 4: Run full suite**

Run: `cargo test`
Expected: all tests pass (579 baseline + 9 new = 588).

- [ ] **Step 5: Commit**

```bash
git add src/test_builtin.rs src/main.rs
git commit -m "v16 task 1: test_builtin module — argument-count recursion"
```

## Context

- Working directory: `/home/john/projects/shuck`
- Branch: `feature/test-builtin`
- Baseline: 579 tests passing
- `evaluate` is the complete POSIX arg-count algorithm. `is_unary_op`/`is_binary_op` are real; `apply_unary`/`apply_binary` are stubs filled in Tasks 2-3. The `!` recursion is fully working in Task 1 for cases that bottom out at 0/1 args.

## Self-Review

- Do all 9 tests pass?
- Does `cargo build` succeed?
- Do all 579 pre-existing tests still pass?

## Report Format

Status, test count, commit SHA, any concerns.

---

## Task 2: Unary operators

Implement `apply_unary` — file tests (`-e`/`-f`/`-d`/`-r`/`-w`/`-x`/`-s`/`-L`) and string tests (`-z`/`-n`).

**Files:**
- Modify: `src/test_builtin.rs`

- [ ] **Step 1: Write failing tests**

Append to the `tests` module in `src/test_builtin.rs`:

```rust
#[test]
fn unary_string_z_and_n() {
    assert_eq!(evaluate(&args(&["-z", ""])), Ok(true));
    assert_eq!(evaluate(&args(&["-z", "x"])), Ok(false));
    assert_eq!(evaluate(&args(&["-n", "x"])), Ok(true));
    assert_eq!(evaluate(&args(&["-n", ""])), Ok(false));
}

#[test]
fn unary_file_exists_and_type() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("regular");
    std::fs::write(&file, b"data").unwrap();
    let subdir = dir.path().join("sub");
    std::fs::create_dir(&subdir).unwrap();
    let file_s = file.to_str().unwrap();
    let dir_s = subdir.to_str().unwrap();

    assert_eq!(evaluate(&args(&["-e", file_s])), Ok(true));
    assert_eq!(evaluate(&args(&["-e", dir_s])), Ok(true));
    assert_eq!(evaluate(&args(&["-f", file_s])), Ok(true));
    assert_eq!(evaluate(&args(&["-f", dir_s])), Ok(false));
    assert_eq!(evaluate(&args(&["-d", dir_s])), Ok(true));
    assert_eq!(evaluate(&args(&["-d", file_s])), Ok(false));
}

#[test]
fn unary_file_nonexistent_is_false_not_error() {
    let r = evaluate(&args(&["-f", "/no/such/huck/path"]));
    assert_eq!(r, Ok(false));
}

#[test]
fn unary_size_nonempty() {
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty");
    std::fs::write(&empty, b"").unwrap();
    let full = dir.path().join("full");
    std::fs::write(&full, b"content").unwrap();
    assert_eq!(evaluate(&args(&["-s", empty.to_str().unwrap()])), Ok(false));
    assert_eq!(evaluate(&args(&["-s", full.to_str().unwrap()])), Ok(true));
}

#[test]
fn unary_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    std::fs::write(&target, b"x").unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    // -L is true for the link, false for the regular target.
    assert_eq!(evaluate(&args(&["-L", link.to_str().unwrap()])), Ok(true));
    assert_eq!(evaluate(&args(&["-L", target.to_str().unwrap()])), Ok(false));
    // -f follows the symlink, so it sees a regular file.
    assert_eq!(evaluate(&args(&["-f", link.to_str().unwrap()])), Ok(true));
}

#[test]
fn unary_readable_true_case() {
    // A freshly written file is readable by its owner. (The false case
    // for -r/-w/-x is environment-sensitive — root bypasses access —
    // so only the robust true case is asserted here.)
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("readable");
    std::fs::write(&file, b"x").unwrap();
    assert_eq!(evaluate(&args(&["-r", file.to_str().unwrap()])), Ok(true));
    assert_eq!(evaluate(&args(&["-w", file.to_str().unwrap()])), Ok(true));
}

#[test]
fn unary_negation_over_file_test() {
    // `! -f <nonexistent>` — 3 args, bang over a 2-arg unary test.
    let r = evaluate(&args(&["!", "-f", "/no/such/huck/path"]));
    assert_eq!(r, Ok(true));
}

#[test]
fn unary_unknown_operator_in_one_arg_position_is_truthiness() {
    // Sanity: a lone `-q` (not a real op) with one arg is truthiness.
    assert_eq!(evaluate(&args(&["-q"])), Ok(true));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test test_builtin::tests::unary_`
Expected: FAIL — `apply_unary` is still the stub returning `Err`.

- [ ] **Step 3: Implement `apply_unary`**

In `src/test_builtin.rs`, replace the stub `apply_unary` with:

```rust
/// Applies a unary operator to its operand.
fn apply_unary(op: &str, operand: &str) -> Result<bool, String> {
    match op {
        "-z" => Ok(operand.is_empty()),
        "-n" => Ok(!operand.is_empty()),
        "-e" => Ok(std::fs::metadata(operand).is_ok()),
        "-f" => Ok(std::fs::metadata(operand)
            .map(|m| m.is_file())
            .unwrap_or(false)),
        "-d" => Ok(std::fs::metadata(operand)
            .map(|m| m.is_dir())
            .unwrap_or(false)),
        "-s" => Ok(std::fs::metadata(operand)
            .map(|m| m.len() > 0)
            .unwrap_or(false)),
        "-L" => Ok(std::fs::symlink_metadata(operand)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)),
        "-r" => Ok(access(operand, libc::R_OK)),
        "-w" => Ok(access(operand, libc::W_OK)),
        "-x" => Ok(access(operand, libc::X_OK)),
        _ => Err(format!("{op}: unknown operator")),
    }
}

/// True if the calling process can access `path` with `mode`
/// (`libc::R_OK` / `W_OK` / `X_OK`), per `access(2)`.
fn access(path: &str, mode: i32) -> bool {
    use std::ffi::CString;
    let Ok(c_path) = CString::new(path) else {
        return false;
    };
    unsafe { libc::access(c_path.as_ptr(), mode) == 0 }
}
```

Add `use libc;` at the top of the file if the crate is not already in scope (other modules use `use libc;`).

- [ ] **Step 4: Run tests**

Run: `cargo test test_builtin::`
Expected: all `test_builtin` tests pass (9 from Task 1 + 8 new = 17).

- [ ] **Step 5: Run full suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/test_builtin.rs
git commit -m "v16 task 2: test unary operators (file + string tests)"
```

## Context

- Working directory: `/home/john/projects/shuck`
- Branch: `feature/test-builtin`
- Baseline: 588 tests passing
- `-e`/`-f`/`-d`/`-s` use `std::fs::metadata` (follows symlinks). `-L` uses `symlink_metadata`. `-r`/`-w`/`-x` use `libc::access` (real UID/GID).
- A nonexistent path makes a file test `Ok(false)`, never an `Err`.

## Self-Review

- Do all 17 `test_builtin` tests pass?
- Does `-L` correctly distinguish a symlink from its target?
- Do all pre-existing tests still pass?

## Report Format

Status, test count, commit SHA, any concerns.

---

## Task 3: Binary operators

Implement `apply_binary` — string comparison (`=`/`==`/`!=`) and integer comparison (`-eq`/`-ne`/`-lt`/`-le`/`-gt`/`-ge`).

**Files:**
- Modify: `src/test_builtin.rs`

- [ ] **Step 1: Write failing tests**

Append to the `tests` module in `src/test_builtin.rs`:

```rust
#[test]
fn binary_string_equality() {
    assert_eq!(evaluate(&args(&["abc", "=", "abc"])), Ok(true));
    assert_eq!(evaluate(&args(&["abc", "=", "xyz"])), Ok(false));
    assert_eq!(evaluate(&args(&["abc", "==", "abc"])), Ok(true));
    assert_eq!(evaluate(&args(&["abc", "!=", "xyz"])), Ok(true));
    assert_eq!(evaluate(&args(&["abc", "!=", "abc"])), Ok(false));
}

#[test]
fn binary_string_empty_operands() {
    assert_eq!(evaluate(&args(&["", "=", ""])), Ok(true));
    assert_eq!(evaluate(&args(&["", "=", "x"])), Ok(false));
}

#[test]
fn binary_integer_comparisons() {
    assert_eq!(evaluate(&args(&["3", "-eq", "3"])), Ok(true));
    assert_eq!(evaluate(&args(&["3", "-ne", "4"])), Ok(true));
    assert_eq!(evaluate(&args(&["3", "-lt", "10"])), Ok(true));
    assert_eq!(evaluate(&args(&["10", "-lt", "3"])), Ok(false));
    assert_eq!(evaluate(&args(&["3", "-le", "3"])), Ok(true));
    assert_eq!(evaluate(&args(&["10", "-gt", "3"])), Ok(true));
    assert_eq!(evaluate(&args(&["3", "-ge", "3"])), Ok(true));
}

#[test]
fn binary_integer_negative_operands() {
    assert_eq!(evaluate(&args(&["-5", "-lt", "0"])), Ok(true));
    assert_eq!(evaluate(&args(&["-5", "-eq", "-5"])), Ok(true));
}

#[test]
fn binary_integer_non_integer_operand_is_error() {
    assert!(evaluate(&args(&["abc", "-eq", "3"])).is_err());
    assert!(evaluate(&args(&["3", "-eq", "abc"])).is_err());
}

#[test]
fn binary_negation_over_comparison() {
    // `! a = b` — 4 args, bang over a 3-arg binary test.
    assert_eq!(evaluate(&args(&["!", "a", "=", "b"])), Ok(true));
    assert_eq!(evaluate(&args(&["!", "a", "=", "a"])), Ok(false));
}

#[test]
fn binary_negation_over_failing_comparison_stays_error() {
    // `! abc -eq 3` — the inner comparison is a usage error; `!` does
    // not turn that into a boolean.
    assert!(evaluate(&args(&["!", "abc", "-eq", "3"])).is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test test_builtin::tests::binary_`
Expected: FAIL — `apply_binary` is still the stub.

- [ ] **Step 3: Implement `apply_binary`**

In `src/test_builtin.rs`, replace the stub `apply_binary` with:

```rust
/// Applies a binary operator to its two operands.
fn apply_binary(op: &str, lhs: &str, rhs: &str) -> Result<bool, String> {
    match op {
        "=" | "==" => Ok(lhs == rhs),
        "!=" => Ok(lhs != rhs),
        "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge" => {
            let l = parse_int(lhs)?;
            let r = parse_int(rhs)?;
            Ok(match op {
                "-eq" => l == r,
                "-ne" => l != r,
                "-lt" => l < r,
                "-le" => l <= r,
                "-gt" => l > r,
                "-ge" => l >= r,
                _ => unreachable!("checked by the outer match"),
            })
        }
        _ => Err(format!("{op}: unknown operator")),
    }
}

/// Parses a `test` integer operand (decimal `i64`).
fn parse_int(s: &str) -> Result<i64, String> {
    s.parse::<i64>()
        .map_err(|_| "integer expression expected".to_string())
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test test_builtin::`
Expected: all `test_builtin` tests pass (17 from Tasks 1-2 + 7 new = 24).

- [ ] **Step 5: Run full suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/test_builtin.rs
git commit -m "v16 task 3: test binary operators (string + integer)"
```

## Context

- Working directory: `/home/john/projects/shuck`
- Branch: `feature/test-builtin`
- Baseline: 596 tests passing (after Tasks 1-2)
- Integer operands parse as decimal `i64` (a leading `+`/`-` is accepted by `i64::from_str`). A parse failure is a usage error.

## Self-Review

- Do all 24 `test_builtin` tests pass?
- Does `! abc -eq 3` stay an `Err` (negation does not mask a usage error)?
- Do all pre-existing tests still pass?

## Report Format

Status, test count, commit SHA, any concerns.

---

## Task 4: `builtins.rs` wiring

Register `test` and `[` as builtins and add the `builtin_test` wrapper.

**Files:**
- Modify: `src/builtins.rs`

- [ ] **Step 1: Write failing tests**

Add to `src/builtins.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn builtin_test_true_expression() {
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    let args = vec!["-n".to_string(), "x".to_string()];
    let outcome = run_builtin("test", &args, &mut out, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
}

#[test]
fn builtin_test_false_expression() {
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    let args = vec!["-z".to_string(), "x".to_string()];
    let outcome = run_builtin("test", &args, &mut out, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn builtin_test_usage_error() {
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    let args = vec!["3".to_string(), "-eq".to_string(), "abc".to_string()];
    let outcome = run_builtin("test", &args, &mut out, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn builtin_bracket_strips_trailing_bracket() {
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    // `[ -n x ]`
    let args = vec![
        "-n".to_string(),
        "x".to_string(),
        "]".to_string(),
    ];
    let outcome = run_builtin("[", &args, &mut out, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
}

#[test]
fn builtin_bracket_missing_close_is_error() {
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    // `[ -n x` — no closing `]`
    let args = vec!["-n".to_string(), "x".to_string()];
    let outcome = run_builtin("[", &args, &mut out, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn builtin_bracket_empty_is_error() {
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    let outcome = run_builtin("[", &[], &mut out, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test builtin_test_true_expression`
Expected: FAIL — `"test"` is not a recognized builtin (`run_builtin` hits its `unreachable!`).

- [ ] **Step 3: Add `test` and `[` to `BUILTIN_NAMES`**

In `src/builtins.rs`, extend the `BUILTIN_NAMES` constant:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
];
```

- [ ] **Step 4: Add the dispatch arm**

In `run_builtin`'s `match name`, add before the `_ =>` arm:

```rust
"test" | "[" => builtin_test(name, args),
```

- [ ] **Step 5: Implement `builtin_test`**

Add this function alongside the other `builtin_*` functions in `src/builtins.rs`:

```rust
fn builtin_test(name: &str, args: &[String]) -> ExecOutcome {
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
    match crate::test_builtin::evaluate(eval_args) {
        Ok(true) => ExecOutcome::Continue(0),
        Ok(false) => ExecOutcome::Continue(1),
        Err(msg) => {
            eprintln!("huck: {name}: {msg}");
            ExecOutcome::Continue(2)
        }
    }
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test`
Expected: all tests pass (603 baseline + 6 new = 609). The 6 new `builtin_test`/`builtin_bracket` tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/builtins.rs
git commit -m "v16 task 4: wire test and [ into the builtin dispatch"
```

## Context

- Working directory: `/home/john/projects/shuck`
- Branch: `feature/test-builtin`
- Baseline: 603 tests passing (after Task 3)
- `run_builtin(name, args, out, shell)` dispatches by `name`; the `test`/`[` arm passes `name` so `builtin_test` knows which form was invoked.
- `builtin_test` produces no stdout and does not touch `Shell`, so it takes only `name` and `args`.
- Error messages are prefixed `huck: {name}:` so a `[`-invoked error reads `huck: [: ...`.

## Self-Review

- Do all 6 new builtin tests pass?
- Does `[` without a closing `]` return `Continue(2)`?
- Do all pre-existing tests still pass?

## Report Format

Status, test count, commit SHA, any concerns.

---

## Task 5: End-to-end integration tests

**Files:**
- Create: `tests/test_builtin_integration.rs`

- [ ] **Step 1: Create the test file**

Create `tests/test_builtin_integration.rs`:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run(script: &str) -> String {
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
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn test_f_on_existing_file_is_true() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("afile");
    std::fs::write(&file, b"x").unwrap();
    let script = format!("test -f {}\necho $?\nexit\n", file.to_str().unwrap());
    let out = run(&script);
    assert!(out.lines().any(|l| l == "0"), "stdout: {out}");
}

#[test]
fn test_f_on_missing_file_is_false() {
    let out = run("test -f /no/such/huck/path\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "1"), "stdout: {out}");
}

#[test]
fn bracket_d_on_directory_is_true() {
    let dir = tempfile::tempdir().unwrap();
    let script = format!("[ -d {} ]\necho $?\nexit\n", dir.path().to_str().unwrap());
    let out = run(&script);
    assert!(out.lines().any(|l| l == "0"), "stdout: {out}");
}

#[test]
fn bracket_string_equality() {
    let out = run("[ abc = abc ]\necho $?\n[ abc = xyz ]\necho $?\nexit\n");
    let codes: Vec<&str> = out.lines().filter(|l| *l == "0" || *l == "1").collect();
    assert_eq!(codes, vec!["0", "1"], "stdout: {out}");
}

#[test]
fn bracket_integer_comparison() {
    let out = run("[ 3 -lt 10 ]\necho $?\n[ 10 -lt 3 ]\necho $?\nexit\n");
    let codes: Vec<&str> = out.lines().filter(|l| *l == "0" || *l == "1").collect();
    assert_eq!(codes, vec!["0", "1"], "stdout: {out}");
}

#[test]
fn bracket_negation() {
    let out = run("[ ! -f /no/such/huck/path ]\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "0"), "stdout: {out}");
}

#[test]
fn bracket_missing_close_sets_status_two() {
    let out = run("[ -f foo\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "2"), "stdout: {out}");
}

#[test]
fn test_non_integer_operand_sets_status_two() {
    let out = run("test abc -eq 1\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "2"), "stdout: {out}");
}

#[test]
fn bracket_with_expanded_variable() {
    // `[ "$x" = foo ]` with x unset → "" = foo → false (status 1).
    let out = run("[ \"$x\" = foo ]\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "1"), "stdout: {out}");
}
```

- [ ] **Step 2: Run the integration tests**

Run: `cargo test --test test_builtin_integration`
Expected: 9 tests pass.

- [ ] **Step 3: Run the full suite**

Run: `cargo test`
Expected: all tests pass (609 + 9 = 618).

- [ ] **Step 4: Commit**

```bash
git add tests/test_builtin_integration.rs
git commit -m "v16 task 5: end-to-end test/[ integration tests"
```

## Context

- Working directory: `/home/john/projects/shuck`
- Branch: `feature/test-builtin`
- Baseline: 609 tests passing
- `bracket_d_on_directory_is_true` and friends confirm a bare `[` command word reaches the builtin (it survives v10 pathname expansion via the invalid-glob-pattern literal fallback).
- `tempfile` is a dev-dependency.

## Self-Review

- Do all 9 integration tests pass?
- Does `[ -f foo` (no `]`) produce status 2?
- Does the full suite pass?

## Report Format

Status, test count, commit SHA, any concerns.

---

## Task 6: README update

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add the v16 row to the status table**

Append after the v15 row:

```
| v16       | `test` / `[` builtin (file, string, integer tests)      |
```

Match the table's column alignment.

- [ ] **Step 2: Add `test` and `[` to the Builtins list**

Find the builtins enumeration in `README.md` and add `test` and `[` to it.

- [ ] **Step 3: Add a features note**

After the Tab-completion (v14) block (or wherever the most recent feature block sits), add:

```markdown
**Conditionals (v16):**
`test EXPR` and `[ EXPR ]` evaluate file tests (`-e`/`-f`/`-d`/
`-r`/`-w`/`-x`/`-s`/`-L`), string tests (`-z`/`-n`/`=`/`!=`),
and integer comparisons (`-eq`/`-ne`/`-lt`/`-le`/`-gt`/`-ge`),
with `!` negation. Exit status is 0 (true), 1 (false), or 2
(usage error). The `-a`/`-o`/`( )` combinators and `[[ ]]` are
not implemented; `if` is a separate iteration.
```

- [ ] **Step 4: Update the test count**

Run: `cargo test 2>&1 | grep 'test result'` and sum the `passed` counts. Update the `cargo test               # full test suite (NNN tests)` line. Expected total ~579 + 39 = ~618 — use the actual number.

- [ ] **Step 5: Update the Not-yet-implemented section**

The not-yet-implemented list mentions control flow `if`/`while`/`for`/`case`. Leave those (still missing), but no `test` removal is needed unless `test` is explicitly listed — check and adjust if so.

- [ ] **Step 6: Commit**

```bash
git add README.md
git commit -m "v16 task 6: README — add v16 row and test/[ section"
```

---

## Final review checkpoint

After Task 6:

- [ ] `cargo test` shows the expected total passing, 0 failing
- [ ] `cargo clippy -- -D warnings` is clean (or any new warnings are intentional)
- [ ] Manual REPL smoke session: `test -f <file>`, `[ -d <dir> ]`, `[ abc = abc ]`, `[ 3 -lt 10 ]`, `[ ! -f /nonexistent ]`, `[ -f foo` (missing `]`), `test abc -eq 1`, and `echo $?` after each
- [ ] Final review the whole branch as a single diff before merging to main
