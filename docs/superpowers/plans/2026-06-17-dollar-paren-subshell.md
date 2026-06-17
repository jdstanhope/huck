# v177: `$((` disambiguation (command substitution of a subshell) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `$((subshell) … )` (a command substitution whose body starts with a subshell, written glued as `$((`) parse like bash, instead of being mis-lexed as an unterminated arithmetic expansion.

**Architecture:** In the lexer's `$((` branch, try arithmetic; if `scan_arith_body` fails (the body doesn't close as `))` — bash's "not arithmetic" signal), rewind the cursor (a cheap `Clone`) and reparse as a command substitution so the inner `(` becomes a subshell. Lexer-only; the executor already handles the resulting `WordPart::CommandSub`.

**Tech Stack:** Rust (the huck lexer, `src/lexer.rs`); bash-diff harness; the parse-compat sweep (`tools/parse_sweep.sh`).

**Spec:** `docs/superpowers/specs/2026-06-17-dollar-paren-subshell-design.md`

**Branch:** `v177-dollar-paren-subshell`

**Background facts (verified):**
- huck rejects `echo $((echo hi) 2>&1)`, `x=$((echo a) | cat)` with "unterminated arithmetic expansion"; bash accepts them as command substitutions of a subshell. Real-world: `timing.sh`, `zdiff`.
- The spaced form `$( (sub) … )` already works (v101); all real arithmetic works (`$((1+2))`, `$(( (1+2)*3 ))`, `$(( ((4)) ))`, `$((1>0?2:3))`).
- `scan_dollar_expansion` (`lexer.rs:1729`) routes `$((` to `scan_arith_body` unconditionally (the `$((` branch is at lines ~1736–1746).
- `scan_arith_body` (`lexer.rs:1985`) returns `Err(LexError::UnterminatedArith)` exactly when the first depth-1 `)` is not followed by another `)` (or on EOF), and `Ok(inner)` at the closing `))`.
- `CharCursor` derives `Clone` (`lexer.rs:36`); `chars: &mut CharCursor<'_>`, so `let saved = chars.clone();` then `*chars = saved;` rewinds (including the line counter).
- `scan_paren_substitution(chars, opts)` scans a `$( … )` command-sub body starting just after the opening `(`. It is already the `else`-branch (non-`$((`) call in the same function.
- `|&` is a SEPARATE unsupported feature (out of scope); `huck -n <file>` is parse-only (side-effect-free).

---

### Task 1: Add the `$((` try-arith / fallback-to-cmdsub disambiguation

**Files:** Modify `src/lexer.rs` (function `scan_dollar_expansion`, the `Some('(') =>` arm, ~lines 1736–1746).

- [ ] **Step 1: Replace the `$((` branch**

