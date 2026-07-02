# v251 — Process substitution (`<(…)`/`>(…)`) on the atom-command path — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the dormant atom-command parser handle process substitution (`<(cmd)`/`>(cmd)`) as a word part, producing ASTs byte-identical to the `command.rs` oracle, by reusing the existing `Mode::CommandSub` body machinery.

**Architecture:** The lexer disambiguates `<`/`>` with a one-char peek: `<`+`(` → a zero-width `ProcSubOpen { dir }` word-part signal; anything else → the existing redirect operators. The procsub body is a paren-delimited command sequence identical to a `$(…)` body, so it reuses `Mode::CommandSub` + `parse_subshell_sequence`. The parser adds a `parse_process_sub` (mirroring `parse_command_sub`) and a `ProcSubOpen` arm in the word assembler.

**Tech Stack:** Rust, `crates/huck-syntax` (`lexer.rs`, `parser.rs`). No new dependencies.

## Global Constraints

- Test ONLY with `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` (narrow with a filter). This box (1 core / ~1.9 GiB) is OOM-KILLED by `cargo test --workspace` or ANY parallel/multi-threaded test run — NEVER run those. `cargo build` is fine.
- Byte-identical: every in-scope procsub input parses to the SAME AST / same error on the atom path as the oracle (`diff_cmd` / error parity). A well-formed in-scope divergence is a v251 BUG to fix, not to pin.
- PRODUCTION UNTOUCHED: `command_atoms` defaults `false`; the production procsub scanner (`lexer.rs` ~2505/2555), `scan_paren_substitution`, `command.rs`, and `process_line` are UNCHANGED. Engine-facing `WordPart::ProcessSub { sequence, dir }` / `ProcDir` AST UNCHANGED.
- The `<`/`>` operator-arm change must NOT affect non-`(` redirects (`<`, `<<`, `<<<`, `<&`, `<>`, `>`, `>>`, `>&`, `>|`) — the existing redirect corpus stays green.
- rust-analyzer/IDE diagnostics can be phantom — trust `cargo`. A hang with growing memory = a real non-progress loop in your code, not a box limit — fix it.
- 0 warnings (`cargo build -p huck-syntax 2>&1 | grep -c warning` → `0`).
- Every commit ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Branch: `v251-process-substitution`. Do NOT commit to `main`; do NOT push/rebase.

## Key existing anchors (verify line numbers with grep — they drift)

