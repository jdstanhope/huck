# v255 — Standalone arith command `(( … ))` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port the standalone arithmetic-evaluation command `(( expr ))` onto the dormant atom-command parser, byte-identical to the `command.rs` oracle.

**Architecture:** The atom scanner already emits command-position glued `((` as two `Op(LParen)` atoms; `parse_command` (parser.rs:2024) already detects them and currently defers. Replace that deferral with `parse_arith_command`, which speculatively delimits the body via v246's `Mode::Arith` + `parse_arith_body`: on the matching `))` (`ArithClose`) it builds `Command::Arith(body)` and wraps trailing redirects; on `ArithBail` (a depth-0 `)` not followed by `)`) it rewinds to before `((` and reparses as a nested subshell `( (…) )` (bash's arith-command backoff). Spaced `( (` keeps a `Blank`, so it never reaches this path — it flows to the existing subshell parser. Parse-time delimiting only; runtime arith evaluation is unchanged.

**Tech Stack:** Rust; `crates/huck-syntax` (`parser.rs` only; `command.rs` and `lexer.rs` untouched).

## Global Constraints

- **Dormant:** both `command_atoms` sites (`lexer.rs:811`, `lexer.rs:4167`) stay `false`. No production behavior change.
- **Byte-identical / differential:** every in-scope input must parse to the SAME AST/error on the atom path (`new_seq`) as the oracle (`old_seq`). Any divergence is fixed on the ATOM path only — never touch the oracle.
- **`command.rs` untouched:** `git diff main -- crates/huck-syntax/src/command.rs` must be EMPTY.
- **`lexer.rs` untouched:** v255 needs no lexer change (the opener is handled by pushing `Mode::Arith { body_started: true }`). `git diff main -- crates/huck-syntax/src/lexer.rs` must be EMPTY.
- **THE RULE:** the lexer emits small atoms + tracks only a running incremental `paren_depth`; the parser owns delimiter-matching/recursion. v255 adds no forward scan.
- **Scope:** standalone `(( expr ))` only. Explicitly OUT: C-style `for (( … ))` (→ v256) and any runtime arith-eval change.
- **Test runner (box is 1 core / 1.9 GiB — `--workspace` or multi-threaded OOM-kills the session):** ONLY `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` (filter while iterating). Warnings gate: `cargo build -p huck-syntax 2>&1 | grep -c warning` → `0`.
- **Progress/OOM safety:** the bail→rewind path must not loop — after `rewind`, `parse_subshell` consumes the first `(` (forward progress) and does not peek2-for-`((`, so it cannot re-enter `parse_arith_command` at the same position.
- **rust-analyzer shows PHANTOM diagnostics** (dead_code / type errors that cargo does not report). Trust `cargo`, not the IDE.
- **Commit trailer** on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

## Reference — existing types/functions this plan reuses (do NOT redefine)

- `Command::Arith(crate::lexer::Word)` — tuple variant (`command.rs:624`).
- `Mode::Arith { paren_depth: u32, in_dquote: bool, body_started: bool }` (`lexer.rs:642`).
- `enum ArithBodyOutcome { Closed(Word), Bail }` (`parser.rs:1198`).
- `fn parse_arith_body(iter: &mut Lexer, _in_dquote: bool) -> Result<ArithBodyOutcome, ParseError>` (`parser.rs:1226`) — pulls body atoms until `ArithClose` (→ `Closed(Word)`) or `ArithBail` (→ `Bail`, consumed). Embedded expansions are `quoted: true` (the `_in_dquote` arg is ignored — pass `false`).
- `pub(crate) fn parse_arith_expansion(iter, quoted)` (`parser.rs:1269`) — the `$((` sibling; `parse_arith_command` mirrors its mark/push/pop lifecycle.
- `fn parse_subshell(iter: &mut Lexer) -> Result<Command, ParseError>` (`parser.rs:2922`).
- `pub(crate) fn maybe_wrap_redirects(cmd: Command, iter: &mut Lexer) -> Result<Command, ParseError>` (`parser.rs:2542`).
- Lexer methods: `iter.mark() -> Mark`, `iter.rewind(&mark)`, `iter.push_mode(Mode)`, `iter.pop_mode()`, `iter.next_kind()? -> Option<TokenKind>`, `iter.peek_kind()`, `iter.peek2_kind()`.
- Test harness (`parser.rs` `mod tests`): `new_seq(s)` (atom path), `old_seq(s)` (oracle; uses `.expect("lex")` — a lex-error input panics, so only `diff_err` inputs that the oracle returns as a *parse* `Err` are safe), `diff_cmd(s)` (asserts `new_seq(s).unwrap() == old_seq(s).unwrap()`), `diff_err(s)` (asserts `new_seq(s) == old_seq(s)`), `diff_unsupported(s)` (asserts `new_seq(s)` is `Err(UnsupportedCommand)`).

