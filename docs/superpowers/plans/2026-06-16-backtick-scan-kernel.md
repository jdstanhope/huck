# v168: backtick-scan kernel — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the backtick (`` `…` ``) command-substitution boundary scan into one private `scan_backtick_body` kernel and route the three current copies through it, mirroring v167 — with no change in observable behavior.

**Architecture:** `scan_backtick_body(chars, out, unterminated)` scans to the matching un-escaped backtick (consumed, not appended), appending the **raw** body (escapes preserved); error variant passed in. The two verbatim arms delegate via a `consume_backtick_verbatim` wrapper (re-adds the closing backtick); the parse path uses the kernel + a new `unescape_backtick` helper + `parse_substitution_body`. Pure refactor.

**Tech Stack:** Rust (edition 2024). `CharCursor` (lexer.rs:37): `new`/`peek`/`next` + `Iterator`. `LexError::{UnterminatedSubstitution, UnterminatedBrace}`. Backticks do not nest and are quote-naive — no depth/quote tracking.

**Spec:** `docs/superpowers/specs/2026-06-16-backtick-scan-kernel-design.md`

**Branch:** `v168-backtick-scan-kernel`

---

## Background the implementer needs

All changes are in `src/lexer.rs`. The three current backtick scanners:

- `scan_backtick_substitution` (~line 2330): un-escapes while scanning (`` \` `` → `` ` ``, `\\` → `\`, `\$` → `$`, `\x` → `\x`), then `parse_substitution_body(&body, opts)`; errors `UnterminatedSubstitution`. Returns `Result<Sequence, LexError>`. Called by the main lexer's `` `…` `` arm (signature unchanged here).
- `scan_braced_operand` backtick arm (~line 2152, added v166): pushes the opening backtick, then a `loop` appending the body raw (`\`+next verbatim; bare backtick → push + break; `None` → `Err(UnterminatedBrace)`).
- `split_modifier_operand` backtick arm (~line 3199, added v165): pushes the opening backtick, then `while let Some(qc) = chars.next()` appending raw (`\` → also push next; bare backtick → break; no error on EOF — its input is pre-balanced by `scan_braced_operand`).

The kernel is the verbatim arms' boundary logic with the error variant parameterized. Verified: `scan_backtick_body`'s raw output passed through `unescape_backtick` reproduces exactly the body `scan_backtick_substitution`'s inline loop builds.

The lexer test module is `#[cfg(test)] mod tests { use super::*; … }`; add unit tests near the `scan_cmdsub_body_*` tests (added v167).

---

### Task 1: Add the kernel, wrapper, and un-escape helper; route the three callers through them

A single cohesive refactor. Kernel/helper unit tests come first (TDD).

**Files:**
- Modify: `src/lexer.rs` — add `scan_backtick_body` + `consume_backtick_verbatim` + `unescape_backtick`; rewire the three callers; add unit tests.

- [ ] **Step 1: Write the failing unit tests**

Add these inside the `#[cfg(test)] mod tests { … }` block in `src/lexer.rs` (next to the `scan_cmdsub_body_*` tests):

```rust
    #[test]
    fn scan_backtick_body_basic_consumes_through_close() {
        let mut chars = CharCursor::new("echo hi`rest");
        let mut out = String::new();
        scan_backtick_body(&mut chars, &mut out, LexError::UnterminatedSubstitution).unwrap();
        assert_eq!(out, "echo hi"); // closing backtick consumed, not appended
        assert_eq!(chars.next(), Some('r'));
    }

    #[test]
    fn scan_backtick_body_escaped_backtick_does_not_close() {
        // Input: a \ ` b `  — the escaped backtick is raw-preserved and does not close.
        let mut chars = CharCursor::new("a\\`b`");
        let mut out = String::new();
        scan_backtick_body(&mut chars, &mut out, LexError::UnterminatedBrace).unwrap();
        assert_eq!(out, "a\\`b"); // raw, escape preserved
    }

    #[test]
    fn scan_backtick_body_unterminated_uses_passed_error() {
        let mut chars = CharCursor::new("echo hi");
        let mut out = String::new();
        assert_eq!(
            scan_backtick_body(&mut chars, &mut out, LexError::UnterminatedSubstitution).unwrap_err(),
            LexError::UnterminatedSubstitution
        );
        let mut chars = CharCursor::new("echo hi");
        let mut out = String::new();
        assert_eq!(
            scan_backtick_body(&mut chars, &mut out, LexError::UnterminatedBrace).unwrap_err(),
            LexError::UnterminatedBrace
        );
    }

    #[test]
    fn unescape_backtick_applies_bash_rules() {
        assert_eq!(unescape_backtick("a\\`b"), "a`b"); // \` -> `
        assert_eq!(unescape_backtick("a\\\\b"), "a\\b"); // \\ -> \
        assert_eq!(unescape_backtick("a\\$b"), "a$b"); // \$ -> $
        assert_eq!(unescape_backtick("a\\xb"), "a\\xb"); // \x -> \x (verbatim)
        assert_eq!(unescape_backtick("plain"), "plain");
    }
