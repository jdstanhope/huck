# v277 — Script Parse Error at a Unit Boundary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a script/`source`d file has a parse error at the start of a unit, huck executes the already-parsed earlier units and reports the error at the offending token's line — matching bash — instead of discarding the preceding unit and reporting the EOF line. Resolves [#86](https://github.com/jdstanhope/huck/issues/86).

**Architecture:** In `parse_and_or_opts` unit mode, ending a unit at a top-level newline calls `collect_heredoc_bodies_after_newline`, whose `peek_kind()?` over-scans into the *next* unit's first token; a lex error there discards the current (already-parsed) unit. Fix: add a lexer predicate `has_pending_heredoc_body()` and short-circuit the heredoc-collect loop on it, so the peek only happens when a heredoc body is actually pending. The reader's existing `pending_lex_err` recovery path then surfaces the error at the correct line and still executes the parsed unit.

**Tech Stack:** Rust (workspace crates `huck-syntax` = lexer/parser, `huck` root = binary + integration tests); bash 5.2.21 differential harnesses under `tests/scripts/*_diff_check.sh`.

## Global Constraints

- **Bash compatibility target is byte-identical stdout + exit code.** stderr *wording* diverges by design here (huck `unterminated quote` vs bash `unexpected EOF while looking for matching`), so the diff harness compares stdout + rc only.
- **This box OOMs on `cargo test --workspace`.** Always test per-crate: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` for lexer/parser unit tests; `cargo test -p huck --test <name> --jobs 1 -- --test-threads 1` for a single integration binary. Build the binary with `cargo build -p huck` (never `cargo build` alone for the binary — huck-cli is a lib).
- **Guard bash-diff harness runs** with `ulimit -v 1500000` + `timeout`.
- **Commit trailer** on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Do not push to main or self-merge.** v277 lands via a PR whose body contains `Closes #86`, for the user to review and merge.

---

### Task 1: Gate the post-newline heredoc-body peek; restore incremental execution + correct line

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — add `has_pending_heredoc_body()` near the other heredoc-body helpers (after `take_heredoc_bodies`, ~line 1372).
- Modify: `crates/huck-syntax/src/parser.rs:2170-2176` — gate `collect_heredoc_bodies_after_newline`.
- Test: `tests/script_line_numbers_integration.rs` — un-`#[ignore]` the existing regression test and add three cases.

**Interfaces:**
- Consumes: `Lexer::pending_heredocs` and `Lexer::atom_pending_heredocs` (private `VecDeque` fields, already used by `maybe_prune_history`); `Lexer::peek_kind() -> Result<Option<TokenKind>, ParseError>`; `parse_heredoc_body`, `Lexer::push_heredoc_body`.
- Produces: `pub(crate) fn Lexer::has_pending_heredoc_body(&self) -> bool`.

- [ ] **Step 1: Un-ignore the existing regression test**

In `tests/script_line_numbers_integration.rs`, delete the `#[ignore]` line on the test (added during CI triage this session):

```rust
#[test]
#[ignore = "known regression, tracked in #86: huck aborts earlier units and reports the EOF line, not the token's line"]
fn lex_error_as_first_token_of_second_unit_reports_its_line() {
```

becomes:

```rust
#[test]
fn lex_error_as_first_token_of_second_unit_reports_its_line() {
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo build -p huck && cargo test -p huck --test script_line_numbers_integration --jobs 1 -- --test-threads 1 lex_error_as_first_token_of_second_unit_reports_its_line`

Expected: FAIL — the first assertion `assert!(so.contains("ok"))` panics (huck currently prints nothing and reports `line 3:` instead of `line 2:`).

- [ ] **Step 3: Add the lexer predicate**

In `crates/huck-syntax/src/lexer.rs`, immediately after the `take_heredoc_bodies` method (the block ending at ~line 1372), add:

```rust
    /// True while one or more heredoc bodies are still pending collection (the
    /// atom-path queue or the legacy queue is non-empty). Lets a caller that has
    /// just consumed a unit-terminating newline decide whether it must peek for a
    /// `HeredocBodyBegin` — avoiding an over-scan into the *next* unit's first
    /// token when no heredoc is pending (issue #86).
    pub(crate) fn has_pending_heredoc_body(&self) -> bool {
        !self.pending_heredocs.is_empty() || !self.atom_pending_heredocs.is_empty()
    }
```

