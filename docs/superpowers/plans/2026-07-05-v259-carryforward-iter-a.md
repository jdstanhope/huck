# v259 Iteration A Carry-Forward Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the dormant atom-command path byte-identical to the `command.rs` oracle for three known carry-forward divergences: CF2 (heredoc-body queue leak on early parse error), CF3 (even-bang before a compound not Pipeline-wrapped), CF4 (`$"…"` locale quoting keeps a stray `$`).

**Architecture:** Three independent, disjoint fixes. CF2 + CF3 live in `crates/huck-syntax/src/parser.rs` (the atom-path parser); CF4 in `crates/huck-syntax/src/lexer.rs` (the atom-only unquoted `$` classifier). The `command.rs` oracle is untouched; `command_atoms` stays `false`. Every fix is validated against the oracle via the existing differential harness (`new_seq` = atom path, `old_seq` = oracle) except CF2, which is state-hygiene and gets a bespoke queue-inspection test.

**Tech Stack:** Rust; huck-syntax crate; existing `parser.rs mod tests` differential harness (`diff_cmd(s)` asserts `new_seq(s).unwrap() == old_seq(s).unwrap()`).

## Global Constraints

- Box is 1 core / 1.9 GB and OOM-kills on parallel test runs. Use EXACTLY: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1`. NEVER `--workspace`, NEVER multiple threads.
- `cargo build -p huck-syntax` → 0 warnings.
- `command.rs` gets NO changes (EMPTY diff vs `main`). Confirm with `git diff --stat main -- crates/huck-syntax/src/command.rs`.
- Both `command_atoms` sites in `lexer.rs` (:819 default, :4237 the other constructor) stay `false`. `new_live_atoms` (:4377) sets it `true` — that is the harness constructor and is correct.
- rust-analyzer shows PHANTOM diagnostics (E0063/E0425/E0308) after edits; trust `cargo build`/`cargo test`, not the IDE.
- Commit trailer VERBATIM on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- Work on branch `v259-carryforward-iter-a`. Do NOT touch `main`.
- `diff_cmd(s)` is the equality gate. Do NOT weaken a test to make it pass; if a `diff_cmd` fails, the atom output genuinely differs from the oracle — fix the implementation. All corpus values below were probed against the oracle on 2026-07-05 and confirmed to match after the fix.

---

## File Structure

- `crates/huck-syntax/src/parser.rs` — CF2 (`parse_sequence` entry-reset) + CF3 (`finish_pipeline` `had_bangs` param + two call sites) + all three tasks' tests (in `mod tests`).
- `crates/huck-syntax/src/lexer.rs` — CF4 (one `Some('"')` arm in `emit_unquoted_dollar_atom`).
- `crates/huck-syntax/src/command.rs` — UNTOUCHED.

---

### Task 1: CF2 — heredoc-body queue hygiene (`parse_sequence` entry-reset)

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` — `parse_sequence` (:2729), add entry-reset.
- Test: `crates/huck-syntax/src/parser.rs` `mod tests` — new `atoms_cf2_heredoc_queue_reset`.

**Interfaces:**
- Consumes: `Lexer::take_heredoc_bodies(&mut self) -> Vec<Word>` (lexer.rs:839, `pub(crate)`, drains the queue) and `Lexer::new_live_atoms(input, aliases, opts) -> Lexer` (lexer.rs:4377).
- Produces: nothing new (behavior fix only).

**Background (do not skip):** `parse_sequence` drains `take_heredoc_bodies()` only on its success path (:2751). On an early `Err` (`skip_newlines?`, `parse_and_or?`, or the stray-terminator `UnexpectedToken` at :2747), any heredoc body already pushed mid-parse stays in the Lexer-owned queue. A `Lexer` is bound to one source `&str` (single parse to EOF), so this cannot currently corrupt a real parse — but the fix hardens `parse_sequence` so a dirty queue from a prior errored parse on the same Lexer is discarded before the next parse, which the finale's live driver may rely on. The atom `parse_sequence` (single-arg) is the single non-reentrant top-level entry (only caller parser.rs:3937; compound bodies recurse via `parse_and_or` / `parse_*_sequence`, never `parse_sequence`), so an unconditional entry-reset is safe.

