# v185: `scan_arith_block` bails on an unbalanced close (resolves L-51) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `scan_arith_block` fail fast on an unbalanced close (a `)` at depth 0 not forming `))` → `Err`) so it never wanders past a non-arith `((` to grab a distant `))`, resolving L-51 (kselftest runner.sh ×2) and completing the structural arith-vs-subshell disambiguation.

**Architecture:** One change in `scan_arith_block` (`src/lexer.rs:2008`, sole caller = the standalone `((` site at `:734`). A valid `((…))` arithmetic block never has a depth-0 unbalanced `)` before its closing `))` (inner groups are entered via `(`, so their `)` is processed at depth ≥1). So a `)` at depth 0 that isn't `))` proves the `((` isn't balanced arithmetic → return `Err`, which triggers v184's rewind-to-nested-subshells. Currently it decrements `depth` below zero and keeps scanning.

**Tech Stack:** Rust (huck shell). Verification via `cargo test`, `tests/scripts/*_diff_check.sh`, and `tools/parse_sweep.sh`.

**Spec:** `docs/superpowers/specs/2026-06-18-arith-block-depth-bail-design.md`

**Branch:** `v185-arith-block-depth-bail` (from `main`; do NOT implement on `main`).

---

## Confirmed bash behavior (the contract)

| Fragment | bash | huck (today) |
|---|---|---|
| `((echo a) \| cat); x=$((1+1)); echo "x=$x"` | `a`⏎`x=2` | ★ wanders → error |
| `((echo hi) >/dev/null); ((n=5)); echo "n=$n"` | `n=5` | ★ wanders → error |
| `(((echo a) \| cat) \| cat); y=$((3*3)); echo "y=$y"` | `a`⏎`y=9` | ★ wanders → error |
| `((echo a) \| cat)` | `a` | `a` (v184, unchanged) |
| `((1+2)); echo $?` | `0` | `0` (unchanged) |
| `(( (a=3) + (b=4) )); echo $((a+b))` | `7` | `7` (unchanged) |
| `((x=(5>3)?1:0)); echo $x` | `1` | `1` (unchanged) |

`★` = currently a hard error (`scan_arith_block` grabs a distant `))`).

---

### Task 1: depth-0 bail in `scan_arith_block`

**Files:**
- Modify: `src/lexer.rs:2018-2026` (the `)` arm of `scan_arith_block`)
- Test: `src/lexer.rs` (`mod tests`, near `arith_block_simple` ~`:7193`)

**Context:** `scan_arith_block` is called only from the standalone `((` site (`:734`); the `$((` expansion path uses a separate `scan_arith_body`. The `(` arm and the EOF→`Err` arm are unchanged.

- [ ] **Step 1: Create the branch**

```bash
cd /home/john/projects/shuck
git checkout main && git checkout -b v185-arith-block-depth-bail
```

- [ ] **Step 2: Restructure the `)` arm to bail on a depth-0 unbalanced close**

In `src/lexer.rs`, the `)` arm of `scan_arith_block` currently reads:

```rust
            ')' => {
                if depth == 0 && chars.peek() == Some(&')') {
                    chars.next(); // consume the second `)`
                    return Ok(collected);
                }
                depth -= 1;
                collected.push(')');
            }
```

Replace it with:

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

- [ ] **Step 3: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: compiles clean.

- [ ] **Step 4: Manual behavior check against bash**

Run:

```bash
B=target/debug/huck
chk() { printf '%s\n' "$2" > /tmp/v185.sh
  printf '%-30s huck=<%s> bash=<%s>\n' "$1" \
    "$($B /tmp/v185.sh 2>&1 | tr '\n' '|')" "$(bash --norc /tmp/v185.sh 2>&1 | tr '\n' '|')"; }
chk 'wander: subshell then $(('  '((echo a) | cat); x=$((1+1)); echo "x=$x"'
chk 'wander: subshell then (('   '((echo hi) >/dev/null); ((n=5)); echo "n=$n"'
chk 'wander: deep then $(('      '(((echo a) | cat) | cat); y=$((3*3)); echo "y=$y"'
chk 'plain nested subshell'      '((echo a) | cat)'
chk 'arith 1+2'                  '((1+2)); echo $?'
chk 'arith grouped sum'          '(( (a=3) + (b=4) )); echo $((a+b))'
chk 'arith ternary group'        '((x=(5>3)?1:0)); echo $x'
```

