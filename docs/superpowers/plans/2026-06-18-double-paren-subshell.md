# v184: `((` command — arithmetic vs nested subshell disambiguation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make huck parse `((` at command position as nested subshells `( (…) )` when there is no matching `))`, instead of hard-erroring as an arithmetic block — clearing the parse-sweep "unterminated `((` arith" cluster (zdiff, kselftest runner.sh ×2).

**Architecture:** One change at the `((` lexer site (`src/lexer.rs:723-729`). Today it calls `scan_arith_block` and propagates its `Err`. The fix saves the cursor at the second `(`, tries `scan_arith_block`, and on `Err` rewinds + emits a single `Token::Op(LParen)` (the second `(` then re-lexes as another LParen → `( (` nested subshells). Mirrors the v177 `$((` disambiguation. Purely additive — only inputs that currently hard-error change.

**Tech Stack:** Rust (huck shell). Verification via `cargo test`, `tests/scripts/*_diff_check.sh`, and `tools/parse_sweep.sh`.

**Spec:** `docs/superpowers/specs/2026-06-18-double-paren-subshell-design.md`

**Branch:** `v184-double-paren-subshell` (from `main`; do NOT implement on `main`).

---

## Confirmed bash behavior (the contract)

| Fragment | bash | huck (today) |
|---|---|---|
| `((echo a) \| cat)` | `a` | ★ unterminated `((` |
| `((echo a) >/dev/null; echo b)` | `b` | ★ |
| `((echo hi >&2) 2>&1 \| cat)` | `hi` | ★ |
| `(((  echo a ) ) )` | `a` | ★ |
| `((1+2)) && echo t` | `t` | `t` (unchanged) |
| `((x=5)); echo $x` | `5` | `5` (unchanged) |
| `((n=3)); ((n++)); echo $n` | `4` | `4` (unchanged) |

`★` = currently a hard "unterminated `((` arithmetic block" error. (`((echo hi))` errors in BOTH shells — `))` present → arith → bad expr — so it is NOT a byte-identical harness case; covered by a unit test.)

---

### Task 1: `((` fallback to nested subshells in the lexer

**Files:**
- Modify: `src/lexer.rs:723-729` (the `((` branch of the `'('` arm)
- Test: `src/lexer.rs` (`mod tests`, near `arith_block_simple` ~`:7179`)

