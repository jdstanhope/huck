# v174: command substitution inside array literals Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make backtick and quote-containing `$()` command substitutions parse correctly inside array literals (`` a=(`cmd`) ``, `a=($(echo ')'))`) by routing the array-element scanner through the existing v167/v168 command-sub kernels.

**Architecture:** One function — `scan_array_element_word` in `src/lexer.rs` — currently breaks array elements on whitespace and hand-rolls `$()` handling (a bare paren-counter that ignores quotes) with no backtick case at all. Replace the hand-rolled `$()` block with `scan_cmdsub_body` and add a backtick arm calling `consume_backtick_verbatim`; the existing re-tokenize step then parses the collected text correctly.

**Tech Stack:** Rust (the huck lexer); bash-diff harness (`tests/scripts/*_diff_check.sh`); the parse-compat sweep (`tools/parse_sweep.sh`).

**Spec:** `docs/superpowers/specs/2026-06-17-array-literal-cmdsub-design.md`

**Branch:** `v174-array-literal-cmdsub`

**Background facts (verified):**
- Bug: `` a=(`echo hi`) `` → huck "syntax error: unterminated command substitution" (the scanner breaks at the space inside the backticks). bash accepts it. ~13 real scripts (pyenv/rbenv family) hit this.
- Latent sibling: `a=($(echo ')'))` → huck "unterminated quote" (the hand-rolled `$()` paren-counter miscounts the `)` inside quotes). bash accepts it.
- The kernels already exist in `src/lexer.rs`: `scan_cmdsub_body(chars, out, unterminated)` (appends a `$(…)` body up to but NOT including the matching `)`, skipping quoted spans + nested parens) and `consume_backtick_verbatim(chars, out)` (appends a backtick body and the closing backtick).
- `huck -n <file>` is parse-only (verified side-effect-free) and returns rc 0 on a clean parse.

---

### Task 1: Fix `scan_array_element_word` to use the command-sub kernels

**Files:** Modify `src/lexer.rs` (function `scan_array_element_word`, ~line 2764).

- [ ] **Step 1: Replace the hand-rolled `$(…)` block with the kernel**

In `scan_array_element_word`, inside the `'$' =>` arm, the `Some('(') =>` block currently is:
```rust
                    Some('(') => {
                        buf.push('(');
                        chars.next();
                        let mut depth: usize = 1;
                        for ch in chars.by_ref() {
                            buf.push(ch);
                            match ch {
                                '(' => depth += 1,
                                ')' => {
                                    depth -= 1;
                                    if depth == 0 {
                                        break;
                                    }
                                }
                                _ => {}
                            }
                        }
                        if depth != 0 {
                            return Err(LexError::UnterminatedSubstitution);
                        }
                    }
```
Replace that block with:
```rust
                    Some('(') => {
                        buf.push('(');
                        chars.next();
                        scan_cmdsub_body(chars, &mut buf, LexError::UnterminatedSubstitution)?;
                        buf.push(')');
                    }
```
(`scan_cmdsub_body` consumes through the matching `)` without pushing it, so we push the `)` ourselves. Leave the sibling `Some('{') =>` brace block UNCHANGED.)

- [ ] **Step 2: Add a backtick arm to the outer character match**

Still in `scan_array_element_word`, the outer `match c { … }` has arms for `')'`, whitespace, `'\''`, `'"'`, `'\\'`, `'$'`, and `_`. Immediately AFTER the `'$' => { … }` arm and BEFORE the `_ => { … }` arm, add:
```rust
            '`' => {
                buf.push('`');
                chars.next();
                consume_backtick_verbatim(chars, &mut buf)?;
            }
```

- [ ] **Step 3: Build**

Run: `cargo build 2>&1 | tail -2`
Expected: `Finished` (both `scan_cmdsub_body` and `consume_backtick_verbatim` already exist in `src/lexer.rs`, so no imports needed).

- [ ] **Step 4: Verify the bug and the latent sibling are fixed (parse + execute, vs bash)**

Run:
```bash
H="$(pwd)/target/debug/huck"
echo "--- parse-only: these must now parse (no output = rc 0 OK) ---"
for frag in 'a=(`echo hi`)' 'a=(`a` `b`)' "a=(\$(echo ')'))" 'a=($(f) `g`)' "IFS=\$'\n' a=(\`echo hi\`)"; do
  printf '%s\n' "$frag" | "$H" -n /dev/stdin 2>&1 | sed "s|^|  FAIL($frag): |"
