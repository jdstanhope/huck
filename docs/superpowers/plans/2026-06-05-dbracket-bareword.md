# Bare-word `[[ word ]]` Truthiness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `[[ word ]]` behave as `[[ -n word ]]` (true iff the single operand is non-empty after expansion), closing a pre-existing M-14 gap that breaks bash-completion sourcing.

**Architecture:** One surgical change to the `[[` expression parser in `src/command.rs`. `parse_test_atom` currently consumes the LHS word then *unconditionally* expects a binary operator; when the next token is the `]]` close it raises `TestExprBadOperator("]]")`. Add a peek-only helper `next_is_test_binary_operator` and, when no operator follows the LHS, return `TestExpr::Unary { op: StringNonEmpty, operand }` — the exact node `[[ -n word ]]` already produces. The evaluator and the precedence layers (`parse_test_or`/`_and`/`_not`, grouping) need no changes.

**Tech Stack:** Rust (binary crate `huck`). Unit tests via `cargo test --bin huck`; integration tests via `cargo test --test dbracket_multiline_integration`; bash-diff harness `tests/scripts/dbracket_multiline_diff_check.sh`.

---

## File Structure

- `src/command.rs` — add `next_is_test_binary_operator` helper (near `is_test_expr_stop`, ~line 1817) and the bare-word early-return in `parse_test_atom` (after the LHS is consumed, ~line 1990, before the operator `match`). Add unit tests in the existing `#[cfg(test)] mod tests` block (alongside the other `parse_dbracket_*` tests, ~line 4196).
- `tests/dbracket_multiline_integration.rs` — extend the existing v87 file (reuses its `run()` helper) with bare-word exit-code tests. (The spec named a new `dbracket_bareword_integration.rs`; we instead extend the existing `[[`-surface file so all `[[` integration tests live together — a deliberate consolidation.)
- `tests/scripts/dbracket_multiline_diff_check.sh` — extend the existing v87 harness with bare-word fragments.
- `docs/bash-divergences.md` — new sub-entry **M-14c `[fixed v92]`**; log the newly-discovered deferrals; changelog entry; bump the Tier-2 fixed count.
- `README.md` — v92 iteration row.

---

### Task 1: Parser fix + unit tests (`src/command.rs`)

**Files:**
- Modify: `src/command.rs` (add helper ~after `is_test_expr_stop` line 1817; early-return inside `parse_test_atom` ~line 1990)
- Test: `src/command.rs` (`#[cfg(test)] mod tests`, ~line 4196 alongside `parse_dbracket_*`)

- [ ] **Step 1: Write the failing unit tests**

Add these five tests inside the `mod tests` block (next to the other `parse_dbracket_*` tests). They mirror the existing helper pattern (`tokenize` → `parse` → match on `Command::DoubleBracket { expr, .. }`):

```rust
#[test]
fn parse_dbracket_bareword_single() {
    // `[[ foo ]]` ≡ `[[ -n foo ]]`.
    let tokens = crate::lexer::tokenize("[[ foo ]]").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty");
    let Command::DoubleBracket { expr, .. } = parsed.first else { panic!() };
    let TestExpr::Unary { op, operand } = &*expr else {
        panic!("expected Unary, got {:?}", expr)
    };
    assert!(matches!(op, TestUnaryOp::StringNonEmpty));
    assert_eq!(word_literal_text(operand), Some("foo"));
}

#[test]
fn parse_dbracket_bareword_and() {
    // `[[ a && b ]]` → And(-n a, -n b).
    let tokens = crate::lexer::tokenize("[[ a && b ]]").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty");
    let Command::DoubleBracket { expr, .. } = parsed.first else { panic!() };
    let TestExpr::And(l, r) = &*expr else { panic!("expected And, got {:?}", expr) };
    assert!(matches!(&**l, TestExpr::Unary { op: TestUnaryOp::StringNonEmpty, .. }));
    assert!(matches!(&**r, TestExpr::Unary { op: TestUnaryOp::StringNonEmpty, .. }));
}

#[test]
fn parse_dbracket_bareword_not() {
    // `[[ ! foo ]]` → Not(-n foo).
    let tokens = crate::lexer::tokenize("[[ ! foo ]]").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty");
    let Command::DoubleBracket { expr, .. } = parsed.first else { panic!() };
    let TestExpr::Not(inner) = &*expr else { panic!("expected Not, got {:?}", expr) };
    assert!(matches!(&**inner, TestExpr::Unary { op: TestUnaryOp::StringNonEmpty, .. }));
}

#[test]
fn parse_dbracket_bareword_grouped() {
    // `[[ ( foo ) ]]` → grouped -n foo.
    let tokens = crate::lexer::tokenize("[[ ( foo ) ]]").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty");
    let Command::DoubleBracket { expr, .. } = parsed.first else { panic!() };
    assert!(matches!(&*expr, TestExpr::Unary { op: TestUnaryOp::StringNonEmpty, .. }));
}

#[test]
fn parse_dbracket_operator_still_wins() {
    // Regression: `[[ word == x ]]` stays a binary `==`, NOT a bare-word test.
    let tokens = crate::lexer::tokenize("[[ word == x ]]").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty");
    let Command::DoubleBracket { expr, .. } = parsed.first else { panic!() };
    let TestExpr::Binary { op, .. } = &*expr else { panic!("expected Binary, got {:?}", expr) };
    assert!(matches!(op, TestBinaryOp::StringEq));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --bin huck parse_dbracket_bareword 2>&1 | tail -20`