- [ ] **Step 4: Gate the heredoc-collect loop**

In `crates/huck-syntax/src/parser.rs`, replace the body of `collect_heredoc_bodies_after_newline` (lines 2170-2176):

```rust
fn collect_heredoc_bodies_after_newline(iter: &mut Lexer) -> Result<(), ParseError> {
    while matches!(iter.peek_kind()?, Some(TokenKind::HeredocBodyBegin { .. })) {
        let body = parse_heredoc_body(iter)?;
        iter.push_heredoc_body(body);
    }
    Ok(())
}
```

with the gated form:

```rust
fn collect_heredoc_bodies_after_newline(iter: &mut Lexer) -> Result<(), ParseError> {
    // #86: only peek for a heredoc body when one is actually pending. `&&`
    // short-circuits, so with no pending heredoc `peek_kind()` is never called
    // and the next unit's first token is not scanned — a unit-terminating
    // newline ends the unit cleanly instead of over-scanning into (and failing
    // on) a following unit that begins with a lex error.
    while iter.has_pending_heredoc_body()
        && matches!(iter.peek_kind()?, Some(TokenKind::HeredocBodyBegin { .. }))
    {
        let body = parse_heredoc_body(iter)?;
        iter.push_heredoc_body(body);
    }
    Ok(())
}
```

- [ ] **Step 5: Run the un-ignored test to verify it passes**

Run: `cargo build -p huck && cargo test -p huck --test script_line_numbers_integration --jobs 1 -- --test-threads 1`

Expected: PASS — all tests in the binary pass, including `lex_error_as_first_token_of_second_unit_reports_its_line` (huck now prints `ok`, reports `line 2:`, rc 2).

- [ ] **Step 6: Add the three new integration cases**

Append these tests at the end of `tests/script_line_numbers_integration.rs` (the file is a flat list of `#[test]` fns with no wrapping module; they use the existing `run_script` helper). Line numbers verified against bash 5.2.21.

```rust
#[test]
fn earlier_units_run_before_a_later_parse_error() {
    // Two clean units, then a unit with an unterminated quote. Both clean units
    // must execute (side effects survive) and the error is reported at the
    // offending token's line (3), matching bash.
    let (so, se, c) = run_script("echo a\necho b\n'unterminated\n");
    assert!(
        so.contains("a") && so.contains("b"),
        "both units should run: {so:?}"
    );
    assert!(se.contains("syntax error"), "lex error must be reported: {se:?}");
    assert!(se.contains("line 3:"), "expected 'line 3:', got: {se:?}");
    assert_eq!(c, 2, "exit code should be 2, got {c}");
}

#[test]
fn heredoc_unit_then_good_unit_still_runs() {
    // A heredoc unit followed by a normal unit: the post-newline heredoc-body
    // collection must still fire (queue non-empty) so the body attaches and the
    // following unit runs — guards against the #86 gate over-suppressing it.
    let (so, _se, c) = run_script("cat <<EOF\nhello\nEOF\necho after\n");
    assert!(so.contains("hello"), "heredoc body should print: {so:?}");
    assert!(so.contains("after"), "following unit should run: {so:?}");
    assert_eq!(c, 0, "exit code should be 0, got {c}");
}

#[test]
fn heredoc_unit_immediately_before_a_parse_error() {
    // The heredoc unit's body is collected and the queue drains, so the loop
    // stops before peeking the next unit's bad token. The heredoc unit runs and
    // the error is reported at the bad token's line (4), not the EOF line.
    let (so, se, c) = run_script("cat <<EOF\nhi\nEOF\n'unterminated\n");
    assert!(so.contains("hi"), "heredoc unit should run: {so:?}");
    assert!(se.contains("syntax error"), "lex error must be reported: {se:?}");
    assert!(se.contains("line 4:"), "expected 'line 4:', got: {se:?}");
    assert_eq!(c, 2, "exit code should be 2, got {c}");
}
```

- [ ] **Step 7: Run the full integration binary + related suites**

Run each and confirm no failures:

