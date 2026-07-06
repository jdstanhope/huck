# v265 — Delete the Oracle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the resident `command.rs` oracle parser and the six forward-scanning lexer functions, leaving one parser (the atom path) and one lexing discipline.

**Architecture:** Compiler-guided deletion. First make nothing call the oracle entry points (`command::parse`, `tokenize`/`tokenize_with_opts`, `from_tokens`) — port `continuation.rs`, repoint the test-only helpers, smoke-convert the differential harness. Then remove the entry points and follow `dead_code` warnings until only shared code (AST types + atom scanners + shared leaves) survives. Finally tidy module ownership (`command.rs` = AST, `lexer.rs` = token production, `parser.rs` = all parsing) and backfill oracle-independent shape tests.

**Tech Stack:** Rust (huck-syntax + huck-engine crates), single-threaded per-crate test runner, bash-diff harnesses.

## Global Constraints

- **Test runner (box is 1 core / 1.9 GB):** ONLY `cargo test -p <crate> --jobs 1 --lib -- --test-threads 1`. NEVER `--workspace` or multi-threaded (OOM-kills the session). Crates: `huck-syntax`, `huck-engine`.
- **Build the binary** with `cargo build -p huck` (root package). `huck-cli` is a lib and does NOT build the binary.
- **Guard every bash-diff harness / binary run** with `ulimit -v 1500000` + `timeout` in a subshell.
- **THE RULE:** the lexer emits small atoms and NEVER forward-scans for a matching delimiter across nesting; the parser owns delimiter-matching, recursion, and structure. Do not add fat-lexer logic.
- **0 warnings** from `cargo build -p huck-syntax` and `-p huck-engine` at task end. Trust `cargo`, not rust-analyzer (phantom `dead_code` diagnostics recur — verify with a real build).
- **Baseline to preserve:** huck-syntax 1042 pass, huck-engine 1738 pass, bash-diff sweep 1688 pass / 1 fail (`funcnest`, pre-existing intentional L-63). Counts shift as noted per task; the bash-diff 1688/1 must not regress.
- **Commit trailer, verbatim on every commit:** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **Do not touch `command.rs` AST types or behavior** except as each task specifies. No feature or bug-fix changes — v265 is deletion + relocation only.

**Task ordering is a hard dependency chain:** Tasks 1–3 remove every caller of the oracle; Task 4 cannot delete the oracle until they are done. Task 5 (tidy) requires Task 4's dead code gone. Task 6 (tests) runs last against the final structure. Do them in order.

---

### Task 1: Port `continuation.rs` off the oracle

The only non-mechanical production change. `continuation::classify` currently tokenizes with the oracle `tokenize_with_opts` and calls `command::parse` twice. Replace with a single atom parse (`parser::parse_sequence` over `new_live_atoms`), mapping the same signals, plus a small atom-scan helper for the trailing-connector check.

**Files:**
- Modify: `crates/huck-engine/src/continuation.rs` (imports at lines 6-7; `classify` at lines 46-96; tests at 128+ are the gate)

**Interfaces:**
- Consumes: `crate::lexer::Lexer::new_live_atoms(input: &str, aliases: &HashMap<String,String>, opts: LexerOptions) -> Lexer`; `crate::lexer::Lexer::next(&mut self) -> Result<Option<Token>, LexError>`; `crate::parser::parse_sequence(&mut Lexer) -> Result<Option<Sequence>, ParseError>`; `ParseError::Lex(Box<LexError>)`.
- Produces: unchanged public API — `classify(buffer: &str, extglob: bool) -> Completeness`.

