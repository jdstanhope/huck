# v249 — Here-strings (`<<<`) on the atom-command path (dormant, differential) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the dormant atom-command parser handle here-strings (`<<< word`) — both as a redirect on a command (`cmd <<< word`) and at command position (leading `<<< word`) — producing ASTs byte-identical to the `command.rs` oracle, by removing two deferral guards. No lexer/oracle changes, no live flip.

**Architecture:** Parser-only, two-guard-removal port. The `<<<` operator atom, `is_redirect_op`/`next_is_redirect` recognition, atom target-assembly, and `build_redirections` (→ `RedirOp::HereString`) already exist and are already wired on the atom path. Task 1 removes the redirect-path deferral (`cmd <<< word`); Task 2 relaxes the command-position guard so a leading `<<<` falls through to `parse_simple`, then adds the full differential corpus and confirms heredocs stay deferred.

**Tech Stack:** Rust, `crates/huck-syntax/src/parser.rs` only. No new dependencies.

## Global Constraints

- Test ONLY with `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` (narrow with a test-name filter). This box (1 core / ~1.9 GiB) is OOM-KILLED by `cargo test --workspace` or any parallel/multi-threaded run — NEVER run those.
- Byte-identical: every in-scope here-string input parses to the SAME AST / SAME error on the atom path as the oracle (`diff_cmd` / error parity). A well-formed in-scope divergence is a v249 BUG to fix, not to pin.
- PRODUCTION UNTOUCHED: `command_atoms` defaults `false`; NO `command.rs` and NO `lexer.rs` changes (both already expose/emit what is needed); `scan_step_command`/`process_line` unchanged. No live flip. v249 edits ONLY `crates/huck-syntax/src/parser.rs`.
- Heredocs stay DEFERRED: both guards must continue to defer `TokenKind::Heredoc` (`<<EOF`/`<<-`). Only the `<<<` (`Operator::HereString`) deferral is lifted.
- `rust-analyzer`/IDE diagnostics can be phantom — trust `cargo`.
- 0 warnings (`cargo build -p huck-syntax 2>&1 | grep -c warning` → `0`).
- Every commit ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Branch: `v249-here-strings`. Do NOT commit to `main`.

## Key existing anchors (read before starting)

- **Atom-path `parse_one_redirect`** — `parser.rs:1250`. The HereString deferral to REMOVE is at `parser.rs:1269–1272`:
  ```rust
  // HereString (`<<<`) — deferred.
  if matches!(op, Operator::HereString) {
      return Err(ParseError::UnsupportedCommand);
  }
  ```
  Immediately below it is the process-substitution guard (`RedirIn`/`RedirOut` + glued `LParen`) — leave that unchanged. Below that is the generic target `match` (skips a `Blank`, assembles via `parse_word_command`, returns `RedirectTargetIsOperator`/`MissingRedirectTarget` for a bad target) and the `crate::command::build_redirections(op, target, fd_prefix)` call.
- **Command-position guard** — `parse_command`, `parser.rs:1552–1556`:
  ```rust
  // Heredoc / `<<<` at command position.
  if matches!(
      iter.peek_kind()?,
      Some(TokenKind::Heredoc { .. }) | Some(TokenKind::Op(Operator::HereString))
  ) {
      return Err(ParseError::UnsupportedCommand);
  }
  ```