**Oracle ground truth (probed 2026-07-03 — the atom path must match these):**
`((1+2))`→`Arith`, `(())`→`Arith(Word([]))`, `(( (1+2) * 3 ))`→`Arith` (inner grouping is depth-tracked, not a bail), `(( $x + 1 ))`→`Arith` with embedded expansion; `((cmd); c2)`/`((echo hi) )`/`(( 3*4 ) )`/`((a) && (b))`→ nested `Subshell` (bail); `( ( 3 * 4 ) )`→`Subshell` (spaced); `(( 1+2 )) >out`→`Redirected{ inner: Arith }`; `((1+2)`→`Err(UnterminatedSubshell)` (oracle falls back to `( (1+2)`; NOT a lex panic — `diff_err` is safe).

---

### Task 1: `parse_arith_command` + dispatch replacement

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` — add `parse_arith_command` after `parse_arith_expansion` (ends at `parser.rs:1297`); edit the dispatch at `parser.rs:2016-2028`.
- Test: `crates/huck-syntax/src/parser.rs` `mod tests` — add `atoms_arith_command`.

**Interfaces:**
- Consumes: `parse_arith_body`, `Mode::Arith`, `Command::Arith`, `maybe_wrap_redirects`, `parse_subshell`, `iter.mark/rewind/push_mode/pop_mode/next_kind` (all listed in Reference above).
- Produces: `fn parse_arith_command(iter: &mut Lexer) -> Result<Command, ParseError>` (used only by `parse_command`'s dispatch; Tasks 2–3 add tests against it, no new callers).

- [ ] **Step 1: Write the failing test**

Add to `parser.rs` `mod tests` (near the other `atoms_*` compound tests, e.g. after `cmd_compound_deferred_still` around `parser.rs:4880`):

```rust
    // v255: standalone arith command `(( … ))`
    #[test]
    fn atoms_arith_command() {
        // Glued `((` that closes on the matching `))` → Command::Arith (byte-identical).
        diff_cmd("(( 1 + 2 ))");
        diff_cmd("((1+2))");
        diff_cmd("(( x = 5 ))");
        diff_cmd("(( x++ ))");
        diff_cmd("(( $x + 1 ))");        // embedded expansion — wires parse_arith_body
        // Primary bail → nested subshell backoff (depth-0 `)` not followed by `)`).
        diff_cmd("((cmd); c2)");
        // Spaced `( (` is NEVER arith — regression guard for the existing subshell path.
        diff_cmd("( ( 3 * 4 ) )");
        // Unterminated glued `((` (no matching `))`): both paths bail → subshell → same
        // parse error (oracle falls back to `( (1+2)` → UnterminatedSubshell; no lex panic).
        diff_err("((1+2)");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_arith_command -- --test-threads 1`
Expected: FAIL — `diff_cmd("(( 1 + 2 ))")` panics on `new_seq(...).unwrap()` (currently `Err(UnsupportedCommand)`).

- [ ] **Step 3: Add `parse_arith_command`**

Insert immediately after `parse_arith_expansion` (after `parser.rs:1297`, before `fn skip_newlines`):

```rust
/// v255: assemble a standalone `(( expr ))` arithmetic command at command
/// position. The atom scanner emits glued `((` as two `Op(LParen)` atoms and the
/// caller (`parse_command`) has already peeked both. Speculatively delimit the
/// body as arith (reusing v246's `Mode::Arith` + `parse_arith_body`): on the
/// matching `))` (`ArithClose`) build `Command::Arith(body)` and wrap trailing
/// redirects; on `ArithBail` (a depth-0 `)` not followed by `)`, e.g. `((cmd);
/// c2)`) rewind to before `((` and reparse as a nested subshell `( (…) )`
/// (matching bash's arith-command backoff). Mirrors `parse_arith_expansion`'s
/// mark/push/pop lifecycle; the `mark` is taken BEFORE consuming/pushing so a
/// bail rewind returns to the pre-`((` position with the pre-push mode stack.
///
/// No lexer change: consuming the two buffered `Op(LParen)` first, then pushing
/// `Mode::Arith { body_started: true }`, makes the next pull enter
/// `scan_step_arith`'s body loop directly — the `$((`-opener branch (and its
/// `$`-assert) is never reached.
fn parse_arith_command(iter: &mut Lexer) -> Result<Command, ParseError> {
    let mark = iter.mark();
    iter.next_kind()?; // consume first `(` (buffered Op(LParen))
    iter.next_kind()?; // consume second `(`
    iter.push_mode(Mode::Arith { paren_depth: 0, in_dquote: false, body_started: true });
    let result = parse_arith_body(iter, false);
    iter.pop_mode();
    match result? {
        ArithBodyOutcome::Closed(body) => maybe_wrap_redirects(Command::Arith(body), iter),
        ArithBodyOutcome::Bail => {
            iter.rewind(&mark);
            parse_subshell(iter)
        }
    }
}
```

- [ ] **Step 4: Replace the dispatch deferral**

At `parser.rs:2016-2028`, the current block is:

```rust
    // `(( expr ))` at command position.  The Word-lexer emits a single
    // `ArithBlock`; the atom scanner instead emits two GLUED `Op(LParen)` atoms
    // (no `Blank` between).  Either way an arith command is DEFERRED (out of
    // scope) → `UnsupportedCommand`.  A SPACED `( (` keeps a `Blank` between the
    // two `(`, so it is a nested subshell (handled by the `LParen` arm below).
    if matches!(iter.peek_kind()?, Some(TokenKind::ArithBlock(..))) {
        return Err(ParseError::UnsupportedCommand);
    }
    if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen)))
        && matches!(iter.peek2_kind()?, Some(TokenKind::Op(Operator::LParen)))
    {
        return Err(ParseError::UnsupportedCommand);
    }