done
echo "--- execute: values match bash ---"
for frag in 'a=(`echo hi`); echo "${a[0]}/${#a[@]}"' 'a=(`echo one` `echo two`); echo "${a[*]}/${#a[@]}"' "a=(\$(echo ')')); echo \"\${a[0]}/\${#a[@]}\""; do
  b=$(printf '%s\n' "$frag" | bash); h=$(printf '%s\n' "$frag" | "$H")
  [ "$b" = "$h" ] && echo "  OK: [$h]  <= $frag" || echo "  MISMATCH bash=[$b] huck=[$h]  <= $frag"
done
```
Expected: the parse-only loop prints NOTHING (all five parse cleanly); the execute loop prints `OK: [hi/1]`, `OK: [one two/2]`, `OK: [)/1]`.

- [ ] **Step 5: Commit**

```bash
git add src/lexer.rs
git commit -m "v174: command substitution inside array literals (backtick + \$()-with-quotes)

scan_array_element_word broke array elements on whitespace and hand-rolled its
\$() handling (a bare paren-counter that ignored quotes) with no backtick case.
Route both through the existing kernels: scan_cmdsub_body for \$() (fixes
a=(\$(echo ')')) miscounting a ) inside quotes) and consume_backtick_verbatim for
backticks (fixes a=(\`cmd\`) breaking at the space inside the backticks). Reuses
the v167/v168 single-source-of-truth scanners instead of a third hand-rolled copy.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Bash-diff harness for command subs in array literals

**Files:** Create `tests/scripts/array_cmdsub_diff_check.sh`.

- [ ] **Step 1: Write the harness**

Create `tests/scripts/array_cmdsub_diff_check.sh` with exactly:
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v174: command substitution inside array
# literals — backtick elements, $()-with-quotes elements, and mixtures. Each case
# EXECUTES the assignment and prints element values/count, asserting identical
# output under bash and huck.
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

check "single backtick"      'a=(`echo hi`); echo "${a[0]} n=${#a[@]}"'
check "two backticks"        'a=(`echo one` `echo two`); echo "${a[*]} n=${#a[@]}"'
check "backtick multi-word"  'a=(`echo a b c`); echo "n=${#a[@]} [${a[*]}]"'
check "mixed dollar+backtick" 'a=($(echo x) `echo y`); echo "${a[*]} n=${#a[@]}"'
check "dollarparen paren-in-quote" 'a=($(echo ")")); echo "[${a[0]}] n=${#a[@]}"'
check "backtick paren-in-quote"    'a=(`echo ")"`); echo "[${a[0]}] n=${#a[@]}"'
check "IFS newline backtick"  $'IFS=$\'\\n\' a=(`printf \'p\\nq\'`); echo "n=${#a[@]} [${a[0]}][${a[1]}]"'
check "nested backtick in sub" 'a=($(echo `echo hi`)); echo "${a[0]}"'
check "regression dollarparen" 'a=($(echo hi) plain); echo "${a[*]} n=${#a[@]}"'
check "literal next to backtick" 'a=(pre`echo X`post); echo "${a[0]} n=${#a[@]}"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Run it**

Run: `chmod +x tests/scripts/array_cmdsub_diff_check.sh && cargo build --quiet && bash tests/scripts/array_cmdsub_diff_check.sh`
Expected: `Total: 10, Pass: 10, Fail: 0`.
If any case FAILs, the fix is incomplete — investigate that exact fragment (do NOT weaken the assertion). Note: `printf` quoting in the "IFS newline backtick" case is delicate — if that one case is hard to express identically through the harness's `printf '%s\n'` plumbing, simplify its fragment to another newline-splitting shape that still exercises `IFS=$'\n' a=(\`…\`)`, and report what you changed.

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/array_cmdsub_diff_check.sh
git commit -m "test: v174 bash-diff harness for command subs in array literals

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Parse-sweep payoff + full regression

**Files:** none (verification only).

- [ ] **Step 1: Confirm the real-world payoff via the parse sweep**

The parse-compat sweep tool and corpus already exist in the working tree
(`tools/parse_sweep.sh`, `tools/scripts.tsv`; untracked). Re-run it and confirm the
"unterminated command substitution" gap cluster collapsed:
```bash
cargo build --quiet
PARSE_TIMEOUT=10 bash tools/parse_sweep.sh tools/scripts.tsv /tmp/v174_sweep.tsv | tail -10
echo "--- remaining 'unterminated command substitution' HUCK_GAPs (was ~13) ---"
awk -F'\t' '$3=="HUCK_GAP" && index($7,"unterminated command substitut")' /tmp/v174_sweep.tsv | wc -l
echo "--- total HUCK_GAP now (was 72) ---"
awk -F'\t' '$3=="HUCK_GAP"' /tmp/v174_sweep.tsv | wc -l
```
Expected: the "unterminated command substitution" count drops to `0` (or near 0), and the total `HUCK_GAP` falls by roughly that many (≈72 → ≈59). No new `HUCK_CRASH`/`HUCK_TIMEOUT`. If any "unterminated command substitution" gaps REMAIN, inspect one (`awk -F'\t' '$3=="HUCK_GAP" && index($7,"unterminated command substitut"){print $6; exit}' /tmp/v174_sweep.tsv` then look at the failing line) — it may be a DIFFERENT construct (e.g. `${…}`-with-quotes, explicitly out of scope) and should be reported, not forced.

- [ ] **Step 2: Full regression**

Run: `cargo clippy --lib --bins --quiet 2>&1 | grep -E 'warning|error' || echo CLEAN` → `CLEAN`.
Run: `cargo test >/tmp/v174.log 2>&1; echo "exit: $?"; grep -cE 'test result: FAILED' /tmp/v174.log` → `exit: 0`, `0`.
Run: `p=0; f=0; for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 && p=$((p+1)) || { f=$((f+1)); echo "FAIL $s"; }; done; echo "$p passed, $f failed"` → `0 failed` (count is now 96 with the new harness).

- [ ] **Step 3: No commit (verification task).** Report the before/after `HUCK_GAP` numbers and the regression results.

---

## Final review (orchestrator, after all tasks)

- Whole-branch diff: `src/lexer.rs` (the ~10-line consolidation in `scan_array_element_word`), the new `tests/scripts/array_cmdsub_diff_check.sh`, the spec/plan docs. Confirm no other function changed and the `${…}` brace arm is untouched.
- Re-run `array_cmdsub_diff_check.sh` (10/10) and the full harness suite (96/96); spot-check a real pyenv script now parses: `./target/debug/huck -n /home/john/.cache/mise/python/pyenv/libexec/pyenv-version-origin && echo "pyenv parses"`.
- Merge `v174-array-literal-cmdsub` to main `--no-ff` after user confirmation (AskUserQuestion); push; delete the branch.
- Record in `project_huck_iterations.md` + `MEMORY.md`. No `bash-divergences.md` change (never a tracked divergence). Note the remaining parse-sweep `HUCK_GAP` clusters (function names, arithmetic termination, etc.) as the candidate backlog for future iterations.

---

## Self-review (plan vs spec)

- **Spec coverage:** route `$()` through `scan_cmdsub_body` (Task 1 Step 1) ✓; add backtick via `consume_backtick_verbatim` (Task 1 Step 2) ✓; `${…}` left unchanged (Task 1 Step 1 note + final review) ✓; new `array_cmdsub_diff_check.sh` with the listed case shapes (Task 2) ✓; parse-sweep payoff confirmation (Task 3 Step 1) ✓; full regression + clippy (Task 3 Step 2) ✓; iteration record, no divergence-doc change (final review) ✓.
- **Placeholder scan:** none — exact before/after code, full harness content, exact commands + expected output. The one flagged delicate spot (the IFS/printf harness case) has explicit fallback guidance, not a placeholder.
- **Type/name consistency:** `scan_cmdsub_body(chars, &mut buf, LexError::UnterminatedSubstitution)` and `consume_backtick_verbatim(chars, &mut buf)` match the real signatures in `src/lexer.rs`; `scan_array_element_word` is the single function touched; harness file name `array_cmdsub_diff_check.sh` and the binary path `target/debug/huck` are used consistently across tasks.
