# v248 — Function definitions on the atom-command path (dormant, differential) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the dormant atom-command parser recognize both function-definition forms (`name() compound` and `function NAME [()] compound`), producing ASTs byte-identical to the `command.rs` oracle, gated by the existing old-vs-new differential harness — no lexer changes, no live flip.

**Architecture:** Parser-only port. Widen two pure oracle validators (`valid_function_name_text`, `is_function_body_shape`) to `pub(crate)` and reuse them; reimplement the three funcdef flow helpers in `parser.rs` so they call the atom-path `parse_command` for the body; wire detection into atom-path `parse_command` (a `Some(Keyword::Function)` dispatch arm + a `mark`/`rewind` `name()` detector that also handles the spaced `f ()` form). The atoms are already emitted by the v247 scanner, so `lexer.rs` is untouched.

**Tech Stack:** Rust, `crates/huck-syntax` (`command.rs`, `parser.rs`). No new dependencies.

## Global Constraints

- Test ONLY with `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` (narrow with filters). This box (1 core / ~1.9 GiB) is OOM-KILLED by `cargo test --workspace` or any parallel/multi-threaded run — NEVER run those.
- Byte-identical: every in-scope funcdef input parses to the SAME AST on the atom path as the oracle (`diff_cmd`). A well-formed in-scope divergence is a v248 BUG to fix, not to pin.
- PRODUCTION IS UNTOUCHED: `command_atoms` defaults `false`; `scan_step_command` / `process_line` / the fat scanners unchanged. The ONLY `command.rs` edits are two `pub(crate)` visibility widenings (no logic change). No live flip.
- No lexer changes at all (funcdef atoms — `Lit(name)`, `Op(LParen)`, `Op(RParen)`, `Blank`, compound-body atoms — are already emitted).
- Body coverage = whatever the atom-path `parse_command` already handles (brace/subshell/if/while/until/for/select/case, incl. redirected wrapping). Funcdefs whose body is itself a still-deferred construct (`f() [[ ]]`, `f() (( ))`) defer cleanly (`UnsupportedCommand`) and are pinned, NOT forced green.
- 0 warnings (`cargo build -p huck-syntax 2>&1 | grep -c warning` → `0`).
- Every commit ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Branch: `v248-function-definitions`. Do NOT commit to `main`.

## Key existing anchors (read before starting)

- **Oracle funcdef code** (the spec — match it): `command.rs:1054` (`Keyword::Function` dispatch), `command.rs:1067–1080` (bare-`(` guard + `name()` two-token detection), `parse_function_def` `command.rs:1190`, `parse_function_keyword_def` `command.rs:1209`, `finish_function_body` `command.rs:1176`, `is_function_body_shape` `command.rs:1148`, `valid_function_name_text` `command.rs:1354`.
- **AST:** `Command::FunctionDef { name: String, body: Box<Command> }` (`command.rs:616`).
- **Atom-path `parse_command`:** `parser.rs:1494`; the keyword-dispatch match at `parser.rs:1529–1537` (add the `Function` arm before `Some(_) => Err(UnsupportedCommand)` at :1536); the current `name()` deferral block at `parser.rs:1539–1549` (REPLACE it).
- **Reused parser helpers (v247):** `consume_command_word` (`parser.rs:1174` — takes a legacy `Word` whole OR assembles atoms via `parse_word_command`), `peek_leading_keyword` (`parser.rs`, classifies a bare `Lit` keyword), `skip_newlines`, `maybe_wrap_redirects`. Reused oracle helpers already exposed: `crate::command::next_is_redirect` (`pub(crate)`), `crate::command::try_split_assignment`.
- **`mark`/`rewind`:** `Lexer::mark()` / `Lexer::rewind(&Mark)` (`lexer.rs:778/811`, `pub(crate)`); parser.rs already uses them at ~1038/1055. `mark` captures cursor offset + history index + mode; `rewind` restores them.
- **Differential harness:** `new_seq` (atoms, `parser.rs:2520`), `old_seq` (oracle, `parser.rs:2516`), `diff_cmd` (`parser.rs:2532`, asserts `new_seq == old_seq`). Deferred assertion pattern: `assert!(matches!(new_seq(s), Err(ParseError::UnsupportedCommand)))`.
- **ParseError variants (exist):** `FunctionName`, `FunctionBody`, `UnterminatedFunction`, `UnsupportedCommand` — grep `enum ParseError` in `command.rs` to confirm before use.