- [ ] **Step 1: Confirm the current test baseline is green**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 continuation`
Expected: PASS (47 continuation tests). Record the exact count.

- [ ] **Step 2: Update the imports**

In `crates/huck-engine/src/continuation.rs`, replace lines 6-7:

```rust
use crate::command::{self, ParseError};
use crate::lexer::{self, ends_with_continuation_backslash, LexError, Operator, TokenKind};
```

with:

```rust
use crate::command::ParseError;
use crate::lexer::{self, ends_with_continuation_backslash, LexError, Operator, TokenKind};
use crate::parser;
```

- [ ] **Step 3: Replace the body of `classify`**

Replace the entire `classify` function (lines 46-96) with:

```rust
pub fn classify(buffer: &str, extglob: bool) -> Completeness {
    if ends_with_continuation_backslash(buffer) {
        return Completeness::Incomplete(ContinuationReason::Backslash);
    }
    let opts = lexer::LexerOptions { extglob, ..Default::default() };
    let empty = std::collections::HashMap::new();
    let mut lx = lexer::Lexer::new_live_atoms(buffer, &empty, opts);
    let parsed = parser::parse_sequence(&mut lx);

    // Run the parser's verdict first so an unterminated `[[ … ]]` is detected
    // even when the buffer ends with `&&`/`||` (which would otherwise
    // short-circuit to `Operator`). Lex errors now arrive via ParseError::Lex.
    if let Err(ParseError::UnterminatedDoubleBracket) = parsed {
        return Completeness::Incomplete(ContinuationReason::DoubleBracket);
    }
    if buffer_ends_with_connector(buffer, extglob) {
        return Completeness::Incomplete(ContinuationReason::Operator);
    }
    match parsed {
        Ok(_) => Completeness::Complete,
        Err(ParseError::Lex(e)) if matches!(*e, LexError::UnterminatedHeredoc) => {
            Completeness::Incomplete(ContinuationReason::Heredoc)
        }
        Err(ParseError::Lex(e)) if is_unterminated_lex(&e) => {
            Completeness::Incomplete(ContinuationReason::OpenQuote)
        }
        Err(ParseError::UnterminatedSubshell) => {
            Completeness::Incomplete(ContinuationReason::Subshell)
        }
        Err(ParseError::UnterminatedIf
            | ParseError::UnterminatedLoop
            | ParseError::UnterminatedCase
            | ParseError::UnterminatedBrace
            | ParseError::UnterminatedFunction) => {
            Completeness::Incomplete(ContinuationReason::Compound)
        }
        Err(_) => Completeness::Error,
    }
}

/// True when the buffer's last significant atom is a `|`, `&&`, or `||`
/// connector — the atom-path replacement for the old `tokens.last()` check
/// (the oracle `tokenize` path is being removed in v265).
fn buffer_ends_with_connector(buffer: &str, extglob: bool) -> bool {
    let opts = lexer::LexerOptions { extglob, ..Default::default() };
    let empty = std::collections::HashMap::new();
    let mut lx = lexer::Lexer::new_live_atoms(buffer, &empty, opts);
    let mut last_significant: Option<TokenKind> = None;
    while let Ok(Some(tok)) = lx.next() {
        match tok.kind {
            TokenKind::Blank | TokenKind::Newline => {}
            other => last_significant = Some(other),
        }
    }
    matches!(
        last_significant,
        Some(TokenKind::Op(Operator::Pipe | Operator::And | Operator::Or))
    )
}
```

Note: `is_unterminated_lex` (already in the file) takes `&LexError`; `*e` derefs the `Box`, and `&e` in the guard re-borrows the deref-bound `e` — write `is_unterminated_lex(&e)` where `e: Box<LexError>` auto-derefs, or bind `let le = &*e;` if the borrow checker objects. The `matches!(*e, …)` in the Heredoc arm moves nothing (it matches on the deref). If ordering of the two `Lex` arms matters, Heredoc must come first (it does).

- [ ] **Step 4: Verify `TokenKind::Blank` and `TokenKind::Newline` exist**

Run: `grep -n "Blank\|Newline," crates/huck-syntax/src/lexer.rs | head`
Expected: both variants present in `enum TokenKind`. If `Blank` is named differently, use the actual word-boundary atom variant (the one `command_atoms_of` skips between words).

- [ ] **Step 5: Build and run the full continuation suite**

Run: `cargo build -p huck-engine 2>&1 | tail -5`
Expected: compiles, 0 warnings.
Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 continuation`
Expected: PASS, same count as Step 1 (47).

If any continuation test fails, the signal-mapping is off — diagnose which `ContinuationReason` diverges (the failing test name identifies it) before proceeding. Do NOT weaken a test. An unterminated heredoc/quote not surfacing as the right `ParseError::Lex` variant is the known risk (spec Risks) — if a variant genuinely cannot be produced by the atom path, report BLOCKED with the specific input and both paths' outputs.

