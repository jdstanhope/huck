# Subshell Inside Command Substitution (Paren Balancing) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `$( (cmd) )`, `$( (a) || b )`, `$(cmd | (sub))`, `$( $((1+2)) )` parse correctly — a subshell/nested-arith inside a command substitution no longer truncates at the inner `)`. Unblocks nvm's `nvm_resolve_alias` (line 1287).

**Architecture:** One-line lexer fix in `scan_paren_substitution` — a bare `(` increments paren-`depth` (it currently doesn't, due to a stale "huck has no subshell syntax" comment), so the command-sub closes only at the true depth-0 `)`. Lexer-only; no parser/executor/AST change.

**Tech Stack:** Rust (binary crate `huck`). Unit `cargo test --bin huck`; integration `cargo test --test <name>`; bash-diff harness under `tests/scripts/`.

---

## File Structure

- `src/lexer.rs` — `scan_paren_substitution`: bare `(` → `depth += 1`; drop the stale comment.
- `tests/cmdsub_subshell_integration.rs`, `tests/scripts/cmdsub_subshell_diff_check.sh` — NEW.
- `docs/bash-divergences.md`, `README.md` — new Tier-2 entry `[fixed v101]` + case-pattern edge note + changelog + README row.

---

### Task 1: The lexer fix + tests

**Files:** `src/lexer.rs`, `tests/cmdsub_subshell_integration.rs` (NEW)

- [ ] **Step 1: Write the failing integration test**

Create `tests/cmdsub_subshell_integration.rs`:

```rust
//! v101: subshell / nested-arith inside command substitution $( … ).
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
fn subshell_in_cmdsub() {
    assert_eq!(run("echo \"$( (echo a) )\"\n").0, "a\n");
}

#[test]
fn subshell_or_in_cmdsub() {
    assert_eq!(run("echo \"$( (echo a) || echo b )\"\n").0, "a\n");
}

#[test]
fn subshell_pipe_stage_in_cmdsub() {
    assert_eq!(run("echo \"$(echo a | (cat))\"\n").0, "a\n");
}

#[test]
fn subshell_with_semis_in_cmdsub() {
    assert_eq!(run("echo \"$( (exit 3); echo done )\"\n").0, "done\n");
}

#[test]
fn nested_arith_in_cmdsub() {
    assert_eq!(run("echo \"$( echo $((1 + 2)) )\"\n").0, "3\n");
}

#[test]
fn subshell_in_default_expansion() {
    assert_eq!(run("echo \"${x:-$( (echo d) )}\"\n").0, "d\n");
}

#[test]
fn subshell_in_array_literal() {
    assert_eq!(run("a=( \"$( (echo x) )\" )\necho \"${a[0]}\"\n").0, "x\n");
}

#[test]
fn nvm_resolve_alias_shape() {
    // $( (pipeline) || fallback ) — the exact nvm shape.
    assert_eq!(run("r=\"$( (printf 'a\\nb\\n' | head -n 1) || echo z )\"\necho \"$r\"\n").0, "a\n");
}

#[test]
fn regression_plain_and_nested_cmdsub() {
    assert_eq!(run("echo \"$(echo a)\"\necho \"$(echo \"$(echo b)\")\"\n").0, "a\nb\n");
}
```
Verify each expected output against bash first (`printf '...' | bash`) and adjust to bash's actual output.

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test --test cmdsub_subshell_integration 2>&1 | tail -20`
Expected: the subshell/nested-arith cases FAIL (`unterminated '('`); the regression case already PASSES.

- [ ] **Step 3: Apply the fix**

In `src/lexer.rs`, `scan_paren_substitution` (the bare-`(` arm, ~line 1651):
```rust
            '(' => {
                // Bare `(` is just a character. huck has no subshell
                // `(cmd)` syntax — only `$(` increments depth (handled in
                // the `$` arm below).
                body.push(c);
            }
```
Change to:
```rust
            '(' => {
                // A subshell `(cmd)` or the inner `(` of `$((…))` raises depth
                // so its matching `)` doesn't close the command substitution
                // early. (huck has had subshell syntax since v28.)
                depth += 1;
                body.push(c);
            }
```
Do NOT change the `)` arm (it already decrements for `depth>0` and closes at
`depth==0`) or the `$` arm (it already counts nested `$(`). Confirm the exact
location with `grep -n 'no subshell' src/lexer.rs`.

- [ ] **Step 4: Add a lexer unit test**

In the `src/lexer.rs` `#[cfg(test)] mod tests`, add a test that `tokenize("$( (echo a) )")`
succeeds (no `Err`) and yields a Word with a `WordPart::CommandSub` whose inner
sequence's first command is a `Command::Subshell` (or, more simply, assert
`tokenize("$( (echo a) )").is_ok()` and the prior behavior `is_err()` is gone).
Mirror neighboring command-sub lexer tests.

- [ ] **Step 5: Build + run integration + full suite + clippy**

Run: `cargo build --bin huck && cargo test --test cmdsub_subshell_integration 2>&1 | tail -20` (all pass).
Run: `cargo test --bin huck 2>&1 | tail -5` and `cargo test 2>&1 | grep -E 'test result' | grep -v 'ok\.' | head` (no failures — especially command-sub / subshell / arith / array suites).
Run: `cargo clippy --all-targets 2>&1 | tail -3` (clean).
Manual: `printf 'echo "$( (echo a) || echo b )"\n' | ./target/debug/huck` → `a`.

- [ ] **Step 6: Commit**

```bash
git add src/lexer.rs tests/cmdsub_subshell_integration.rs
git commit -m "fix: subshell/nested-arith inside command substitution balances parens

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

### Task 2: bash-diff harness (26th)

**Files:** `tests/scripts/cmdsub_subshell_diff_check.sh` (NEW)

- [ ] **Step 1: Create the harness**

Mirror `tests/scripts/dbracket_multiline_diff_check.sh`'s `check` helper. Deterministic fragments:
```bash
check "subshell"       'echo "$( (echo a) )"'
check "subshell ||"    'echo "$( (echo a) || echo b )"'
check "subshell pipe"  'echo "$(echo a | (cat))"'
check "subshell semis" 'echo "$( (exit 3); echo done )"'
check "nested arith"   'echo "$( echo $((1 + 2)) )"'
check "in default"     'echo "${x:-$( (echo d) )}"'
check "in array lit"   'a=( "$( (echo x) )" ); echo "${a[0]}"'
check "nvm shape"      $'r="$( (printf \'a\\nb\\n\' | head -n 1) || echo z )"; echo "$r"'
check "plain regress"  'echo "$(echo a)"'
check "nested regress" 'echo "$(echo "$(echo b)")"'
check "backtick sub"   'echo "`(echo a)`"'
```
After writing, RUN it and confirm fragments are well-formed (a malformed fragment FAILs spuriously). Watch the shell-quoting on the `nvm shape` fragment especially.

- [ ] **Step 2: Run the harness**

Run: `cargo build --bin huck && bash tests/scripts/cmdsub_subshell_diff_check.sh 2>&1 | tail -20`
Expected: every line PASS, `Fail: 0`. Investigate any FAIL (bash is the oracle); real bug → STOP and report. If the `in array lit` or `nvm shape` fragment is hard to quote correctly in the harness, simplify it to an equivalent deterministic form (the integration tests already cover the exact shapes).

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/cmdsub_subshell_diff_check.sh
git commit -m "test: bash-diff harness for subshell in command substitution (26th)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

### Task 3: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`

- [ ] **Step 1: Read structure**

`grep -n 'command substitution\|^## Change log\|Missing features (Tier 2)\|2026-06-0' docs/bash-divergences.md | head` and `grep -n '| v10\|| v9' README.md`. Match v99/v100 style; find the next free `M-` number (highest is M-96).

- [ ] **Step 2: Add the Tier-2 entry**

New Tier-2 entry, next free `M-`, `[fixed v101]`: "subshell / nested-arith inside command substitution" — `scan_paren_substitution` now increments paren-depth on a bare `(` (it didn't, due to a stale pre-v28 comment), so a subshell `( … )` or `$((…))` inside `$( … )` no longer truncates the body at the inner `)`. Note the nvm `nvm_resolve_alias` driver and that the **`case`-pattern bare `)` inside `$(…)`** remains a pre-existing low edge (naive paren-counting; not in scope). Bump the Tier-2 count + roster narrative.

- [ ] **Step 3: Change-log + README row**

`2026-06-06` v101 change-log entry mirroring v99/v100 style (the one-arm lexer fix; subshell + nested-arith in command-subs; transitively fixes `${:-}`/array-literal cases; 26th harness; nvm advances past 1287). v101 README row after v100.

- [ ] **Step 4: Verify + commit**

`grep -n 'v101\|fixed v101' docs/bash-divergences.md README.md` (confirm, no placeholders).
```bash
git add docs/bash-divergences.md README.md
git commit -m "docs: v101 subshell in command substitution fixed — changelog, README

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

## Self-Review

- **Spec coverage:** §1 fix → Task 1; testing → Tasks 1/2; new Tier-2 entry + case-pattern note → Task 3. Covered.
- **Placeholder scan:** none — the exact before/after of the one arm is shown.
- **Type consistency:** the change is `depth += 1` inside the existing `'(' =>` arm of `scan_paren_substitution`; no signatures change.
- **Edge cases:** `(` inside quotes/backslash already shielded; `$((…))` double-count balanced by `))`; case-pattern `)` documented as pre-existing out-of-scope; plain/nested/backtick command-subs regression-tested.