## File Structure

- `crates/huck-syntax/src/command.rs` — two `fn` → `pub(crate) fn` visibility widenings ONLY (`valid_function_name_text`, `is_function_body_shape`). No logic change.
- `crates/huck-syntax/src/parser.rs` — the three reimplemented flow helpers (`parse_function_def`, `parse_function_keyword_def`, `finish_function_body`); the two detection edits in `parse_command`; the differential tests.

---

### Task 1: `function NAME [()] compound` form + reuse plumbing

**Files:**
- Modify: `crates/huck-syntax/src/command.rs` (widen `valid_function_name_text` + `is_function_body_shape` to `pub(crate)`)
- Modify: `crates/huck-syntax/src/parser.rs` (add `finish_function_body` + `parse_function_keyword_def`; add the `Some(Keyword::Function)` dispatch arm)
- Test: `crates/huck-syntax/src/parser.rs` `mod tests`

**Interfaces:**
- Consumes: `consume_command_word` (`fn(&mut Lexer) -> Result<Word, ParseError>`), `skip_newlines`, atom-path `parse_command` (`fn(&mut Lexer) -> Result<Command, ParseError>`), `crate::command::valid_function_name_text` (`fn(&Word) -> Option<String>`), `crate::command::is_function_body_shape` (`fn(&Command) -> bool`).
- Produces:
  - `fn finish_function_body(name: String, iter: &mut Lexer) -> Result<Command, ParseError>` (parser.rs).
  - `fn parse_function_keyword_def(iter: &mut Lexer) -> Result<Command, ParseError>` (parser.rs).

- [ ] **Step 1: Widen the two oracle validators to `pub(crate)`.** In `command.rs`, change `fn valid_function_name_text(` (`command.rs:1354`) to `pub(crate) fn valid_function_name_text(`, and `fn is_function_body_shape(` (`command.rs:1148`) to `pub(crate) fn is_function_body_shape(`. No other change to either function. Build to confirm nothing else breaks: `cargo build -p huck-syntax 2>&1 | tail -3`.

- [ ] **Step 2: Write the failing test** (in `parser.rs` `mod tests`, near the v247 atom tests):

```rust
    // ── v248: function definitions on the atom path ──────────────────────────
    #[test]
    fn atoms_function_keyword_form() {
        diff_cmd("function f { :; }");
        diff_cmd("function f() { :; }");
        diff_cmd("function f ()  { :; }");        // spaced ()
        diff_cmd("function greet { echo hi; }");
        diff_cmd("function f\n{ :; }");           // newline before body
    }
```

