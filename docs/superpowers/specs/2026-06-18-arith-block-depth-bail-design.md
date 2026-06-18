# v185: `scan_arith_block` bails on an unbalanced close (resolves L-51) — Design

**Status:** approved 2026-06-18
**Iteration:** v185
**Origin:** Tracked divergence **L-51** (added v184). After v184 made `((` at
command position fall back to nested subshells when `scan_arith_block` finds no
matching `))`, deeply-nested subshell pipelines (kselftest `runner.sh` line 128,
`((((( … (read xs)) … )`) plus a LATER `$(( test_num + 1 ))` still failed:
`scan_arith_block` wandered past the construct and grabbed a distant `))`.

## The model (confirmed against bash)

A command starting with `(` resolves purely structurally:
- a single `(` (or `( (` with a space) → **subshell** — unambiguous;
- an adjacent `((` → **arithmetic command `((expr))` iff the two opening parens
  close as an adjacent `))` with balanced content between them**, else nested
  subshells `( (…) )`.

The deciding factor is the *close shape*, NOT whether the content is valid
arithmetic: `((echo hi))` closes as `))` → bash treats it as arithmetic and
*errors* (does not fall back); `((echo a) | cat)` does not close as `))` →
subshell. So the disambiguation is a matched-pair/tokenizing question about the
whole group. huck already implements this shape — `scan_arith_block` is the
matched-pair reader, "found `))`" → `ArithBlock`, "not `))`" → v184 rewind →
`( (`. The bug is that the reader computes the close *incorrectly*.

## Problem

`scan_arith_block` (`src/lexer.rs:2008`, the SOLE caller is the standalone `((`
site at `:734`; the `$((` expansion path uses a separate `scan_arith_body`)
tracks `(`/`)` nesting and returns `Ok` when it sees `))` at depth 0:

```rust
')' => {
    if depth == 0 && chars.peek() == Some(&')') {
        chars.next();
        return Ok(collected);
    }
    depth -= 1;          // <-- on a depth-0 `)` not forming `))`, goes to -1
    collected.push(')');
}
```

On a `)` at depth 0 that is NOT followed by `)`, it decrements `depth` below
zero and keeps scanning. But a depth-0 `)` not forming `))` means the two
opening parens of the `((` cannot close as an adjacent `))` — the group is not a
balanced arithmetic block. Instead of concluding that, the scanner runs past the
construct (into the rest of the file) until it grabs some unrelated later `))`
(e.g. a `$(( … ))`), returns a bogus `Ok(body)`, and fails downstream as
"unterminated command substitution" — so v184's `Err`→subshell fallback never
fires.

Confirmed minimal reproducers (huck error, bash clean):
- `((echo a) | cat); x=$((1+1)); echo "x=$x"` → bash `a` / `x=2`; huck errors
  (wanders into the `$((1+1))`).
- `((echo hi) >/dev/null); ((n=5)); echo "n=$n"` → bash `n=5`; huck errors
  (grabs the `((n=5))` close).

A valid `((…))` arithmetic block never contains a depth-0 unbalanced `)` before
its closing `))` — any inner paren group (`(a)`, `(5>3)`) is entered with a `(`
first, so its `)` is processed at depth ≥1 (it decrements 1→0), never at depth 0.

## Goal

Make `scan_arith_block` *fail fast* on an unbalanced close: a `)` at depth 0 not
forming `))` → return `Err` immediately. The caller then rewinds (v184) and
re-lexes as nested subshells, and the scanner never wanders to a distant `))`.
This resolves L-51 (runner.sh ×2) and completes the structural disambiguation.

## Design

In `scan_arith_block`, restructure the `)` arm so a depth-0 `)` is terminal —
either it forms `))` (→ `Ok`) or it is unbalanced (→ `Err`):