Expected (every line huck byte-equal to bash): `a`/`x=2`; `n=5`; `a`/`y=9`; `a`; `0`; `7`; `1`.

- [ ] **Step 5: Add lexer unit tests**

In `src/lexer.rs` `mod tests`, near `arith_block_simple` (~`:7193`), add:

```rust
    #[test]
    fn scan_arith_block_bails_on_unbalanced_close() {
        // v185 (L-51): a `)` at depth 0 not forming `))` means the `((` can't be
        // a balanced arith block — bail (Err) immediately instead of scanning on
        // for a distant `))`. The caller then falls back to nested subshells.
        let mut chars = CharCursor::new("echo a) z))");
        assert!(scan_arith_block(&mut chars).is_err());
    }

    #[test]
    fn scan_arith_block_valid_inner_group() {
        // Regression: a valid arith block whose content closes a paren group
        // (`(a)`) before the final `))` still scans — the inner `)` is processed
        // at depth 1 (decrement 1->0), never the depth-0 bail branch.
        let mut chars = CharCursor::new("(a)+1))");
        assert_eq!(scan_arith_block(&mut chars).unwrap(), "(a)+1");
    }

    #[test]
    fn double_paren_no_wander_to_distant_close() {
        // v185 (L-51): `((echo a)|cat)` has no matching `))`; the scanner must
        // NOT wander to a later `$((1+1))`'s `))`. The head lexes as nested
        // subshells (two LParens), not an ArithBlock.
        let toks = tokenize("((echo a)|cat); x=$((1+1))").unwrap();
        assert!(
            !matches!(toks[0], Token::ArithBlock(..)),
            "head must not be an ArithBlock: {toks:?}"
        );
        assert!(matches!(toks[0], Token::Op(Operator::LParen)));
        assert!(matches!(toks[1], Token::Op(Operator::LParen)));
    }
```

- [ ] **Step 6: Run the new unit tests**

Run: `cargo test --lib scan_arith_block_bails scan_arith_block_valid_inner double_paren_no_wander 2>&1 | tail -10`
(If `cargo test` rejects multiple positional filters, run them one at a time, or `cargo test --lib scan_arith_block 2>&1 | tail -10` then `cargo test --lib double_paren 2>&1 | tail -10`.)
Expected: all pass.

- [ ] **Step 7: No-regression check on existing arith/double-paren tests**

Run: `cargo test --lib arith_block double_paren 2>&1 | tail -20`
Expected: all existing `arith_block_*` (simple, with_semicolons, nested_parens, with_internal_whitespace, empty_body, unclosed_falls_back_to_lparens, single_paren_at_end_falls_back_to_lparens) and `double_paren_*` tests pass. If any fail, STOP and report (the change must preserve valid-arith + the v184 fallback).

- [ ] **Step 8: clippy**

Run: `cargo clippy 2>&1 | tail -5`
Expected: no new warnings.

- [ ] **Step 9: Commit**