- [ ] **Step 3: Run test to verify it fails.** Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_function_keyword_form -- --test-threads 1`. Expected: FAIL (atom path returns `UnsupportedCommand` for the `function` keyword today — it hits `Some(_) => Err(UnsupportedCommand)`).

- [ ] **Step 4: Add `finish_function_body` in `parser.rs`.** Place it near the other compound parsers. Mirrors `command.rs:1176` but calls the atom-path `parse_command`:

```rust
/// Shared tail of both funcdef forms (mirrors `command.rs`'s
/// `finish_function_body`): skip newlines, require a body, parse it via the
/// atom-path `parse_command`, and validate its shape. A body that is itself a
/// still-deferred construct makes `parse_command` return `UnsupportedCommand`,
/// which propagates — the funcdef defers cleanly (pinned case).
fn finish_function_body(name: String, iter: &mut Lexer) -> Result<Command, ParseError> {
    skip_newlines(iter)?;
    if iter.peek_kind()?.is_none() {
        return Err(ParseError::UnterminatedFunction);
    }
    let body = parse_command(iter)?;
    if !crate::command::is_function_body_shape(&body) {
        return Err(ParseError::FunctionBody);
    }
    Ok(Command::FunctionDef { name, body: Box::new(body) })
}
```

- [ ] **Step 5: Add `parse_function_keyword_def` in `parser.rs`.** Mirrors `command.rs:1209`; consumes the `function` keyword word via `consume_command_word`, skips `Blank`s the atom scanner emits, reads + validates the name, optionally consumes `( )`:

```rust
/// `function NAME [()] compound` (mirrors `command.rs`'s
/// `parse_function_keyword_def`). Caller confirmed the leading keyword is
/// `function` via `peek_leading_keyword`. Skips the atom-stream `Blank`s the
/// Word-lexer never emitted.
fn parse_function_keyword_def(iter: &mut Lexer) -> Result<Command, ParseError> {
    consume_command_word(iter)?; // consume the `function` keyword word
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) { iter.next_kind()?; }
    // Name: a single valid identifier word.
    let name_word = consume_command_word(iter)?;
    let name = crate::command::valid_function_name_text(&name_word)
        .ok_or(ParseError::FunctionName)?;
    // Optional `()` (blanks may sit between name/`(`/`)` in the atom stream).
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) { iter.next_kind()?; }
    if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen))) {
        iter.next_kind()?; // `(`
        while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) { iter.next_kind()?; }
        match iter.next_kind()? {
            Some(TokenKind::Op(Operator::RParen)) => {}
            _ => return Err(ParseError::FunctionBody),
        }
    }
    finish_function_body(name, iter)
}
```

Note: `consume_command_word` returns an empty/invalid `Word` for a non-word next token; `valid_function_name_text` then returns `None` → `FunctionName`, matching the oracle's "name must be a Word" rejection. (If `consume_command_word` errors on a leading `Op`/boundary instead, that Err also propagates as a rejection — either way `function` with no valid name is an error, which the Task 3 error-parity test pins against the oracle.)

- [ ] **Step 6: Wire the dispatch arm.** In `parse_command` (`parser.rs`), in the `match peek_leading_keyword(iter)?` block, add the `Function` arm BEFORE the `Some(_) => Err(UnsupportedCommand)` catch-all at `parser.rs:1536`:

```rust
        Some(Keyword::Case)   => return parse_case(iter),
        Some(Keyword::Function) => return parse_function_keyword_def(iter),
        Some(_) => return Err(ParseError::UnsupportedCommand),
```

- [ ] **Step 7: Run test to verify it passes.** Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_function_keyword_form -- --test-threads 1`. Expected: PASS. Debug any AST mismatch against `old_seq` (`name` string + body `Command` must match).

- [ ] **Step 8: Warnings + commit.** `cargo build -p huck-syntax 2>&1 | grep -c warning` → `0`. Commit:

```bash
git add crates/huck-syntax/src/command.rs crates/huck-syntax/src/parser.rs
git commit -m "v248 T1: function NAME [()] form on the atom path + reuse oracle validators

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `name() compound` form (incl. spaced `f ()`) via mark/rewind

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` (add `parse_function_def`; replace the `name()` deferral block in `parse_command`)
- Test: `crates/huck-syntax/src/parser.rs` `mod tests`

**Interfaces:**
- Consumes: `finish_function_body` (Task 1), `consume_command_word`, `crate::command::valid_function_name_text`, `Lexer::mark`/`Lexer::rewind`.
- Produces: `fn parse_function_def(name_word: Word, iter: &mut Lexer) -> Result<Command, ParseError>` (parser.rs).

- [ ] **Step 1: Write the failing test** (in `parser.rs` `mod tests`):

```rust
    #[test]
    fn atoms_function_paren_form() {
        diff_cmd("f(){ :; }");
        diff_cmd("f() { :; }");
        diff_cmd("f ()  { :; }");                 // spaced name/()
        diff_cmd("f() ( a; b )");                  // subshell body
        diff_cmd("f() if x; then y; fi");          // if body
        diff_cmd("f() while x; do y; done");       // while body
        diff_cmd("f() for i in a b; do echo $i; done");
        diff_cmd("f() case $x in a) echo a;; esac");
        diff_cmd("f() select x in a b; do echo $x; break; done");
        diff_cmd("f() until x; do y; done");        // until body
        diff_cmd("f() { :; } >log");               // redirected body
        diff_cmd("f() { :; } 2>&1");
        diff_cmd("f() { g() { :; }; }");           // nested funcdef
        diff_cmd("if true; then f() { :; }; fi");  // funcdef inside a compound
    }
    #[test]
    fn atoms_function_not_a_def() {
        diff_cmd("f");                             // bare word = plain command
        diff_cmd("echo function");                 // `function` mid-command = arg
        diff_cmd("func --opt");                    // prefix of `function` = plain command (mark/rewind restores)
    }
```

