# v262 F2 — leading `!` in compound body/condition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On the dormant atom-command path, count a leading `!` as pipeline negation even when an inter-token `Blank` precedes it (after a compound opener / keyword / connector), so `{ ! a; }`, `while ! a; do …`, etc. match the `command.rs` oracle instead of swallowing the `!` into the program word.

**Architecture:** One `while`-skip added at the top of `parse_pipeline` (parser.rs) before the bang-count loop, plus a differential corpus. `command.rs` and `lexer.rs` UNTOUCHED. `command_atoms` stays `false` — dormant/differential, verified via `new_seq` (atom) vs `old_seq` (oracle) full-AST equality.

**Tech Stack:** Rust, single crate `huck-syntax`.

## Global Constraints

- `command.rs` diff-vs-`main` = EMPTY. `lexer.rs` UNTOUCHED. The only source change is in `parse_pipeline` (parser.rs); tests are added to `parser.rs mod tests`.
- `command_atoms` stays `false` at both constructor sites.
- Box is 1 core / 1.9 GB. The ONLY test command is `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` (append a test name before `--` to run one). NEVER `--workspace`, NEVER multi-threaded — it OOM-kills the session.
- `cargo build -p huck-syntax` → 0 warnings.
- `diff_cmd(s)` asserts `new_seq(s).unwrap() == old_seq(s).unwrap()` (full-AST). `old_seq` uses `.expect("lex")`, so a lex-error input PANICS — every corpus input here is parse-clean, so `diff_cmd` is safe.
- Commit trailer VERBATIM: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- rust-analyzer PHANTOM diagnostics — trust `cargo`, not the editor.

---

### Task 1: Leading-`Blank` skip in `parse_pipeline` + differential corpus

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` — `parse_pipeline` (the bang-count entry) + a new test in `mod tests`.

**Interfaces:**
- Consumes: `iter.peek_kind()? -> Result<Option<TokenKind>, ParseError>`, `iter.next_kind()`, `TokenKind::Blank`, `is_bang_word`, `finish_pipeline` — all already in scope in `parse_pipeline`. `diff_cmd(s)` in `mod tests`.
- Produces: no new public interface.

- [ ] **Step 1: Write the failing test**

Add to `crates/huck-syntax/src/parser.rs mod tests`:
```rust
    #[test]
    fn atoms_bang_in_compound_body_and_condition() {
        // v262 F2: a leading `!` preceded by an inter-token Blank (after a
        // compound opener / keyword / connector) must count as pipeline negation,
        // not be swallowed into the program word. Conditions AND bodies of every
        // compound routed through parse_pipeline were divergent (probed EQ=false).
        diff_cmd("{ ! a; }");
        diff_cmd("{ ! ! a; }");
        diff_cmd("if x; then ! ! a; fi");
        diff_cmd("if ! a; then :; fi");
        diff_cmd("while ! a; do :; done");
        diff_cmd("until ! a; do :; done");
        diff_cmd("while x; do ! a; done");
        diff_cmd("for i in 1; do ! a; done");
        diff_cmd("{ ! a && b; }");
        diff_cmd("{ ! a || b; }");
        diff_cmd("{ ! a | b; }");
        // Regression guards — already correct, must STAY byte-identical.
        diff_cmd("! a");                       // top-level
        diff_cmd("( ! a )");                   // subshell (bespoke path)
        diff_cmd("case x in a) ! b;; esac");   // bespoke case-item path
        diff_cmd("{ a; }");                    // no bang
        diff_cmd("!a");                        // glued — not a bang word
    }
```

- [ ] **Step 2: Run the test to verify it FAILS**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_bang_in_compound_body_and_condition -- --test-threads 1`
Expected: FAIL — the first divergent case (`{ ! a; }`) panics in `diff_cmd`'s `assert_eq!` (atom `Simple(program:"!")` vs oracle `Pipeline{negate:true,[Simple(a)]}`).

- [ ] **Step 3: Apply the fix in `parse_pipeline`**

In `crates/huck-syntax/src/parser.rs`, at the very start of `parse_pipeline` (immediately after the function's opening `{` and the existing leading comment, BEFORE `let mut bangs = 0usize;`), insert:
```rust
    // v262 F2: skip any leading inter-token Blank the atom scanner emits after a
    // compound opener / keyword / connector (`{ ! a; }`, `while ! a`, `then ! a`),
    // so the bang-count loop below sees the `!` rather than the Blank in front of
    // it. (The loop already skips blanks BETWEEN successive bangs; this covers the
    // one before the FIRST bang.) A command never begins with a meaningful Blank,
    // so this is a no-op for the paths that already arrive blank-free.
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
        iter.next_kind()?;
    }
```
Do NOT change anything else in `parse_pipeline`, `finish_pipeline`, or any caller.

- [ ] **Step 4: Run the test to verify it PASSES**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_bang_in_compound_body_and_condition -- --test-threads 1`
Expected: PASS (all 16 `diff_cmd` cases — 11 fixed + 5 regression guards — byte-identical).

- [ ] **Step 5: Run the full suite + gates (non-regression net)**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → all green.
Run: `cargo build -p huck-syntax` → 0 warnings.
Run: `git diff --stat main -- crates/huck-syntax/src/command.rs` → EMPTY.
Run: `git diff --stat main -- crates/huck-syntax/src/lexer.rs` → EMPTY.

If any pre-existing test now fails, STOP and report it (the fix should be a strict improvement — it only changes cases that were wrong). Do NOT weaken any test.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v262: skip leading Blank before the bang-count loop in parse_pipeline (F2)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

- **Spec coverage:** The fix (leading-Blank skip in `parse_pipeline`) → Step 3. The fixed corpus (11 cases) + regression guards (5 cases) → Step 1. `command.rs`/`lexer.rs` EMPTY-diff → Step 5. ✓
- **Placeholder scan:** none. The fix code and all 16 test inputs are verbatim.
- **Type consistency:** `iter.peek_kind()?`, `iter.next_kind()?`, `TokenKind::Blank`, `matches!` — identical to the existing after-bang skip inside the same function; no new names introduced.
