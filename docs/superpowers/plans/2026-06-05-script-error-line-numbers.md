# Script-Error Line Numbers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Sourced-file syntax errors (`source`/`.`/`--rcfile`/`huck SCRIPT`) report the physical line number — `huck: FILE: line N: syntax error: MSG` — instead of today's unlocated `huck: FILE: syntax error: MSG`.

**Architecture:** `run_sourced_contents` (src/builtins.rs) already loops over physical lines, accumulating into `buf` until a logical command completes. Add a 1-based physical-line counter and remember the line where the current `buf` started; emit that line in the lex-error and parse-error arms. No lexer/parser/Token changes.

**Tech Stack:** Rust (binary crate `huck`). Integration tests via `tempfile` + the `huck` binary.

---

## File Structure

- `src/builtins.rs` — `run_sourced_contents`: add `physical_line` + `cmd_start_line`; include `line {N}:` in the two error `eprintln!` arms.
- `tests/script_line_numbers_integration.rs` — NEW integration tests (temp script with a known error line, run via the binary, assert `line N:`).
- `docs/bash-divergences.md`, `README.md` — change-log + README v94 row + a Low-impact note on the multi-line limitation.

---

### Task 1: Line tracking in `run_sourced_contents` + integration tests

**Files:**
- Modify: `src/builtins.rs` (`run_sourced_contents`)
- Test: `tests/script_line_numbers_integration.rs` (NEW)

- [ ] **Step 1: Write the failing integration test**

Create `tests/script_line_numbers_integration.rs`:

```rust
//! v94: sourced-script syntax errors report the physical line number.
use std::io::Write;
use std::process::Command;

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Writes `body` to a temp file, runs `huck <file>`, returns (stdout, stderr, code).
fn run_script(body: &str) -> (String, String, i32) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("script.sh");
    std::fs::File::create(&path).unwrap().write_all(body.as_bytes()).unwrap();
    let out = Command::new(huck_bin()).arg(&path).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn parse_error_reports_line() {
    // `fi` with no `if` is a parse error on line 3.
    let (_o, se, _c) = run_script("echo one\necho two\nfi\n");
    assert!(se.contains("line 3:"), "stderr missing 'line 3:': {se:?}");
    assert!(se.contains("syntax error"), "stderr missing 'syntax error': {se:?}");
}

#[test]
fn error_line_is_command_start_not_line_one() {
    // Valid commands first; the error is on line 4.
    let (_o, se, _c) = run_script("x=1\necho hi\n: ok\n)\n");
    assert!(se.contains("line 4:"), "expected 'line 4:', got: {se:?}");
}

#[test]
fn lex_error_reports_line() {
    // An unterminated single quote on line 2 is a lex error.
    let (_o, se, _c) = run_script("echo ok\necho 'unterminated\n");
    assert!(se.contains("line 2:"), "expected 'line 2:', got: {se:?}");
    assert!(se.contains("syntax error"), "stderr: {se:?}");
}

#[test]
fn multiline_construct_points_at_first_line() {
    // A 3-line function whose body has a stray `done`; documented limitation:
    // the reported line is the construct's FIRST line (function def, line 2).
    let (_o, se, _c) = run_script("echo a\nf() {\n  done\n}\n");
    assert!(se.contains("syntax error"), "stderr: {se:?}");
    assert!(se.contains("line 2:"), "expected first-line 'line 2:', got: {se:?}");
}
```

Note on the multi-line fragment: confirm during Step 4 that `f() {\n done\n}` actually produces a parse error in huck (a `done` with no loop). If huck parses it differently, adjust the fragment to a multi-line construct that DOES error, while still asserting the reported line is the construct's first line. The point of this test is to pin the documented "first line" behavior — keep that assertion meaningful.

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test --test script_line_numbers_integration 2>&1 | tail -20`
Expected: FAIL — current output has no `line N:` substring.

- [ ] **Step 3: Add line tracking to `run_sourced_contents`**

In `src/builtins.rs`, `run_sourced_contents`. Before the `for line in contents.lines()` loop, after `let mut buf = String::new();`, add:

```rust
    let mut physical_line: usize = 0;
    let mut cmd_start_line: usize = 0;
```

At the very top of the loop body (before the `set -v` verbose echo), add:

```rust
        physical_line += 1;
        if buf.is_empty() {
            cmd_start_line = physical_line;
        }
```

(Setting `cmd_start_line` when `buf` is empty marks the first physical line of each new logical command; continuation lines leave it unchanged.)

Change the **lex-error** arm from:
```rust
                eprintln!(
                    "huck: {}: syntax error{}",
                    path.display(),
                    crate::shell::lex_error_message(e)
                );