- [ ] **Step 1: Write the failing test**

Add to `parser.rs mod tests`:

```rust
    #[test]
    fn atoms_cf2_heredoc_queue_reset() {
        // A parse that collects a heredoc body then errors on a stray `;;`
        // leaves the body in the Lexer-owned queue (early-Err path does not
        // drain). A subsequent parse_sequence on the SAME Lexer must discard
        // that leaked body at entry, so the queue is empty afterward.
        let mut lx = Lexer::new_live_atoms("cat <<E\nx\nE\n;;", &Default::default(), LexerOptions::default());
        let first = parse_sequence(&mut lx); // collects the heredoc body, then `;;` → Err
        assert!(first.is_err(), "expected UnexpectedToken on the `;;`, got {first:?}");
        let _ = parse_sequence(&mut lx);     // entry-reset must drain the leaked body
        assert!(
            lx.take_heredoc_bodies().is_empty(),
            "parse_sequence entry-reset should have drained the leaked heredoc body"
        );
    }
```

- [ ] **Step 2: Run the test to verify it FAILS**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_cf2_heredoc_queue_reset -- --test-threads 1`
Expected: FAIL — the assertion `is_empty()` fails because without the fix the leaked body is still queued after the second call.

(If instead it PASSES here, STOP and report — it means the leak does not reproduce as modeled; do not proceed.)

- [ ] **Step 3: Implement the entry-reset**

In `parse_sequence` (parser.rs:2729), insert as the FIRST statement, before `skip_newlines(iter)?;`:

```rust
    // v259 CF2: discard any heredoc bodies leaked by a prior parse that errored
    // after pushing them (take_heredoc_bodies drains only on this fn's success
    // path). Safe: the atom parse_sequence is the single non-reentrant top-level
    // entry, so nothing legitimately carries a body into a fresh call.
    let _ = iter.take_heredoc_bodies();
```

- [ ] **Step 4: Run the test to verify it PASSES**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_cf2_heredoc_queue_reset -- --test-threads 1`
Expected: PASS.

- [ ] **Step 5: Run the full crate suite + build**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → all pass.
Run: `cargo build -p huck-syntax` → 0 warnings.
Run: `git diff --stat main -- crates/huck-syntax/src/command.rs` → EMPTY.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v259 CF2: parse_sequence entry-reset drains leaked heredoc bodies