- [ ] **Step 2: Run test to verify it fails.** Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_function_paren_form -- --test-threads 1`. Expected: FAIL (`name()` currently defers to `UnsupportedCommand`; spaced `f ()` currently mis-routes into `parse_simple`).

- [ ] **Step 3: Add `parse_function_def` in `parser.rs`.** Mirrors `command.rs:1190`; the caller has already consumed the name into `name_word` and confirmed a `(` is next:

```rust
/// `name() compound` (mirrors `command.rs`'s `parse_function_def`). The caller
/// consumed the name (`name_word`) and confirmed the next non-`Blank` token is
/// `Op(LParen)`. Skips atom-stream `Blank`s inside `( )`.
fn parse_function_def(name_word: Word, iter: &mut Lexer) -> Result<Command, ParseError> {
    let name = crate::command::valid_function_name_text(&name_word)
        .ok_or(ParseError::FunctionName)?;
    iter.next_kind()?; // `(`
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) { iter.next_kind()?; }
    match iter.next_kind()? {
        Some(TokenKind::Op(Operator::RParen)) => {}
        _ => return Err(ParseError::FunctionBody),
    }
    finish_function_body(name, iter)
}
```

- [ ] **Step 4: Replace the `name()` deferral block in `parse_command`.** At `parser.rs:1539–1549`, replace the deferral `if matches!(...) { return Err(UnsupportedCommand); }` with mark/rewind detection (mirrors the oracle's consume-then-check; also handles the spaced form because it skips `Blank`s before checking `(`):

```rust
    // Function definition `name() compound` (POSIX form). The oracle consumes
    // the leading word then checks for `(`; the Word-lexer ate any space, so
    // `f()` and `f ()` both reach it with `(` next. The atom stream keeps the
    // `Blank` explicit, so mirror the oracle via mark/consume-name/skip-Blank/
    // check-`(`, rewinding when it is NOT a funcdef so `parse_simple` re-parses
    // the same bytes. Only a bare word (`Lit`/legacy `Word`) can start a name.
    if matches!(
        iter.peek_kind()?,
        Some(TokenKind::Word(_)) | Some(TokenKind::Lit { quoted: false, .. })
    ) {
        let m = iter.mark();
        let name_word = consume_command_word(iter)?;
        while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) { iter.next_kind()?; }
        if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen))) {
            return parse_function_def(name_word, iter);
        }
        iter.rewind(&m); // not a funcdef — restore and fall through
    }
    // Simple command: parse and return BARE.  `parse_pipeline` wraps it.
    parse_simple(iter)
```

- [ ] **Step 5: Run test to verify it passes.** Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_function_paren_form atoms_function_not_a_def -- --test-threads 1`. Expected: PASS. If `atoms_function_not_a_def` fails, the `rewind` is not fully restoring the stream — verify `mark`/`rewind` resets cursor + history + mode (lexer.rs:778/811) and that `consume_command_word` did not trigger a mode push that survives rewind.

- [ ] **Step 6: Guard against a subtle rewind regression.** Run the FULL atom-path suite to confirm the new mark/rewind in the hot `parse_command` path did not disturb existing simple-command / compound parsing: `cargo test -p huck-syntax --jobs 1 --lib atoms_ -- --test-threads 1`. Expected: all PASS. (Every simple command now takes a `mark`, assembles the first word, finds no `(`, and rewinds — this MUST be behavior-preserving.)

- [ ] **Step 7: Warnings + commit.** `grep -c warning` → `0`. Commit:

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v248 T2: name() function-def form (incl. spaced f ()) on the atom path via mark/rewind

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Error parity, deferred-body pinning, full green gate

**Files:**
- Test: `crates/huck-syntax/src/parser.rs` `mod tests`

**Interfaces:**
- Consumes: T1 + T2 (both funcdef forms live on the atom path); `new_seq`/`old_seq`/`diff_cmd`.
- Produces: the comprehensive differential gate for v248.

- [ ] **Step 1: Write the error-parity test.** The exact error variant is whatever the oracle returns, so compare `new_seq` to `old_seq` (normalize Ok/Err to unit + error-debug):

