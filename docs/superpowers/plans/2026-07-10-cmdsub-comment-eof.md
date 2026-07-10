# v281 — Fix #109: comment/empty `$()` and `<()` body at EOF — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a command/process substitution whose body is only a comment (or only whitespace) and ends before `)` parse as an *unterminated substitution* (so the stdin/REPL reader keeps reading) instead of a hard error.

**Architecture:** Add the `peek == None → UnterminatedSubshell` guard that `parse_subshell` already has (parser.rs:4664) to the two substitution-body parsers `parse_command_sub` and `parse_process_sub`, which currently lack it and so surface `MissingCommand` for a comment-only body. `continuation::classify` already maps `UnterminatedSubshell → Incomplete(Subshell)`, so no classifier change is needed. Then remove the #109 XFAIL quarantine from the diff-check harness.

**Tech Stack:** Rust (huck-syntax parser, huck-engine continuation), bash diff-check harness.

## Global Constraints

- **Only these code paths change:** `crates/huck-syntax/src/parser.rs` (two guard insertions) + its test module `crates/huck-syntax/src/parser/tests.rs`, `crates/huck-engine/src/continuation.rs` (tests only), and `tests/scripts/cmdsub_comment_diff_check.sh` (flip the XFAIL). No `classify`/lexer logic change.
- **Both substitution parsers get the identical guard** — `parse_command_sub` AND `parse_process_sub`. Fixing only one is an incomplete fix (sibling-site gap).
- **Run tests per-crate, single-threaded** (box OOMs on `--workspace`): `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` and `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. Build the binary with `cargo build -p huck`.
- **`cargo fmt --all` before each commit**; CI enforces `cargo fmt --all --check`.
- **Every commit** ends with the trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Branch `v281-cmdsub-comment-eof`; do not push to main / do not merge.** PR (`Closes #109`) is for the user.
- The guard code (identical in both functions):
  ```rust
      if iter.peek_kind()?.is_none() {
          iter.pop_mode();
          return Err(ParseError::UnterminatedSubshell);
      }
  ```

---

### Task 1: Add the peek-None guard to `parse_command_sub` and `parse_process_sub`

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` (`parse_command_sub` ~line 1619; `parse_process_sub` ~line 1689)
- Test: `crates/huck-syntax/src/parser/tests.rs`

**Interfaces:**
- Consumes: existing `new_cs(s: &str, quoted: bool) -> Result<WordPart, ParseError>` and `new_seq(s: &str) -> Result<Option<Sequence>, ParseError>` test helpers; `ParseError::UnterminatedSubshell`.
- Produces: `parse_command_sub`/`parse_process_sub` return `Err(UnterminatedSubshell)` for a comment-only/empty-at-EOF body.

- [ ] **Step 1: Write the failing parser tests**

Add to the test module `crates/huck-syntax/src/parser/tests.rs` (near the other `new_cs` tests, e.g. after `cs_simple`):
```rust
#[test]
fn cs_comment_only_body_at_eof_is_unterminated() {
    // #109: a `$(` body that is only a comment (or empty) reaching EOF before
    // `)` is an UNTERMINATED substitution — so the stdin/REPL reader keeps
    // reading — not a MissingCommand error. Mirrors parse_subshell's guard.
    assert!(
        matches!(new_cs("$(# c", false), Err(ParseError::UnterminatedSubshell)),
        "comment-only body: got {:?}", new_cs("$(# c", false)
    );
    assert!(
        matches!(new_cs("$(", false), Err(ParseError::UnterminatedSubshell)),
        "bare $( at EOF: got {:?}", new_cs("$(", false)
    );
    // Regression guards: empty `$()` is still a valid empty substitution.
    assert!(new_cs("$()", false).is_ok(), "empty $(): {:?}", new_cs("$()", false));
}

#[test]
fn process_sub_comment_only_body_at_eof_is_unterminated() {
    // #109 sibling: `<(` with a comment-only body at EOF is likewise unterminated.
    assert!(
        matches!(new_seq("cat <(# c"), Err(ParseError::UnterminatedSubshell)),
        "procsub comment-only body: got {:?}", new_seq("cat <(# c")
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 cs_comment_only_body_at_eof_is_unterminated process_sub_comment_only_body_at_eof_is_unterminated`
Expected: both FAIL — current behavior returns `Err(MissingCommand)`, not `UnterminatedSubshell`.

- [ ] **Step 3: Add the guard to `parse_command_sub`**

In `crates/huck-syntax/src/parser.rs`, in `parse_command_sub`, immediately AFTER the leading-blank/newline skip loop and BEFORE the `let sequence = if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::RParen)))` line, insert the guard. The surrounding context becomes:
```rust
    while matches!(
        iter.peek_kind()?,
        Some(TokenKind::Blank) | Some(TokenKind::Newline)
    ) {
        iter.next_kind()?;
    }
    // #109: a body that is only whitespace/comments reaching EOF before `)` is
    // an UNTERMINATED substitution, not a missing command — mirror
    // parse_subshell's guard (~4664) so the REPL/stdin reader keeps reading.
    if iter.peek_kind()?.is_none() {
        iter.pop_mode();
        return Err(ParseError::UnterminatedSubshell);
    }
    let sequence = if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::RParen))) {
```

- [ ] **Step 4: Add the identical guard to `parse_process_sub`**

In the same file, in `parse_process_sub`, immediately AFTER its leading-blank/newline skip loop and BEFORE its `let sequence = if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::RParen)))` line, insert the same guard:
```rust
    // #109: same guard as parse_command_sub — a comment-only/empty body at EOF
    // is an unterminated process substitution.
    if iter.peek_kind()?.is_none() {
        iter.pop_mode();
        return Err(ParseError::UnterminatedSubshell);
    }