```
to:
```rust
                eprintln!(
                    "huck: {}: line {}: syntax error{}",
                    path.display(),
                    cmd_start_line,
                    crate::shell::lex_error_message(e)
                );
```

Change the **parse-error** arm from:
```rust
                eprintln!(
                    "huck: {}: syntax error: {}",
                    path.display(),
                    crate::shell::parse_error_message(e)
                );
```
to:
```rust
                eprintln!(
                    "huck: {}: line {}: syntax error: {}",
                    path.display(),
                    cmd_start_line,
                    crate::shell::parse_error_message(e)
                );
```

Leave everything else (status `2`, `buf.clear()`, control flow) unchanged.

- [ ] **Step 4: Run the integration tests**

Run: `cargo build --bin huck && cargo test --test script_line_numbers_integration 2>&1 | tail -20`
Expected: all 4 PASS. If `multiline_construct_points_at_first_line`'s fragment doesn't error in huck, adjust it per the Step 1 note (keep the first-line assertion meaningful), then re-run.

- [ ] **Step 5: Check for existing tests that assert the old message format**

Run: `grep -rn 'syntax error' tests/ src/ | grep -i 'source\|sourced\|rcfile\|\.sh\|line ' | head` and `cargo test 2>&1 | grep -E 'test result: FAILED|FAILED' | head`.
If any existing test asserted the exact `FILE: syntax error:` string for a sourced file, update it to expect `FILE: line N: syntax error:`. (Tests that feed fragments via stdin are unaffected — they use the no-file REPL path.)

- [ ] **Step 6: Full suite + clippy**

Run: `cargo test 2>&1 | grep -E 'test result' | grep -v 'ok\.' | head` (expect empty — no failures) and `cargo clippy --all-targets 2>&1 | tail -3` (clean).

- [ ] **Step 7: Commit**

```bash
git add src/builtins.rs tests/script_line_numbers_integration.rs
git commit -m "feat: line numbers in sourced-script syntax errors

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

### Task 2: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`

- [ ] **Step 1: Read the change-log top + README table + Low-impact tier**

Run: `grep -n '2026-06-05\|^## Change log\|Low-impact\|^- \*\*L-' docs/bash-divergences.md | head` and `grep -n '| v9' README.md`.

- [ ] **Step 2: Add a change-log entry**

Add a `2026-06-05` v94 entry (diagnostics, no `M-*` flip — like v80) at the top of the change log: sourced-file syntax errors now report `FILE: line N: syntax error: MSG` (bash `line N` convention), tracking the logical command's first physical line in `run_sourced_contents`; scope = lex + parse errors for `source`/`.`/`--rcfile`/`huck SCRIPT`; runtime errors and exact token line:col deferred; multi-line constructs point at the construct's first line.

- [ ] **Step 3: Add a Low-impact note**

In the Low-impact (Tier) section, add a short entry (next free `L-` number, find the highest via the grep in Step 1) noting: huck reports a sourced-file syntax error at the logical command's FIRST physical line, whereas bash reports the offending token's line; they differ only for multi-line constructs. `[intentional]` / low — exact token line:col deferred (would need per-Token position tracking).

- [ ] **Step 4: Add the README v94 row**

Add a v94 row after v93, matching the column format: "line numbers in sourced-script syntax errors (`FILE: line N: syntax error`); diagnostics".

- [ ] **Step 5: Verify + commit**

Run: `grep -n 'v94\|line N' docs/bash-divergences.md README.md` (confirm entries present, no placeholders).
```bash
git add docs/bash-divergences.md README.md
git commit -m "docs: v94 line numbers in script errors — changelog, README, L-note

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

## Self-Review

- **Spec coverage:** the run_sourced_contents change + integration tests → Task 1; changelog/README/L-note → Task 2. Covered.
- **Placeholder scan:** none — all code shown; the only judgment step (Step 5 existing-test check, Step 1 multi-line fragment confirm) is inherent and bounded.
- **Type consistency:** `physical_line`/`cmd_start_line` are `usize`; the two `eprintln!` arms use them identically; `path.display()` / `lex_error_message` / `parse_error_message` calls unchanged except for the inserted `line {N}:`.
- **Edge cases:** continuation lines don't move `cmd_start_line` (only set when `buf` empty); blank/comment lines still advance `physical_line` so counts stay file-aligned; stdin/REPL path untouched (harnesses unaffected).