In `scan_dollar_expansion`, the current `Some('(') =>` arm is:
```rust
        Some('(') => {
            chars.next(); // consume first '('
            if chars.peek() == Some(&'(') {
                chars.next(); // consume second '(' — this is `$((`
                let inner = scan_arith_body(chars)?;
                let body = arith_string_to_word(&inner, opts)?;
                parts.push(WordPart::Arith { body, quoted });
            } else {
                let sequence = scan_paren_substitution(chars, opts)?;
                parts.push(WordPart::CommandSub { sequence, quoted });
            }
        }
```
Replace it with:
```rust
        Some('(') => {
            chars.next(); // consume first '('
            if chars.peek() == Some(&'(') {
                // `$((` is EITHER an arithmetic expansion `$(( … ))` OR a command
                // substitution whose body starts with a subshell written glued:
                // `$( (subshell) … )`. Try arithmetic; if the body does not close
                // as `))` (scan_arith_body Err — bash's "not arithmetic" signal),
                // rewind to just after the first `(` and reparse as a command
                // substitution so the inner `(` parses as a subshell. Mirrors bash.
                let saved = chars.clone();
                chars.next(); // consume the second '('
                match scan_arith_body(chars) {
                    Ok(inner) => {
                        let body = arith_string_to_word(&inner, opts)?;
                        parts.push(WordPart::Arith { body, quoted });
                    }
                    Err(_) => {
                        *chars = saved; // rewind to just after the first '('
                        let sequence = scan_paren_substitution(chars, opts)?;
                        parts.push(WordPart::CommandSub { sequence, quoted });
                    }
                }
            } else {
                let sequence = scan_paren_substitution(chars, opts)?;
                parts.push(WordPart::CommandSub { sequence, quoted });
            }
        }
```

- [ ] **Step 2: Build**

Run: `cargo build 2>&1 | tail -2`
Expected: `Finished`. (`scan_arith_body`, `arith_string_to_word`, `scan_paren_substitution`, and `CharCursor::clone` are all already in scope in `lexer.rs`; no imports.)

- [ ] **Step 3: Verify the fix + real-arith regressions (vs bash)**

Run:
```bash
H="$(pwd)/target/debug/huck"
echo "--- now parse: glued \$((subshell) … ) command substitutions ---"
for frag in \
  'echo $((echo hi) 2>&1)' \
  'x=$((echo a) | cat); echo "$x"' \
  'echo $((printf X; printf Y) 2>/dev/null)' \
  'echo $((time -p true) 2>&1)'; do
  printf '%s\n' "$frag" | "$H" -n /dev/stdin 2>&1 | sed "s/^/  FAIL: /"
done
echo "--- real arithmetic must STILL parse (no fallback) ---"
for frag in 'echo $((1+2))' 'echo $(( (1+2)*3 ))' 'echo $(( ((4)) ))' 'echo $((1>0?2:3))' 'echo $( (echo s) 2>&1 )'; do
  printf '%s\n' "$frag" | "$H" -n /dev/stdin 2>&1 | sed "s/^/  REGRESSION-FAIL: /"
done
echo "--- execute: outputs match bash ---"
for frag in 'echo $((echo hi) 2>&1)' 'echo $((1+2))' 'echo $(( (1+2)*3 ))' 'v=$((printf X; printf Y) 2>/dev/null); echo "[$v]"'; do
  b=$(bash -c "$frag" 2>&1); h=$("$H" -c "$frag" 2>&1)
  [ "$b" = "$h" ] && echo "  OK: [$h]" || echo "  MISMATCH bash=[$b] huck=[$h]  <= $frag"
done
```
Expected: the first loop prints NOTHING (the four glued forms now parse); the real-arith loop prints NOTHING (no regression); the execute loop prints `OK:` for all four (`hi`, `3`, `9`, `[XY]`).

- [ ] **Step 4: Commit**

```bash
git add src/lexer.rs
git commit -m "v177: disambiguate \$(( — command substitution of a subshell vs arithmetic

scan_dollar_expansion committed \$(( unconditionally to arithmetic. bash treats it
as tentative: if the body doesn't close as )), it reparses as \$( (subshell) … )
command substitution. Mirror that: on \$((, try scan_arith_body; on Err (the first
depth-1 ) isn't followed by ) — the not-arithmetic signal) rewind the cloned
cursor and reparse via scan_paren_substitution, so \`echo \$((cmd) 2>&1)\` parses
like bash. Real arithmetic (closes with ))) is unaffected.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Bash-diff harness for `$((subshell) … )` command substitution

**Files:** Create `tests/scripts/dollar_paren_subshell_diff_check.sh`.

- [ ] **Step 1: Write the harness**

Create `tests/scripts/dollar_paren_subshell_diff_check.sh` with exactly:
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v177: `$((` disambiguation. A command
# substitution whose body starts with a subshell, written glued as `$((`, must
# parse as command substitution (not arithmetic) and match bash; real arithmetic
# expansions must be unaffected. Each case EXECUTES and asserts identical
# stdout+exit.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash --norc --noprofile -c "$frag" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- the bug: glued $(( subshell ) ... ) is a command substitution ---
check "subshell + 2>&1"        'echo $((echo hi) 2>&1)'
check "subshell piped"         'echo $((echo a) | tr a-z A-Z)'
check "subshell multi-cmd"     'echo $((printf X; printf Y) 2>/dev/null)'
check "subshell redirect capt" 'v=$( (printf P; printf Q) 2>/dev/null ); echo "[$v]"'
check "glued capt + nl"        'v=$((printf m; printf n)); echo "[$v]"'

# --- regressions: real arithmetic, unaffected ---
check "plain arith"            'echo $((1+2))'
check "arith paren subexpr"    'echo $(( (1+2)*3 ))'
check "arith double paren"     'echo $(( ((4)) ))'
check "arith ternary"          'echo $((1>0?2:3))'
check "spaced subshell form"   'echo $( (echo s) 2>&1 )'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Run it**

Run: `chmod +x tests/scripts/dollar_paren_subshell_diff_check.sh && cargo build --quiet && bash tests/scripts/dollar_paren_subshell_diff_check.sh`
Expected: `Total: 10, Pass: 10, Fail: 0`.
If a "subshell …" case FAILs, the disambiguation missed that shape; if a "arith …" case FAILs, the fallback wrongly fired on real arithmetic — investigate, do NOT weaken the assertion.

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/dollar_paren_subshell_diff_check.sh
git commit -m "test: v177 bash-diff harness for \$((subshell) … ) command substitution

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Parse-sweep payoff + full regression

**Files:** none (verification only).

- [ ] **Step 1: Confirm the payoff via the parse sweep**

```bash
cargo build --quiet
PARSE_TIMEOUT=10 bash tools/parse_sweep.sh tools/scripts.tsv /tmp/v177_sweep.tsv | tail -10
echo "--- arith-termination HUCK_GAPs remaining ---"
awk -F'\t' '$3=="HUCK_GAP" && ($7 ~ /arithmetic|unterminated .\(\(/){print $6"\t"$7}' /tmp/v177_sweep.tsv | sort -u
echo "--- total HUCK_GAP now (was 32 after v176) ---"
awk -F'\t' '$3=="HUCK_GAP"' /tmp/v177_sweep.tsv | wc -l
echo "--- HUCK_LENIENT / HUCK_CRASH (must stay 0) ---"
awk -F'\t' '$3=="HUCK_LENIENT"' /tmp/v177_sweep.tsv | wc -l
awk -F'\t' '$3=="HUCK_CRASH"' /tmp/v177_sweep.tsv | wc -l
echo "--- confirm timing.sh + zdiff now parse ---"
for p in /home/john/go/pkg/mod/golang.org/x/exp*/shootout/timing.sh /usr/bin/zdiff; do
  f=$(ls $p 2>/dev/null | head -1); [ -n "$f" ] && { ./target/debug/huck -n "$f" >/dev/null 2>&1 && echo "  PARSES: $f" || echo "  still fails: $f ($(./target/debug/huck -n "$f" 2>&1 | head -1))"; }
done
```
Expected: `timing.sh` and `zdiff` now parse (removed from the cluster); `HUCK_GAP` falls (≈32 → ≈29–30); `HUCK_LENIENT`/`HUCK_CRASH` stay `0`. Any remaining arith-termination entries should be the `|&` users (`test_cpuset_prs`, possibly `runner.sh`) or the test-harness-file noise (`arith_extglob_diff_check.sh`, the `…/file-history/…` copy) — report which remain and confirm the remaining ones are `|&`/noise (out of scope), not a missed `$((` shape.

- [ ] **Step 2: Full regression**

Run: `cargo clippy --lib --bins --quiet 2>&1 | grep -E 'warning|error' || echo CLEAN` → `CLEAN`.
Run: `cargo test >/tmp/v177.log 2>&1; echo "exit: $?"; grep -cE 'test result: FAILED' /tmp/v177.log` → `exit: 0`, `0`.
Run: `p=0; f=0; for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 && p=$((p+1)) || { f=$((f+1)); echo "FAIL $s"; }; done; echo "$p passed, $f failed"` → `0 failed` (count is now 99 with the new harness).

Note: there are existing lexer unit tests for `$((…))` (e.g. around `src/lexer.rs:5091`/`5113` — `tokenize("$((1+2))")`, `tokenize("$(( (1+2) * 3 ))")`). These exercise REAL arithmetic (close with `))`) so they must still pass unchanged; if any fails, the fallback wrongly fired — investigate, do not edit the test to pass.

- [ ] **Step 3: No commit (verification task).** Report the before/after `HUCK_GAP` numbers, which arith-termination scripts remain (and why), and the regression results.

---

## Final review (orchestrator, after all tasks)

- Whole-branch diff: `src/lexer.rs` (the `$((` branch in `scan_dollar_expansion` only) + the new `tests/scripts/dollar_paren_subshell_diff_check.sh`. Confirm `scan_arith_body`, `scan_paren_substitution`, and the executor are untouched.
- Re-run `dollar_paren_subshell_diff_check.sh` (10/10) and the full harness suite (99/99); spot-check `timing.sh`/`zdiff` parse.
- Merge `v177-dollar-paren-subshell` to main `--no-ff` after user confirmation (AskUserQuestion); push; delete the branch.
- Record in `project_huck_iterations.md` + `MEMORY.md`; update the parse-sweep backlog note (HUCK_GAP ≈29–30 left; arith-termination cluster reduced to the `|&` users + test-harness noise). No `bash-divergences.md` change. Note `|&` as the next adjacent gap if `test_cpuset_prs`/`runner.sh` warrant it.

---

## Self-review (plan vs spec)

- **Spec coverage:** the `$((` try-arith / fallback-to-cmdsub disambiguation (Task 1 Step 1) ✓; cursor clone/restore via `CharCursor: Clone` (Task 1 Step 1, background facts) ✓; real-arith unaffected — verified inline + the existing `$((…))` unit tests (Task 1 Step 3, Task 3 Step 2 note) ✓; new executing harness with glued-subshell cases + real-arith regressions + the spaced-form regression (Task 2) ✓; parse-sweep payoff with honest coverage incl. `|&`/noise residue (Task 3 Step 1) ✓; full regression + clippy (Task 3 Step 2) ✓; iteration record, no divergence doc, `|&` noted as out of scope (final review) ✓.
- **Placeholder scan:** none — full before/after for the branch, complete harness, exact verification commands with expected output.
- **Type/name consistency:** `scan_arith_body(chars) -> Result<String, LexError>`, `arith_string_to_word(&inner, opts)`, `scan_paren_substitution(chars, opts)`, `WordPart::Arith { body, quoted }` / `WordPart::CommandSub { sequence, quoted }`, and `chars.clone()` / `*chars = saved` all match the existing code in `scan_dollar_expansion`; harness filename and `target/debug/huck` consistent across tasks.