- **`TokenKind` enum** — `lexer.rs` ~420 (`CmdSubOpen`). Add `ProcSubOpen { dir: ProcDir }` near it.
- **`ProcDir`** — `pub enum ProcDir` (`lexer.rs:297`), variants `In`/`Out`. `WordPart::ProcessSub { sequence: crate::command::Sequence, dir: ProcDir }` (`lexer.rs:327`).
- **`scan_command_operator_atom`** — `lexer.rs:3107`; entry does `let first = self.cursor.next()` (consumes the `<`/`>`), so on the procsub branch the cursor sits on `(`. The `<` arm is at :3154 (`match self.cursor.peek() { Some('<')…, Some('&')…, Some('>')…, _ => RedirIn }`); the `>` arm at :3184 (`Some('>')…, Some('&')…, Some('|')…, _ => RedirOut`). The function ends with an UNCONDITIONAL `self.boundary_reset(); Ok(Step::Produced)` at :3192.
- **`boundary_reset`** — sets `cmd_at_word_start = true`, `in_assignment_value = false`, `assign_val_tilde_ok = false`. Procsub must NOT run it (it's a word continuation, not a boundary).
- **`scan_step_command_sub`** — `lexer.rs:1681`; the `!body_started` opener branch requires the cursor on `$`, consumes `$(`, emits `CmdSubOpen`, flips `body_started` (with the `$((` arith → `DeferredExpansion` sub-case); when `body_started` it calls `self.scan_step_command()` and the parser owns `)`.
- **`parse_command_sub`** — `parser.rs:765` (the template for `parse_process_sub`).
- **`parse_word_command`** — `parser.rs:118`; its `CmdSubOpen` arm at :161 (`iter.next_kind()?` to discard the signal, then `parts.push(parse_command_sub(iter, quoted)?)`).
- **`parse_simple_with_leading_word` word-start set** — `parser.rs:1620-1634` (the `matches!(peek, Some(Lit|DollarLit|QuoteRun|DollarName|ParamOpen|CmdSubOpen|BeginBacktick|ArithOpen|Tilde|BeginDquote|AssignPrefix))` that gates the call to `parse_word_command`). A FRESH-WORD procsub (`cat <(x)`) needs `ProcSubOpen` added here, else it falls to the `_ => UnsupportedCommand` arm at :1646.
- **`parse_one_redirect` target** — `parser.rs:1491` (`parse_word_command(iter, false)?`) — redirect-target procsub (`wc < <(x)`) routes through here for free once `parse_word_command` has the arm.
- **Current deferral** — `atoms_procsub_deferred` (`parser.rs:3154`): today `<(` lexes as `Op(RedirIn)` + `Op(LParen)` and the parser rejects the procsub. Once the lexer emits `ProcSubOpen` (Task 1) and the parser handles it (Task 2), the old reject path is simply never reached; retarget the test to `diff_cmd`.

## File Structure

- `crates/huck-syntax/src/lexer.rs` — the `ProcSubOpen` atom; the `<`/`>` operator-arm `(`-branch; the `scan_step_command_sub` bare-`(` opener; lexer atom-stream unit tests.
- `crates/huck-syntax/src/parser.rs` — `parse_process_sub`; the `parse_word_command` `ProcSubOpen` arm; the `parse_simple_with_leading_word` word-start-set addition; the differential tests.

---

### Task 1: Lexer — `ProcSubOpen` atom + `<`/`>` disambiguation + `Mode::CommandSub` bare-`(` opener (dormant)

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs`
- Test: `crates/huck-syntax/src/lexer.rs` `mod tests`

**Interfaces:**
- Produces: `TokenKind::ProcSubOpen { dir: ProcDir }` (zero-width word-part signal, cursor left on `(`); `scan_step_command_sub` accepts a bare `(` opener (procsub) in addition to `$(`.

- [ ] **Step 1: Add the atom.** In `pub enum TokenKind` (near `CmdSubOpen`, lexer.rs ~420):

```rust
    ProcSubOpen { dir: ProcDir },  // v251: `<(`/`>(` word-part signal (unquoted); parser assembles WordPart::ProcessSub via Mode::CommandSub. Cursor is left on `(`.
```

- [ ] **Step 2: Write the failing lexer unit test** (in `lexer.rs` `mod tests`, near `heredoc_opener_atom_parses_delim`; use the same `command_atoms_of` helper the heredoc/here-string atom tests use):

```rust
    #[test]
    fn procsub_open_atoms_disambiguate() {
        // `<(`/`>(` → ProcSubOpen signal (NOT a redirect op); the `(` is NOT consumed.
        let a = command_atoms_of("cat <(echo hi)");
        assert!(a.iter().any(|t| matches!(t, TokenKind::ProcSubOpen { dir: ProcDir::In })),
            "expected ProcSubOpen In, got {a:?}");
        assert!(!a.iter().any(|t| matches!(t, TokenKind::Op(Operator::RedirIn))),
            "`<(` must NOT emit RedirIn: {a:?}");
        let b = command_atoms_of("tee >(cat)");
        assert!(b.iter().any(|t| matches!(t, TokenKind::ProcSubOpen { dir: ProcDir::Out })),
            "expected ProcSubOpen Out, got {b:?}");
        // Non-`(` `<`/`>` are unaffected.
        let r = command_atoms_of("cat < f");
        assert!(r.iter().any(|t| matches!(t, TokenKind::Op(Operator::RedirIn))), "plain `<` still RedirIn: {r:?}");
        let rr = command_atoms_of("echo >> f");
        assert!(rr.iter().any(|t| matches!(t, TokenKind::Op(Operator::RedirAppend))), "`>>` still RedirAppend: {rr:?}");
    }
```

(If `command_atoms_of` stops collecting at a `ProcSubOpen`/mode boundary, extend its stop set minimally so the test can observe the signal — but do NOT make it drive the CommandSub body. Report any helper change.)

- [ ] **Step 3: Run, expect FAIL.** `cargo test -p huck-syntax --jobs 1 --lib procsub_open_atoms_disambiguate -- --test-threads 1`. Expected: FAIL (today `<(` emits `RedirIn` + `LParen`).

- [ ] **Step 4: Emit `ProcSubOpen` from the `<`/`>` arms.** In `scan_command_operator_atom` (lexer.rs:3154), add a `(`-branch to the `<` arm's inner `match` (before the `_ =>` fallthrough) that emits the signal and RETURNS EARLY (skipping the `boundary_reset` tail — procsub is a word continuation):

```rust
            '<' => match self.cursor.peek().copied() {
                Some('(') => {
                    // v251: `<(` process substitution. Zero-width word-part
                    // signal; DON'T consume `(` (Mode::CommandSub consumes it).
                    // Word continuation, so no boundary_reset: mark that a word
                    // has started (mirrors scan_command_word_atom emitting a Lit).
                    self.history.push(Token::new(TokenKind::ProcSubOpen { dir: ProcDir::In }, Span::new(off, l, c)));
                    self.cmd_at_word_start = false;
                    return Ok(Step::Produced);
                }
                Some('<') => { /* … existing here-string/heredoc … */ }
                Some('&') => { /* … existing … */ }
                Some('>') => { /* … existing … */ }
                _ => push!(TokenKind::Op(Operator::RedirIn)),
            },
```

And the same for the `>` arm (lexer.rs:3184), with `ProcDir::Out`:

```rust
            '>' => match self.cursor.peek().copied() {
                Some('(') => {
                    self.history.push(Token::new(TokenKind::ProcSubOpen { dir: ProcDir::Out }, Span::new(off, l, c)));
                    self.cmd_at_word_start = false;
                    return Ok(Step::Produced);
                }
                Some('>') => { /* … existing RedirAppend … */ }
                Some('&') => { /* … existing DupOut … */ }
                Some('|') => { /* … existing RedirClobber … */ }
                _ => push!(TokenKind::Op(Operator::RedirOut)),
            },
```

(Keep the existing arms exactly as they are; only ADD the `Some('(')` arm. Confirm `off/l/c` are the fn params — they are. If setting `cmd_at_word_start = false` alone does not reproduce the oracle's word-state — verify against `diff_cmd` in Task 2/3 — mirror whatever `scan_command_word_atom` does when it emits a word-starting `Lit`; do NOT call `boundary_reset`.)

- [ ] **Step 5: Widen `scan_step_command_sub` to accept a bare `(` opener.** In the `!body_started` branch (lexer.rs:1681+), the current code requires the cursor on `$`. Restructure so it ALSO accepts a bare `(` (the procsub case — the `<`/`>` was already consumed by the operator arm). Keep the `$(` path (including its `$((` → `DeferredExpansion` sub-case) exactly as-is:

```rust
        if !body_started {
            let off = self.cursor.offset();
            let l   = self.cursor.line();
            let c   = self.cursor.column();
            match self.cursor.peek().copied() {
                Some('$') => {
                    self.cursor.next(); // consume `$`
                    if self.cursor.peek() != Some(&'(') {
                        self.history.push(Token::new(TokenKind::DeferredExpansion, Span::new(off, l, c)));
                        return Ok(Step::Produced);
                    }
                    self.cursor.next(); // consume `(`
                    if self.cursor.peek() == Some(&'(') && !self.retokenize_arith_as_cmdsub {
                        self.history.push(Token::new(TokenKind::DeferredExpansion, Span::new(off, l, c)));
                        return Ok(Step::Produced);
                    }
                    self.retokenize_arith_as_cmdsub = false;
                }
                Some('(') => {
                    // v251: process-substitution opener. The `<`/`>` was already
                    // consumed by scan_command_operator_atom; consume the `(`.
                    self.cursor.next();
                }
                _ => {
                    self.history.push(Token::new(TokenKind::DeferredExpansion, Span::new(off, l, c)));
                    return Ok(Step::Produced);
                }
            }
            if let Some(Mode::CommandSub { body_started }) = self.modes.last_mut() {
                *body_started = true;
            }
            self.history.push(Token::new(TokenKind::CmdSubOpen, Span::new(off, l, c)));
            Ok(Step::Produced)
        } else {
            self.scan_step_command()
        }
```

(This is a refactor of the SAME logic — preserve the `$`/`$(`/`$((` behavior byte-for-byte; only ADD the `Some('(')` arm. Re-read the current function to confirm you carried over every early-return and the `retokenize_arith_as_cmdsub` handling.)

- [ ] **Step 6: Run, expect PASS.** `cargo test -p huck-syntax --jobs 1 --lib procsub_open_atoms_disambiguate -- --test-threads 1`. Then confirm the deferral test is UNCHANGED (parser still defers — the parser has no `ProcSubOpen` arm yet, so `parse_simple_with_leading_word` hits `_ => UnsupportedCommand`): `cargo test -p huck-syntax --jobs 1 --lib atoms_procsub_deferred -- --test-threads 1` → PASS. Also run the existing comsub + redirect lexer/parser tests to confirm no regression: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → `0 failed`.

- [ ] **Step 7: Warnings + commit.** `cargo build -p huck-syntax 2>&1 | grep -c warning` → `0`. Commit:

```bash
git add crates/huck-syntax/src/lexer.rs
git commit -m "v251 T1: ProcSubOpen atom + <(/>( disambiguation + CommandSub bare-( opener (dormant)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Parser — `parse_process_sub` + word-assembler arm + word-start set; remove the deferral

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs`
- Test: `crates/huck-syntax/src/parser.rs` `mod tests`

**Interfaces:**
- Consumes: Task 1 (`ProcSubOpen { dir }`, the `Mode::CommandSub` bare-`(` opener).
- Produces: procsub parses to `WordPart::ProcessSub { sequence, dir }` on the atom path for the core positions.

- [ ] **Step 1: Write the failing tests:**

```rust
    #[test]
    fn atoms_procsub_core() {
        diff_cmd("cat <(echo hi)");
        diff_cmd("tee >(cat)");
        diff_cmd("echo <(a) >(b)");        // multiple, both dirs
        diff_cmd("diff <(sort x) <(sort y)");
        diff_cmd("x<(y)");                  // glued to leading literal
        diff_cmd("wc < <(sort f)");         // procsub as a redirect TARGET
        diff_cmd("sort > >(uniq)");
    }
```

- [ ] **Step 2: Run, expect FAIL** (parser defers). `cargo test -p huck-syntax --jobs 1 --lib atoms_procsub_core -- --test-threads 1`.

- [ ] **Step 3: Add `parse_process_sub`** (mirror `parse_command_sub`, parser.rs:765; ensure `ProcDir` is in scope — add to the `use crate::lexer::{…}` import or use the full path). Place it next to `parse_command_sub`:

```rust
/// v251: assemble a `WordPart::ProcessSub` for a `<(…)`/`>(…)` process
/// substitution. Mirrors `parse_command_sub`: the body is a paren-delimited
/// command sequence lexed under `Mode::CommandSub` (the lexer's bare-`(` opener
/// path; the word-mode `ProcSubOpen` signal was already consumed by the caller).
/// `dir` comes from that signal.
pub(crate) fn parse_process_sub(iter: &mut Lexer, dir: ProcDir) -> Result<WordPart, ParseError> {
    iter.push_mode(Mode::CommandSub { body_started: false });
    match iter.next_kind()? {
        Some(TokenKind::CmdSubOpen) => {} // the real opener, scanned under CommandSub mode
        _ => { iter.pop_mode(); return Err(ParseError::UnsupportedExpansion); }
    }
    let sequence = if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::RParen))) {
        iter.next_kind()?; // consume `)`
        Sequence {
            first: Command::Pipeline(Pipeline { negate: false, commands: Vec::new() }),
            rest: Vec::new(),
            background: false,
        }
    } else {
        match parse_subshell_sequence(iter) {
            Ok(mut seq) => { zero_lines_in_sequence(&mut seq); seq }
            Err(e) => {
                iter.pop_mode();
                let mapped = match e {
                    ParseError::UnsupportedCommand => ParseError::UnsupportedExpansion,
                    other => other,
                };
                return Err(mapped);
            }
        }
    };
    iter.pop_mode();
    Ok(WordPart::ProcessSub { sequence, dir })
}
```

(Cross-check the empty-body `Sequence` construction against `parse_command_sub`'s exactly — copy it verbatim so `<()`/`>()` matches the oracle. Confirm `parse_subshell_sequence`/`zero_lines_in_sequence` are in scope (same module).)

- [ ] **Step 4: Add the `ProcSubOpen` arm to `parse_word_command`** (parser.rs, right after the `CmdSubOpen` arm at :161):

```rust
            Some(TokenKind::ProcSubOpen { dir }) => {
                let dir = *dir;
                iter.next_kind()?;            // discard the signal (cursor stays on `(`)
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_process_sub(iter, dir)?);
            }
```

- [ ] **Step 5: Add `ProcSubOpen` to the word-start set** in `parse_simple_with_leading_word` (parser.rs:1620-1634) so a FRESH-WORD procsub routes to `parse_word_command` instead of the `_ => UnsupportedCommand` arm:

```rust
                    | TokenKind::CmdSubOpen
                    | TokenKind::ProcSubOpen { .. }
                    | TokenKind::BeginBacktick
```

- [ ] **Step 6: Retarget the deferral test.** Replace `atoms_procsub_deferred` (parser.rs:3154) — procsub is no longer deferred:

```rust
    #[test]
    fn atoms_procsub_supported() {
        // process substitution now parses on the atom path, byte-identical to the oracle.
        for s in ["cat <(echo hi)", "tee >(cat)", "echo <(a) >(b)"] {
            diff_cmd(s);
        }
    }
```

- [ ] **Step 7: Run, expect PASS; debug against `old_seq`.** `cargo test -p huck-syntax --jobs 1 --lib atoms_procsub -- --test-threads 1`. If a `diff_cmd` fails, print both ASTs — the oracle is ground truth (check the empty-body `Sequence`, the `dir`, and glued/redirect-target word shapes). Then the full atom suite: `cargo test -p huck-syntax --jobs 1 --lib atoms_ -- --test-threads 1` → `0 failed` (watch for a hang = a non-progress path in the CommandSub opener).

- [ ] **Step 8: Warnings + commit.** `grep -c warning` → 0. Commit `v251 T2: parser parse_process_sub + word-assembler arm; remove deferral (diff_cmd green)`.

---

### Task 3: Full corpus — nested, bodies, quoted-literal, adjacency, error/deferred parity + gate

**Files:** `crates/huck-syntax/src/parser.rs` (tests; production edits only if a case exposes a real atom-path bug).

**Interfaces:** Consumes T1+T2. Produces the comprehensive v251 differential gate.

- [ ] **Step 1: Write the corpus tests:**

```rust
    #[test]
    fn atoms_procsub_corpus() {
        // nested
        diff_cmd("cat <( cat <(echo x) )");
        diff_cmd("echo >( tee >(cat) )");
        // bodies: pipelines / expansions / compounds inside
        diff_cmd("cat <(echo $x | sort)");
        diff_cmd("cat <(echo ${y:-d})");
        diff_cmd("cat <(if true; then echo a; fi)");
        diff_cmd("cat <(a && b || c)");
        // adjacency with other word parts
        diff_cmd("echo pre$(c)<(d)post");
        diff_cmd("cat <(a)<(b)");            // two procsubs glued into one word
        // empty body
        diff_cmd("cat <()");
        diff_cmd("tee >()");
    }

    #[test]
    fn atoms_procsub_quoted_literal() {
        // inside quotes `<(`/`>(` are LITERAL — no procsub (matches the oracle).
        diff_cmd("echo \"<(x)\"");
        diff_cmd("echo '<(x)'");
        diff_cmd("echo \\<(x)");             // escaped `<` — verify vs oracle
    }

    #[test]
    fn atoms_procsub_errors() {
        // body-deferred constructs inside a procsub → same posture as $(…);
        // malformed → match the oracle. Split lexer-level vs parser-level by
        // OBSERVATION, mirroring the existing error-parity tests (e.g.
        // atoms_error_parity): for inputs where old_seq panics at the lexer level
        // (`.expect("lex")`), assert only new_seq(s).is_err(); otherwise compare
        // normalized results with diff-style equality.
        for s in ["cat <( [[ x ]] )", "cat <( f() { :; } )", "cat <("] {
            let n = new_seq(s).map(|_| ()).map_err(|e| format!("{e:?}"));
            let o = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| old_seq(s)));
            match o {
                Ok(res) => assert_eq!(n, res.map(|_| ()).map_err(|e| format!("{e:?}")), "procsub error parity {s:?}"),
                Err(_) => assert!(new_seq(s).is_err(), "atom path must reject lexer-level {s:?}"),
            }
        }
    }
```

(If `catch_unwind` around `old_seq` is awkward with the harness, hand-classify each input by running it once and writing either the `assert_eq!` or the `is_err()` form — do NOT leave a case that silently passes. If a well-formed `diff_cmd` case diverges, that is a v251 BUG: debug against `old_seq` and FIX it — do not pin. The only sanctioned way to record a genuinely-unavoidable divergence is a `*_divergence` test + a note, and only after confirming the oracle is the one that's wrong; if you think you found one, STOP and report it.)

- [ ] **Step 2: Run, expect PASS or debug.** `cargo test -p huck-syntax --jobs 1 --lib atoms_procsub -- --test-threads 1`. Likely trouble spots to debug against `old_seq`: nested procsub (mode-stack depth), a procsub whose body defers (`[[`/funcdef → `UnsupportedExpansion` via the `parse_subshell_sequence` mapping), quoted-literal (must NOT produce `ProcessSub`), and two glued procsubs in one word. For-list/case-pattern positions (`for x in <(a)`) are unusual — if the oracle accepts them and the atom path doesn't, that's because the for/case word loops (parser.rs ~2372/2421/2481) use a different dispatch; add a targeted `diff_cmd` only if the oracle actually parses it, and extend those dispatch sets if a real divergence appears (otherwise leave out of scope).

- [ ] **Step 3: Full gate.** Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_ -- --test-threads 1` → `0 failed`; `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → `0 failed`; `cargo test -p huck-syntax --jobs 1 --doc -- --test-threads 1` → `0 failed`. `cargo build -p huck-syntax 2>&1 | grep -c warning` → `0`.

- [ ] **Step 4: Commit.** `v251 T3: full process-substitution corpus (nested/bodies/quoted-literal/error parity) + gate`.

---

## Self-Review checklist (run before the whole-branch review)

- Every in-scope construct (spec §4) has a `diff_cmd` case: both dirs, standalone, glued, redirect-target, multiple, nested, body-with-expansions/pipelines/compounds, quoted-literal, empty body, adjacency, error/deferred parity.
- Production untouched: `git diff main -- crates/huck-syntax/src/command.rs` EMPTY; the production procsub scanner + `scan_paren_substitution` unchanged; `command_atoms` still defaults `false`.
- Non-`(` `<`/`>` redirects unaffected (the existing redirect corpus is green).
- The `Mode::CommandSub` opener refactor preserved `$(`/`$((`/`retokenize_arith_as_cmdsub` behavior byte-for-byte (the comsub/arith corpus is green).
- Full `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` is `0 failed`; doctests `0 failed`; 0 warnings.
- All commits carry the trailer; branch is `v251-process-substitution`.