- **Reused oracle helpers (already `pub(crate)`, already called by the atom path):** `crate::command::is_redirect_op` (`command.rs:1902`), `crate::command::next_is_redirect` (`command.rs:2044`, returns `is_redirect_op(op)` for `Op(op)` — already `true` for `HereString`), `crate::command::build_redirections` (`command.rs:1935`, maps `Operator::HereString => Redirection { fd: plain_fd(), op: RedirOp::HereString(target) }`).
- **`parse_simple`** — `parser.rs:1336`; its loop calls `crate::command::next_is_redirect` then the atom `parse_one_redirect`, and handles a redirect with NO words (finalizes an empty-words command carrying the redirect) — the same shape the oracle's `parse_simple_stage` produces for a leading redirect.
- **Differential harness** — `new_seq` (atoms, `parser.rs`), `old_seq` (oracle), `diff_cmd(s)` (asserts `new_seq(s) == old_seq(s)`). Error-parity precedent that SPLITS lexer-level rejects (where `old_seq` panics via `.expect("lex")`) from parser-level ones: the `atoms_error_parity` test — read it before writing the error test.
- **AST:** `Redirect::HereString(Word)` slotted to stdin; `RedirOp::HereString(Word)` (`command.rs:322/380`).

## File Structure

- `crates/huck-syntax/src/parser.rs` — the two guard removals (T1 in `parse_one_redirect`, T2 in `parse_command`) and the differential tests. No other file changes.

---

### Task 1: `cmd <<< word` — lift the redirect-path deferral

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` (`parse_one_redirect`: remove the HereString deferral)
- Test: `crates/huck-syntax/src/parser.rs` `mod tests`

**Interfaces:**
- Consumes: the existing atom `parse_one_redirect` target-assembly + `crate::command::build_redirections`.
- Produces: `<<<` as a working trailing/interleaved redirect on a command (`RedirOp::HereString(target)`), byte-identical to the oracle.

- [ ] **Step 1: Write the failing test** (in `parser.rs` `mod tests`, near the v248 funcdef atom tests):

```rust
    // ── v249: here-strings (`<<<`) on the atom path ──────────────────────────
    #[test]
    fn atoms_here_string_redirect() {
        diff_cmd("cat <<< hello");
        diff_cmd("wc -l <<<foo");                 // glued, no space
        diff_cmd("cat <<< \"$x\"");                // quoted expansion target
        diff_cmd("cat <<< 'lit'");
        diff_cmd("cat <<< $'a\\tb'");              // ANSI-C target
        diff_cmd("cat <<< $var");
        diff_cmd("cat <<< a b");                    // target is `a`; `b` is an arg
        diff_cmd("cmd <<< x > out");                // here-string + file redirect, source order
        diff_cmd("cmd 2>&1 <<< x");                 // fd-dup + here-string
        diff_cmd("cmd <<< a <<< b");                // two here-strings, ordered list
    }
```

- [ ] **Step 2: Run test to verify it fails.** Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_here_string_redirect -- --test-threads 1`. Expected: FAIL (`<<<` currently returns `UnsupportedCommand` from `parse_one_redirect`).

- [ ] **Step 3: Remove the HereString deferral.** In `parse_one_redirect` (`parser.rs:1269–1272`), DELETE these four lines:

```rust
            // HereString (`<<<`) — deferred.
            if matches!(op, Operator::HereString) {
                return Err(ParseError::UnsupportedCommand);
            }
```

Leave everything else in `parse_one_redirect` unchanged (the process-sub guard below it, the target `match`, and the `build_redirections` call). With the deferral gone, `Operator::HereString` uses the same generic target-assembly + `build_redirections` path as every other redirect operator; `build_redirections` already maps it to `RedirOp::HereString(target)`.

- [ ] **Step 4: Run test to verify it passes.** Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_here_string_redirect -- --test-threads 1`. Expected: PASS. Debug any AST mismatch against `old_seq` (the redirect list — fd, `RedirOp::HereString(Word)`, source order — and the assembled target Word / `quoted` flags must match).

- [ ] **Step 5: Confirm no redirect regression.** The change is inside the shared redirect path, so re-run the existing redirect/structure atom tests: `cargo test -p huck-syntax --jobs 1 --lib atoms_structure -- --test-threads 1`. Expected: PASS (file redirects / fd-dups unaffected).

- [ ] **Step 6: Warnings + commit.** `cargo build -p huck-syntax 2>&1 | grep -c warning` → `0`. Commit:

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v249 T1: cmd <<< word here-string redirect on the atom path

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: leading `<<<` + full corpus, error parity, deferred-heredoc gate

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` (`parse_command`: relax the command-position guard to `<<<`-not-deferred)
- Test: `crates/huck-syntax/src/parser.rs` `mod tests`