```

- [ ] **Step 5: Run the new tests + the full crate suite**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1`
Expected: the two new tests PASS and the whole `huck-syntax` suite is green (no regressions — empty `$()`/`$( )`/`<()` and `$(cmd)` cases still pass).

- [ ] **Step 6: Format and commit**

```bash
cargo fmt --all && cargo fmt --all --check
git add crates/huck-syntax/src/parser.rs crates/huck-syntax/src/parser/tests.rs
git commit -m "fix: unterminated (not missing-command) for comment/empty \$() and <() body at EOF (#109)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Verify via `classify`, close the #109 harness quarantine

**Files:**
- Test: `crates/huck-engine/src/continuation.rs` (test module)
- Modify: `tests/scripts/cmdsub_comment_diff_check.sh`

**Interfaces:**
- Consumes: the guard from Task 1 (via `parse_command_sub`/`parse_process_sub`, reached through `parse_sequence`); `classify`, `Completeness`, `ContinuationReason` (already in scope in the test module).
- Produces: end-to-end proof the REPL/stdin reader now continues; a green `cmdsub_comment_diff_check.sh` (8/8).

- [ ] **Step 1: Write the failing `classify` tests**

Add to the test module in `crates/huck-engine/src/continuation.rs` (near `open_command_substitution_is_incomplete`):
```rust
#[test]
fn classify_cmdsub_comment_only_body_is_incomplete() {
    // #109: a `$(` body that is only a comment, reaching EOF before `)`, must
    // request continuation (the stdin/REPL reader keeps reading), not Error.
    assert_eq!(
        classify("echo $(# c", false),
        Completeness::Incomplete(ContinuationReason::Subshell)
    );
}

#[test]
fn classify_procsub_comment_only_body_is_incomplete() {
    assert_eq!(
        classify("cat <(# c", false),
        Completeness::Incomplete(ContinuationReason::Subshell)
    );
}
```

- [ ] **Step 2: Run them to verify they fail (before Task 1 is present) or pass (after)**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 classify_cmdsub_comment_only_body_is_incomplete classify_procsub_comment_only_body_is_incomplete`
Expected: PASS (Task 1's guard is already committed, so `classify` now returns `Incomplete(Subshell)`). If run against a tree without Task 1, they FAIL with `Error`.

- [ ] **Step 3: Flip the #109 XFAIL back to a hard check in the harness**

In `tests/scripts/cmdsub_comment_diff_check.sh`:

(a) Delete the `xfail()` helper and its preceding comment block (added in v280) — the three `# Expected-fail: …` comment lines plus the `xfail() { … }` function:
```bash
# Expected-fail: huck currently diverges from bash on this input (tracked in
# #109 — comment inside $() ). Passes the harness while it stays broken, and
# self-flags the day it is silently fixed so we restore check() and close #109.
xfail() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" != "$h" ]]; then printf 'XFAIL: %s (#109)\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s unexpectedly passes — close #109 and restore check()\n' "$label"; FAIL=$((FAIL+1)); fi
}
```

(b) Change the quarantined call from `xfail` back to `check` (label and fragment unchanged):
```bash
check "comment after open"    'echo "[$(# c with ) paren
echo yo)]"'
```

- [ ] **Step 4: Run the harness — expect 8/8**

Ensure the binary is current: `cargo build -p huck`. Then:
```bash
HUCK_BIN="$(pwd)/target/debug/huck" bash tests/scripts/cmdsub_comment_diff_check.sh; echo "exit=$?"
```
Expected: no `XFAIL`/`FAIL` lines, `Total: 8, Pass: 8, Fail: 0`, `exit=0`.

- [ ] **Step 5: Run the full diff-check sweep — still green**

```bash
cargo build -p huck && cargo build --release -p huck
tests/scripts/run_diff_checks.sh; echo "exit=$?"
```
Expected: `Diff-check sweep: 180 passed, 0 failed`, `exit=0` (now with `cmdsub_comment` passing on merit).

- [ ] **Step 6: Format and commit**

```bash
cargo fmt --all && cargo fmt --all --check
git add crates/huck-engine/src/continuation.rs tests/scripts/cmdsub_comment_diff_check.sh
git commit -m "test: classify continuation for comment-only \$()/<() body; close #109 harness quarantine (#109)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final verification (after all tasks)

- [ ] Both crate suites green:
```bash
cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1
```
- [ ] End-to-end behavior matches bash (stdin path):
```bash
HUCK="$(pwd)/target/debug/huck"
printf 'echo "[$(# c with ) paren\necho yo)]"\n' | "$HUCK"   # -> [yo]
printf 'echo $(\n# just a comment\n)\n' | "$HUCK"            # -> (empty), rc 0
```
Expected: `[yo]` and an empty line, both rc 0 — identical to bash.

## Notes for the whole-branch review

- The two guards must be byte-identical in intent; confirm BOTH `parse_command_sub` and `parse_process_sub` got it (sibling-site check).
- Out of scope (per spec): backticks `` `#c` `` and the interactive history-collapse joiner cosmetics.
- The merged PR auto-closes #109 (`Closes #109`); #109 is a real divergence, so no `docs/bash-divergences.md` entry.