- [ ] **Step 6: Run the whole huck-engine suite (nothing else regressed)**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`
Expected: 1738 pass (continuation still uses `command::parse` in `tokenize`? No — Task 1 removed continuation's oracle use; the test-helper sites remain until Task 2). 0 failures.

- [ ] **Step 7: Guarded interactive multiline spot-check**

Run (guarded):
```bash
cargo build -p huck 2>&1 | tail -2
export HUCK_BIN=$(pwd)/target/debug/huck
for buf in $'if true\nthen echo hi\nfi' $'while true\ndo echo x\ndone' $'for i in a b\ndo echo $i\ndone' $'case x in\nx) echo m;;\nesac' $'{ echo a\n echo b; }' $'echo a &&\necho b' $'echo "open' $'cat <<EOF\nhi\nEOF'; do
  echo "=== $buf ==="
  diff <(printf '%s\n' "$buf" | bash) <( ulimit -v 1500000; timeout 8 "$HUCK_BIN" <<<"$buf" )
done
```
Expected: no `diff` output for any fragment (each multiline construct completes and runs identically to bash). This exercises the REPL continuation path end to end.

- [ ] **Step 8: Commit**

```bash
git add crates/huck-engine/src/continuation.rs
git commit -m "v265 T1: port continuation.rs off the oracle onto the atom path

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Repoint the test-only `command::parse` helpers