Expected: the four `bareword` tests FAIL (panic with `TestExprBadOperator("]]")` surfacing as a parse error / `expect("non-empty")` unwrap on `Err`). `parse_dbracket_operator_still_wins` PASSES already (operator path unaffected).

- [ ] **Step 3: Add the `next_is_test_binary_operator` helper**

Insert immediately after the `is_test_expr_stop` function (~line 1822 in `src/command.rs`):

```rust
/// Peeks (consumes nothing) and reports whether the next token is a recognized
/// `[[ ]]` binary operator. Used by `parse_test_atom` to distinguish a binary
/// test (`lhs OP rhs`) from a bare-word test (`[[ word ]]` ≡ `[[ -n word ]]`).
///
/// KEEP THIS OPERATOR SET IN SYNC with the operator match arms in
/// `parse_test_atom` below. `<` / `>` arrive as `Op(RedirIn)` / `Op(RedirOut)`;
/// every other operator arrives as a `Word` because the lexer has no dedicated
/// token for it.
fn next_is_test_binary_operator<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> bool {
    match iter.peek() {
        Some(Token::Op(Operator::RedirIn)) | Some(Token::Op(Operator::RedirOut)) => true,
        Some(tok @ Token::Word(_)) => matches!(
            word_literal_text_tok(tok),
            Some("==" | "=" | "!=" | "=~" | "-eq" | "-ne" | "-lt" | "-gt"
                | "-le" | "-ge" | "-nt" | "-ot" | "-ef")
        ),
        _ => false,
    }
}
```

Note on `word_literal_text`: the existing operator match calls `word_literal_text(&op_word)` on an owned `Word`. For the peek path we have a `&Token`. If a `word_literal_text` that takes `&Token` does not already exist, add a tiny shim next to the helper:

```rust
/// `word_literal_text` for a borrowed token: returns the single unquoted
/// literal text of a `Token::Word`, else `None`.
fn word_literal_text_tok(tok: &Token) -> Option<&str> {
    match tok {
        Token::Word(w) => word_literal_text(w),
        _ => None,
    }
}
```

(If `word_literal_text` already accepts something usable from a `&Token`, prefer reusing it and drop the shim. Check its signature first — it is the same fn used at the existing `word_literal_text(&op_word)` call site in `parse_test_atom`.)

- [ ] **Step 4: Add the bare-word early-return in `parse_test_atom`**

In `parse_test_atom`, the binary path currently reads (after the unary-op block):

```rust
    // Binary / regex path: lhs op rhs.
    // Consume the LHS word (first_word peeked above).
    iter.next();
    let lhs = first_word;

    // Peek at the operator token. ...
    let op_token = iter.next();
    match op_token {
```

Insert the bare-word check between `let lhs = first_word;` and `let op_token = iter.next();`:

```rust
    iter.next();
    let lhs = first_word;

    // Bash: `[[ word ]]` ≡ `[[ -n word ]]`. When no binary operator follows the
    // operand (next token is `]]` / `)` / `&&` / `||` / end-of-input), the lhs
    // alone is a non-empty-string test. See `next_is_test_binary_operator` —
    // keep its operator set in sync with the match arms below.
    if !next_is_test_binary_operator(iter) {
        return Ok(TestExpr::Unary { op: TestUnaryOp::StringNonEmpty, operand: lhs });
    }

    // Peek at the operator token. ...
    let op_token = iter.next();
    match op_token {
```

Leave the entire operator `match` (including its `None` / `Some(other_tok)` / `other =>` defensive arms) **unchanged**.

- [ ] **Step 5: Run the unit tests to verify they pass**

Run: `cargo test --bin huck parse_dbracket 2>&1 | tail -25`
Expected: all `parse_dbracket_*` tests PASS (the four new bareword tests + the regression + all pre-existing ones).

- [ ] **Step 6: Run the full unit suite + clippy (no regressions)**

Run: `cargo test --bin huck 2>&1 | tail -15 && cargo clippy --all-targets 2>&1 | tail -15`
Expected: all tests pass; clippy clean (no new warnings).

- [ ] **Step 7: Commit**