```rust
    #[test]
    fn atoms_function_defs_errors() {
        for s in [
            "f() echo",          // non-compound body → FunctionBody
            "function",          // no name → FunctionName
            "function 1 { :; }", // invalid name → FunctionName
            "f(",                // unterminated
            "f()",               // `()` then EOF → UnterminatedFunction/FunctionBody
            "f ( a )",           // `(` not followed by `)` → FunctionBody (NOT a command)
        ] {
            assert_eq!(
                new_seq(s).map(|_| ()).map_err(|e| format!("{e:?}")),
                old_seq(s).map(|_| ()).map_err(|e| format!("{e:?}")),
                "funcdef error parity for {s:?}",
            );
        }
    }
```

- [ ] **Step 2: Run it; debug to parity.** Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_function_defs_errors -- --test-threads 1`. Expected: PASS. If a case diverges, fix the atom-path helper to return the oracle's error (do NOT weaken the test). NOTE: if the oracle path errors at the LEXER level for any case (e.g. `tokenize_with_opts` rejects it so `old_seq` would panic via `.expect("lex")`), drop that specific input and instead assert only `new_seq(s).is_err()` for it, with a comment — mirror how `atoms_error_parity` splits lexer-level rejects from parser-level ones.

- [ ] **Step 3: Write the deferred-body pinning test.** A funcdef whose body is a still-deferred construct defers cleanly:

```rust
    #[test]
    fn atoms_function_defs_deferred() {
        // Body is itself deferred → whole funcdef defers (lifts when [[ ]]/arith land).
        for s in ["f() [[ x ]]", "f() (( 1 ))", "f() for ((i=0;i<2;i++)); do :; done"] {
            assert!(matches!(new_seq(s), Err(ParseError::UnsupportedCommand)),
                "expected UnsupportedCommand (deferred body) for {s:?}, got {:?}", new_seq(s));
        }
    }
```

- [ ] **Step 4: Run it.** Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_function_defs_deferred -- --test-threads 1`. Expected: PASS. (If the oracle ALSO errors on one of these at parse time with a different variant, that is irrelevant — the assertion only checks the atom path defers to `UnsupportedCommand`. If the atom path returns a NON-`UnsupportedCommand` error, investigate: the body construct must defer, not hard-error.)

- [ ] **Step 5: Full atom-suite + full lib green.** Run the whole atom suite, then the whole huck-syntax lib:
  - `cargo test -p huck-syntax --jobs 1 --lib atoms_ -- --test-threads 1` → `0 failed`.
  - `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → `0 failed`.
  - `cargo test -p huck-syntax --jobs 1 --doc -- --test-threads 1` → `0 failed`.
  Investigate + FIX any `diff_cmd` regression (a well-formed in-scope divergence is a v248 bug). Watch for a hang (growing memory) — a mark/rewind or Blank-skip loop that fails to make progress; if one appears, the offending loop is in the new funcdef helpers or the `parse_command` detector.

- [ ] **Step 6: Warnings + commit.** `cargo build -p huck-syntax 2>&1 | grep -c warning` → `0`. Commit:

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v248 T3: funcdef error parity + deferred-body pinning + full atom-suite green

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review checklist (run before the whole-branch review)

- Both funcdef forms have `diff_cmd` coverage (keyword form: Task 1; `name()` incl. spaced: Task 2), every supported body shape, redirected body, nested, and funcdef-inside-compound. Error parity + deferred-body pinning present (Task 3).
- Over-eager detection guarded: `f`, `echo function`, `func --opt` parse as plain commands (`atoms_function_not_a_def`) — the mark/rewind restores the stream when no `(` follows.
- Production untouched: `git diff main -- crates/huck-syntax/src/command.rs` shows ONLY the two `fn`→`pub(crate) fn` lines; `lexer.rs` unchanged (`git diff main -- crates/huck-syntax/src/lexer.rs` empty); `command_atoms` still defaults `false`; `scan_step_command`/`process_line` unchanged.
- Reuse: `parser.rs` calls `crate::command::valid_function_name_text` + `crate::command::is_function_body_shape` (one implementation of each rule, no duplication).
- Full `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` is `0 failed`; doctests `0 failed`; 0 warnings.
- All commits carry the trailer; branch is `v248-function-definitions`.
