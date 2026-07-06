# v264 THE FLIP — atom path becomes the production parser Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Repoint the two production command-EXECUTION entry points (`shell.rs process_line_in_sinks` + `builtins.rs run_sourced_contents_in_sinks`) from the `command.rs` oracle (`command::parse` / `command::parse_one_unit`) to the atom-command path (`Lexer::new_live_atoms` + `parser::parse_sequence` / a NEW `parser::parse_one_unit`), so production command execution runs on the atom parser. Delete NOTHING.

**Architecture:** T1 adds an atom `parse_one_unit` (newline-stop mode in `parse_and_or`) + a differential `old_unit`/`new_unit` — huck-syntax only. T2 makes the two fns `pub`, re-exports `parser` from huck-engine, and repoints the two call sites — THE flip. T3 re-judges the documented atom≠oracle pins against bash now that the atom side is production. The oracle, the 6 forward-scanning scanners, and the differential harness all stay RESIDENT (they remain the atom path's substitution-body engine + the AST-level safety net; deletion is milestone 2, out of scope).

**Tech Stack:** Rust workspace (crates huck-syntax, huck-engine).

## Global Constraints

- **`command.rs` diff-vs-`main` = EMPTY.** The oracle is retained untouched. The 6 scanners (`scan_step_command` / `scan_dollar_expansion` / `scan_arith_body` / `scan_legacy_arith_body` / `scan_backtick_body` / `scan_braced_param_expansion`) are retained. NO deletion in v264.
- Box is 1 core / 1.9 GB. Test per-crate single-threaded ONLY: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` and `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. NEVER `--workspace`, NEVER multi-threaded — it OOM-kills the session.
- `cargo build -p huck-syntax` and `-p huck-engine` → 0 warnings.
- `diff_cmd`/`diff_err` in parser.rs assert `new_seq(s) == old_seq(s)`; `old_seq` uses `.expect("lex")`, so lex-error inputs PANIC — keep corpus inputs parse-clean.
- Commit trailer VERBATIM: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- rust-analyzer PHANTOM diagnostics — trust `cargo`.

---

### Task 1: Atom `parse_one_unit` + newline-stop mode + `old_unit`/`new_unit` differential

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` — `parse_and_or` (~2572; split into a `_opts` variant + wrapper, add the newline-stop arm), add `parse_one_unit` (~after `parse_sequence`), + differential harness & test in `mod tests`.

**Interfaces:**
- Consumes: `parse_command_then_pipeline`, `skip_newlines`, `collect_heredoc_bodies_after_newline`, `fill_sequence`, `iter.take_heredoc_bodies()`, `iter.peek_kind()?`, `TokenKind::Newline`/`Blank`, `Keyword`, `Sequence`, `ParseError` — all already in scope in parser.rs. In tests: `tokenize_with_opts`, `Lexer::from_tokens`, `Lexer::new_live_atoms`, `crate::command::parse_one_unit`.
- Produces: `pub fn parse_one_unit(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError>` (used by T2).

- [ ] **Step 1: Write the failing test**

Add to `crates/huck-syntax/src/parser.rs mod tests` (near the other differential helpers/tests):
```rust
    // ── v264 parse_one_unit differential ─────────────────────────────────────
    // Drive BOTH the oracle `command::parse_one_unit` and the atom
    // `parse_one_unit` in a loop over the same script, comparing unit-by-unit.
    fn old_unit(s: &str) -> Vec<Result<Option<Sequence>, ParseError>> {
        let toks = tokenize_with_opts(s, LexerOptions::default()).expect("lex");
        let mut lx = Lexer::from_tokens(toks);
        drive_units(&mut |i: &mut Lexer| crate::command::parse_one_unit(i), &mut lx)
    }
    fn new_unit(s: &str) -> Vec<Result<Option<Sequence>, ParseError>> {
        let mut lx = Lexer::new_live_atoms(s, &Default::default(), LexerOptions::default());
        drive_units(&mut super::parse_one_unit, &mut lx)
    }
    fn drive_units(
        f: &mut dyn FnMut(&mut Lexer) -> Result<Option<Sequence>, ParseError>,
        lx: &mut Lexer,
    ) -> Vec<Result<Option<Sequence>, ParseError>> {
        let mut out = Vec::new();
        loop {
            let r = f(lx);
            let stop = matches!(r, Ok(None) | Err(_));
            out.push(r);
            if stop { break; }
        }
        out
    }
    fn diff_unit(s: &str) {
        assert_eq!(new_unit(s), old_unit(s), "parse_one_unit mismatch for {s:?}");
    }

    #[test]
    fn atoms_parse_one_unit_matches_oracle() {
        diff_unit("a\nb\nc");              // three units on three lines
        diff_unit("a; b\nc");             // `;` stays intra-unit; newline splits
        diff_unit("a && b\nc || d");      // connectors intra-unit
        diff_unit("a &\nb");             // background then newline
        diff_unit("\n\na\n\nb\n");        // leading/among/trailing blank lines
        diff_unit("a\n");                // single unit, trailing newline
        diff_unit("");                    // empty → one Ok(None)
        diff_unit("   \n  a  \n");        // blank-ish lines + surrounding blanks
        diff_unit("if x; then y; fi\nz"); // compound spanning `;`, then next unit
        diff_unit("f() {\n:\n}\ng");      // compound spanning NEWLINES, then next unit
        diff_unit("for i in 1 2; do echo $i; done\ndone_marker");
        diff_unit("cat <<EOF\nhi $x\nEOF\necho next"); // heredoc body drained in-unit
        diff_unit("cat <<'EOF'\nlit\nEOF\nafter");     // literal heredoc, then next unit
        diff_unit("a | b\nc");            // pipeline intra-unit
    }
```

- [ ] **Step 2: Run the test to verify it FAILS**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_parse_one_unit_matches_oracle -- --test-threads 1`
Expected: FAIL to COMPILE — `super::parse_one_unit` does not exist yet (the atom `parse_one_unit` is added in Step 4).

- [ ] **Step 3: Split `parse_and_or` into a `_opts` variant with the newline-stop arm**

In `crates/huck-syntax/src/parser.rs`, change the `parse_and_or` definition (~2572) from:
```rust
pub(crate) fn parse_and_or(iter: &mut Lexer, stop_at: &[Keyword]) -> Result<Sequence, ParseError> {
```
to a private `_opts` variant plus a thin wrapper preserving the existing 2-arg signature (so the 3 existing callers at ~2967/3011/3417 are byte-unchanged):
```rust
pub(crate) fn parse_and_or(iter: &mut Lexer, stop_at: &[Keyword]) -> Result<Sequence, ParseError> {
    parse_and_or_opts(iter, stop_at, false)
}

/// The shared body of [`parse_and_or`]. When `stop_at_top_newline` is set, a
/// top-level `TokenKind::Newline` terminates the command UNIT (used by
/// [`parse_one_unit`] for the non-interactive script reader); otherwise a
/// top-level newline is a Semi-like continue connector. Mirrors the oracle's
/// `command::parse_sequence_opts`.
fn parse_and_or_opts(
    iter: &mut Lexer,
    stop_at: &[Keyword],
    stop_at_top_newline: bool,
) -> Result<Sequence, ParseError> {
```
(i.e. rename the ORIGINAL function body to `parse_and_or_opts` with the extra `stop_at_top_newline` param, and add the 2-arg `parse_and_or` wrapper above it.)

Then, inside `parse_and_or_opts`, in the `TokenKind::Op(Operator::Semi) | TokenKind::Newline =>` arm (~2646), add the stop-check as the FIRST statement of the arm (before `skip_newlines(iter)?;`):
```rust
            TokenKind::Op(Operator::Semi) | TokenKind::Newline => {
                // v264 unit mode: a top-level NEWLINE ends the command unit
                // (already consumed as `token`). Drain any heredoc-body atom
                // groups the lexer emitted for THIS unit's line — the atom path
                // emits them after the newline, unlike the oracle which
                // pre-collects during tokenization — so `fill_sequence` can
                // attach them; then end the unit WITHOUT skipping inter-unit
                // newlines or parsing the next command. `;` still separates
                // within a unit.
                if stop_at_top_newline && matches!(token, TokenKind::Newline) {
                    collect_heredoc_bodies_after_newline(iter)?;
                    break;
                }
                skip_newlines(iter)?;
                // ... rest of the arm unchanged ...
```

- [ ] **Step 4: Add the atom `parse_one_unit`**

In `crates/huck-syntax/src/parser.rs`, immediately AFTER the `parse_sequence` function, add:
```rust
/// v264: parse ONE top-level command unit from the atom stream, stopping at
/// (and consuming) the next top-level newline or EOF. Skips leading blank
/// lines. Returns `Ok(None)` when only newlines/blanks/EOF remain. The atom
/// analog of `command::parse_one_unit`, used by the non-interactive script
/// reader (`run_sourced_contents_in_sinks`).
pub fn parse_one_unit(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError> {
    // Discard any heredoc bodies leaked by a prior unit that errored after
    // pushing them (mirrors parse_sequence's CF2 hygiene). take_heredoc_bodies
    // drains only on the success path below, so on a clean loop this is a no-op.
    let _ = iter.take_heredoc_bodies();
    // Skip leading Newline/Blank atoms (and any heredoc-body groups) — mirrors
    // parse_sequence's leading skip and the oracle's leading-newline skip.
    skip_newlines(iter)?;
    if iter.peek_kind()?.is_none() {
        return Ok(None);
    }
    let mut seq = parse_and_or_opts(iter, &[], true)?;
    // Attach heredoc bodies collected for this unit (no stray-terminator check —
    // more units may follow; the caller loops).
    let mut bodies = iter.take_heredoc_bodies().into_iter();
    fill_sequence(&mut seq, &mut bodies);
    Ok(Some(seq))
}
```

- [ ] **Step 5: Run the test to verify it PASSES**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_parse_one_unit_matches_oracle -- --test-threads 1`
Expected: PASS (all `diff_unit` cases byte-identical to the oracle loop).

If a case fails, inspect the per-unit `Vec` mismatch. Likely culprits: the heredoc-in-unit case (verify `collect_heredoc_bodies_after_newline` drains the body BEFORE the break so `fill_sequence` attaches it), or a blank-line/EOF boundary (verify the leading `skip_newlines` + `Ok(None)` reduction matches the oracle's leading-newline skip). Do NOT weaken a test — fix the mechanism.

- [ ] **Step 6: Full huck-syntax suite + gates**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → all green (the existing `diff_cmd`/`diff_err`/pins must still pass — `parse_and_or`'s 2-arg wrapper keeps every existing caller byte-identical).
Run: `cargo build -p huck-syntax` → 0 warnings.
Run: `git diff --stat main -- crates/huck-syntax/src/command.rs` → EMPTY.

- [ ] **Step 7: Commit**

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v264 T1: atom parse_one_unit (newline-stop mode) + old_unit/new_unit differential

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Wiring + repoint the two execution paths (THE flip)

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` — make `parse_sequence` and `parse_one_unit` `pub`.
- Modify: `crates/huck-engine/src/lib.rs:53` — add `parser` to the re-export.
- Modify: `crates/huck-engine/src/shell.rs` — `process_line_in_sinks` parse block (~384-385).
- Modify: `crates/huck-engine/src/builtins.rs` — `run_sourced_contents_in_sinks` (~6330 + ~6380).

**Interfaces:**
- Consumes: `parser::parse_sequence`, `parser::parse_one_unit` (from T1), `lexer::Lexer::new_live_atoms`. In huck-engine, reached as `crate::parser::…` via the new re-export.
- Produces: no new interface; production now parses via the atom path.

- [ ] **Step 1: Make the two atom fns `pub`**

In `crates/huck-syntax/src/parser.rs`:
- `pub(crate) fn parse_sequence(iter: &mut Lexer)` → `pub fn parse_sequence(iter: &mut Lexer)`.
- `parse_one_unit` is already `pub` (from T1). Confirm.

- [ ] **Step 2: Re-export `parser` from huck-engine**

In `crates/huck-engine/src/lib.rs:53`, change:
```rust
pub use huck_syntax::{brace_expand, command, generate, lexer};
```
to:
```rust
pub use huck_syntax::{brace_expand, command, generate, lexer, parser};
```

- [ ] **Step 3: Repoint `shell.rs process_line_in_sinks`**

In `crates/huck-engine/src/shell.rs`, in `process_line_in_sinks` (~384-385), change:
```rust
    let mut lx = lexer::Lexer::new_live(line, aliases, opts);
    match command::parse(&mut lx) {
```
to:
```rust
    let mut lx = lexer::Lexer::new_live_atoms(line, aliases, opts);
    match parser::parse_sequence(&mut lx) {
```
Leave the `Ok(Some(sequence))` / `Ok(None)` / `Err(e)` arms unchanged.

- [ ] **Step 4: Repoint the `builtins.rs` source loop**

In `crates/huck-engine/src/builtins.rs`, in `run_sourced_contents_in_sinks`:
- change the lexer construction (~6330):
```rust
        let mut iter = crate::lexer::Lexer::new_live(&contents[start..], &aliases_now, opts);
```
to:
```rust
        let mut iter = crate::lexer::Lexer::new_live_atoms(&contents[start..], &aliases_now, opts);
```
- change the parse call (~6380):
```rust
            match crate::command::parse_one_unit(&mut iter) {
```
to:
```rust
            match crate::parser::parse_one_unit(&mut iter) {
```
Leave everything else (`set_base_line`, the newline-skip peek loop, `cursor_pos()`/`peek_span()` offset tracking, `set_aliases` between units, the lex-error branch, the `Ok(None)`/`Ok(Some(seq))` arms) unchanged.

- [ ] **Step 5: Build + run the behavioral suites**

Run: `cargo build -p huck-engine` → 0 warnings.
Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` → all green. This is THE proof: the huck-engine executor/builtin/shell tests now run through the repointed atom path.
Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → still green.

If a huck-engine test fails, it is a REAL production-behavior divergence introduced by the flip — STOP and report it with the failing test + the atom-vs-oracle difference (do NOT weaken the test). The differential harness (T1 / existing) should have caught most; a huck-engine-only failure points at a path the differential corpus misses (alias refresh between units, `$LINENO`/offset tracking, trap/eval/PROMPT_COMMAND routing, `-c`/script-file execution).

- [ ] **Step 6: Commit**

```bash
git add crates/huck-syntax/src/parser.rs crates/huck-engine/src/lib.rs crates/huck-engine/src/shell.rs crates/huck-engine/src/builtins.rs
git commit -m "v264 T2: flip production execution onto the atom path (shell.rs + source loop)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Pin re-evaluation vs bash + doc updates

**Files:**
- Modify (comments only): `crates/huck-syntax/src/parser.rs` — reword the now-false "dormant / no production impact" comments on the pin tests.
- Modify (if a genuine divergence): `docs/bash-divergences.md`.

**Interfaces:** none (verification + documentation task).

- [ ] **Step 1: Probe each pin against bash**

Build the huck binary (`cargo build -p huck-cli` or the workspace binary) and, for each pin input, compare `bash -c '<fragment>'` output/behavior against `huck -c '<fragment>'`:
- `atoms_heredoc_redirect_target_before_arg_pin` (parser.rs ~5049) — its two inputs (multi-heredoc redirect-target-before-arg order).
- backtick backslash-run decode (parser.rs ~6172, the 4 `new_bt`/`old_bt` inputs).
- `atoms_legacy_arith_backslash_quote_carryforward` (parser.rs ~5767) — `\'`/`\"` in `$[ ]`.
- `atoms_legacy_arith_quote_backslash_carryforward` (~5736) and `atoms_arith_for_header_semi_in_subexpansion_carryforward` (~5614) and `atoms_regex_glued_redir_carryforward` (CF5) — confirm the atom's now-production behavior.
Record for each: does the atom side match bash, diverge-but-both-reject, or is it closer-to-bash than the oracle was?

- [ ] **Step 2: Accept or record**

For each pin: if the atom behavior matches bash / both-reject / is closer-to-bash → **accept**: keep the test, reword its comment to state it is now PRODUCTION behavior judged against bash (not a dormant oracle divergence). If a pin makes production genuinely diverge from bash in a non-exotic way → **record** a new `[intentional]` or `[deferred]` entry in `docs/bash-divergences.md` with the exact fragment and bash's behavior. (Expectation from the design: all accept — these were chosen as exotic edges where the atom errors safely or tracks bash.)

- [ ] **Step 3: Commit**

```bash
git add crates/huck-syntax/src/parser.rs docs/bash-divergences.md
git commit -m "v264 T3: re-judge atom-path pins against bash (now production behavior)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Branch-level gate (before merge — not a task)

- Build the huck binary and run every `tests/scripts/*_diff_check.sh` (bash vs huck byte-identical). All must pass — the gold-standard bash-compat gate, now exercising the atom path in the real binary.
- Opus whole-branch review: probe source-loop edges (alias refresh between units, `$LINENO` through the atom path, per-unit error isolation, `-c`/script-file/`source` execution), trap/eval/PROMPT_COMMAND routing through `process_line`, and any huck-engine behavior the differential corpus can't reach.

## Self-Review

- **Spec coverage:** atom `parse_one_unit` + newline-stop → T1; wiring + repoint the two execution paths → T2; pin re-eval vs bash → T3; 3-layer verification → T1 Step 6 + T2 Step 5 + branch gate. `command.rs` EMPTY-diff → T1 Step 6. ✓
- **Placeholder scan:** none. Fix code and all test inputs verbatim. (Line numbers are `~` approximations — anchor by the quoted code, not the number.)
- **Type consistency:** `parse_and_or_opts(iter, stop_at, stop_at_top_newline: bool)`, `parse_one_unit(iter) -> Result<Option<Sequence>, ParseError>`, `parser::parse_sequence` / `parser::parse_one_unit`, `Lexer::new_live_atoms`, `collect_heredoc_bodies_after_newline` — all match existing signatures in parser.rs / command.rs / lexer.rs. The `parser` module is already `pub mod` in huck-syntax lib.rs:63.
