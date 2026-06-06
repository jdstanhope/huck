# Subshell/Compound-Headed Pipeline in Any Position (M-11a) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A subshell/compound-headed pipeline (`( … ) | cmd`, `{ …; } | cmd`, `if…fi | cmd`, …) parses in ANY sequence position (after `;`/`&&`/`||`/`&` or inside a compound/function body), not just first. Closes M-11a; unblocks nvm's `nvm_list_aliases`.

**Architecture:** Factor the existing first-position "wrap in a pipeline if `|` follows" block (inlined identically in `parse_sequence` and `parse_subshell_sequence`) into one `parse_command_then_pipeline` helper, and call it at every sequence-element position. Parser-only; no AST/executor change (a `Pipeline` with a `Subshell`/compound first stage already executes correctly).

**Tech Stack:** Rust (binary crate `huck`). Unit `cargo test --bin huck`; integration `cargo test --test <name>`; bash-diff harness under `tests/scripts/`.

---

## File Structure

- `src/command.rs` — new `parse_command_then_pipeline` helper; applied at `parse_sequence` (first + Semi/And/Or/Amp) and `parse_subshell_sequence` (first + Semi/`&`/And/Or), removing the now-redundant inlined pipeline blocks.
- `tests/subshell_pipeline_position_integration.rs`, `tests/scripts/subshell_pipeline_position_diff_check.sh` — NEW.
- `docs/bash-divergences.md`, `README.md` — M-11a `[fixed v100]` + changelog + README row.

---

### Task 1: `parse_command_then_pipeline` helper + apply everywhere

**Files:** `src/command.rs`

- [ ] **Step 1: Write the failing integration test**

Create `tests/subshell_pipeline_position_integration.rs`:

```rust
//! v100: subshell/compound-headed pipeline in any sequence position (M-11a).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn subshell_pipe_after_semi() {
    assert_eq!(run("echo z; ( echo a ) | sort\n").0, "z\na\n");
}

#[test]
fn subshell_pipe_after_and() {
    assert_eq!(run("true && ( printf 'b\\na\\n' ) | sort\n").0, "a\nb\n");
}

#[test]
fn subshell_pipe_after_or() {
    assert_eq!(run("false || ( echo x ) | cat\n").0, "x\n");
}

#[test]
fn brace_group_pipe_after_semi() {
    assert_eq!(run("echo z; { echo a; echo b; } | sort\n").0, "z\na\nb\n");
}

#[test]
fn if_pipe_after_semi() {
    assert_eq!(run("echo z; if true; then echo a; fi | cat\n").0, "z\na\n");
}

#[test]
fn subshell_pipe_in_function_body() {
    assert_eq!(run("f() { echo z; ( echo a ) | sort; }\nf\n").0, "z\na\n");
}

#[test]
fn subshell_pipe_in_for_body() {
    assert_eq!(run("for i in 1 2; do ( echo $i ) | cat; done\n").0, "1\n2\n");
}

#[test]
fn nvm_shaped_function() {
    // local + ( for ... & done; wait ) | sort  inside a function (the nvm shape).
    assert_eq!(
        run("f() {\n  local X\n  ( for n in b a; do echo $n & done; wait ) | sort\n}\nf\n").0,
        "a\nb\n");
}

#[test]
fn subshell_pipe_after_amp() {
    // `( ) | cmd` as an Amp-separated element (v98).
    assert_eq!(run("( echo a ) | cat & wait\necho done\n").0, "a\ndone\n");
}

#[test]
fn negated_subshell_pipe_after_semi() {
    // `! ( false ) | cat` negation through the helper.
    assert_eq!(run("echo z; ! ( false ) | cat; echo rc=$?\n").0, "z\nrc=0\n");
}

#[test]
fn regression_plain_sequences_unchanged() {
    assert_eq!(run("echo a; echo b\ntrue && echo y\nfalse || echo n\necho p | cat\n").0,
               "a\nb\ny\nn\np\n");
}

#[test]
fn regression_subshell_pipe_first_position() {
    assert_eq!(run("( echo a ) | sort; echo z\n").0, "a\nz\n");
}
```
Verify each expected output against bash first (`printf '...' | bash`) and adjust to bash's actual output (esp. the negation rc and the Amp case).

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test --test subshell_pipeline_position_integration 2>&1 | tail -20`
Expected: the `after_semi`/`after_and`/`after_or`/in-body/nvm tests FAIL (`unexpected token after command`); the regression + first-position tests already PASS.

- [ ] **Step 3: Add the `parse_command_then_pipeline` helper**

Add to `src/command.rs` (near `parse_command`/`parse_sequence`):
```rust
/// Parses one sequence ELEMENT: a command, plus — if a `|` immediately
/// follows — the rest of the pipeline (the command is the first stage).
/// `parse_command` already consumes a pipeline when the first stage is a
/// SIMPLE command; this helper adds the wrap for a COMPOUND/subshell first
/// stage (which `parse_command` returns without checking for a trailing `|`).
/// Returns `raw` unchanged when no `|` follows — so non-pipeline elements are
/// byte-identical to before.
fn parse_command_then_pipeline<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Command, ParseError> {
    let raw = parse_command(iter)?;
    if matches!(iter.peek(), Some(Token::Op(Operator::Pipe))) {
        let mut stages = vec![raw];
        iter.next(); // consume `|`
        skip_newlines(iter);
        let mut more = true;
        while more {
            let (cmd, next_pipe) = parse_next_stage(iter)?;
            stages.push(cmd);
            if next_pipe {
                // simple stage consumed its own `|`
            } else if matches!(iter.peek(), Some(Token::Op(Operator::Pipe))) {
                iter.next();
                skip_newlines(iter);
            } else {
                more = false;
            }
        }
        Ok(Command::Pipeline(Pipeline { negate: false, commands: stages }))
    } else {
        Ok(raw)
    }
}
```
(This is the exact logic inlined at `parse_sequence:600-628` and
`parse_subshell_sequence:1439-1462`, moved into a shared fn.)

- [ ] **Step 4: Apply in `parse_sequence`**

- Replace the first-position block (`:600-630`, `let raw_first = parse_command(iter)?; let first = if … { …pipeline… } else { raw_first };`) with:
  ```rust
  let first = parse_command_then_pipeline(iter)?;
  ```
- In the loop's connector arms, replace `parse_command(iter)?` with
  `parse_command_then_pipeline(iter)?` at:
  - `Amp` (`:690`), `Semi` (`:707`), `And` (`:711`), `Or` (`:715`).

- [ ] **Step 5: Apply in `parse_subshell_sequence`**

- Replace its first-position block (`:1439-1462`) with `let first = parse_command_then_pipeline(iter)?;`.
- The `&` arm (`:1487`) and the Semi/Newline arm (`:1535`) currently do
  `let raw = parse_command(iter)?;` followed by a HAND-ROLLED pipeline-assembly
  block (peek `|`, loop `parse_next_stage`, build `Command::Pipeline`). Replace
  each `let raw = parse_command(iter)?; <hand-rolled pipeline block>` with
  `let cmd = parse_command_then_pipeline(iter)?;` and use `cmd` where `raw`/the
  assembled pipeline was pushed. (Remove the now-redundant hand-rolled blocks.)
- Replace `parse_command(iter)?` in the `And` (`:1562`) and `Or` (`:1567`) arms
  with `parse_command_then_pipeline(iter)?`.
- Do NOT touch the function-body `parse_command(iter)?` calls at `:918` and
  `:958` (a function body is a single compound, not a sequence element — leave
  unchanged).

- [ ] **Step 6: Build + parser unit tests**

`cargo build --bin huck`. Add parser unit tests: `echo z; ( echo a ) | cat` →
`Sequence` whose `rest[0].1` is a `Command::Pipeline` whose first stage is a
`Command::Subshell`; `x; a` (no pipe) → `rest[0].1` is NOT a pipeline (unchanged).
Mirror neighboring parser-test helpers.

- [ ] **Step 7: Run integration + full suite + clippy**

Run: `cargo build --bin huck && cargo test --test subshell_pipeline_position_integration 2>&1 | tail -20` (all pass).
Run: `cargo test --bin huck 2>&1 | tail -5` and `cargo test 2>&1 | grep -E 'test result' | grep -v 'ok\.' | head` (no failures — ESPECIALLY pipeline/subshell/sequence/`&`-v98/function/pipefail suites).
Run: `cargo clippy --all-targets 2>&1 | tail -3` (clean).

- [ ] **Step 8: Commit**

```bash
git add src/command.rs tests/subshell_pipeline_position_integration.rs
git commit -m "feat: subshell/compound-headed pipeline parses in any sequence position (M-11a)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

