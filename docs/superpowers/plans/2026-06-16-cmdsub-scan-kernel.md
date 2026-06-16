# v167: unify the `$()`-scan loops onto one kernel — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the `$(…)` command-substitution body scan into one private `scan_cmdsub_body` kernel and route the three current copies (`scan_paren_substitution`, `consume_paren_cmdsub_verbatim`, `split_modifier_operand`) through it, with no change in observable behavior.

**Architecture:** `scan_cmdsub_body(chars, out, unterminated)` consumes a `$(…)` body (opening `$(` already consumed) through the matching `)` (consumed, not appended), tracking nested parens + `'…'`/`"…"` spans + `\`-escapes, appending the body verbatim to `out`, with the error variant passed in. The parse path collects-then-parses through it; the verbatim path re-adds the `)`; the splitter delegates and drops its inline paren tracking. Pure refactor.

**Tech Stack:** Rust (edition 2024). `CharCursor` (lexer.rs:37): `new`/`peek` (`Option<&char>`)/`next` (`Option<char>`)/`by_ref()`. `LexError::{UnterminatedSubstitution, UnterminatedBrace}`.

**Spec:** `docs/superpowers/specs/2026-06-16-cmdsub-scan-kernel-design.md`

**Branch:** `v167-cmdsub-scan-kernel`

---

## Background the implementer needs

All changes are in `src/lexer.rs`. The three functions today (current line numbers approximate):

- `scan_paren_substitution` (~lexer.rs:2254): `depth: usize = 0`; on `)` at depth 0 returns `parse_substitution_body(&body, opts)` (the `)` is **not** appended); on other `)` decrements + pushes; on `(` increments + pushes; `\`+next pushed (EOF → `Err(UnterminatedSubstitution)`); `'…'` and `"…"` spans (double honors `\`); a `$` arm that consumes `$(` as a unit (redundant — a bare `(` already raises depth); errors `UnterminatedSubstitution`. Returns `Result<Sequence, LexError>`.
- `consume_paren_cmdsub_verbatim` (~lexer.rs:2009, added v166): `depth: u32 = 1`; appends the body **including** the closing `)` to `out`; errors `UnterminatedBrace`. Returns `Result<(), LexError>`. Caller (`scan_braced_operand`'s `$(` case) has already pushed `$(`.
- `split_modifier_operand` (~lexer.rs:3203, added v165): tracks `paren_depth` inline; its `\` arm has a `paren_depth==0` (un-escape `\delim`/`\\`) vs `>0` (verbatim) split; `'('`/`')'` arms gated on `paren_depth > 0`; `'{'`/`'}'` arms gated on `paren_depth == 0`; the `$` arm raises `paren_depth` on `$(`; the delimiter guard is `c == delim && paren_depth == 0 && brace_depth == 0 && !delim_seen`. Returns `(String, Option<String>)` (no `Result`). Its input is always an operand body already extracted (and `$()`-balanced) by `scan_braced_operand`, so it never receives an unterminated `$(`.

The kernel is `scan_paren_substitution`'s loop with: the error variant parameterized, the redundant `$` arm dropped (verified equivalent — a bare `(` raises depth identically), and the closing `)` consumed-but-not-appended (matching `scan_paren_substitution`).

---

### Task 1: Extract the kernel and route all three callers through it

A single cohesive refactor. Unit tests for the kernel come first (TDD).

**Files:**
- Modify: `src/lexer.rs` — add `scan_cmdsub_body` + unit tests; rewire the three callers.

- [ ] **Step 1: Write the failing unit tests for the kernel**

Add these inside the `#[cfg(test)] mod tests { … }` block in `src/lexer.rs` (next to the `scan_braced_operand_*` tests):

```rust
    #[test]
    fn scan_cmdsub_body_basic_consumes_through_close_paren() {
        let mut chars = CharCursor::new("echo hi)rest");
        let mut out = String::new();
        scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedSubstitution).unwrap();
        assert_eq!(out, "echo hi"); // closing ) consumed, not appended
        assert_eq!(chars.next(), Some('r')); // cursor left just past the )
    }

    #[test]
    fn scan_cmdsub_body_balances_nested_and_arith() {
        let mut chars = CharCursor::new("echo $(echo x))");
        let mut out = String::new();
        scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedBrace).unwrap();
        assert_eq!(out, "echo $(echo x)");

        // $((1+2)) — caller consumed the outer `$(`, body starts at the inner `(`
        let mut chars = CharCursor::new("(1+2))");
        let mut out = String::new();
        scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedBrace).unwrap();
        assert_eq!(out, "(1+2)");
    }

    #[test]
    fn scan_cmdsub_body_skips_quoted_paren() {
        let mut chars = CharCursor::new("echo \")\")");
        let mut out = String::new();
        scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedBrace).unwrap();
        assert_eq!(out, "echo \")\"");
    }

    #[test]
    fn scan_cmdsub_body_unterminated_uses_passed_error() {
        let mut chars = CharCursor::new("echo hi");
        let mut out = String::new();
        assert_eq!(
            scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedSubstitution).unwrap_err(),
            LexError::UnterminatedSubstitution
        );
        let mut chars = CharCursor::new("echo hi");
        let mut out = String::new();
        assert_eq!(
            scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedBrace).unwrap_err(),
            LexError::UnterminatedBrace
        );
    }
```

- [ ] **Step 2: Run them to confirm they fail (don't compile)**

Run: `cargo test --lib scan_cmdsub_body 2>&1 | grep -E 'error\[|cannot find' | head`
Expected: `cannot find function `scan_cmdsub_body` in this scope`.

- [ ] **Step 3: Add the `scan_cmdsub_body` kernel**

Insert immediately **before** `fn consume_paren_cmdsub_verbatim(` in `src/lexer.rs`:

```rust
/// Scans a `$(…)` command-substitution body, the opening `$(` having already
/// been consumed by the caller. Consumes through the matching `)` (which is
/// consumed but NOT appended); any unquoted `(` raises the paren depth and any
/// unquoted `)` lowers it, so nested `$(…)`, `$((…))`, and `$( (…) )` balance;
/// `'…'`/`"…"` spans are skipped (double-quote honors `\`) and `\` escapes the
/// next char — none affect depth. The body (excluding the closing `)`) is
/// appended to `out`. Running out of input unterminated returns `Err(unterminated)`.
/// The single source of truth for `$()` scanning (see `scan_paren_substitution`,
/// `consume_paren_cmdsub_verbatim`, `split_modifier_operand`).
fn scan_cmdsub_body(
    chars: &mut CharCursor<'_>,
    out: &mut String,
    unterminated: LexError,
) -> Result<(), LexError> {
    let mut depth: usize = 0;
    loop {
        match chars.next() {
            None => return Err(unterminated),
            Some(')') if depth == 0 => return Ok(()),
            Some(')') => {
                depth -= 1;
                out.push(')');
            }
            Some('(') => {
                depth += 1;
                out.push('(');
            }
            Some('\\') => {
                out.push('\\');
                match chars.next() {
                    Some(c) => out.push(c),
                    None => return Err(unterminated),
                }
            }
            Some('\'') => {
                out.push('\'');
                loop {
                    match chars.next() {
                        Some('\'') => {
                            out.push('\'');
                            break;
                        }
                        Some(c) => out.push(c),
                        None => return Err(unterminated),
                    }
                }
            }
            Some('"') => {
                out.push('"');
                loop {
                    match chars.next() {
                        Some('"') => {
                            out.push('"');
                            break;
                        }
                        Some('\\') => {
                            out.push('\\');
                            match chars.next() {
                                Some(c) => out.push(c),
                                None => return Err(unterminated),
                            }
                        }
                        Some(c) => out.push(c),
                        None => return Err(unterminated),
                    }
                }
            }
            Some(c) => out.push(c),
        }
    }
}
```

- [ ] **Step 4: Rewire `consume_paren_cmdsub_verbatim` to the kernel**

Replace the entire body of `consume_paren_cmdsub_verbatim` (the function added in v166, ~60 lines) with:

```rust
fn consume_paren_cmdsub_verbatim(
    chars: &mut CharCursor<'_>,
    out: &mut String,
) -> Result<(), LexError> {
    // The kernel consumes (but does not append) the closing `)`; re-add it so
    // the command substitution is reconstructed verbatim in `out`.
    scan_cmdsub_body(chars, out, LexError::UnterminatedBrace)?;
    out.push(')');
    Ok(())
}
```

(Keep the existing doc comment above the function.)

- [ ] **Step 5: Rewire `scan_paren_substitution` to the kernel**

Replace the entire body of `scan_paren_substitution` (its `let mut body = String::new(); let mut depth …; while … { match … } Err(…)` loop) with a call to the kernel, leaving the signature and the `parse_substitution_body` call intact:

```rust
fn scan_paren_substitution(
    chars: &mut CharCursor<'_>,
    opts: LexerOptions,
) -> Result<crate::command::Sequence, LexError> {
    let mut body = String::new();
    scan_cmdsub_body(chars, &mut body, LexError::UnterminatedSubstitution)?;
    parse_substitution_body(&body, opts)
}
```

(Keep any doc comment above the function.)

- [ ] **Step 6: Rewire `split_modifier_operand` to delegate `$()` to the helper**

Make these edits to `split_modifier_operand`:

(a) Delete the `paren_depth` field declaration line:
```rust
    let mut paren_depth: u32 = 0; // > 0 while inside a $( … ) command substitution
```

(b) Simplify the `'\\'` arm (now always at top level — `$()` interiors are consumed by the helper) to:
```rust
            '\\' => {
                let dst = if delim_seen { &mut second } else { &mut first };
                match chars.peek().copied() {
                    Some(d) if d == delim => {
                        chars.next();
                        dst.push(delim);
                    }
                    Some('\\') => {
                        chars.next();
                        dst.push('\\');
                    }
                    _ => dst.push('\\'),
                }
            }
```

(c) Replace the `'$'` arm with one that delegates to `consume_paren_cmdsub_verbatim`:
```rust
            '$' => {
                let dst = if delim_seen { &mut second } else { &mut first };
                dst.push('$');
                if chars.peek() == Some(&'(') {
                    chars.next();
                    dst.push('(');
                    // Skip the whole command substitution verbatim so a delimiter
                    // inside it is ignored (L-10). The `Result` is unreachable: the
                    // operand body was already $()-balanced by scan_braced_operand;
                    // on the impossible error the partial is appended and the cursor
                    // is exhausted, so the outer loop ends with identical segments.
                    let _ = consume_paren_cmdsub_verbatim(&mut chars, dst);
                }
            }
```

(d) Delete the two paren-tracking arms entirely:
```rust
            '(' if paren_depth > 0 => {
                paren_depth += 1;
                if delim_seen { second.push('('); } else { first.push('('); }
            }
            ')' if paren_depth > 0 => {
                paren_depth -= 1;
                if delim_seen { second.push(')'); } else { first.push(')'); }
            }
```

(e) Remove the `if paren_depth == 0` guard from both brace arms (always true now):
```rust
            '{' => {
                brace_depth += 1;
                if delim_seen { second.push('{'); } else { first.push('{'); }
            }
            '}' => {
                brace_depth = brace_depth.saturating_sub(1);
                if delim_seen { second.push('}'); } else { first.push('}'); }
            }
```

(f) Simplify the delimiter guard (drop `paren_depth == 0`):
```rust
            c if c == delim && brace_depth == 0 && !delim_seen => {
                delim_seen = true;
            }
```

- [ ] **Step 7: Run the kernel + sibling unit tests**

Run: `cargo test --lib 'scan_cmdsub_body' && cargo test --lib 'split_modifier_operand' && cargo test --lib 'scan_braced_operand'`
Expected: all pass (the 4 new kernel tests + the v165 `split_modifier_operand_*` + the v166 `scan_braced_operand_*`, unchanged — they are the behavior-preservation guard).

- [ ] **Step 8: Full lib suite + clippy**

Run: `cargo test --lib 2>&1 | grep -E 'test result: ok|test result: FAILED' | tail -1`
Expected: `test result: ok.` with 0 failed (≈2206 passed: 2202 + 4 new kernel tests).
Run: `cargo clippy --lib --bins --quiet 2>&1 | grep -E 'warning|error' || echo CLEAN`
Expected: `CLEAN`.

- [ ] **Step 9: Build + spot-check L-10/L-52 (no regression) + the plain `$()` path**

Run:
```bash
cargo build --quiet && H=./target/debug/huck
for f in 's=a-b-c; echo "${s/$(echo a/x)/Z}"' 's=a}b; echo "${s/$(echo a}b)/Z}"' 'echo "$(echo hi)"' 'x=$(echo $(echo nested)); echo "$x"' 'echo "${s:-$(echo d)}"'; do
  b=$(bash -c "$f" 2>&1); h=$($H -c "$f" 2>&1); [ "$b" = "$h" ] && echo "OK   $f -> [$h]" || echo "DIFF $f -> bash[$b] huck[$h]"; done
```
Expected: five `OK` lines (`[Z]`, `[Z]`, `[hi]`, `[nested]`, `[d]`).

- [ ] **Step 10: Confirm the net is a code reduction**

Run: `git diff --stat`
Expected: `src/lexer.rs` shows more deletions than insertions in non-test code (three loop bodies → one kernel + thin callers; the added unit tests offset some of it).

- [ ] **Step 11: Commit**

```bash
git add src/lexer.rs
git commit -m "v167: unify the \$()-scan loops onto one scan_cmdsub_body kernel

Extract the \$(…) body scan (paren depth + quote/escape skip, error variant as
a param) into one private kernel. scan_paren_substitution collects-then-parses
through it; consume_paren_cmdsub_verbatim re-adds the closing ); and
split_modifier_operand delegates on \$( (dropping its inline paren tracking).
Pure refactor, behavior-preserving — one source of truth for \$() scanning, so
the drift that caused L-10/L-52 can't recur.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Full bash-diff regression

No new harness (no behavior change). Run the whole differential suite as the safety net for the hot-path refactor.

**Files:** none (verification only).

- [ ] **Step 1: Run ALL bash-diff harnesses**

Run: `p=0; f=0; for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 && p=$((p+1)) || { f=$((f+1)); echo "FAIL $s"; }; done; echo "$p passed, $f failed"`
Expected: `92 passed, 0 failed`.

- [ ] **Step 2: Run the full test suite (unit + integration)**

Run: `cargo test >/tmp/v167.log 2>&1; echo "exit: $?"; grep -cE 'test result: FAILED' /tmp/v167.log`
Expected: `exit: 0` and a FAILED count of `0`.

(No commit — this task is the regression gate; if anything fails, STOP and investigate before proceeding to merge.)

---

## Final review (orchestrator, after both tasks)

- Whole-branch diff: only `src/lexer.rs` changed (kernel + unit tests + three rewired callers). Confirm the three callers keep their signatures and that `parse_substitution_body` is still invoked by `scan_paren_substitution`.
- Confirm the net non-test LOC dropped.
- Re-verify the L-10/L-52 cases and a couple of ordinary `$(…)` / backtick scripts by hand against bash.
- Merge `v167-cmdsub-scan-kernel` to main `--no-ff` after user confirmation (AskUserQuestion); push; delete the local branch.
- Record the iteration in `project_huck_iterations.md` + `MEMORY.md`; note the analogous backtick triplication as the remaining cleanup.

---

## Self-review (plan vs spec)

- **Spec coverage:** `scan_cmdsub_body` kernel with paren depth + quote/escape skip + param error + excludes `)` (Task 1 Step 3) ✓; `scan_paren_substitution` collects-then-parses (Step 5) ✓; `consume_paren_cmdsub_verbatim` wrapper re-adds `)` (Step 4) ✓; `split_modifier_operand` delegates + drops inline paren tracking + guard simplification (Step 6 a–f) ✓; kernel unit test incl. nested/arith/quoted/unterminated-with-both-variants (Step 1) ✓; behavior-preservation via v165/v166 unit tests + full suite + 92 harnesses + e2e spot-check (Steps 7–9, Task 2) ✓; LOC-reduction check (Step 10) ✓; scope — backtick/arith untouched, no divergence-doc edit ✓.
- **Placeholder scan:** none — every step shows exact code or a precise replacement + expected output.
- **Type consistency:** `scan_cmdsub_body(&mut CharCursor, &mut String, LexError) -> Result<(), LexError>` used identically by all three callers; `consume_paren_cmdsub_verbatim` and `scan_paren_substitution` keep their existing signatures; `split_modifier_operand` keeps `(&str, char) -> (String, Option<String>)`; `chars.peek()` compared with `Some(&'(')`.