**Interfaces:**
- Consumes: Task 1 (`<<<` works as a redirect); `parse_simple`'s leading-redirect handling.
- Produces: leading `<<< word` parses as an empty-words command with a stdin `HereString` redirect; the comprehensive v249 differential gate.

- [ ] **Step 1: Write the failing test** (leading here-strings):

```rust
    #[test]
    fn atoms_here_string_leading() {
        diff_cmd("<<< word");
        diff_cmd("<<<foo");                         // glued
        diff_cmd("<<< \"$x\"");
        diff_cmd("<<< word > out");                 // leading here-string + file redirect
    }
```

- [ ] **Step 2: Run test to verify it fails.** Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_here_string_leading -- --test-threads 1`. Expected: FAIL (a leading `<<<` hits the `parse_command` command-position guard → `UnsupportedCommand`).

- [ ] **Step 3: Relax the command-position guard.** In `parse_command` (`parser.rs:1552–1556`), replace the combined Heredoc/HereString guard with a Heredoc-only guard:

```rust
    // Heredoc at command position — deferred (heredoc BODIES are future work).
    // `<<<` (here-string) is NOT deferred: it flows to parse_simple as a leading
    // redirect (an empty-words command reading stdin from the here-string),
    // matching the oracle (which falls through to parse_pipeline → parse_simple_stage).
    if matches!(iter.peek_kind()?, Some(TokenKind::Heredoc { .. })) {
        return Err(ParseError::UnsupportedCommand);
    }
```

A leading `<<<` now passes this guard, is not a keyword (`peek_leading_keyword` → `None`) and not a funcdef (peek is an `Op`, not `Lit`/`Word`), so it reaches `parse_simple`, whose loop's `next_is_redirect` fires on the `<<<` and builds the redirect with no words.

- [ ] **Step 4: Run test to verify it passes.** Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_here_string_leading -- --test-threads 1`. Expected: PASS. Debug any mismatch against `old_seq` (empty-words command + stdin `HereString` redirect).

- [ ] **Step 5: Add the fd-prefix pin (determine-by-observation).** Add:

```rust
    #[test]
    fn atoms_here_string_fd_prefix() {
        diff_cmd("3<<< word");                      // fd-prefixed here-string
    }
```

Run it: `cargo test -p huck-syntax --jobs 1 --lib atoms_here_string_fd_prefix -- --test-threads 1`. If it PASSES, keep it. If `old_seq` PANICS (the oracle rejects `3<<<` at the LEXER level via `.expect("lex")`), the input can't go through `diff_cmd`; replace this test body with an atom-path-only assertion and a comment:

```rust
    #[test]
    fn atoms_here_string_fd_prefix() {
        // `3<<<` is rejected at the lexer level by the oracle's batch tokenizer,
        // so old_seq can't produce a Result to compare — assert the atom path
        // rejects it too (parity of rejection). (Determined by observation.)
        assert!(new_seq("3<<< word").is_err());
    }
```

Pick whichever branch matches ACTUAL observed behavior; do not guess. Note which branch you took in your report.

- [ ] **Step 6: Add the pipeline-stage pin (determine-by-observation).** Add to `atoms_here_string_leading` (or a small test):

```rust
        diff_cmd("<<< x | cat");                    // here-string stage in a pipeline
```

Run the leading test. If it PASSES, keep the line. If the oracle does NOT accept a leading `<<<` as a pipeline stage (AST mismatch or `old_seq` error that diverges from the atom path), REMOVE the line and note it in your report (out of scope — a leading here-string pipeline stage is an edge the oracle treats differently). Do not force parity by changing parser logic — v249 is guard-removal only; a genuine divergence here is reported, not papered over.

