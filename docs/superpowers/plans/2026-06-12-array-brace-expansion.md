# huck v144 — brace expansion in array-literal elements Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Apply brace expansion to BARE array-literal elements so `a=({1..3})` → `1 2 3`, `a=(f{1,2}.x)` → `f1.x f2.x`, matching bash.

**Architecture:** Reuse huck's existing lexer-level brace machinery (`emit_word_with_braces` + the sentinel scheme that protects expansions/quotes) by factoring out a `brace_expand_parts` helper, then call it for each bare element in `read_array_literal`. Brace expansion runs at lex time (textual, first); the executor's existing `expand_array_elements`→`glob_expand_word` then does param/cmdsub + word-split + glob per product — bash's order exactly.

**Tech Stack:** Rust; `src/lexer.rs` (`emit_word_with_braces` ~1200, `read_array_literal` ~2415). Subscripted elements stay single (out of scope, documented).

**Reference:** spec at `docs/superpowers/specs/2026-06-12-array-brace-expansion-design.md`.

**GIT SAFETY:** Do NOT `git checkout <sha>` (a detached HEAD lost commits in a prior iteration). Stay on `v144-array-brace-expansion`. Every commit ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**Build note:** BINARY crate — `cargo test --bin huck <filter>`, `cargo clippy --all-targets` (NOT `--lib`). The lexer tests are in-crate (`src/lexer.rs` mod tests). Builds take minutes.

---

### Task 1: Brace-expand bare array-literal elements

**Files:**
- Modify: `src/lexer.rs` (`emit_word_with_braces` ~1200-1218; `read_array_literal` ~2442-2443; add tests in `mod tests`)