parse_sequence drained take_heredoc_bodies() only on the success path, so a
parse that errored after collecting a heredoc body left it in the Lexer-owned
queue. Add an unconditional entry-reset (safe: single non-reentrant top-level
entry) so a dirty queue from a prior errored parse cannot leak into the next.
Atom-path only; command.rs EMPTY-diff; command_atoms false.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: CF3 — even-bang before a compound (`finish_pipeline` `had_bangs`)

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` — `finish_pipeline` (:2429, add param + widen arm), `parse_pipeline` (:2423, call site), `parse_coproc_body` (:2369, call site).
- Test: `crates/huck-syntax/src/parser.rs` `mod tests` — new `atoms_cf3_even_bang_compound`.

**Interfaces:**
- Consumes: `finish_pipeline(iter: &mut Lexer, first: Command, negate: bool) -> Result<Command, ParseError>` (current signature; you are changing it).
- Produces: `finish_pipeline(iter, first, negate, had_bangs)` — a new 4th `bool` param. Only two callers exist (grep-confirmed: parse_pipeline:2423, parse_coproc_body:2369); update both.

**Background:** `finish_pipeline` wraps a compound first-stage only when `negate` is true (arm :2448). For an even bang count `negate = bangs % 2 == 1` is `false`, so `! ! { a; }` falls to the bare-compound arm. The oracle wraps ANY nonzero-bang compound as `Pipeline{negate, [cmd]}` regardless of parity (probed general across brace/subshell/if/while/for/case/`[[`/`((`/coproc). Simple commands already always wrap; the zero-bang bare compound and the odd-bang wrap already match.

- [ ] **Step 1: Write the failing tests**

Add to `parser.rs mod tests`:

```rust
    #[test]
    fn atoms_cf3_even_bang_compound() {
        // Even (>=2) bang count before a compound: oracle wraps
        // Pipeline{negate:false,[compound]}; the atom path used to return the
        // bare compound. Covers every compound family.
        diff_cmd("! ! { a; }");
        diff_cmd("! ! (a)");
        diff_cmd("! ! if x; then y; fi");
        diff_cmd("! ! while x; do y; done");
        diff_cmd("! ! for i in a; do y; done");
        diff_cmd("! ! case x in a) :; esac");
        diff_cmd("! ! [[ x ]]");
        diff_cmd("! ! (( 1 ))");
        diff_cmd("! ! coproc cat");
        // Regressions (must still match): odd-bang wraps negate:true, zero-bang
        // stays bare, simple always wraps.
        diff_cmd("! { a; }");
        diff_cmd("{ a; }");
        diff_cmd("! ! a");
    }
```

- [ ] **Step 2: Run to verify it FAILS**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_cf3_even_bang_compound -- --test-threads 1`
Expected: FAIL on the first even-bang compound line (atom returns bare, oracle wraps).

- [ ] **Step 3: Add the `had_bangs` param and widen the wrap arm**

In `finish_pipeline` (parser.rs:2429), change the signature to add a 4th param:

```rust
fn finish_pipeline(
    iter: &mut Lexer,
    first: Command,
    negate: bool,
    had_bangs: bool,
) -> Result<Command, ParseError> {
```

In the no-`|` return match (the block starting `return Ok(match first {` at ~:2446), change the compound arm from `cmd if negate` to also fire when `had_bangs`:

```rust
        return Ok(match first {
            Command::Simple(_)         => Command::Pipeline(Pipeline { negate, commands: vec![first] }),
            cmd if negate || had_bangs => Command::Pipeline(Pipeline { negate, commands: vec![cmd] }),
            cmd                        => cmd,
        });
```

- [ ] **Step 4: Update the two call sites**

In `parse_pipeline` (parser.rs:2423), pass `bangs > 0`:

```rust
    finish_pipeline(iter, first, negate, bangs > 0)
```

In `parse_coproc_body` (parser.rs:2369), pass `false` (a coproc body counts no leading pipeline-negation `!`):

```rust
        finish_pipeline(iter, first, false, false)
```

- [ ] **Step 5: Run the test + full suite + build**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_cf3_even_bang_compound -- --test-threads 1` → PASS.
Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → all pass.
Run: `cargo build -p huck-syntax` → 0 warnings.
Run: `git diff --stat main -- crates/huck-syntax/src/command.rs` → EMPTY.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v259 CF3: even-bang before a compound wraps Pipeline{negate:false}

finish_pipeline wrapped a compound first-stage only when negate was true, so an
even leading-bang count (! !) returned the bare compound instead of the oracle's
Pipeline{negate:false,[compound]}. Thread had_bangs into finish_pipeline and wrap
when negate || had_bangs; parse_pipeline passes bangs>0, parse_coproc_body passes
false (a leading ! in a coproc body is the program name). General across all
compound families. Atom-path only; command.rs EMPTY-diff; command_atoms false.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: CF4 — `$"…"` locale quoting (`emit_unquoted_dollar_atom` `$"` arm)

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — `emit_unquoted_dollar_atom` (:3854), add a `Some('"')` arm.
- Test: `crates/huck-syntax/src/parser.rs` `mod tests` — new `atoms_cf4_locale_dquote`.

**Interfaces:**
- Consumes: `TokenKind::BeginDquote` (lexer.rs:460), `Span::new(off, l, c)`, `self.cursor` / `self.history` (as the sibling arms in the same fn use them).
- Produces: nothing new (classifier fix only).

**Background:** `emit_unquoted_dollar_atom` (the UNQUOTED `$` classifier) has no `Some('"')` arm, so `$"` hits the `_ =>` catch-all → consumes the lone `$`, emits `DollarLit` (a stray `Literal "$"`), then a bare dquote opens. The oracle (`scan_dollar_expansion:5134`, `Some('"') if !quoted => {}`) drops the `$` and leaves `"` for the normal dquote handler. `$"…"` is bash locale quoting; huck's translation is the identity, so `$"…" ≡ "…"`. Fixing this one classifier fixes BOTH command position and the `=~` regex operand (scan_step_regex reuses this fn). The INSIDE-double-quote case is handled by a different classifier and already matches — do not touch it.

- [ ] **Step 1: Write the failing tests**

Add to `parser.rs mod tests`:

```rust
    #[test]
    fn atoms_cf4_locale_dquote() {
        // $"…" is locale quoting == "…"; the oracle drops the `$`. The atom path
        // used to keep a stray Literal "$".
        diff_cmd("echo $\"hi\"");
        diff_cmd("echo $\"a\"$\"b\"");        // multiple, all drop the `$`
        diff_cmd("[[ $x =~ $\"abc\" ]]");      // regex operand (shared classifier)
        // Regression: a $" INSIDE a double-quoted span stays a literal `$` on
        // both paths — must remain unchanged.
        diff_cmd("echo \"a$\"b\"c\"");
    }
```

- [ ] **Step 2: Run to verify it FAILS**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_cf4_locale_dquote -- --test-threads 1`
Expected: FAIL on `echo $"hi"` — atom emits `[Literal "$", Quoted{Double,…}]`, oracle emits `[Quoted{Double,…}]`.

- [ ] **Step 3: Add the `Some('"')` arm**

In `emit_unquoted_dollar_atom` (lexer.rs:3854), add a `Some('"')` arm to the `match probe.peek().copied()` block — place it adjacent to the other openers (e.g. right before the `Some('[')` legacy-arith arm), matching the span-construction style the sibling arms use:

```rust
            // `$"…"` — bash locale quoting; huck's translation is the identity,
            // so `$"…" ≡ "…"`. Drop the `$` and emit the zero-width BeginDquote
            // (cursor left on `"`), exactly mirroring a bare `"`; the parser's
            // Mode::DoubleQuote then consumes the `"` and scans the body. (Oracle:
            // scan_dollar_expansion's `Some('"') if !quoted => {}`.)
            Some('"') => {
                self.cursor.next(); // consume `$` only, leave `"`
                self.history.push(Token::new(TokenKind::BeginDquote, Span::new(off, l, c)));
            }
```

- [ ] **Step 4: Run the test + full suite + build**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_cf4_locale_dquote -- --test-threads 1` → PASS.
Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → all pass.
Run: `cargo build -p huck-syntax` → 0 warnings.
Run: `git diff --stat main -- crates/huck-syntax/src/command.rs` → EMPTY.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs
git commit -m "v259 CF4: \$\"…\" locale quoting drops the \$ on the atom path

emit_unquoted_dollar_atom had no Some('\"') arm, so \$\" kept a stray Literal \"\$\"
before the dquote span; the oracle drops the \$ (identity locale quoting, \$\"…\" ≡
\"…\"). Add a Some('\"') arm that consumes only the \$ and emits BeginDquote,
mirroring a bare \". Fixes both command position and the =~ regex operand (shared
classifier); inside-dquote unchanged. Atom-path only; command.rs EMPTY-diff;
command_atoms false.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

**Spec coverage:** CF2 → Task 1; CF3 → Task 2; CF4 → Task 3. All three spec fixes are covered, one task each.

**Placeholder scan:** none — every step has verbatim code, exact commands, and expected output.

**Type consistency:** `finish_pipeline`'s new 4th param `had_bangs: bool` is defined in Task 2 Step 3 and consumed at both call sites in Step 4 (`bangs > 0` in parse_pipeline, `false` in parse_coproc_body). `take_heredoc_bodies` (Task 1) and `BeginDquote`/`Span::new` (Task 3) are existing symbols confirmed in the code. The CF2 test uses the same `Lexer::new_live_atoms(...)` constructor as the `new_seq` harness.

**Test-mechanism note (CF2):** the spec described a "reused-Lexer test asserting the second parse equals a fresh-Lexer `echo hi`." A Lexer is bound to one `&str` and `parse_sequence` runs to EOF, so that exact framing is not mechanically realizable; the plan's queue-inspection test (assert the queue is empty after a second parse on the same Lexer) validates the identical fix — that the entry-reset drains a leaked queue — and is the correct concrete form.