```bash
git add src/command.rs
git commit -m "feat: bare-word [[ word ]] truthiness in parse_test_atom (M-14c)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Integration tests + bash-diff harness

**Files:**
- Modify: `tests/dbracket_multiline_integration.rs` (append tests; reuse existing `run()` helper)
- Modify: `tests/scripts/dbracket_multiline_diff_check.sh` (append fragments before the totals line)

- [ ] **Step 1: Write the failing integration tests**

Append to `tests/dbracket_multiline_integration.rs` (the `run()` helper returns `(stdout, exit_code)`):

```rust
#[test]
fn bareword_nonempty_true() {
    assert_eq!(run("[[ foo ]] && echo Y || echo N\n").0, "Y\n");
}

#[test]
fn bareword_empty_false() {
    assert_eq!(run("[[ \"\" ]] && echo Y || echo N\n").0, "N\n");
}

#[test]
fn bareword_var_set_vs_empty() {
    assert_eq!(run("x=hi\n[[ $x ]] && echo Y || echo N\n").0, "Y\n");
    assert_eq!(run("x=\"\"\n[[ $x ]] && echo Y || echo N\n").0, "N\n");
    assert_eq!(run("unset x\n[[ $x ]] && echo Y || echo N\n").0, "N\n");
}

#[test]
fn bareword_in_connectives() {
    assert_eq!(run("[[ -n foo && foo ]] && echo Y || echo N\n").0, "Y\n");
    assert_eq!(run("[[ \"\" || foo ]] && echo Y || echo N\n").0, "Y\n");
    assert_eq!(run("[[ foo && \"\" ]] && echo Y || echo N\n").0, "N\n");
}

#[test]
fn bareword_negated_empty_true() {
    assert_eq!(run("[[ ! \"\" ]] && echo Y || echo N\n").0, "Y\n");
}

#[test]
fn bareword_grouped() {
    assert_eq!(run("[[ ( foo ) ]] && echo Y || echo N\n").0, "Y\n");
}
```

- [ ] **Step 2: Build the binary, then run the integration tests to verify they fail**

Run: `cargo build --bin huck && cargo test --test dbracket_multiline_integration bareword 2>&1 | tail -25`
Expected: the new `bareword_*` tests FAIL on the pre-fix binary (output `N`/empty where `Y` expected, or a syntax error). On a build that already includes Task 1 they PASS — if Task 1 is committed, instead confirm they PASS here and skip to Step 4.

- [ ] **Step 3: (Only if Task 1 not yet built into the binary) rebuild**

Run: `cargo build --bin huck`
Expected: builds clean (Task 1 already implemented the parser change).

- [ ] **Step 4: Run the integration tests to verify they pass**

Run: `cargo test --test dbracket_multiline_integration 2>&1 | tail -20`
Expected: all tests in the file PASS (new `bareword_*` plus the pre-existing multiline/`-v`/`-nt` tests).

- [ ] **Step 5: Extend the bash-diff harness**

In `tests/scripts/dbracket_multiline_diff_check.sh`, insert these `check` lines immediately before the final `echo ""; echo "Total: ..."` line:

```bash
# Bare-word truthiness: [[ word ]] ≡ [[ -n word ]]  (M-14c, v92)
check "bareword nonempty"  '[[ foo ]]; echo $?'
check "bareword empty"     '[[ "" ]]; echo $?'
check "bareword var set"   's=x; [[ $s ]]; echo $?'
check "bareword var empty" 'e=; [[ $e ]]; echo $?'
check "bareword var unset" 'unset u; [[ $u ]]; echo $?'
check "bareword and"       '[[ a && b ]]; echo $?'
check "bareword or empty"  '[[ "" || z ]]; echo $?'
check "bareword and empty" '[[ a && "" ]]; echo $?'
check "bareword not empty" '[[ ! "" ]]; echo $?'
check "bareword grouped"   '[[ ( a ) ]]; echo $?'
check "bareword op wins"   '[[ word == x ]]; echo $?'
check "bareword op match"  '[[ word == word ]]; echo $?'
```

- [ ] **Step 6: Run the harness to verify byte-identical to bash**

Run: `cargo build --bin huck && bash tests/scripts/dbracket_multiline_diff_check.sh 2>&1 | tail -25`
Expected: every line `PASS`, final `Fail: 0`. If any `FAIL`, the diff block shows the bash-vs-huck divergence — investigate before committing (do NOT adjust expected output to match huck; bash is the oracle).

- [ ] **Step 7: Commit**

```bash
git add tests/dbracket_multiline_integration.rs tests/scripts/dbracket_multiline_diff_check.sh
git commit -m "test: bare-word [[ word ]] integration + bash-diff coverage (M-14c)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Documentation (`docs/bash-divergences.md`, `README.md`)