```

Replace it with (leave the `ArithBlock` arm as-is — it is never hit on the atom path, which emits two `Op(LParen)`, but keeps `parse_command` total; only the two-`LParen` arm changes):

```rust
    // `(( expr ))` at command position.  The Word-lexer emits a single
    // `ArithBlock`; the atom scanner instead emits two GLUED `Op(LParen)` atoms
    // (no `Blank` between) — v255 handles those via `parse_arith_command`
    // (speculative arith with an `ArithBail`→nested-subshell backoff).  A SPACED
    // `( (` keeps a `Blank` between the two `(`, so it never matches here and
    // flows to the single-`(` subshell arm below (never arith).
    if matches!(iter.peek_kind()?, Some(TokenKind::ArithBlock(..))) {
        return Err(ParseError::UnsupportedCommand);
    }
    if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen)))
        && matches!(iter.peek2_kind()?, Some(TokenKind::Op(Operator::LParen)))
    {
        return parse_arith_command(iter);
    }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_arith_command -- --test-threads 1`
Expected: PASS.

- [ ] **Step 6: Full suite + gates**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → all pass (0 failed).
Run: `cargo build -p huck-syntax 2>&1 | grep -c warning` → `0`.
Run: `git diff main -- crates/huck-syntax/src/command.rs crates/huck-syntax/src/lexer.rs | wc -l` → `0`.

- [ ] **Step 7: Commit**

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v255 T1: parse_arith_command + dispatch (close→Arith, bail→subshell)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Disambiguation hardening corpus

Pure-test task exercising the Task 1 mechanism across the full arith-vs-subshell corpus. No production code changes; the differential harness is the deliverable (it is what proves byte-identical parity and catches any latent edge in the Task 1 mechanism).

**Files:**
- Test: `crates/huck-syntax/src/parser.rs` `mod tests` — add `atoms_arith_command_disambiguation`.

**Interfaces:**
- Consumes: `parse_arith_command` (Task 1), `diff_cmd`. Produces: nothing (tests only).

- [ ] **Step 1: Write the test**

Add after `atoms_arith_command`:

```rust
    #[test]
    fn atoms_arith_command_disambiguation() {
        // ── Close cleanly → Command::Arith ──────────────────────────────────
        diff_cmd("(())");                // empty body → Arith(Word([]))
        diff_cmd("(( ))");               // single-space body → Arith([Literal " "])
        diff_cmd("(( (1+2) * 3 ))");     // inner grouping parens: depth-tracked, NOT a bail
        diff_cmd("(( a[0] + 1 ))");      // subscript brackets are plain body text
        diff_cmd("(( a + $b + ${c} ))"); // multiple embedded expansions
        // ── Bail → nested subshell (depth-0 `)` not followed by `)`) ─────────
        diff_cmd("((echo hi) )");        // glued open, inner closes with a single `)`
        diff_cmd("(( 3*4 ) )");          // glued open, SPACED close
        diff_cmd("((a) && (b))");        // `)` after `a` at depth 0 not followed by `)`
        diff_cmd("((a); (b))");
        // ── Spaced `( (` → subshell (existing path; regression guards) ───────
        diff_cmd("( (echo hi) )");
        diff_cmd("( ( a ) )");
    }