```rust
            ')' => {
                if depth == 0 {
                    if chars.peek() == Some(&')') {
                        chars.next(); // consume the second `)`
                        return Ok(collected);
                    }
                    // A `)` at depth 0 not forming `))` means the two opening
                    // `(` of the `((` cannot close as an adjacent `))` — this is
                    // not a balanced arithmetic block. Fail fast so the caller
                    // (the `((` lexer site) rewinds and re-lexes as nested
                    // subshells `( (`, instead of scanning on to an unrelated
                    // distant `))` elsewhere in the input (L-51).
                    return Err(LexError::UnterminatedArithBlock);
                }
                depth -= 1;
                collected.push(')');
            }
```

(`depth` can no longer go negative. The `(` arm and the EOF→`Err` arm are
unchanged.)

### Why this is correct and bounded

- **Valid arith unaffected:** inner paren groups in a valid `((…))` (e.g.
  `(a)+1`, `(5>3)?1:0`, `(a=3)+(b=4)`) are entered via a `(` (depth→1), so their
  `)` is processed at depth 1 (decrement 1→0), never hitting the depth-0 branch.
  The final close is still `))` at depth 0.
- **Resolves L-51:** for `((echo a) | cat)` and runner.sh's `(((((…)`, the first
  unbalanced depth-0 `)` (`a)` / `3>&1)`) now returns `Err` immediately → v184
  fallback → nested subshells (which balance at the shell-grouping level). The
  scanner stops at the construct instead of hunting a distant `))`.
- **Contained:** `scan_arith_block`'s only caller is the standalone `((` site;
  the `$((` arith-expansion path (`scan_arith_body`) is untouched.

### Behavior after the fix

- runner.sh ×2: `huck -n` silent, rc 0 = bash.
- `((echo a) | cat); x=$((1+1)); echo "x=$x"` → `a` / `x=2`; `((echo hi)
  >/dev/null); ((n=5)); echo "n=$n"` → `n=5`.
- `((1+2))`, `(( (a=3) + (b=4) ))`, `((x=(5>3)?1:0))`, `((echo a) | cat)`
  (single, no trailing `$(( ))`) — all unchanged.

## Verification

- **New bash-diff harness** `tests/scripts/arith_block_bail_diff_check.sh`
  (executing, byte-identical stdout+exit): the two L-51 reproducers (`((`-no-`))`
  subshell followed by a later `$(( ))` / `(( ))`); a runner.sh-shaped
  nested-subshell pipeline; and arith controls `((1+2))`, `(( (a=3)+(b=4) ))`,
  `((x=(5>3)?1:0))`, plus the plain `((echo a) | cat)` nested-subshell.
- **Lexer unit tests** (`src/lexer.rs` `mod tests`, near `arith_block_simple`):
  `scan_arith_block` over `"echo a) z))"` returns `Err` (bails on the depth-0
  `)`, does not reach the later `))`); over `"(a)+1))"` returns `Ok("(a)+1")`
  (valid inner group, no misfire); and `tokenize("((echo a)|cat); x=$((1+1))")`
  begins with two `Token::Op(Operator::LParen)` and contains no `ArithBlock` for
  the head (no wander to the `$((1+1))`).
- **Parse-sweep payoff:** re-run `tools/parse_sweep.sh`; confirm both runner.sh
  copies parse (`huck -n` rc 0, no stderr). Report `HUCK_GAP` from the 10
  baseline; LENIENT/CRASH stay 0; confirm no arith `HUCK_GAP` rows remain.
- **Full `cargo test`** (0 failures). UP-FRONT grep `tests/` + `src/` for
  `scan_arith_block` / arith-block tests; the existing `arith_block_*` and the
  v184 `double_paren_*` tests must stay green (real arith + the no-`))` fallback
  are unchanged). Update only genuine old-behavior tests (none expected — the
  change only affects inputs that previously wandered/errored).
- All `tests/scripts/*_diff_check.sh` green; clippy clean.

## Docs / close-out

**Resolves L-51:** delete the L-51 entry from `docs/bash-divergences.md` and
decrement the Tier-4 count (41→40). Record the iteration in
`project_huck_iterations.md` + `MEMORY.md`; update the backlog note.

## Scope boundary

In scope: the depth-0 bail in `scan_arith_block`, the new harness + lexer unit
tests, the L-51 deletion. **Not** in scope: making `scan_arith_block`
quote/`$()`-aware (a separate robustness layer for pathological `(( ")" ))` /
`$()`-with-`)`-in-string cases not seen in real code — explicitly deferred per
the v185 scope decision); the `$((` path (`scan_arith_body`); the v184 `((`
disambiguation site (unchanged — it already does the right rewind); the runtime
arith engine; the other sweep clusters. No consolidation of the four delimiter
scanners (the others are already quote/expansion-aware; only `scan_arith_block`
was the naive outlier, and this fix makes it correctly bounded).
