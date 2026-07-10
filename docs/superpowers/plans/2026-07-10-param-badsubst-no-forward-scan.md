# Param bad-subst forward-scan removal — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete the last lexer forward-scanner (`scan_braced_operand` + its 4 verbatim helpers); make the parser assemble the `${…}` bad-substitution raw from the source span instead. Behavior-preserving for the inputs that route through it.

**Architecture:** `TokenKind::ParamBadSubst { raw }` becomes a payload-free marker. `emit_bad_subst!` stops forward-scanning and just emits the marker (cursor left on the offending char). The parser, on the marker, reuses `word_in_mode!(ParamWordOperand…)` + `expect_close!` to consume the rest of the body to the matching `}` (correct nesting via the atom stream), discards the parsed word, and builds `ParamModifier::BadSubst { raw = source[${ ..= }] }` via a new `Lexer::source_span` accessor.

**Tech Stack:** Rust (2021), cargo; crate `huck-syntax` (`lexer.rs`, `parser.rs`). Bash-diff/characterization harnesses under `tests/scripts/`.

**Issue:** [#107](https://github.com/jdstanhope/huck/issues/107). **Spec:** `docs/superpowers/specs/2026-07-10-param-badsubst-no-forward-scan-design.md`. Does NOT close #84.

## Global Constraints

- **Behavior-preserving** for every input that currently routes through `scan_braced_operand` (all emit the verbatim `source[${ ..= }]` slice as raw; `source_span` must reproduce the identical bytes). The `@`-transform / indirect over-flags (`${x@Z}`, `${x@}`, `${!x@Z}` → huck rc=1) and the `$'…'`-name raw format (`${$'y'}`) must remain **exactly** as today.
- **Only the `${…}` bad-subst path changes.** No production behavior change elsewhere; no change to `ParamModifier::BadSubst`'s shape (it keeps `raw: String`) — only *where* raw is produced. Downstream (`generate.rs`, `expand.rs`, `param_expansion.rs`) is untouched.
- **Never `cargo test --workspace`** (OOMs the box). Use `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` (baseline ~439) and `-p huck-engine` (~1773). Build the binary with `cargo build -p huck`.
- `cargo fmt --all` before every commit; `cargo fmt --all --check` must be clean.
- **Commit trailer** on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Branch:** `v279-param-badsubst-no-forward-scan` (off `main`; do not commit to `main`).

## The input set (used by Tasks 1 & 4)

Inputs that route through `scan_braced_operand` today, with current huck behavior (rc + verbatim raw in the message):

| input | huck rc | huck raw | bash rc | huck==bash? |
|-------|--------:|----------|--------:|:-----------:|
| `${}` | 1 | `${}` | 1 | yes |
| `${@Z}` | 1 | `${@Z}` | 1 | yes |
| `${#x@}` | 1 | `${#x@}` | 1 | yes |
| `${x@Z}` | 1 | `${x@Z}` | 0 | no (preserved over-flag) |
| `${x@}` | 1 | `${x@}` | 0 | no (preserved over-flag) |
| `${!x@Z}` | 1 | `${!x@Z}` | 1 | msg differs (preserved) |
| `${$'y'}` | 1 | `${$'y'}` | 1 | raw differs (preserved) |
| `${a$'b'}` | 1 | `${a$'b'}` | 1 | raw differs (preserved) |
| `${x` (EOF) | 2 | unterminated | 2 | yes |

---

### Task 1: Characterization harness (baseline lock — runs before any code change)

Captures current huck behavior so the refactor is provably behavior-preserving.

**Files:**
- Create: `tests/scripts/param_badsubst_char_check.sh`

**Interfaces:** Consumes nothing. Produces the baseline gate reused by Tasks 3–4.

- [ ] **Step 1: Study an existing harness for the house pattern**

Read one existing harness, e.g. `tests/scripts/cmdsub_comment_diff_check.sh`, to copy: how it locates the huck binary, runs a fragment through both shells, and normalizes the `progname:` prefix (huck prints `…/huck: line N:` vs bash `bash: line N:`). Follow that pattern exactly.

- [ ] **Step 2: Write the characterization harness**

Create `tests/scripts/param_badsubst_char_check.sh`. For each input in the table above it must, running through the **huck binary** (`cargo build -p huck` first), assert the recorded `(rc, message-after-prefix)` baseline. Capture the baseline by running current huck once and pasting the exact outputs (do NOT hand-guess — run it). For the three `huck==bash` rows (`${}`, `${@Z}`, `${#x@}`) additionally assert byte-identical vs bash (after normalizing the `progname:` prefix). Include a comment block listing each input and its expected `(rc, raw)`.

```sh
#!/usr/bin/env bash
# Characterization guard for ${…} bad-substitution raw assembly (#107).
# These inputs route through the bad-subst path; the refactor that deletes
# scan_braced_operand must keep huck's output byte-identical to this baseline.
# ... (follow the existing *_diff_check.sh structure) ...
```

- [ ] **Step 3: Run it against current huck — must PASS**

Run: `bash tests/scripts/param_badsubst_char_check.sh`
Expected: all cases pass (baseline == current behavior). If a case fails, the baseline was mis-recorded — fix the expected value from the actual current output.

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/param_badsubst_char_check.sh
git commit -m "$(printf 'test: characterization harness for ${…} bad-subst raw (#107)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: Add `Lexer::source_span` accessor

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (add method on `impl<'a> Lexer<'a>`; add a `#[cfg(test)]` unit test in `lexer/*_tests.rs` per the v278 layout — or an inline `#[cfg(test)]` test module if none exists for this)

**Interfaces:** Produces `pub(crate) fn source_span(&self, start_off: usize, close_off: usize) -> &str` and `pub(crate) fn param_start_off(&self) -> usize` (reads the current `Mode::ParamExpansion` frame's `start_off`). Consumed by Task 3.

- [ ] **Step 1: Write the failing unit test**

Add to a `#[cfg(test)]` module in the lexer:
```rust
#[test]
fn source_span_returns_inclusive_slice() {
    // "abc${d}ef": bytes $=3 {=4 d=5 }=6
    let lx = Lexer::new("abc${d}ef");
    assert_eq!(lx.source_span(3, 6), "${d}");
}
```
(Use the crate's actual `Lexer` constructor — check how existing lexer tests build one; adjust if `Lexer::new` needs different args.)

- [ ] **Step 2: Run it — verify it fails to compile (method missing)**

Run: `cargo test -p huck-syntax --jobs 1 --lib source_span_returns_inclusive_slice 2>&1 | tail -5`
Expected: compile error, `no method named source_span`.

- [ ] **Step 3: Implement**

`Lexer` holds `cursor: CharCursor<'a>`, and `CharCursor` holds `s: &'a str` with an existing `slice_from(start) -> &s[start..pos]` (lexer.rs:218). Add a bounded inclusive sibling on `CharCursor` (mirror `slice_from`'s injected-guard):
```rust
/// Verbatim source `&s[start..=end]`. `end` is the byte offset of the last
/// char to include (the `}` of a `${…}`). Mirrors `slice_from`'s guard.
pub fn slice_inclusive(&self, start: usize, end: usize) -> &str {
    debug_assert!(
        self.injected.is_empty(),
        "slice_inclusive must not straddle an injected alias body"
    );
    &self.s[start..=end]
}
```
Then on `impl<'a> Lexer<'a>` delegate:
```rust
/// Verbatim source of a `${…}` from its `$` (`start_off`) through its `}`
/// (`close_off`), inclusive — used by the parser to reconstruct a
/// bad-substitution's raw without forward-scanning.
pub(crate) fn source_span(&self, start_off: usize, close_off: usize) -> &str {
    self.cursor.slice_inclusive(start_off, close_off)
}
```
Also add:
```rust
/// Byte offset of the leading `$` of the innermost `${` currently open.
pub(crate) fn param_start_off(&self) -> usize {
    match self.modes.iter().rev().find_map(|m| match m {
        Mode::ParamExpansion { start_off, .. } => Some(*start_off),
        _ => None,
    }) {
        Some(off) => off,
        None => 0,
    }
}
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p huck-syntax --jobs 1 --lib source_span_returns_inclusive_slice 2>&1 | tail -5`
Expected: PASS. Then `cargo fmt --all`.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs
git commit -m "$(printf 'feat: add Lexer::source_span / param_start_off accessors (#107)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 3: Core change — marker + parser-assembled raw

The atomic behavioral change. `scan_braced_operand` remains present (test-covered) after this task; Task 4 deletes it.

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (`TokenKind::ParamBadSubst` def @711; `emit_bad_subst!` macro @1753)
- Modify: `crates/huck-syntax/src/parser.rs` (`parse_param_expansion`: capture `start_off`; hoist `word_in_mode!`/`expect_close!`; rewrite the `ParamBadSubst` arms @988 and @1146; reconcile `${}` @1108)
- Modify: any `#[cfg(test)]` test that constructs/matches `TokenKind::ParamBadSubst { raw }`

**Interfaces:** Consumes `source_span`/`param_start_off` (Task 2). Produces no new public API. `ParamModifier::BadSubst { raw }` shape unchanged.

- [ ] **Step 1: Change the token to a payload-free marker**

In `lexer.rs`, change the variant (currently `ParamBadSubst { raw: String }` @711) to a unit variant `ParamBadSubst`. Fix the doc comment.

- [ ] **Step 2: Rewrite `emit_bad_subst!` (lexer.rs:1753) to stop scanning**

Replace its body with (no `scan_braced_operand`, no `raw`, cursor left on the offending char):
```rust
macro_rules! emit_bad_subst {
    () => {{
        let sp = Span::new(
            self.cursor.offset(),
            self.cursor.line(),
            self.cursor.column(),
        );
        // Mark seen_name so the head mode won't re-enter and re-detect before
        // the parser switches to the operand tail. Do NOT forward-scan: the
        // parser drives the rest of the body to `}` and assembles the raw.
        if let Some(Mode::ParamExpansion { seen_name, .. }) = self.modes.last_mut() {
            *seen_name = true;
        }
        self.history.push(Token::new(TokenKind::ParamBadSubst, sp));
        return Ok(Step::Produced);
    }};
}
```

- [ ] **Step 3: In `parse_param_expansion`, capture `start_off` and hoist the operand macros**

After the `ParamOpen` is consumed (parser.rs:957, `set_param_start_off_from_cursor`), add:
```rust
let start_off = iter.param_start_off();
```
Move the `macro_rules! word_in_mode` (currently ~1172) and `macro_rules! expect_close` (~1189) definitions to **above** the name-dispatch `match` (before ~982) so both `ParamBadSubst` arms can use them. Verify they only reference bindings in scope at the new location (`iter`, `quoted`, `restore_dq!`, `pop_mode`).

- [ ] **Step 4: Rewrite the name-position arm (parser.rs:988)**

```rust
Some(TokenKind::ParamBadSubst) => {
    // Consume the rest of the body to the matching `}` via the operand
    // machinery (correct nesting/quote matching); discard the word.
    let _ = word_in_mode!(
        Mode::ParamWordOperand { in_dquote: false, enclosing_dquote: quoted },
        quoted
    );
    let close_off = iter.peek_span()?.map(|s| s.offset).unwrap_or(start_off);
    expect_close!();
    let raw = iter.source_span(start_off, close_off).to_string();
    restore_dq!();
    iter.pop_mode();
    return Ok(WordPart::ParamExpansion {
        name: String::new(),
        modifier: ParamModifier::BadSubst { raw },
        quoted,
        subscript: None,
        indirect: false,
    });
}
```
Note: if the body is unterminated (no `}` before EOF), `word_in_mode!` itself returns the lexer's `UnterminatedBrace`/`UnterminatedQuote` error → propagates as rc=2 (the intended behavior). Confirm `word_in_mode!` pops the `ParamWordOperand` mode it pushes (it does on the happy path at 1206–1213); if it does not pop on the error path, ensure the outer error handling still pops `ParamExpansion` (mirror the existing `_ =>` arm at 999).

- [ ] **Step 5: Rewrite the post-name arm (parser.rs:1146) the same way**

Apply the identical pattern to the `Some(TokenKind::ParamBadSubst)` arm at ~1146 (post-name / operator position). Same `word_in_mode!` + `expect_close!` + `source_span` sequence.

- [ ] **Step 6: Reconcile the hardcoded `${}` (parser.rs:1108)**

`${}` now reaches the marker path (empty name → `emit_bad_subst!` @1815). Confirm via the harness it still yields raw `${}`. If the `raw: "${}"` hardcode at 1108 is now unreachable, remove it; if still reachable for a distinct case, leave it. Do not guess — determine reachability by testing `${}`.

- [ ] **Step 7: Fix any test referencing the old token shape**

Run: `grep -rn 'ParamBadSubst {' crates/huck-syntax/src`
Update every remaining `ParamBadSubst { raw }` construction/match (tests included) to the unit form. `ParamModifier::BadSubst { raw }` (a different type) stays unchanged.

- [ ] **Step 8: Build, run unit suite, run the characterization harness**

Run:
```bash
cargo build -p huck 2>&1 | tail -3
cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
bash tests/scripts/param_badsubst_char_check.sh
```
Expected: builds green; `huck-syntax` passes (count may drop only if a now-invalid token-shape test was removed — a *behavioral* test must not change result); `huck-engine` unchanged (1773/0); **characterization harness passes** (behavior preserved).

- [ ] **Step 9: Probe nested-quote edges + adjudicate**

Run these malformed-with-nested-quote inputs through huck and bash and record results:
```bash
for s in '${@Z"}"}' '${@Z"}x"}' '${#x"}"@}'; do
  printf '%-12s bash:' "$s"; printf 'echo %s\n' "$s" | bash 2>&1 | head -1; echo " rc=$?"
  printf '%-12s huck:' "$s"; printf 'echo %s\n' "$s" | target/debug/huck 2>&1 | head -1; echo " rc=$?"
done
```
For any case whose huck output CHANGED from the old scanner's (an over-consumption correction), add it to `param_badsubst_char_check.sh` with the NEW expected value and a comment adjudicating it against bash. If a case is neither "reaches `}`" nor clean EOF, STOP and report it as DONE_WITH_CONCERNS for controller adjudication (the defensive-fallback situation).

- [ ] **Step 10: Format & commit**

Run `cargo fmt --all`, then:
```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs tests/scripts/param_badsubst_char_check.sh
git commit -m "$(printf 'refactor: parser assembles ${…} bad-subst raw; drop the forward-scan action (#107)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 4: Delete `scan_braced_operand` + the verbatim helpers

Now unused (only their own tests reference them).

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (delete 5 fns + their `#[cfg(test)]` unit tests)

**Interfaces:** Consumes nothing. Produces nothing.

- [ ] **Step 1: Confirm they are unused by non-test code**

Run: `grep -nE 'scan_braced_operand|consume_backtick_verbatim|consume_paren_cmdsub_verbatim|scan_cmdsub_body|scan_backtick_body' crates/huck-syntax/src/lexer.rs`
Expected: the only remaining references are the `fn` definitions and their `#[cfg(test)]` unit tests (no live callers). If a live caller remains, STOP — Task 3 was incomplete.

- [ ] **Step 2: Delete the five functions and their unit tests**

Remove `scan_braced_operand`, `consume_backtick_verbatim`, `consume_paren_cmdsub_verbatim`, `scan_cmdsub_body`, `scan_backtick_body`, and every `#[cfg(test)]` test that calls them. Leave `scan_raw_ansi_c_body` and `push_quoted_span` (still used by `scan_braced_name_ext` / quote handling).

- [ ] **Step 3: Build (0 warnings), run suites + harness**

Run:
```bash
cargo build -p huck 2>&1 | tail -5
cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
bash tests/scripts/param_badsubst_char_check.sh
```
Expected: builds with **no dead-code warnings**; `huck-syntax` green (count drops by the deleted helper tests only); harness still passes. Then `cargo fmt --all --check` clean.

- [ ] **Step 4: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs
git commit -m "$(printf 'refactor: delete scan_braced_operand + verbatim helpers, the last lexer forward-scanner (#107)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Self-review notes

- **Spec coverage:** division of labor + marker (Task 3), source-slice seam (Task 2), deletions (Task 4), characterization verification (Task 1 + Task 3 Step 8/9). The "behavior-preserving" claim is enforced by Task 1's baseline gate held green through Tasks 3–4.
- **`ParamModifier::BadSubst { raw }` (AST node) is NOT changed** — only `TokenKind::ParamBadSubst` (token). generate.rs/expand.rs/param_expansion.rs consume the AST node and are untouched. Do not confuse the two.
- **EOF / unterminated** needs no special code: `word_in_mode!`'s operand parse hits the lexer's existing `Unterminated*` error and propagates as rc=2. Verified in Task 3 Step 8 via the `${x` characterization row.
- **Not a #84 fix** — the harness must not assert any happy-path `%`/`-` behavior; #84 stays open.
- **Risk:** the subsystem is thrash-prone; mitigation is the characterization baseline (Task 1) plus the unchanged detection conditions. Any behavior delta shows up as a Task 1 harness failure and must be adjudicated, not absorbed.