```
cargo test -p huck --test script_line_numbers_integration --jobs 1 -- --test-threads 1
cargo test -p huck --test heredoc_integration --jobs 1 -- --test-threads 1
cargo test -p huck --test linear_source_reader_integration --jobs 1 -- --test-threads 1
cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1
```

Expected: all `test result: ok`. The two heredoc cases prove no regression to heredoc-body collection; the syntax lib suite proves the parser change is clean.

- [ ] **Step 8: Format and commit**

```bash
cargo fmt --all
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs tests/script_line_numbers_integration.rs
git commit -m "$(cat <<'EOF'
v277: don't over-scan past a unit-terminating newline (#86)

parse_and_or_opts unit mode ended a unit at a top-level newline, then
collect_heredoc_bodies_after_newline peeked the next unit's first token to look
for a heredoc body. When that token was a lex error the peek failed and the
already-parsed unit was discarded and reported at the EOF line. Gate the peek on
Lexer::has_pending_heredoc_body() so it only fires when a heredoc body is
pending; a no-heredoc unit now ends cleanly and the reader's pending_lex_err
path reports the error at the offending token's line and still runs the unit.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Bash-differential harness for stdout + rc parity

**Files:**
- Create: `tests/scripts/script_parse_error_diff_check.sh`

**Interfaces:**
- Consumes: the `huck` binary at `target/debug/huck` (built via `cargo build -p huck`); the behavior delivered by Task 1.
- Produces: nothing consumed by later tasks (leaf verification artifact).

- [ ] **Step 1: Create the harness**

Create `tests/scripts/script_parse_error_diff_check.sh` with exactly:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #86: a parse error at the start of a
# unit must still execute earlier already-parsed units, matching bash on stdout
# AND exit code. stderr wording diverges by design (huck "unterminated quote"
# vs bash "unexpected EOF while looking for matching"), so only stdout+rc are
# compared (stderr is sent to /dev/null).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

check() { local l="$1" f="$2" b h scr
  scr="$(mktemp)"; printf '%b' "$f" > "$scr"
  b=$(bash --norc "$scr" 2>/dev/null; echo "EXIT:$?")
  h=$("$HUCK_BIN" "$scr" 2>/dev/null; echo "EXIT:$?")
  rm -f "$scr"
  if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
  else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

check "one unit then bad"    'echo ok\n'"'"'unterminated\n'
check "two units then bad"   'echo a\necho b\n'"'"'unterminated\n'
check "heredoc then good"    'cat <<EOF\nhello\nEOF\necho after\n'
check "heredoc then bad"     'cat <<EOF\nhi\nEOF\n'"'"'unterminated\n'
check "clean multi-unit"     'echo x\necho y\n'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
```

- [ ] **Step 2: Make it executable**

Run: `chmod +x tests/scripts/script_parse_error_diff_check.sh`

- [ ] **Step 3: Run the harness (guarded) and verify all pass**

Run:

```
cargo build -p huck
( ulimit -v 1500000; timeout 60 bash tests/scripts/script_parse_error_diff_check.sh )
```

Expected final line: `5 passed, 0 failed` (exit 0). Each `check` is byte-identical between bash and huck on stdout + rc.

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/script_parse_error_diff_check.sh
git commit -m "$(cat <<'EOF'
v277: bash-diff harness for parse-error-at-unit-boundary stdout+rc parity (#86)

Adds tests/scripts/script_parse_error_diff_check.sh — asserts byte-identical
stdout + exit code vs bash for scripts whose later unit has a parse error
(earlier units must still run). stderr wording diverges by design and is
excluded from the compare.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Notes for the whole-branch review

- The change also touches the second call site of `collect_heredoc_bodies_after_newline` (`parser.rs:4138`, inside `for`/`select` header parsing). That site is within a unit; the gate is behavior-neutral there because the surrounding loop re-`peek_kind()`s on its next iteration. Confirm no compound-command / heredoc-in-loop-header regression via `heredoc_integration` + `for_integration`.
- CI (`.github/workflows/ci.yml`) runs the full `--workspace` suite on push — it is the authoritative full-suite gate (the dev box cannot run `--workspace`). Expect it green after the branch is pushed.