```

- [ ] **Step 2: Run them to confirm they fail (don't compile)**

Run: `cargo test --lib 'scan_backtick_body' 2>&1 | grep -E 'error\[|cannot find' | head`
Expected: `cannot find function `scan_backtick_body` …` (and `unescape_backtick`).

- [ ] **Step 3: Add the kernel + wrapper + un-escape helper**

Insert these three functions immediately **after** `fn consume_paren_cmdsub_verbatim(` (i.e., right after its closing `}`) in `src/lexer.rs`:

```rust
/// Scans a backtick (`` `…` ``) command-substitution body, the opening backtick
/// having already been consumed by the caller. Consumes through the matching
/// un-escaped backtick (consumed but NOT appended); a `\` escapes the next char
/// (so `` \` `` does not close — the `\` and next char are appended raw). The
/// raw body (escapes preserved, excluding the closing backtick) is appended to
/// `out`. Backticks are quote-naive and do not nest. EOF → `Err(unterminated)`.
/// The single source of truth for backtick boundary scanning (see
/// `scan_backtick_substitution`, `consume_backtick_verbatim`).
fn scan_backtick_body(
    chars: &mut CharCursor<'_>,
    out: &mut String,
    unterminated: LexError,
) -> Result<(), LexError> {
    loop {
        match chars.next() {
            None => return Err(unterminated),
            Some('`') => return Ok(()),
            Some('\\') => {
                out.push('\\');
                match chars.next() {
                    Some(c) => out.push(c),
                    None => return Err(unterminated),
                }
            }
            Some(c) => out.push(c),
        }
    }
}

/// Appends a backtick command substitution to `out` verbatim, the opening
/// backtick having already been pushed by the caller: the kernel collects the
/// raw body (excluding the closing backtick); this re-adds the closing backtick.
fn consume_backtick_verbatim(
    chars: &mut CharCursor<'_>,
    out: &mut String,
) -> Result<(), LexError> {
    scan_backtick_body(chars, out, LexError::UnterminatedBrace)?;
    out.push('`');
    Ok(())
}