**Context:** In the main tokenizer's `'('` arm, after flushing any pending word, the code checks for a second `(`. `scan_arith_block` (`:1994`) scans to a depth-0 `))`, returning `Ok(body)` or `Err(UnterminatedArithBlock)`. `Token`, `Operator`, `CharCursor` are in scope. `CharCursor` is `Clone` (v177 uses `chars.clone()` / `*chars = saved` for the same rewind pattern in `scan_dollar_expansion`). The shared `push_pos!(c_off, c_line)` at `:730` runs once after the if/else.

- [ ] **Step 1: Create the branch**

```bash
cd /home/john/projects/shuck
git checkout main && git checkout -b v184-double-paren-subshell
```

- [ ] **Step 2: Add the rewind-on-failure fallback**

In `src/lexer.rs`, the `((` branch currently reads:

```rust
                if chars.peek() == Some(&'(') {
                    chars.next(); // consume the second `(`
                    let body = scan_arith_block(&mut chars)?;
                    tokens.push(Token::ArithBlock(body, opts));
                } else {
                    tokens.push(Token::Op(Operator::LParen));
                }
```

Replace it with:

```rust
                if chars.peek() == Some(&'(') {
                    // `((` is an arithmetic command ONLY if a matching `))` is
                    // found; otherwise bash treats it as nested subshells `( (`.
                    // Save the cursor at the second `(`, try the arith block, and
                    // on failure rewind + emit a single LParen (the first `(`); the
                    // second `(` then re-lexes as another LParen. A `((` that DOES
                    // close as `))` but isn't valid arithmetic stays an ArithBlock
                    // → arith error at parse/eval, matching bash. Mirrors the v177
                    // `$((` disambiguation.
                    let saved = chars.clone();
                    chars.next(); // consume the second `(`
                    match scan_arith_block(&mut chars) {
                        Ok(body) => tokens.push(Token::ArithBlock(body, opts)),
                        Err(_) => {
                            *chars = saved;
                            tokens.push(Token::Op(Operator::LParen));
                        }
                    }
                } else {
                    tokens.push(Token::Op(Operator::LParen));
                }
```

- [ ] **Step 3: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: compiles clean.

- [ ] **Step 4: Manual behavior check against bash**

Run:

```bash
B=target/debug/huck
chk() { printf '%b\n' "$2" > /tmp/v184.sh
  printf '%-24s huck=<%s> bash=<%s>\n' "$1" \
    "$($B /tmp/v184.sh 2>&1 | tr '\n' '|')" "$(bash --norc /tmp/v184.sh 2>&1 | tr '\n' '|')"; }
chk 'nested pipe'      '((echo a) | cat)'
chk 'nested redir'     '((echo a) >/dev/null; echo b)'
chk 'nested pipe redir' '((echo hi >&2) 2>&1 | cat)'
chk 'deep nested'      '(((  echo a ) ) )'
chk 'arith control'    '((1+2)) && echo t'
chk 'arith assign'     '((x=5)); echo $x'
chk 'arith incr'       '((n=3)); ((n++)); echo $n'
```

Expected (every line huck byte-equal to bash): `a`, `b`, `hi`, `a`, `t`, `5`, `4`.

- [ ] **Step 5: Add lexer unit tests**

In `src/lexer.rs` `mod tests`, near `arith_block_simple` (~`:7179`), add:

```rust
    #[test]
    fn double_paren_nested_subshell_not_arith() {
        // v184: `((echo a) | cat)` has no matching `))` → nested subshells `( (`,
        // NOT an arithmetic block. Lexes to two LParens, no ArithBlock.
        let toks = tokenize("((echo a) | cat)").unwrap();
        assert!(
            !toks.iter().any(|t| matches!(t, Token::ArithBlock(..))),
            "must not be an ArithBlock: {toks:?}"
        );
        assert!(matches!(toks[0], Token::Op(Operator::LParen)));
        assert!(matches!(toks[1], Token::Op(Operator::LParen)));
    }

    #[test]
    fn double_paren_real_arith_still_arithblock() {
        // v184 regression: a `((` that DOES close as `))` stays an ArithBlock.
        let toks = tokenize("((1+2))").unwrap();
        assert_eq!(toks.len(), 1);
        assert!(matches!(toks[0], Token::ArithBlock(..)));
    }

    #[test]
    fn double_paren_deep_nesting_not_arith() {
        // v184: `((( echo a ) ) )` — the closing parens are not adjacent, so no
        // `))` for the outer `((` → LParens, not an ArithBlock.
        let toks = tokenize("((( echo a ) ) )").unwrap();
        assert!(
            !toks.iter().any(|t| matches!(t, Token::ArithBlock(..))),
            "must not be an ArithBlock: {toks:?}"
        );
    }
```

- [ ] **Step 6: Run the new unit tests**

Run: `cargo test --lib double_paren 2>&1 | tail -10`
Expected: 3 passed, 0 failed.

- [ ] **Step 7: Run the existing arith-block + dbracket tests (no regression)**

Run: `cargo test --lib arith_block dbracket 2>&1 | tail -15`
Expected: all pass (real arith blocks unchanged; `[[ ((` suppression unaffected — the fix only touches the `Err` path).

- [ ] **Step 8: clippy**

Run: `cargo clippy 2>&1 | tail -5`
Expected: no new warnings.

- [ ] **Step 9: Commit**

```bash
git add src/lexer.rs
git commit -m "$(cat <<'EOF'
v184: `((` falls back to nested subshells when not a valid arith block

`((` at command position was always lexed as an arithmetic block; when no
matching `))` was found it hard-errored ("unterminated '((' arithmetic
block"). bash treats `((` as arithmetic only if `))` is found, else as nested
subshells `( (`. Now scan_arith_block failure rewinds and emits a single
LParen (the second `(` re-lexes as another), matching bash. Purely additive
(only currently-erroring inputs change). Mirrors the v177 `$((`
disambiguation. Clears the parse-sweep `((`-arith cluster (zdiff, runner.sh).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS, commit SHA, the Step 4 table (all must match), Step 6 + Step 7 results, any concerns. If a code site doesn't match the line numbers above, find it by the surrounding code shown and report the discrepancy.

---

### Task 2: Bash-diff harness `double_paren_subshell_diff_check.sh`

**Files:**
- Create: `tests/scripts/double_paren_subshell_diff_check.sh`

**Context:** Harnesses pipe each fragment to bash AND huck and assert byte-identical output. All v184 cases produce clean stdout with rc 0 in bash. Model: `tests/scripts/dollar_quote_forms_diff_check.sh`.

- [ ] **Step 1: Write the harness**

Create `tests/scripts/double_paren_subshell_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v184: `((` at command position is
# nested subshells `( (…) )` when there is no matching `))` (huck used to
# hard-error "unterminated '((' arithmetic block": kernel zdiff / runner.sh).
# A `((` that DOES close as `))` stays arithmetic. rc 0 in bash → compare full
# stdout+exit.
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

# `((` as nested subshells (no matching `))`).
check "nested subshell + pipe"  '((echo a) | cat)'
check "nested + redir + seq"    '((echo a) >/dev/null; echo b)'
check "nested pipe with redir"  '((echo hi >&2) 2>&1 | cat)'
check "deeply nested spaced"    '(((  echo a ) ) )'
check "nested two subshells"    '((echo a) (echo b))2>/dev/null || echo both-fail'

# Arithmetic controls (DO close as `))` — unchanged).
check "arith true"              '((1+2)) && echo arith-true'
check "arith assign"            '((x=5)); echo "x=$x"'
check "arith increment"         '((n=3)); ((n++)); echo "n=$n"'
check "arith false exit"        '((0)); echo "rc=$?"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

NOTE on `nested two subshells`: `((echo a) (echo b))` errors in BOTH shells (`))` present → arith → bad expr); appending `2>/dev/null || echo both-fail` makes the case byte-identical (both print `both-fail`, exit 0). If on running it the bash and huck output still differ (different error routing), REPLACE this one case with another no-`))` nested-subshell shape that both accept (e.g. `(( echo a ; echo b ) )` → `a`/`b`) and report the change. Do NOT weaken the assertion mechanism.

- [ ] **Step 2: Make executable, build, run**

Run:

```bash
chmod +x tests/scripts/double_paren_subshell_diff_check.sh
cargo build 2>&1 | tail -2
bash tests/scripts/double_paren_subshell_diff_check.sh
```

Expected: `Total: 9, Pass: 9, Fail: 0`, exit 0. If a case FAILs, STOP and report the diff; for the `nested two subshells` case apply the NOTE above; otherwise do NOT weaken a case.

- [ ] **Step 3: Prove the harness is non-tautological**

```bash
git worktree add /tmp/huck-prefix main 2>&1 | tail -1
( cd /tmp/huck-prefix && cargo build 2>&1 | tail -1 )
HUCK_BIN=/tmp/huck-prefix/target/debug/huck bash tests/scripts/double_paren_subshell_diff_check.sh; echo "prefix-exit=$?"
git worktree remove --force /tmp/huck-prefix
```

Expected: the 4-5 nested-subshell cases FAIL against the pre-fix binary (they hard-errored); the arith controls pass pre-fix; prefix-exit=1. If it PASSES pre-fix, the harness is tautological — STOP and report. (`main` is the pre-v184 baseline.)

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/double_paren_subshell_diff_check.sh
git commit -m "$(cat <<'EOF'
v184: bash-diff harness for `((` arith-vs-nested-subshell

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS, commit SHA, Step 2 Total/Pass/Fail line, Step 3 prefix-exit + which cases failed pre-fix (proving non-tautology), and whether the `nested two subshells` NOTE was applied.

---

### Task 3: Parse-sweep payoff and regression

**Files:**
- Modify (only if found): old-behavior tests from the up-front grep
- Run: full `cargo test`, all `tests/scripts/*_diff_check.sh`, `tools/parse_sweep.sh`

**Context — UP-FRONT grep (v178+ lesson):** grep for tests asserting the old `((`-hard-error behavior or `((`-at-command-position lexing. The existing `arith_block_*` and `dbracket_*` tests assert unchanged behavior (real arith / `[[ ((` suppression) — they must stay green, not be updated.

- [ ] **Step 1: Up-front grep**

Run:

```bash
grep -rn "unterminated.*arith\|ArithBlock\|((.*subshell\|arith.*block" src/ tests/ | grep -iv "fn scan_arith_block\|fn .*arith_block(" | head -30
grep -rln '((' tests/scripts/ | head
```

Classify any hit UPDATE (encodes the old `((`-hard-error) vs LEAVE (real-arith / dbracket / unrelated). Report each. (None expected to need UPDATE — the change is additive.)

- [ ] **Step 2: Full test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: 0 failures across all unit + integration binaries. A failure on a `((`-related assertion: verify against bash — if bash treats the input as nested subshells the test encoded the bug (UPDATE); if it's a real-arith / dbracket test, STOP and report (the fix is too broad).

- [ ] **Step 3: Whole bash-diff harness suite**

Run:

```bash
cargo build 2>&1 | tail -1
for s in tests/scripts/*_diff_check.sh; do
  out=$(bash "$s" 2>&1); rc=$?
  printf '%s rc=%s %s\n' "$s" "$rc" "$(echo "$out" | tail -1)"
done | grep -v 'Fail: 0' || echo "ALL HARNESSES GREEN"
```

Expected: `ALL HARNESSES GREEN`.

- [ ] **Step 4: Parse-sweep payoff**

```bash
tools/parse_sweep.sh tools/scripts.tsv tools/parse_results.tsv 2>&1 | tail -15
for F in /usr/bin/zdiff \
  /usr/src/linux-headers-6.8.0-110/tools/testing/selftests/kselftest/runner.sh; do
  echo "=== $F ==="; ./target/debug/huck -n "$F"; echo "huck-n rc=$?"; bash -n "$F"; echo "bash-n rc=$?"
done
echo "=== remaining arith gaps? ==="
awk -F'\t' '$3=="HUCK_GAP" && tolower($7) ~ /arith/{print $6}' tools/parse_results.tsv || echo "(none)"
```

Expected: zdiff + runner.sh `huck -n` SILENT rc 0 = bash; no arith `HUCK_GAP` rows remain. Report the new HUCK_GAP vs the 11 baseline; LENIENT/CRASH stay 0; note whether any remaining row is a DIFFERENT construct.

- [ ] **Step 5: Commit (only if Step 1/2 changed a file)**

If the up-front grep / test run produced a legitimate test update, commit it; else state that no commit was needed (Steps 2-4 are verification only).

```bash
git add -A
git commit -m "$(cat <<'EOF'
v184: regression + parse-sweep pass

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS, commit SHA (or "no commit needed"), the Step 1 grep classification, Step 2 cargo test summary, Step 3 result, Step 4 per-file rc + new HUCK_GAP vs 11 + which arith rows remain (if any).

---

## Final review (whole-branch, before merge)

- [ ] **Step 1: Review the full diff** — `git diff main...HEAD`. Confirm only the `((` lexer `Err`-fallback + the three lexer unit tests, plus the new harness (and any Task-3 residual), changed.
- [ ] **Step 2: Scope boundary** — `scan_arith_block`, the `$((` path, the `[[ ((` suppression, and the runtime unchanged; no `bash-divergences.md` change.
- [ ] **Step 3: Hand off** — project merge ritual (AskUserQuestion before merging to main): merge `--no-ff`, push, delete the local branch, record v184 in `project_huck_iterations.md` + `MEMORY.md`, update the backlog note.

---

## Self-Review (plan vs spec)

**1. Spec coverage:**
- `((` `Err`-fallback (rewind + single LParen) → Task 1 Step 2. ✓
- Lexer unit tests (nested-subshell not arith; real arith still ArithBlock; deep nesting) → Task 1 Step 5. ✓
- New harness with nested-subshell + redirect + deep cases + arith controls → Task 2. ✓
- Parse-sweep: zdiff + runner.sh clear + HUCK_GAP report → Task 3 Step 4. ✓
- Full `cargo test` + up-front grep (arith_block/dbracket stay green) → Task 3 Steps 1-2 + Task 1 Step 7. ✓
- All `*_diff_check.sh` green + clippy → Task 3 Step 3 + Task 1 Step 8. ✓
- No `bash-divergences.md` change; record + backlog note → Final review Step 3. ✓

**2. Placeholder scan:** No TBD/TODO. Every code step shows complete before/after code; every run step shows command + expected output. The one ambiguous harness case (`nested two subshells`) carries an explicit fallback NOTE with a concrete replacement.

**3. Type consistency:** `scan_arith_block(&mut CharCursor) -> Result<String, LexError>` and the `chars.clone()` / `*chars = saved` rewind match the v177 pattern. `Token::ArithBlock(body, opts)` and `Token::Op(Operator::LParen)` are the existing emit forms (verified at `:726`/`:728`). The unit tests use the established `tokenize(...)` + `matches!(t, Token::ArithBlock(..) | Token::Op(Operator::LParen))` idiom (verified at `:3733`, `:7179`).

**Resolved during planning:** verified all 7 contract fragments' bash results; that the deep-nesting case needs the SPACED form (`((( echo a ) ) )`, not `(((echo a)))` which has an adjacent `))`); that `((echo hi))` / `((echo a)(echo b))` error in BOTH shells (so excluded from the byte-identical harness or made identical via `2>/dev/null || echo`); and the existing `arith_block_simple` token shape.