Six `command::parse` call-sites remain, all inside test functions. Repoint each to the atom parser so the oracle has no callers. Mechanical, identical return type (`Result<Option<Sequence>, ParseError>`).

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs:11334`, `:11351`, `:11865`
- Modify: `crates/huck-engine/src/expand.rs:3669`
- Modify: `crates/huck-engine/src/executor.rs:7257`, `:8190`

**Interfaces:**
- Consumes: `crate::parser::parse_sequence`, `crate::lexer::Lexer::new_live_atoms`, `crate::lexer::LexerOptions` (from Task 1's usage; unchanged).
- Produces: nothing new (test-internal).

- [ ] **Step 1: Repoint each site**

At every site, replace the pattern:

```rust
crate::command::parse(&mut crate::lexer::Lexer::from_tokens(crate::lexer::tokenize(SRC).unwrap()))
```

with:

```rust
crate::parser::parse_sequence(&mut crate::lexer::Lexer::new_live_atoms(SRC, &Default::default(), crate::lexer::LexerOptions::default()))
```

where `SRC` is that site's source expression (e.g. `"myfn(){ :; }"`, `&src`, `src`). Concretely:

- `builtins.rs:11334` (`type_default_function`): `SRC` = `"myfn(){ :; }"`.
- `builtins.rs:11351` (`type_prints_function_body`): `SRC` = `"tf(){ echo a; }"`.
- `builtins.rs:11865` (`define_fn`): `SRC` = `src`.
- `expand.rs:3669` (`first_arg_word`): the local is `tokens` from `tokenize(&src)`; replace both the `tokenize` line and the `parse` line — delete `let tokens = crate::lexer::tokenize(&src).expect("lex");` and write `let seq = crate::parser::parse_sequence(&mut crate::lexer::Lexer::new_live_atoms(&src, &Default::default(), crate::lexer::LexerOptions::default())).expect("parse").expect("non-empty");`.
- `executor.rs:7257` (`render_test_leaf_forms`, inside the `parse_expr` closure): the local is `toks` from `tokenize(src)`; replace both lines — delete the `tokenize` line and write `match crate::parser::parse_sequence(&mut crate::lexer::Lexer::new_live_atoms(src, &Default::default(), crate::lexer::LexerOptions::default())).expect("parse").expect("seq").first {`.
- `executor.rs:8190` (`run_exec_single_function_call_…`): the `if let Some(tokens) = crate::lexer::tokenize(...).ok() && let Ok(Some(seq)) = crate::command::parse(...)` becomes a single `if let Ok(Some(seq)) = crate::parser::parse_sequence(&mut crate::lexer::Lexer::new_live_atoms("myfunc() { echo ok; }", &Default::default(), crate::lexer::LexerOptions::default()))`.

- [ ] **Step 2: Confirm no `command::parse` / `tokenize` / `from_tokens` callers remain outside command.rs/lexer.rs**

Run:
```bash
grep -rn "command::parse\|::tokenize(\|tokenize_with_opts\|from_tokens" crates/huck-engine/src crates/huck-syntax/src/parser.rs
```
Expected: NO matches in huck-engine and NO `command::parse`/`from_tokens`/`tokenize` in parser.rs (the differential helpers `old_seq`/`old_unit`/etc. in parser.rs still reference them — those are handled in Task 3, so a few parser.rs matches inside `mod tests` helpers are expected here; huck-engine must be clean).

- [ ] **Step 3: Build + test both crates**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`
Expected: 1738 pass, 0 fail, 0 warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/huck-engine/src/builtins.rs crates/huck-engine/src/expand.rs crates/huck-engine/src/executor.rs
git commit -m "v265 T2: repoint test-only command::parse helpers onto the atom parser

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Smoke-convert the differential harness

Change only the differential *helper bodies* in parser.rs `mod tests` so they stop calling the oracle, keeping all ~882 call-sites (`diff_cmd` × 830, etc.) as parse-level `Ok`/`Err` regression guards. Then delete the now-unused oracle helpers.

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` — helper defs at 4489-4536 (`old_seq`/`new_seq`/`diff_cmd`/`diff_err`/`old_seq_al`/`new_seq_al`/`diff_al`), 7362-7418 (`old_unit`/`new_unit`/`diff_unit`/`old_eg`/`new_eg`/`diff_eg`)

**Interfaces:**
- Consumes: `new_seq`, `new_seq_al`, `new_unit`, `new_eg` (unchanged atom-path helpers).
- Produces: unchanged helper signatures `diff_cmd(&str)`, `diff_err(&str)`, `diff_al(&str, &[(&str,&str)])`, `diff_unit(&str)`, `diff_eg(&str)` (bodies changed) — all 830+ call-sites compile unchanged.

- [ ] **Step 1: Rewrite the five differential helper bodies**

`diff_cmd` (was `assert_eq!(new_seq(s).unwrap(), old_seq(s).unwrap(), …)`):
```rust
    fn diff_cmd(s: &str) {
        assert!(new_seq(s).is_ok(), "expected Ok for {s:?}, got {:?}", new_seq(s));
    }
```

`diff_err` (was `assert_eq!(new_seq(s), old_seq(s), …)`):
```rust
    fn diff_err(s: &str) {
        assert!(new_seq(s).is_err(), "expected Err for {s:?}, got {:?}", new_seq(s));
    }
```

`diff_al` (was `assert_eq!(new_seq_al(s, pairs), old_seq_al(s, pairs), …)`):
```rust
    fn diff_al(s: &str, pairs: &[(&str, &str)]) {
        assert!(new_seq_al(s, pairs).is_ok(), "expected Ok for {s:?}, got {:?}", new_seq_al(s, pairs));
    }
```

`diff_unit` (was `assert_eq!(new_unit(s), old_unit(s), …)`):
```rust
    fn diff_unit(s: &str) {
        assert!(new_unit(s).iter().all(|r| r.is_ok()), "expected all-Ok units for {s:?}, got {:?}", new_unit(s));
    }
```

`diff_eg` (was `assert_eq!(new_eg(s), old_eg(s), …)`):
```rust
    fn diff_eg(s: &str) {
        assert!(new_eg(s).is_ok(), "expected Ok for {s:?}, got {:?}", new_eg(s));
    }
```

Leave `diff_unsupported` unchanged (it already checks only `new_seq`).

- [ ] **Step 2: Delete the now-unused oracle helpers**

Delete `old_seq` (4489-4492), `old_seq_al` (4520-4525), `old_unit` (7362-7366), `old_eg` (7410-7417). Keep `new_seq`, `new_seq_al`, `new_unit`, `new_eg`.

Also fix `atoms_scaffolding_exists` (≈4497) which calls `old_seq`: change its body to `assert_eq!(new_seq("").unwrap(), None);` (empty input parses to `None`).

- [ ] **Step 3: Confirm the oracle is no longer referenced from parser.rs**

Run: `grep -n "old_seq\|old_unit\|old_eg\|command::parse\|tokenize(\|from_tokens" crates/huck-syntax/src/parser.rs`
Expected: NO matches (the harness is fully decoupled from the oracle).

- [ ] **Step 4: Build + test huck-syntax**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1`
Expected: all pass, 0 warnings. Count stays ≈1042 (call-sites unchanged; only helper bodies + 4 deleted non-`#[test]` helpers). No test flips: every `diff_cmd` input already had `new_seq(s) == Ok` (it `.unwrap()`ed it) and every `diff_err` input already errored.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v265 T3: smoke-convert the differential harness (decouple from the oracle)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Delete the oracle (compiler-guided)

Nothing calls `command::parse`, `tokenize`/`tokenize_with_opts`, or `from_tokens` now. Remove them and follow the `dead_code` cascade until only shared code survives.

**Files:**
- Modify: `crates/huck-syntax/src/command.rs` (remove `parse`, `parse_one_unit`, `parse_cursor` at 782-804 + the recursive-descent tree they reach)
- Modify: `crates/huck-syntax/src/lexer.rs` (remove `tokenize`/`tokenize_with_opts`/`from_tokens`, the `Vec<Token>` replay, `Mode::Command`'s non-atom branch, the 6 forward-scanners, and transitively-dead helpers)

**Interfaces:**
- Consumes: nothing (deletion).
- Produces: `command.rs` and `lexer.rs` with the oracle gone; all shared types/helpers/atom-scanners intact.

- [ ] **Step 1: Remove the oracle entry points**

In `command.rs`, delete `pub fn parse` (796), `pub fn parse_one_unit` (804), and `fn parse_cursor` (782). In `lexer.rs`, delete `pub fn tokenize`, `pub fn tokenize_with_opts`, and `pub fn from_tokens` (and the constructor path they use). Do NOT yet delete anything else.

- [ ] **Step 2: Build and read the dead_code cascade**

Run: `cargo build -p huck-syntax 2>&1 | grep -E "never used|dead_code|error" | head -60`
Expected: a list of `function is never used` warnings (the recursive-descent oracle fns in command.rs: `parse_command_then_pipeline`, `parse_sequence` [the command.rs one, NOT parser::parse_sequence], `parse_command`, `parse_command_inner`, `parse_function_def`, `parse_if`, `parse_for_*`, `parse_case`, `parse_while`, `parse_pipeline`, … ; and in lexer.rs: `scan_step_command`, `scan_dollar_expansion`, `scan_arith_body`, `scan_backtick_body`, `scan_braced_param_expansion`, `scan_legacy_arith_body`, `emit_word_with_braces`, likely `collect_heredoc_bodies`, and the `Mode::Command` non-atom dispatch). If there are hard `error`s (a shared helper you removed by hand), restore that helper — only delete what the compiler reports as never-used.

- [ ] **Step 3: Delete the reported dead functions, iterate**

Delete every function the build reports as `never used`. Rebuild. Repeat: each deletion may make more code dead. Continue until `cargo build -p huck-syntax 2>&1 | grep -c "never used"` prints `0`.

Guard rails:
- If a warning names a function the ATOM path still needs, that is impossible by construction (it would still be referenced, so not dead) — but if a `pub`/`pub(crate)` item is reported dead only because it was `pub` for the oracle, confirm no atom-path or engine caller with `grep -rn "<name>" crates/` before deleting. The AST types and the pure predicate functions listed in the spec (`is_assignment_word`, `valid_function_name_text`, etc.) MUST remain referenced — if one shows as dead, stop and investigate (a caller was wrongly removed).
- `Mode::Command`'s non-atom branch: the `scan_step` dispatch that calls `scan_step_command` when `!self.command_atoms`. Since `command_atoms` is now always effectively true in every live path, remove the `!command_atoms` arm. Verify no remaining `command_atoms == false` constructor exists (the deleted `from_tokens` set it false).

- [ ] **Step 4: Full green + 0 warnings, both crates**

Run: `cargo build -p huck-syntax 2>&1 | tail -3` → 0 warnings.
Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → all pass.
Run: `cargo build -p huck-engine 2>&1 | tail -3` → 0 warnings.
Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` → 1738 pass.

- [ ] **Step 5: bash-diff sweep (behavior unchanged)**

Run (guarded, per harness):
```bash
cargo build -p huck 2>&1 | tail -2
export HUCK_BIN=$(pwd)/target/debug/huck
pass=0; fail=0
for f in tests/scripts/*_diff_check.sh; do
  if ( ulimit -v 1500000; timeout 90 bash "$f" ) >/dev/null 2>&1; then pass=$((pass+1)); else fail=$((fail+1)); echo "FAIL: $f"; fi
done
echo "harness files: pass=$pass fail=$fail"
```
Expected: only `funcnest` fails (pre-existing L-63). If the box struggles with the full sweep, run harnesses in small batches — do NOT run unguarded.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-syntax/src/command.rs crates/huck-syntax/src/lexer.rs
git commit -m "v265 T4: delete the oracle parser + 6 forward-scanners + tokenize/from_tokens

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Module tidy — command.rs = AST, lexer.rs = token production

Move parser-internal helpers into parser.rs so `command.rs` holds only the AST (types + pure predicates) and `lexer.rs` holds only token-production code.

**Files:**
- Modify: `crates/huck-syntax/src/command.rs`, `crates/huck-syntax/src/parser.rs`, `crates/huck-syntax/src/lexer.rs`

**Interfaces:**
- Consumes/Produces: same functions, new home. Call-sites drop the `crate::command::` / `crate::lexer::` prefix where they become module-local.

- [ ] **Step 1: Verify caller sets for the command→parser candidates**

For each of `next_is_redirect`, `build_redirections`, `next_is_test_binary_operator`, `skip_test_newlines`, `dup_op`:
Run: `grep -rn "\b<name>\b" crates/huck-syntax/src crates/huck-engine/src`
Expected: callers only in `parser.rs` (and the def in `command.rs`). Any helper with a huck-engine caller STAYS in `command.rs` — note it and skip its move.

- [ ] **Step 2: Move the parser-only helpers command.rs → parser.rs**

Cut each confirmed parser-only helper from `command.rs`, paste into `parser.rs` (near the other parse helpers), and drop the `crate::command::` prefix at its parser.rs call-sites. Keep the pure predicates (`is_assignment_word`, `try_split_assignment`, `valid_function_name_text`, `valid_identifier_text`, `is_function_body_shape`, `is_compound_opener`, `try_unary_op`, `is_bang_word`, `word_literal_text`, `is_redirect_op`, `lit_word`, `slots_for_simple_path`) in `command.rs`.

- [ ] **Step 3: Build + test after the command→parser move**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → pass, 0 warnings.
Run: `cargo build -p huck-engine 2>&1 | tail -3` → 0 warnings (no engine caller of moved helpers).

- [ ] **Step 4: Commit the command→parser move**

```bash
git add crates/huck-syntax/src/command.rs crates/huck-syntax/src/parser.rs
git commit -m "v265 T5a: move parser-internal helpers command.rs -> parser.rs

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 5: Move `brace_expand_parts` + `word_contains_unquoted_brace` lexer.rs → parser.rs**

Confirm callers are parser-only now that `emit_word_with_braces` is gone:
Run: `grep -rn "brace_expand_parts\|word_contains_unquoted_brace" crates/huck-syntax/src crates/huck-engine/src`
Expected: callers only in `parser.rs` (2 sites) + their tests. Cut both functions (and their unit tests `brace_expand_parts_literal_splits`, `brace_expand_parts_no_brace_passthrough`) from `lexer.rs`, paste into `parser.rs`, drop the prefix at call-sites.

- [ ] **Step 6: Full green + 0 warnings**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → pass, 0 warnings.
Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` → 1738 pass.

- [ ] **Step 7: Commit the lexer→parser move**

```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs
git commit -m "v265 T5b: move brace_expand_parts + word_contains_unquoted_brace lexer.rs -> parser.rs

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Focused, oracle-independent lexer + parser tests

Backfill AST-shape coverage the smoke-convert dropped, with explicit expected values across grammar families. Curated representative set (dozens per module), not a re-encoding of the smoke corpus.

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (`mod tests`) — add a token-stream test group
- Modify: `crates/huck-syntax/src/parser.rs` (`mod tests`) — add an AST test group

**Interfaces:**
- Consumes: `Lexer::new_live_atoms`, `Lexer::next_token`, `parser::parse_sequence`, `TokenKind`, `Sequence`/`Command`/`WordPart` (all `Debug + PartialEq`).

- [ ] **Step 1: Add lexer token-stream tests (exact atom `Vec`)**

In `lexer.rs` `mod tests`, reuse the existing `command_atoms_of(s) -> Vec<TokenKind>` pattern (or `head_atoms`). Add explicit-expected tests, e.g.:

```rust
    #[test]
    fn atom_stream_simple_command() {
        assert_eq!(
            command_atoms_of("echo hi"),
            vec![
                TokenKind::Word(words_of("echo")),
                TokenKind::Blank,
                TokenKind::Word(words_of("hi")),
            ],
        );
    }
```

(Use whatever `Word`-constructor the existing token-stream tests use — see `command_atoms_stream_shape` at lexer.rs:12786 for the exact expected-value idiom, and mirror it.) Cover one to three cases in each mode: quotes (`'…'`, `"…"`, `$'…'`), `${…}` (plain + an operator like `${x:-y}`), `$(…)`, backtick, `$((…))`, `$[…]`, `[[ … =~ re ]]`, extglob (`@(a|b)` with `extglob:true` opts), array literal (`a=(1 2)`), heredoc body (literal + expanding), process sub (`<(cmd)`), brace expansion (`{a,b}`), redirect ops (`>`, `>>`, `2>&1`, `<<<`).

- [ ] **Step 2: Run the lexer tests, verify they pass**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 atom_stream`
Expected: PASS. If an expected `Vec` is wrong, correct the expected value to the atom stream the (production, bash-validated) lexer emits — but reason about what it SHOULD be first; a surprising stream is a finding to note, not auto-accept.

- [ ] **Step 3: Add parser AST tests (exact `Sequence`/`Command`)**

In `parser.rs` `mod tests`, add explicit-AST tests using `new_seq(s)`, e.g.:

```rust
    #[test]
    fn ast_pipeline_two_stages() {
        let seq = new_seq("a | b").unwrap().unwrap();
        // Assert the exact shape: a Pipeline of two simple commands, not negated.
        match seq.first {
            Command::Pipeline(p) => {
                assert!(!p.negated);
                assert_eq!(p.commands.len(), 2);
            }
            other => panic!("expected Pipeline, got {other:?}"),
        }
        assert!(seq.rest.is_empty());
    }
```

Prefer full-value `assert_eq!` against a constructed expected `Sequence` where the AST is small enough to write out; fall back to structural matching (as above) for large nodes. Cover: simple command (program + args), pipeline (+ negation `! a`), redirects (`>`, `>>`, `<`, `2>&1`, heredoc, `<<<`), and-or (`a && b || c`), subshell `( … )`, brace group `{ …; }`, `if`/`while`/`until`/`for`/`select`/`case`, C-for `for ((…))`, arith command `(( … ))`, `[[ … ]]` + `=~`, function defs (`f() { … }` and `function f { … }`), coproc, assignment + array literal, and word-part nesting (a `${…}`, a `$(…)`, an `$((…))`, a backtick — assert the `WordPart` variants).

- [ ] **Step 4: Run the parser tests, verify they pass**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 ast_`
Expected: PASS.

- [ ] **Step 5: Full suite green**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → all pass (count up by the number of new tests), 0 warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs
git commit -m "v265 T6: focused oracle-independent lexer token-stream + parser AST tests

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final Verification (before the whole-branch review + merge)

- `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` — all pass, 0 warnings.
- `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` — 1738 pass, 0 warnings.
- Guarded bash-diff sweep — 1688 pass / 1 fail (`funcnest`).
- `grep -rn "command::parse\|fn tokenize\|from_tokens\|scan_step_command\b\|scan_dollar_expansion\|scan_arith_body\|scan_backtick_body\|scan_braced_param_expansion\|scan_legacy_arith_body\|emit_word_with_braces" crates/` — no live references (only possibly the atom `scan_step_command_atoms`, which is distinct and stays).
- `command.rs` contains no token-consuming code; `lexer.rs` contains no word-assembly/brace-expansion code; `parser.rs` owns both.

## Self-Review notes

- **Spec coverage:** Goal 1 (delete oracle parser) → T4. Goal 2 (delete scanners + tokenize/from_tokens) → T4. Goal 3 (port continuation) → T1. Goal 4 (smoke-convert harness) → T3. Goal 5 (module tidy) → T5. Goal 6 (focused tests) → T6. Test-helper repoint (mechanism prerequisite) → T2.
- **Ordering:** T1–T3 remove all oracle callers before T4 deletes it; T5 needs T4's dead code gone; T6 last.
- **Type consistency:** repoint pattern uses `parser::parse_sequence(&mut Lexer::new_live_atoms(SRC, &Default::default(), LexerOptions::default()))` everywhere (T1, T2); `Result<Option<Sequence>, ParseError>` return type matches the old `command::parse`.