- [ ] **Step 7: Add the error-parity test.** Compare `new_seq` to `old_seq` normalized to `Ok(())`/error-debug (mirror the existing `atoms_error_parity` split of lexer-level vs parser-level rejects — read that test first):

```rust
    #[test]
    fn atoms_here_string_errors() {
        // Parser-level rejects (the oracle lexes successfully): same error both paths.
        for s in ["cat <<<", "<<<", "cat <<< |", "cat <<< <", "cat <<< ;"] {
            assert_eq!(
                new_seq(s).map(|_| ()).map_err(|e| format!("{e:?}")),
                old_seq(s).map(|_| ()).map_err(|e| format!("{e:?}")),
                "here-string error parity for {s:?}",
            );
        }
    }
```

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_here_string_errors -- --test-threads 1`. If ANY input makes `old_seq` panic at the lexer level (so the `assert_eq!` can't run), move that specific input to an atom-path-only `assert!(new_seq(s).is_err())` bucket with a comment, exactly as `atoms_error_parity` does — determine by observation, note it in your report.

- [ ] **Step 8: Add the heredoc-still-deferred pin.** Prove the guard change is `<<<`-only:

```rust
    #[test]
    fn atoms_here_string_heredoc_still_deferred() {
        // Heredocs remain deferred on the atom path (v249 lifts ONLY `<<<`).
        assert!(matches!(new_seq("cat <<EOF\nx\nEOF"), Err(ParseError::UnsupportedCommand)),
            "trailing heredoc must stay deferred, got {:?}", new_seq("cat <<EOF\nx\nEOF"));
        assert!(matches!(new_seq("<<EOF\nx\nEOF"), Err(ParseError::UnsupportedCommand)),
            "leading heredoc must stay deferred, got {:?}", new_seq("<<EOF\nx\nEOF"));
    }
```

- [ ] **Step 9: Run the new + full gate.** Run each new test, then the full atom suite and full lib suite:
  - `cargo test -p huck-syntax --jobs 1 --lib atoms_here_string -- --test-threads 1` → all PASS.
  - `cargo test -p huck-syntax --jobs 1 --lib atoms_ -- --test-threads 1` → `0 failed`.
  - `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → `0 failed`.
  - `cargo test -p huck-syntax --jobs 1 --doc -- --test-threads 1` → `0 failed`.
  Investigate + FIX any `diff_cmd` regression (a well-formed in-scope divergence is a v249 bug). Watch for a hang (growing memory) — none is expected (guard removal only), but if one appears it is a non-progress loop in the leading-redirect path.

- [ ] **Step 10: Warnings + commit.** `cargo build -p huck-syntax 2>&1 | grep -c warning` → `0`. Commit:

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v249 T2: leading <<< + here-string error/deferred parity + full gate

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review checklist (run before the whole-branch review)

- Both here-string positions have `diff_cmd` coverage: `cmd <<< word` (Task 1: quoted/glued/expansion/ANSI-C targets, interleaving with file + fd-dup redirects, repeated) and leading `<<< word` (Task 2). Error parity + the fd-prefix and pipeline-stage determine-by-observation pins present. Heredoc-still-deferred pin present.
- Only `<<<` was lifted: `git diff main -- crates/huck-syntax/src/parser.rs` shows the two guards changed to defer only `TokenKind::Heredoc`; the heredoc-still-deferred test passes.
- Production untouched: `git diff main -- crates/huck-syntax/src/command.rs` and `... src/lexer.rs` are EMPTY; `command_atoms` still defaults `false`; `scan_step_command`/`process_line` unchanged.
- No new duplication: the redirect handling reuses `crate::command::build_redirections`/`is_redirect_op`/`next_is_redirect` (no reimplementation).
- Full `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` is `0 failed`; doctests `0 failed`; 0 warnings.
- All commits carry the trailer; branch is `v249-here-strings`.