```bash
git add src/lexer.rs
git commit -m "$(cat <<'EOF'
v185: scan_arith_block bails on a depth-0 unbalanced close (resolves L-51)

A `)` at depth 0 not forming `))` means the two opening `(` of a `((` cannot
close as an adjacent `))` — so it is not a balanced arithmetic block. Return
Err immediately (instead of going to negative depth and scanning on) so the
caller rewinds and re-lexes as nested subshells `( (`, rather than wandering
to an unrelated distant `))` (e.g. a later `$(( ))`). Resolves L-51
(kselftest runner.sh ×2). Valid arith is unaffected — inner paren groups
close at depth >=1, never the depth-0 branch.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS, commit SHA, the Step 4 table (all must match), Step 6 + Step 7 results, any concerns. If a code site doesn't match the line numbers above, find it by the surrounding code shown and report the discrepancy.

---

### Task 2: Bash-diff harness `arith_block_bail_diff_check.sh`

**Files:**
- Create: `tests/scripts/arith_block_bail_diff_check.sh`

**Context:** Harnesses pipe each fragment to bash AND huck and assert byte-identical output. All cases produce clean stdout with rc 0 in bash. Model: `tests/scripts/double_paren_subshell_diff_check.sh`.

- [ ] **Step 1: Write the harness**

Create `tests/scripts/arith_block_bail_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v185 (resolves L-51): a `((` at command
# position that is NOT a balanced arith block (no adjacent `))`) must lex as
# nested subshells and NOT make the arith-block scanner wander to an unrelated
# distant `))` (e.g. a later `$(( ))`). Kernel runner.sh hit this. rc 0 in bash
# → compare full stdout+exit.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# L-51: nested-subshell `((` (no `))`) followed by a later $(( )) / (( )).
check "subshell then dollar-arith" '((echo a) | cat); x=$((1+1)); echo "x=$x"'
check "subshell then arith cmd"    '((echo hi) >/dev/null); ((n=5)); echo "n=$n"'
check "deep subshell then arith"   '(((echo a) | cat) | cat); y=$((3*3)); echo "y=$y"'
check "two such constructs"        '((echo a)|cat); ((echo b)|cat); z=$((4+4)); echo "z=$z"'

# Controls — plain nested subshell + valid arith (unchanged).
check "plain nested subshell"      '((echo a) | cat)'
check "arith 1+2 exit"             '((1+2)); echo "rc=$?"'
check "arith grouped sum"          '(( (a=3) + (b=4) )); echo "sum=$((a+b))"'
check "arith ternary group"        '((x=(5>3)?1:0)); echo "x=$x"'
check "arith increment"            '((n=3)); ((n++)); echo "n=$n"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make executable, build, run**

Run:

```bash
chmod +x tests/scripts/arith_block_bail_diff_check.sh
cargo build 2>&1 | tail -2
bash tests/scripts/arith_block_bail_diff_check.sh
```

Expected: `Total: 9, Pass: 9, Fail: 0`, exit 0. If a case FAILs, STOP and report (do NOT weaken).

- [ ] **Step 3: Prove the harness is non-tautological**

```bash
git worktree add /tmp/huck-prefix main 2>&1 | tail -1
( cd /tmp/huck-prefix && cargo build 2>&1 | tail -1 )
HUCK_BIN=/tmp/huck-prefix/target/debug/huck bash tests/scripts/arith_block_bail_diff_check.sh; echo "prefix-exit=$?"
git worktree remove --force /tmp/huck-prefix
```

Expected: the 4 "wander" cases FAIL against the pre-fix binary (the scanner grabbed a distant `))`); the 5 controls pass pre-fix; prefix-exit=1. If it PASSES pre-fix, the harness is tautological — STOP and report. (`main` is the pre-v185 baseline.)

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/arith_block_bail_diff_check.sh
git commit -m "$(cat <<'EOF'
v185: bash-diff harness for arith-block depth-0 bail (L-51)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS, commit SHA, Step 2 Total/Pass/Fail line, Step 3 prefix-exit + which cases failed pre-fix (proving non-tautology).

---

### Task 3: Parse-sweep payoff, regression, and L-51 deletion

**Files:**
- Modify: `docs/bash-divergences.md` (DELETE L-51, decrement Tier-4 count 41→40)
- Modify (only if found): old-behavior tests from the up-front grep
- Run: full `cargo test`, all `tests/scripts/*_diff_check.sh`, `tools/parse_sweep.sh`

- [ ] **Step 1: Up-front grep**

Run:

```bash
grep -rn "scan_arith_block\|UnterminatedArithBlock\|arith.*block" src/ tests/ | grep -iv "fn scan_arith_block\|fn .*arith_block(" | head -30
```

Classify any hit UPDATE (encodes the old wander/no-bail behavior) vs LEAVE (real-arith / v184 fallback / the v185 new tests / unrelated). Report each. (None expected to need UPDATE — the change only affects inputs that previously wandered/errored.)

- [ ] **Step 2: Full test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: 0 failures across all unit + integration binaries. A failure on an arith-block assertion: verify against bash; if it encoded the old wander behavior, UPDATE; if it's a real-arith / v184-fallback test, STOP and report (the fix is too broad).

- [ ] **Step 3: Whole bash-diff harness suite**

Run:

```bash
cargo build 2>&1 | tail -1
for s in tests/scripts/*_diff_check.sh; do
  out=$(bash "$s" 2>&1); rc=$?
  printf '%s rc=%s %s\n' "$s" "$rc" "$(echo "$out" | tail -1)"
done | grep -v 'Fail: 0' || echo "ALL HARNESSES GREEN"
```

Expected: `ALL HARNESSES GREEN` (incl. `double_paren_subshell_diff_check.sh` from v184).

- [ ] **Step 4: Parse-sweep payoff**

```bash
tools/parse_sweep.sh tools/scripts.tsv tools/parse_results.tsv 2>&1 | tail -15
for F in /usr/src/linux-headers-6.8.0-110/tools/testing/selftests/kselftest/runner.sh \
  /usr/src/linux-headers-6.8.0-124/tools/testing/selftests/kselftest/runner.sh; do
  echo "=== $F ==="; ./target/debug/huck -n "$F"; echo "huck-n rc=$?"; bash -n "$F"; echo "bash-n rc=$?"
done
echo "=== remaining arith gaps? ==="
awk -F'\t' '$3=="HUCK_GAP" && tolower($7) ~ /arith/{print $6}' tools/parse_results.tsv || echo "(none)"
```

Expected: both runner.sh copies `huck -n` SILENT rc 0 = bash; no arith `HUCK_GAP` rows remain. Report the new HUCK_GAP vs the 10 baseline; LENIENT/CRASH stay 0; note any remaining row that's a DIFFERENT construct.

- [ ] **Step 5: Delete the L-51 entry**

In `docs/bash-divergences.md`, delete the entire `L-51` bullet (the paragraph starting `- **L-51: \`scan_arith_block\` mismatches`). Then decrement the Tier-4 count in the Summary table (`| Low-impact (Tier 4) | 41 |` → `40`). Verify: `grep -n "L-51" docs/bash-divergences.md || echo "no L-51 remains"`.

- [ ] **Step 6: Commit**

```bash
git add docs/bash-divergences.md
# add any test files updated in Step 1/2
git commit -m "$(cat <<'EOF'
v185: resolve L-51 (delete entry); regression + parse-sweep pass

runner.sh ×2 now parse (huck -n rc 0 = bash); no arith gaps remain. Full
cargo test + all bash-diff harnesses green.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS, commit SHA, the Step 1 grep classification, Step 2 cargo test summary, Step 3 result, Step 4 per-file rc + new HUCK_GAP vs 10 + confirmation no arith rows remain, and that L-51 is gone with the count decremented.

---

## Final review (whole-branch, before merge)

- [ ] **Step 1: Review the full diff** — `git diff main...HEAD`. Confirm only the `scan_arith_block` `)` arm + 3 lexer unit tests, the new harness, and the L-51 deletion changed.
- [ ] **Step 2: Scope boundary** — `scan_arith_block`'s `(`/EOF arms unchanged; the `$((` path (`scan_arith_body`), the v184 `((` disambiguation site, and the runtime unchanged; no quote/`$()`-awareness added (deferred per scope).
- [ ] **Step 3: Hand off** — project merge ritual (AskUserQuestion before merging to main): merge `--no-ff`, push, delete the local branch, record v185 in `project_huck_iterations.md` + `MEMORY.md`, update the backlog note.

---

## Self-Review (plan vs spec)

**1. Spec coverage:**
- depth-0 bail in `scan_arith_block` `)` arm → Task 1 Step 2. ✓
- Lexer unit tests (bail on unbalanced; valid inner group still scans; no wander to distant `))`) → Task 1 Step 5. ✓
- New harness with wander reproducers + arith/subshell controls + non-tautology → Task 2. ✓
- Parse-sweep: runner.sh ×2 clear + HUCK_GAP report + no arith rows remain → Task 3 Step 4. ✓
- Full `cargo test` + up-front grep (arith_block/double_paren stay green) → Task 3 Steps 1-2 + Task 1 Step 7. ✓
- Delete L-51 + decrement count → Task 3 Step 5. ✓
- All `*_diff_check.sh` green + clippy → Task 3 Step 3 + Task 1 Step 8. ✓

**2. Placeholder scan:** No TBD/TODO. Every code step shows complete before/after code; every run step shows command + expected output.

**3. Type consistency:** `scan_arith_block(&mut CharCursor) -> Result<String, LexError>` signature unchanged (only the `)` arm body). `LexError::UnterminatedArithBlock` is the existing variant returned on EOF — reused for the unbalanced-close case (same downstream handling: the `((` caller's `Err` arm rewinds). Unit tests use the established `CharCursor::new(...)` + `tokenize(...)` + `matches!(t, Token::ArithBlock(..) | Token::Op(Operator::LParen))` idioms.

**Resolved during planning:** verified all 7 contract fragments' bash output (3 wander reproducers fail pre-fix, 4 controls unchanged); that valid arith with inner groups (`(a)+1`, `(a=3)+(b=4)`, `(5>3)?1:0`) closes inner `)` at depth ≥1 (no bail misfire); that `scan_arith_block`'s sole caller is the standalone `((` site (the `$((` path uses `scan_arith_body`).