### Task 2: bash-diff harness (25th)

**Files:** `tests/scripts/subshell_pipeline_position_diff_check.sh` (NEW)

- [ ] **Step 1: Create the harness**

Mirror `tests/scripts/dbracket_multiline_diff_check.sh`'s `check` helper. Deterministic fragments:
```bash
check "sub pipe ;"      'echo z; ( echo a ) | sort'
check "sub pipe &&"     "true && ( printf 'b\\na\\n' ) | sort"
check "sub pipe ||"     'false || ( echo x ) | cat'
check "brace pipe ;"    'echo z; { echo a; echo b; } | sort'
check "if pipe ;"       'echo z; if true; then echo a; fi | cat'
check "fn body sub pipe" 'f() { echo z; ( echo a ) | sort; }; f'
check "for body sub pipe" 'for i in 1 2; do ( echo $i ) | cat; done'
check "negated sub pipe" 'echo z; ! ( false ) | cat; echo $?'
check "first-pos regress" '( echo a ) | sort; echo z'
check "plain seq regress" 'echo a; echo b; true && echo y'
check "nvm shape"        $'f() {\n  local X\n  ( for n in b a; do echo $n & done; wait ) | sort\n}\nf'
```
After writing, RUN it and confirm fragments are well-formed.

- [ ] **Step 2: Run the harness**

Run: `cargo build --bin huck && bash tests/scripts/subshell_pipeline_position_diff_check.sh 2>&1 | tail -20`
Expected: every line PASS, `Fail: 0`. Investigate any FAIL (bash is the oracle); real bug → STOP and report.

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/subshell_pipeline_position_diff_check.sh
git commit -m "test: bash-diff harness for subshell-headed pipeline position (25th)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

### Task 3: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`

- [ ] **Step 1: Read structure**

`grep -n 'M-11a\|^## Change log\|Missing features (Tier 2)\|2026-06-0' docs/bash-divergences.md | head` and `grep -n '| v9\|| v10' README.md`. Read the M-11a entry (`:191`) + recent change-log/README rows.

- [ ] **Step 2: Flip M-11a to fixed**

Update the M-11a entry from `[deferred]` to `[fixed v100]`: a subshell/compound-headed pipeline now parses in any sequence position via a factored `parse_command_then_pipeline` helper applied to first + all rest connectors in `parse_sequence`/`parse_subshell_sequence`; note it was the `nvm_list_aliases` blocker. Bump the Tier-2 count + roster narrative (`M-11a fixed by v100`).

- [ ] **Step 3: Change-log + README row**

`2026-06-06` v100 change-log entry (the helper + uniform application; parser-only; nvm.sh `nvm_list_aliases` now parses; 25th harness). v100 README row after v99.

- [ ] **Step 4: Verify + commit**

`grep -n 'v100\|fixed v100\|M-11a' docs/bash-divergences.md README.md` (confirm, no placeholders).
```bash
git add docs/bash-divergences.md README.md
git commit -m "docs: v100 M-11a fixed — subshell-headed pipeline in any position; changelog, README

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

## Self-Review

- **Spec coverage:** §1 helper + §2 apply → Task 1; §3 no-AST-change confirmed; testing → Tasks 1/2; M-11a flip → Task 3. Covered.
- **Placeholder scan:** none — the helper is shown in full; the apply sites are named by exact line + arm.
- **Type consistency:** `parse_command_then_pipeline(iter) -> Result<Command, ParseError>`; reuses `parse_command`, `parse_next_stage`, `Pipeline`, `Command::Pipeline`. Replaces the inlined first-position blocks verbatim.
- **Edge cases:** non-`|` element returns `raw` (byte-identical → zero regression); negation via `parse_command`'s bang handling; function-body call sites (`:918`,`:958`) intentionally NOT changed; first-position + plain sequences regression-tested.