**Files:**
- Modify: `docs/bash-divergences.md` (M-14c entry + deferrals + changelog + count)
- Modify: `README.md` (v92 row)

- [ ] **Step 1: Read the M-14 entry + changelog top + Tier-2 count**

Run: `grep -n 'M-14\|^## Change log\|^### v91\|Tier 2\|Missing features' docs/bash-divergences.md | head -30`
Read the surrounding context so the new sub-entry matches the existing M-14a/M-14b formatting and the changelog matches the v91 entry style.

- [ ] **Step 2: Add the M-14c sub-entry**

Under the M-14 family (next to M-14a/M-14b), add:

```markdown
- **M-14c** `[[ word ]]` bare-word (single-operand) test — `[fixed v92]`.
  A lone operand inside `[[ ]]` is a non-empty-string test: `[[ word ]]` ≡
  `[[ -n word ]]` (true iff the operand is non-empty after expansion). v30's
  M-14 parser required `lhs OP rhs` or a unary op, so a bare word raised
  `unrecognised operator in '[[ ]]': ']]'` — which cascaded into spurious
  `unexpected else/fi/}` when sourcing bash-completion (it uses `[[ $x ]]`
  pervasively). Fixed in `parse_test_atom`: when no binary operator follows the
  operand, emit `TestExpr::Unary { StringNonEmpty }`. Composes with
  `&&`/`||`/`!`/`( )` for free. Harness: bare-word fragments added to
  `dbracket_multiline_diff_check.sh`.
```

- [ ] **Step 3: Log the newly-discovered deferrals**

In the Missing-features (Tier 2) section, add these as `[deferred]` entries (assign new M-numbers following the current highest; severity as noted). Match the existing one-line-per-entry style:

```markdown
- **M-XX** `command CMD` bare form (without `-v`/`-V`) — run CMD bypassing
  functions/aliases. `[deferred]` (medium) — surfaced sourcing oh-my-posh/mise.
- **M-XX** `${var@OP}` parameter transforms (`@Q` `@U` `@L` `@P` `@A` `@a` `@k`).
  `[deferred]` (medium) — surfaced sourcing oh-my-posh.
- **M-XX** `${arr[@]:-word}` / `:OP` modifiers applied to array values
  (M-82 follow-on; currently errors "modifier … not supported on array").
  `[deferred]` (medium).
- **M-XX** arithmetic `${...}` / `arr[i]` evaluation inside `(( ))`.
  `[deferred]` (low).
- **M-XX** `export -f` / `export -a` flags. `[deferred]` (low).
```

(Use the actual next free M-numbers; keep the dashes/severity tags consistent with neighbors.)

- [ ] **Step 4: Add the change-log entry + bump counts**

At the top of the change log add a v92 entry mirroring the v91 style (date 2026-06-05, M-14c, what changed, harness count note: the `dbracket_multiline` harness now also covers bare-word — still 18 harness files, no new file). Bump any "N fixed" Tier-2 tally that the doc maintains.

- [ ] **Step 5: Add the README v92 row**

In the README iteration table, add the v92 row after v91, matching the column format of neighboring rows (version, M-id, one-line summary: "bare-word `[[ word ]]` truthiness (`[[ word ]]` ≡ `[[ -n word ]]`)").

- [ ] **Step 6: Verify the docs build/links**

Run: `grep -n 'M-14c\|v92' docs/bash-divergences.md README.md`
Expected: M-14c shows `[fixed v92]`; README has the v92 row; no stray placeholder text.

- [ ] **Step 7: Commit**

```bash
git add docs/bash-divergences.md README.md
git commit -m "docs: M-14c bare-word [[ ]] fixed v92; log new deferrals; README row

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

- **Spec coverage:** §1 parser fix → Task 1. §"Testing" unit/integration/harness → Tasks 1 & 2. §"Newly-discovered deferrals" → Task 3 Step 3. M-14c + README → Task 3. All spec sections covered.
- **Placeholder scan:** the only intentional `M-XX` placeholders are in Task 3 Step 3, explicitly instructing the implementer to substitute the next free M-numbers (the doc's numbering is not knowable until the file is read). No code-step placeholders.
- **Type consistency:** `TestExpr::Unary { op, operand }`, `TestUnaryOp::StringNonEmpty`, `TestBinaryOp::StringEq`, `TestExpr::And(Box, Box)`, `TestExpr::Not(Box)`, `Token::Op(Operator::RedirIn/RedirOut)`, `Token::Word`, `word_literal_text` — all match the names read from `src/command.rs`. The `next_is_test_binary_operator` signature mirrors `is_test_expr_stop` (both `&mut Peekable<I>`).
- **Edge cases:** `[[ ]]` empty body still errors (the `is_test_expr_stop` guard fires first); `[[ a == ]]` still errors (operator present → binary path → `next_test_word` fails). Both unchanged by this plan.