/// Applies bash's backtick un-escaping to a raw backtick body: `` \` `` → `` ` ``,
/// `\\` → `\`, `\$` → `$`, and `\x` (any other char) → `\x` verbatim. A trailing
/// lone `\` is kept. Only the parse path un-escapes, so it lives in one function.
fn unescape_backtick(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('`') => out.push('`'),
                Some('\\') => out.push('\\'),
                Some('$') => out.push('$'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}
```

- [ ] **Step 4: Rewire `scan_backtick_substitution` to the kernel**

Replace the entire body of `scan_backtick_substitution` (its `let mut body …; while … { match … } Err(…)` loop) with:

```rust
fn scan_backtick_substitution(
    chars: &mut CharCursor<'_>,
    opts: LexerOptions,
) -> Result<crate::command::Sequence, LexError> {
    let mut raw = String::new();
    scan_backtick_body(chars, &mut raw, LexError::UnterminatedSubstitution)?;
    parse_substitution_body(&unescape_backtick(&raw), opts)
}
```

(Keep any doc comment above the function.)

- [ ] **Step 5: Rewire the `scan_braced_operand` backtick arm**

Replace its backtick arm (the `Some('`') => { … }` block with the inner `loop`, ~lines 2152–2173) with:

```rust
            Some('`') => {
                // Backtick command substitution: consume verbatim through the
                // matching unescaped backtick so a `}` inside it does not close
                // the operand (L-52). `\` escapes the next char inside.
                body.push('`');
                consume_backtick_verbatim(chars, &mut body)?;
            }
```

- [ ] **Step 6: Rewire the `split_modifier_operand` backtick arm**

Replace its backtick arm (the `'`' => { … }` block with the inner `while let`, ~lines 3199–3212) with:

```rust
            '`' => {
                let dst = if delim_seen { &mut second } else { &mut first };
                dst.push('`');
                // Skip the backtick command substitution verbatim. The Result is
                // unreachable (operand body pre-balanced by scan_braced_operand);
                // on the impossible EOF the closing backtick is not re-added and
                // the cursor is exhausted, so the loop ends with identical segments.
                let _ = consume_backtick_verbatim(&mut chars, dst);
            }
```

- [ ] **Step 7: Run the kernel/helper + sibling unit tests**

Run: `cargo test --lib 'scan_backtick_body' && cargo test --lib 'unescape_backtick' && cargo test --lib 'scan_braced_operand' && cargo test --lib 'split_modifier_operand'`
Expected: all pass (4 new + the v165/v166/v167 unit tests unchanged).

- [ ] **Step 8: Full lib suite + clippy**

Run: `cargo test --lib 2>&1 | grep -E 'test result: ok|test result: FAILED' | tail -1`
Expected: `test result: ok.` with 0 failed (≈2210 passed: 2206 + 4 new).
Run: `cargo clippy --lib --bins --quiet 2>&1 | grep -E 'warning|error' || echo CLEAN`
Expected: `CLEAN`.

- [ ] **Step 9: Build + spot-check against bash**

Run:
```bash
cargo build --quiet && H=./target/debug/huck
for f in 'echo `echo hi`' 'echo "`echo a b`"' 's=a}b; echo "${s/`echo a}b`/Z}"' 'x=5; echo `echo $x`' 'echo `echo a\`b 2>/dev/null || echo esc`'; do
  b=$(bash -c "$f" 2>&1); h=$($H -c "$f" 2>&1); [ "$b" = "$h" ] && echo "OK   $f -> [$h]" || echo "DIFF $f -> bash[$b] huck[$h]"; done
```
Expected: five `OK` lines.

- [ ] **Step 10: Confirm net code reduction**

Run: `git diff --stat`
Expected: `src/lexer.rs` shows the three inline loops replaced by one kernel + two small helpers + thin callers (net non-test reduction; new unit tests offset some).

- [ ] **Step 11: Commit**

```bash
git add src/lexer.rs
git commit -m "v168: unify the backtick-scan loops onto one scan_backtick_body kernel

Extract the backtick boundary scan (to the first un-escaped backtick, error
variant as a param) into one private kernel. scan_braced_operand and
split_modifier_operand delegate via consume_backtick_verbatim; the parse path
scan_backtick_substitution uses the kernel + a new unescape_backtick helper,
then parses. Pure refactor, behavior-preserving — mirrors v167's \$() kernel.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Full bash-diff regression

No new harness (no behavior change). Run the whole differential suite as the safety net (the parse path is hit by every `` `…` `` in every test).

**Files:** none (verification only).

- [ ] **Step 1: Run ALL bash-diff harnesses**

Run: `p=0; f=0; for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 && p=$((p+1)) || { f=$((f+1)); echo "FAIL $s"; }; done; echo "$p passed, $f failed"`
Expected: `92 passed, 0 failed`.

- [ ] **Step 2: Run the full test suite (unit + integration)**

Run: `cargo test >/tmp/v168.log 2>&1; echo "exit: $?"; grep -cE 'test result: FAILED' /tmp/v168.log`
Expected: `exit: 0` and a FAILED count of `0`.

(No commit — regression gate; if anything fails, STOP and investigate before merge.)

---

## Final review (orchestrator, after both tasks)

- Whole-branch diff: only `src/lexer.rs` changed (kernel + 2 helpers + unit tests + three rewired callers). Confirm `scan_backtick_substitution` keeps its signature and still calls `parse_substitution_body`.
- Confirm net non-test LOC dropped and no stray inline backtick `loop`/`while let` remains in the two arms.
- Re-verify a few backtick scripts (plain, with `$VAR`, with an escaped inner backtick, in a `${…}` operand) by hand against bash.
- Merge `v168-backtick-scan-kernel` to main `--no-ff` after user confirmation (AskUserQuestion); push; delete the local branch.
- Record the iteration in `project_huck_iterations.md` + `MEMORY.md`; note `$()`+backtick kernels both done, L-24 (arith.rs) the remaining scanner follow-on.

---

## Self-review (plan vs spec)

- **Spec coverage:** `scan_backtick_body` kernel (raw, param error, excludes close) (Task 1 Step 3) ✓; `consume_backtick_verbatim` wrapper re-adds backtick (Step 3) ✓; `unescape_backtick` helper (Step 3) ✓; `scan_backtick_substitution` = kernel + un-escape + parse (Step 4) ✓; the two verbatim arms delegate, split ignores the unreachable Result (Steps 5–6) ✓; unit tests for kernel + un-escape + unterminated-both-variants (Step 1) ✓; behavior-preservation via v165/v166/v167 tests + full suite + 92 harnesses + e2e (Steps 7–9, Task 2) ✓; LOC check (Step 10) ✓; scope — `$()`/arith untouched, no divergence-doc edit ✓.
- **Placeholder scan:** none — every step shows exact code or a precise replacement + expected output.
- **Type consistency:** `scan_backtick_body(&mut CharCursor, &mut String, LexError) -> Result<(), LexError>`, `consume_backtick_verbatim(&mut CharCursor, &mut String) -> Result<(), LexError>`, `unescape_backtick(&str) -> String` used consistently; `scan_backtick_substitution` keeps its `(&mut CharCursor, LexerOptions) -> Result<Sequence, LexError>` signature.