- [ ] **Step 1: Write the failing tests** — add to `src/lexer.rs` `mod tests`. First add a small helper (if one like it isn't already present), then the tests:

```rust
fn array_lit(w: &Word) -> &[ArrayLiteralElement] {
    w.0.iter()
        .find_map(|p| match p {
            WordPart::ArrayLiteral(els) => Some(els.as_slice()),
            _ => None,
        })
        .expect("ArrayLiteral part present")
}

#[test]
fn brace_expand_parts_literal_splits() {
    let parts = vec![WordPart::Literal { text: "x{a,b}".to_string(), quoted: false }];
    let out = brace_expand_parts(parts).unwrap();
    assert_eq!(out.len(), 2);
}

#[test]
fn brace_expand_parts_no_brace_passthrough() {
    let parts = vec![WordPart::Literal { text: "plain".to_string(), quoted: false }];
    let out = brace_expand_parts(parts).unwrap();
    assert_eq!(out.len(), 1);
}

#[test]
fn array_literal_brace_expands_bare_range() {
    let assigns = parse_assignments("a=({1..3} z)");
    let els = array_lit(&assigns[0].value);
    assert_eq!(els.len(), 4); // 1 2 3 z
    assert!(els.iter().all(|e| e.subscript.is_none()));
}

#[test]
fn array_literal_brace_cartesian() {
    let assigns = parse_assignments("a=({a,b}{1,2})");
    let els = array_lit(&assigns[0].value);
    assert_eq!(els.len(), 4); // a1 a2 b1 b2
}

#[test]
fn array_literal_single_element_brace_is_literal() {
    let assigns = parse_assignments("a=({1} z)");
    let els = array_lit(&assigns[0].value);
    assert_eq!(els.len(), 2); // {1} stays one element
}

#[test]
fn array_literal_quoted_brace_not_expanded() {
    let assigns = parse_assignments("a=(\"{1,2}\" x)");
    let els = array_lit(&assigns[0].value);
    assert_eq!(els.len(), 2); // "{1,2}" stays one element
}

#[test]
fn array_literal_subscripted_brace_stays_single() {
    let assigns = parse_assignments("a=([2]=x{a,b} z)");
    let els = array_lit(&assigns[0].value);
    // subscripted element NOT brace-expanded (1) + bare `z` (1) = 2
    assert_eq!(els.len(), 2);
    assert!(els[0].subscript.is_some());
    assert!(els[1].subscript.is_none());
}
```
NOTE: confirm `parse_assignments`, `Word`, `WordPart`, `ArrayLiteralElement` are in scope in the test module (existing tests like `compound_rhs_is_array_literal` use `parse_assignments` + the same `find_map`, so they are). If an `array_lit`-equivalent helper already exists, reuse it instead of adding a duplicate.

- [ ] **Step 2: Run — verify failure**

Run: `cargo test --bin huck array_literal_brace 2>&1 | tail -20` and `cargo test --bin huck brace_expand_parts 2>&1 | tail -10`
Expected: `brace_expand_parts` undefined (compile error) until Step 3a; then `array_literal_brace_expands_bare_range` FAILs (els.len()==2 not 4) and `array_literal_brace_cartesian` FAILs (1 not 4). The `subscripted`/`quoted`/`single` tests may pass even before (they assert the unchanged-count cases). Record.

- [ ] **Step 3a: Factor `brace_expand_parts` out of `emit_word_with_braces`** — `src/lexer.rs`. The current `emit_word_with_braces` (lines ~1200-1218) is:
```rust
fn emit_word_with_braces(
    tokens: &mut Vec<Token>,
    parts: Vec<WordPart>,
) -> Result<usize, LexError> {
    if !word_contains_unquoted_brace(&parts) {
        tokens.push(Token::Word(Word(parts)));
        return Ok(1);
    }
    let (concat, placeholders) = build_concat_with_sentinels(&parts);
    let expansions = crate::brace_expand::expand(&concat)
        .map_err(|_| LexError::BraceExpansionLimit)?;
    let mut count = 0;
    for s in expansions {
        let new_parts = split_on_sentinels(&s, &placeholders);
        tokens.push(Token::Word(Word(new_parts)));
        count += 1;
    }
    Ok(count)
}
```
Replace it with a `brace_expand_parts` helper plus a thin `emit_word_with_braces` wrapper (behavior-preserving — same return-count contract):
```rust
/// Brace-expands a word's `parts` into one-or-more parts-lists. With no
/// unquoted brace, returns the single input list unchanged. Non-literal
/// parts (expansions, quoted runs) are sentinel-protected so only literal
/// source braces expand. Shared by `emit_word_with_braces` (command words)
/// and `read_array_literal` (bare array elements).
fn brace_expand_parts(parts: Vec<WordPart>) -> Result<Vec<Vec<WordPart>>, LexError> {
    if !word_contains_unquoted_brace(&parts) {
        return Ok(vec![parts]);
    }
    let (concat, placeholders) = build_concat_with_sentinels(&parts);
    let expansions = crate::brace_expand::expand(&concat)
        .map_err(|_| LexError::BraceExpansionLimit)?;
    Ok(expansions
        .into_iter()
        .map(|s| split_on_sentinels(&s, &placeholders))
        .collect())
}

/// Emits the word for `parts` into `tokens`, expanding any unquoted braces.
/// Returns the number of tokens pushed (1 normally, or one per brace-expansion
/// product) so offset-tracking callers keep the offset sidecar in lockstep.
fn emit_word_with_braces(
    tokens: &mut Vec<Token>,
    parts: Vec<WordPart>,
) -> Result<usize, LexError> {
    let products = brace_expand_parts(parts)?;
    let count = products.len();
    for p in products {
        tokens.push(Token::Word(Word(p)));
    }
    Ok(count)
}
```

- [ ] **Step 3b: Brace-expand bare elements in `read_array_literal`** — `src/lexer.rs`. The loop body currently ends (~2442-2443):
```rust
        let value = read_array_element_word(chars, opts)?;
        elements.push(ArrayLiteralElement { subscript, value });
```
Replace with:
```rust
        let value = read_array_element_word(chars, opts)?;
        match subscript {
            // Subscripted `[i]=value` keeps single-value semantics (brace stays
            // literal — matches bash for associative subscripts; the indexed
            // `[i]=val{brace}` edge is a documented low-impact divergence).
            Some(sub) => {
                elements.push(ArrayLiteralElement { subscript: Some(sub), value });
            }
            // Bare elements brace-expand (textual, first) into N elements; the
            // executor then word-splits/globs each. Reuses the command-word path.
            None => {
                for p in brace_expand_parts(value.0)? {
                    elements.push(ArrayLiteralElement { subscript: None, value: Word(p) });
                }
            }
        }
```

- [ ] **Step 4: Build, run tests, no-regression, clippy**

Run: `cargo build 2>&1 | tail -5`
Run: `cargo test --bin huck array_literal 2>&1 | tail -15` → the new array tests pass.
Run: `cargo test --bin huck brace 2>&1 | tail -15` → `brace_expand_parts` tests pass AND existing brace-expansion tests (command words) still pass (the wrapper is behavior-preserving).
Run: `cargo clippy --all-targets 2>&1 | tail -8` → no new warnings.

- [ ] **Step 5: Commit**
```bash
git add src/lexer.rs
git commit -m "$(printf 'feat: brace expansion in bare array-literal elements\n\n`a=({1..3})` / `a=(f{1,2}.x)` now brace-expand like bash. Factors a\nbrace_expand_parts helper out of emit_word_with_braces and calls it for\nbare elements in read_array_literal. Subscripted elements stay single.\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: 64th bash-diff harness

**Files:**
- Create: `tests/scripts/array_brace_expansion_diff_check.sh`

- [ ] **Step 1: Write the harness** — look at an existing one first (`cat tests/scripts/array_brace_expansion_diff_check.sh` won't exist; use `cat tests/scripts/cd_completion_diff_check.sh` for house style). Create `tests/scripts/array_brace_expansion_diff_check.sh`:
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v144: brace expansion in array literals.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "range"        'a=({1..3} z); declare -p a'
check "cartesian"    'a=({a,b}{1,2}); declare -p a'
check "single brace" 'a=({1} z); declare -p a'
check "literal nums" 'a=(f{1,2}.x); declare -p a'
check "quoted brace" 'a=("{1,2}" x); declare -p a'
check "var brace"    'v={1,2}; a=($v); declare -p a'
check "brace+cmdsub" 'a=(pre{1,2}$(echo m n)); declare -p a'
check "sub then bare" 'a=([0]=p q{1,2} r); declare -p a'
check "append"       'a=(x); a+=({1,2}); declare -p a'
check "local array"  'f(){ local a=({1..3}); echo "${#a[@]}:${a[*]}"; }; f'
check "assoc literal" 'declare -A m=([k]=x{a,b}); echo "[${m[k]}]"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```
NOTE: the assoc case uses `echo "[${m[k]}]"` (value check) rather than `declare -p m`, to avoid any pre-existing associative-`declare -p` formatting divergence confounding the brace feature. The indexed cases use `declare -p a` (it encodes indices+values+count exactly — what we're testing).

- [ ] **Step 2: chmod + build + run**

Run: `chmod +x tests/scripts/array_brace_expansion_diff_check.sh && cargo build 2>&1 | tail -2 && bash tests/scripts/array_brace_expansion_diff_check.sh`
Expected: `Total: 11, Pass: 11, Fail: 0`.
IMPORTANT: If a case FAILs, paste the diff and STOP — do NOT weaken the harness. EXCEPTION: if an INDEXED `declare -p a` case fails ONLY due to a pre-existing `declare -p` output-format difference (spacing/quoting) UNRELATED to element count/indices, report it as a separate finding; otherwise treat any FAIL as a real brace-feature divergence to fix.

- [ ] **Step 3: Commit**
```bash
git add tests/scripts/array_brace_expansion_diff_check.sh
git commit -m "$(printf 'test: 64th bash-diff harness for array-literal brace expansion\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 3: Docs — delete M-102, add the indexed-subscript-brace divergence

**Files:**
- Modify: `docs/bash-divergences.md`

- [ ] **Step 1: Delete the stale M-102 entry**

Find M-102 (`grep -n "M-102" docs/bash-divergences.md`) — it's in the Tier-2 (Missing features) "Builtins (other)" group. DELETE the entire `- **M-102: …**` entry (it's verified already fixed by v117). Decrement the Tier-2 count in the Summary table: `| Missing features (Tier 2) | 20 |` → `19` (verify the current value is 20 before editing).

- [ ] **Step 2: Add the new low-impact entry**

In the Tier-4 (Low-impact) section, add a new entry with the next free L-number (`grep -n "L-36\|L-37" docs/bash-divergences.md` — L-36 is latest from v143, so use **L-37**), matching the house format:
```
- **L-37: indexed subscripted array element with a literal brace** — `[deferred]`, low (v144). v144 brace-expands BARE array-literal elements (`a=({1..3})`). An INDEXED subscripted element whose value contains a literal brace — `a=([2]=x{a,b})` — keeps the value literal in huck (`a[2]="x{a,b}"`), whereas bash brace-expands the whole `[i]=…` word into BARE literals dropping the subscript (`[0]="[2]=xa" [1]="[2]=xb"`). ASSOCIATIVE subscripts (`declare -A m=([k]=x{a,b})`) keep the brace literal in BOTH shells (huck matches bash). Pathological (no real script writes `[i]=val{brace}`); bash's own indexed-vs-associative behavior here is surprising. Low impact.
```
Increment the Tier-4 count in the Summary table: `| Low-impact (Tier 4) | 31 |` → `32` (verify current value is 31 before editing).

- [ ] **Step 3: Verify + commit**

Run: `grep -n "M-102\|L-37\|Missing features (Tier 2)\|Low-impact (Tier 4)" docs/bash-divergences.md` → M-102 gone, L-37 present, counts 19 and 32.
```bash
git add docs/bash-divergences.md
git commit -m "$(printf 'docs: delete fixed M-102; add L-37 (indexed subscript+brace edge)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 4: Full regression

**Files:** none (verification only)

- [ ] **Step 1: Full suite**

Run: `cargo test 2>&1 | grep -E "test result: FAILED|[1-9][0-9]* failed|error\[" | head -20 || echo NONE`
Expected: NONE (zero failures; baseline ~3100 tests after v143, plus the ~7 new lexer tests).

- [ ] **Step 2: Lexer + array + brace paths explicitly**

Run: `cargo test --bin huck lexer 2>&1 | tail -6` (the file this iteration touches).
Run: `cargo test --bin huck array 2>&1 | tail -6` (array-literal expansion tests, incl. v117's).
Run: `cargo test --bin huck brace 2>&1 | tail -6` (command-word brace tests must be unchanged).

- [ ] **Step 3: All bash-diff harnesses**

Run: `cargo build 2>&1 | tail -2 && for f in tests/scripts/*_diff_check.sh; do printf '== %s == ' "$f"; bash "$f" >/dev/null 2>&1 && echo OK || echo "FAIL ($f)"; done`
Expected: every harness `OK` (incl. the new `array_brace_expansion_diff_check.sh`).

- [ ] **Step 4: Clippy**

Run: `cargo clippy --all-targets 2>&1 | tail -8` → clean.

- [ ] **Step 5: Payoff spot-check**

Run (after `cargo build`):
`target/debug/huck -c 'a=({1..3} f{x,y}.txt); declare -p a'`
Expected (matches bash): `declare -a a=([0]="1" [1]="2" [2]="3" [3]="fx.txt" [4]="fy.txt")`. Paste it next to `bash -c '…'`.

- [ ] **Step 6: Commit (only if a verification-driven fix was needed)**

If Steps 1-4 surfaced a real issue, make the SMALLEST fix, re-run, commit with the trailer. Otherwise no commit — verification only.

---

## Notes for the implementer
- **`emit_word_with_braces` must stay behavior-preserving** — it now wraps `brace_expand_parts`; the return-count contract (callers push the start offset that many times) is unchanged. The existing command-word brace tests are the guard.
- **Only BARE elements expand** — subscripted `[i]=value` elements are pushed unchanged. Do not brace-expand the subscript or a subscripted value.
- **`Word` is `Word(pub Vec<WordPart>)`** — `value.0` moves the inner parts; `Word(p)` rewraps each product.
- **Brace expansion is lex-time/textual** — it acts only on literal source braces; `$var`/`$(…)` are sentinel-protected, so `v={1,2}; a=($v)` stays one element (no re-expansion). The `var brace` harness case guards this.