```

- [ ] **Step 2: Run the test**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_arith_command_disambiguation -- --test-threads 1`
Expected: PASS. (If any line fails, the atom path diverges from the oracle on that shape — investigate the `parse_arith_command`/`parse_arith_body` handling for that case and reconcile on the atom path; do NOT change the oracle. If a genuine, defensible divergence emerges, convert that one line to a pinned carry-forward — assert both `is_err()` agree or the specific behavior — and record it in the ledger, rather than weakening the rest.)

- [ ] **Step 3: Gates**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → all pass.
Run: `cargo build -p huck-syntax 2>&1 | grep -c warning` → `0`.

- [ ] **Step 4: Commit**

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v255 T2: arith-command disambiguation corpus (arith/bail/spaced)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Composition + flip the deferral test

Compose `(( … ))` with pipelines / lists / redirects / compound bodies, and remove the now-obsolete deferral assertions.

**Files:**
- Test: `crates/huck-syntax/src/parser.rs` `mod tests` — add `atoms_arith_command_composition`; edit `cmd_compound_deferred_still` (`parser.rs:4872`).

**Interfaces:**
- Consumes: `parse_arith_command` (Task 1), `diff_cmd`. Produces: nothing (tests only).

- [ ] **Step 1: Write the composition test**

Add after `atoms_arith_command_disambiguation`:

```rust
    #[test]
    fn atoms_arith_command_composition() {
        diff_cmd("(( 1 )) && echo hi");   // in an && list
        diff_cmd("(( 1 )) || echo no");   // in an || list
        diff_cmd("(( 1 )); echo done");   // in a `;` list
        diff_cmd("(( 1+2 )) >out");       // trailing redirect → Redirected{ inner: Arith }
        diff_cmd("(( 1 )) | cat");        // pipeline stage
        diff_cmd("if (( x > 0 )); then y; fi");        // arith as an if-condition
        diff_cmd("while (( i < 3 )); do x; done");     // arith as a while-condition
        diff_cmd("for i in a; do (( n++ )); done");    // arith in a for body
    }
```

- [ ] **Step 2: Run the composition test**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_arith_command_composition -- --test-threads 1`
Expected: PASS.

- [ ] **Step 3: Flip the deferral assertions**

In `cmd_compound_deferred_still` (`parser.rs:4872`), delete the two now-obsolete lines:

```rust
        diff_unsupported("(( 1+2 ))");                              // arith command (ArithBlock seam)
        diff_unsupported("(( x + $y ))");
```

Leave the remaining assertions in that test (`coproc x`, `for ((i=0;i<3;i++)); do x; done` — still deferred) unchanged. Add a short comment where the lines were, mirroring the existing “removed: now in-scope” notes:

```rust
        // `(( 1+2 ))` / `(( x + $y ))` (standalone arith command) removed: now in-scope, v255 T1.
```

- [ ] **Step 4: Run the edited deferral test + full suite**

Run: `cargo test -p huck-syntax --jobs 1 --lib cmd_compound_deferred_still -- --test-threads 1` → PASS.
Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → all pass.
Run: `cargo build -p huck-syntax 2>&1 | grep -c warning` → `0`.
Run: `git diff main -- crates/huck-syntax/src/command.rs crates/huck-syntax/src/lexer.rs | wc -l` → `0`.
Run: `grep -c 'command_atoms: false' crates/huck-syntax/src/lexer.rs` → `2`.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v255 T3: arith-command composition corpus + flip the deferral test

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-review (plan author)

**Spec coverage:** ✅ standalone `(( ))` close→`Command::Arith` (T1/T2), bail→subshell backoff incl. `(( 3*4 ) )` spaced-close (T1/T2), spaced `( (`→subshell regression (T1/T2), embedded expansions/empty/inner-grouping (T2), composition incl. redirect-wrap (T3), unterminated `diff_err` parity (T1), flip the deferral test (T3), no-lexer-change opener + progress/OOM + v248-hazard reasoning (T1 `parse_arith_command` doc + Global Constraints). C-style `for (( ))` explicitly out of scope. ✅

**Placeholder scan:** none — every step has concrete code/commands and the corpus values are probed ground truth.

**Type consistency:** `parse_arith_command(&mut Lexer) -> Result<Command, ParseError>`; `Command::Arith(Word)`; `Mode::Arith { paren_depth, in_dquote, body_started }`; `ArithBodyOutcome::{Closed(Word), Bail}`; `maybe_wrap_redirects(Command, &mut Lexer)`; `parse_subshell(&mut Lexer)` — all match the Reference block and the existing code.
