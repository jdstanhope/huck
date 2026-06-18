# v183: comments inside array literals — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make huck skip `#` comments between elements of an array literal `name=( … )` so a `)` inside a comment no longer closes the array early — clearing the parse-sweep "expected a command" cluster (ioam6, sysctl, huck's own param_cmdsub harness, file-history snapshots).

**Architecture:** One change in `scan_array_literal` (`src/lexer.rs:2744`). Its loop skips whitespace/newlines via `skip_array_literal_whitespace` but not `#` comments; a comment at element-start is scanned as elements and a `)` in the comment text is taken as the array's closing paren. Add a `#`-comment skip right after the whitespace-skip (consume to end-of-line, `continue`). That position is always an element boundary, so a `#` there is unambiguously a comment, matching bash; mid-word `#` (`x#y`), `$#`, and `[i]=#x` never reach it.

**Tech Stack:** Rust (huck shell). Verification via `cargo test`, `tests/scripts/*_diff_check.sh` bash-diff harnesses, and `tools/parse_sweep.sh`.

**Spec:** `docs/superpowers/specs/2026-06-18-array-literal-comments-design.md`

**Branch:** `v183-array-literal-comments` (create from `main`; do NOT implement on `main`).

---

## Confirmed bash behavior (the contract)

Verified against bash 5.x (`"${a[@]}"` after each):

| Fragment | bash result | current huck |
|---|---|---|
| `a=(` ⏎ `1` ⏎ `# note (paren) and ) here` ⏎ `2` ⏎ `)` | `1 2` | ★ expected a command |
| `a=( # lead` ⏎ `x y` ⏎ `)` (comment right after `(`) | `x y` | ★ expected a command |
| `a=(` ⏎ `p q  # trailing ) brace` ⏎ `r` ⏎ `)` | `p q r` | ★ expected a command |
| `a=(x#y a#b)` (mid-word `#`) | `x#y a#b` | `x#y a#b` (unchanged control) |
| `set -- a b c; a=($#)` | `3` | `3` (unchanged control) |
| `a=([0]=#x [1]=y)` (subscript value) | `#x` / `y` | `#x` / `y` (unchanged control) |

`★` = currently broken. Minimal parse trigger: `a=(\n  # has (parens) here\n)\necho ok` (huck "expected a command", bash OK).

---

### Task 1: Skip `#` comments in `scan_array_literal`

**Files:**
- Modify: `src/lexer.rs:2749-2759` (the loop head of `scan_array_literal`)
- Test: `src/lexer.rs` (`mod tests`, near the other `array_literal_*` tests ~`:7457`)

**Context:** `scan_array_literal(chars, opts)` loops: `skip_array_literal_whitespace(chars)` (skips whitespace + newlines, `:2790`), then `match chars.peek()` — `)` closes, `None` errors, else parse an optional `[expr]=` subscript and a `scan_array_element_word`. Test helpers in `mod tests`: `parse_assignments("a=(…)")` returns the parsed assignments, and `array_lit(&word)` returns `&[ArrayLiteralElement]` (each has `subscript: Option<Word>`, `value: Word`).

- [ ] **Step 1: Create the branch**

```bash
cd /home/john/projects/shuck
git checkout main && git checkout -b v183-array-literal-comments
```

- [ ] **Step 2: Add the `#`-comment skip**

In `src/lexer.rs`, the loop head currently reads:

```rust
    loop {
        // Skip whitespace AND newlines (array literals span lines in bash).
        skip_array_literal_whitespace(chars);
        match chars.peek() {
            Some(&')') => {
                chars.next();
                return Ok(elements);
            }
            None => return Err(LexError::UnterminatedArrayLiteral),
            _ => {}
        }
```

Insert the comment skip between the whitespace-skip and the `match`:

```rust
    loop {
        // Skip whitespace AND newlines (array literals span lines in bash).
        skip_array_literal_whitespace(chars);
        // A `#` at element-start (right after `(` or inter-element whitespace)
        // begins a comment to end-of-line — bash allows comments between array
        // elements, and a `)` inside the comment must NOT close the literal.
        // Mid-word `#` (`x#y`), `$#`, and `[i]=#x` never reach here (the word /
        // `$` / `[` is hit first), so those stay literal. Skip to end-of-line
        // and re-loop; the next `skip_array_literal_whitespace` eats the `\n`.
        if chars.peek() == Some(&'#') {
            while let Some(&c) = chars.peek() {
                if c == '\n' {
                    break;
                }
                chars.next();
            }
            continue;
        }
        match chars.peek() {
            Some(&')') => {
                chars.next();
                return Ok(elements);
            }
            None => return Err(LexError::UnterminatedArrayLiteral),
            _ => {}
        }
```

- [ ] **Step 3: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: compiles clean.

- [ ] **Step 4: Manual behavior check against bash**

Run:

```bash
B=target/debug/huck
chk() { printf '%b\n' "$2" > /tmp/v183.sh
  printf '%-26s huck=<%s> bash=<%s>\n' "$1" \
    "$($B /tmp/v183.sh 2>&1 | tr '\n' '|')" "$(bash --norc /tmp/v183.sh 2>&1 | tr '\n' '|')"; }
chk 'comment with paren' 'a=(\n  1\n  # note (paren) and ) here\n  2\n)\necho "${a[@]}"'
chk 'comment after ('    'a=(  # lead\n  x y\n)\necho "${a[@]}"'
chk 'trailing comment'   'a=(\n  p q  # trailing ) brace\n  r\n)\necho "${a[@]}"'
chk 'midword hash'       'a=(x#y a#b)\necho "${a[@]}"'
chk 'dollar-hash'        'set -- a b c; a=($#)\necho "${a[@]}"'
chk 'subscript hash'     'a=([0]=#x [1]=y)\necho "${a[0]}-${a[1]}"'
```

Expected (every line huck byte-equal to bash): `1 2`, `x y`, `p q r`, `x#y a#b`, `3`, `#x-y`.

- [ ] **Step 5: Add lexer unit tests**

In `src/lexer.rs`, inside `mod tests` near the other `array_literal_*` tests (~`:7457`), add:

```rust
    #[test]
    fn array_literal_skips_comment_with_paren() {
        // v183: a `#` comment between elements (incl. one whose text contains
        // `)`) is skipped — the `)` must NOT close the array early. Body has a
        // comment line `# c )` then a single element `1`.
        let assigns = parse_assignments("a=(\n# c )\n1\n)");
        let els = array_lit(&assigns[0].value);
        assert_eq!(els.len(), 1);
        assert!(els[0].subscript.is_none());
    }

    #[test]
    fn array_literal_midword_hash_is_literal() {
        // v183 regression: a `#` MID-word (`x#y`) is NOT a comment — the word /
        // start char is matched before any element-start `#` check.
        let assigns = parse_assignments("a=(x#y z)");
        let els = array_lit(&assigns[0].value);
        assert_eq!(els.len(), 2);
    }
```

- [ ] **Step 6: Run the new unit tests**

Run: `cargo test --lib array_literal_skips_comment_with_paren array_literal_midword_hash_is_literal 2>&1 | tail -15`
(If `cargo test` rejects multiple positional filters, run `cargo test --lib array_literal_ 2>&1 | tail -20` to run all `array_literal_*` tests.)
Expected: both new tests pass; all `array_literal_*` tests 0 failed.

- [ ] **Step 7: clippy**

Run: `cargo clippy 2>&1 | tail -5`
Expected: no new warnings.

- [ ] **Step 8: Commit**

```bash
git add src/lexer.rs
git commit -m "$(cat <<'EOF'
v183: skip # comments inside array literals

scan_array_literal skipped whitespace/newlines between elements but not `#`
comments, so a comment at element-start was scanned as elements and a `)`
inside the comment text closed the array early (generic "expected a command"
cascade). Now a `#` at element-start is consumed to end-of-line, matching
bash; mid-word `#` (`x#y`), `$#`, and `[i]=#x` are untouched (the word/`$`/`[`
is matched first). Clears the parse-sweep array-literal-comment cluster
(ioam6, sysctl, param_cmdsub_split harness).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS (DONE / DONE_WITH_CONCERNS / NEEDS_CONTEXT / BLOCKED), commit SHA, the Step 4 huck-vs-bash table (all must match), the Step 6 result, any concerns. If a code site doesn't match the line numbers above, find it by the surrounding code shown and report the discrepancy.

---

### Task 2: Bash-diff harness `array_comment_diff_check.sh`

**Files:**
- Create: `tests/scripts/array_comment_diff_check.sh`

**Context:** Harnesses under `tests/scripts/*_diff_check.sh` run fragments through bash AND huck and assert byte-identical output. The `check` helper pipes each fragment to each shell's stdin, so a fragment may be a MULTI-LINE single-quoted string (real newlines preserved). All v183 cases print clean stdout, rc 0 in bash — compare full stdout+exit. Model: `tests/scripts/dollar_quote_forms_diff_check.sh`. (The harness file itself is parsed by bash only — huck never parses it — so it is safe to embed the array-comment constructs as fragments.)

- [ ] **Step 1: Write the harness**

Create `tests/scripts/array_comment_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v183: `#` comments inside an array
# literal `name=( … )`. A comment at element-start runs to end-of-line; a `)`
# inside the comment text must NOT close the array (huck used to mis-parse this
# → "expected a command", e.g. kernel ioam6.sh / sysctl.sh arrays with commented
# rows). Mid-word `#`, `$#`, and `[i]=#x` stay literal. rc 0 in bash → compare
# full stdout+exit.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

check() {  # label ; fragment — assert byte-identical stdout+stderr+exit
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# Comment lines inside the array (the `)` / `(` inside must not close it).
check "comment with paren and close" 'a=(
  1
  # note (paren) and ) here
  2
); echo "${a[@]}"'
check "comment right after open"     'a=(  # lead comment )
  x y
); echo "${a[@]}"'
check "trailing comment after elem"  'a=(
  p q  # trailing ) brace
  r
); echo "${a[@]}"'
check "multiple comment lines"       'a=(
  # first ) comment
  alpha
  # second (paren) comment
  beta
); echo "${a[@]}"'

# Controls — these `#` are NOT comments and must stay literal.
check "midword hash literal"         'a=(x#y a#b); echo "${a[@]}"'
check "dollar-hash count"            'set -- a b c; a=($#); echo "${a[@]}"'
check "subscript hash value"         'a=([0]=#x [1]=y); echo "${a[0]}-${a[1]}"'
check "plain array no comment"       'a=(one two three); echo "${a[@]}"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make executable, build, run**

Run:

```bash
chmod +x tests/scripts/array_comment_diff_check.sh
cargo build 2>&1 | tail -2
bash tests/scripts/array_comment_diff_check.sh
```

Expected: `Total: 8, Pass: 8, Fail: 0`, exit 0. If any case FAILs, STOP and report the diff — investigate whether the implementation regressed or a fragment's quoting is wrong (do NOT weaken a case).

- [ ] **Step 3: Prove the harness is non-tautological**

Build the pre-fix binary in a throwaway worktree and confirm the harness FAILS against it:

```bash
git worktree add /tmp/huck-prefix main 2>&1 | tail -1
( cd /tmp/huck-prefix && cargo build 2>&1 | tail -1 )
HUCK_BIN=/tmp/huck-prefix/target/debug/huck bash tests/scripts/array_comment_diff_check.sh; echo "prefix-exit=$?"
git worktree remove --force /tmp/huck-prefix
```

Expected: against the pre-fix binary the harness FAILS — the 4 comment cases mis-parse ("expected a command"); the 4 control cases pass pre-fix; prefix-exit=1. If it PASSES pre-fix, the harness is tautological — STOP and report. (`main` is the pre-v183 baseline.)

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/array_comment_diff_check.sh
git commit -m "$(cat <<'EOF'
v183: bash-diff harness for # comments inside array literals

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS, commit SHA, Step 2 Total/Pass/Fail line, Step 3 prefix-exit and which cases failed pre-fix (proving non-tautology).

---

### Task 3: Parse-sweep payoff and regression

**Files:**
- Modify (only if found): old-behavior tests surfaced by the up-front grep
- Run: full `cargo test`, all `tests/scripts/*_diff_check.sh`, `tools/parse_sweep.sh`

**Context — UP-FRONT grep (v178/v180/v181/v182 lesson):** Before any regression run, grep ALL of `tests/` and `src/` for array-literal tests that might encode the pre-fix behavior (a `#` inside an array expected to be a literal element). Integration tests are separate binaries surfacing only on a full `cargo test`.

- [ ] **Step 1: Up-front grep**

Run:

```bash
grep -rn 'scan_array_literal\|ArrayLiteral\|=(' src/ tests/ | grep -i '#' | head -30
grep -rln 'a=(\|=(' tests/scripts/ | head
```

Read each hit and classify UPDATE (asserts the old behavior where a `#` inside an array was treated as a literal element / where a comment-`)` closed the array) vs LEAVE (unrelated, or a `#` that is genuinely mid-word/`$#`). Report the classification of every hit. Most hits will be LEAVE; report any genuine UPDATE and what you changed.

- [ ] **Step 2: Full test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: 0 failures across all unit + integration binaries. If a test fails because it fed a leading `#` into an array expecting it as a literal element, verify against bash first — if bash treats it as a comment, that test encoded the bug (UPDATE it); if not, STOP and report (the fix may be too broad).

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

- [ ] **Step 4: Parse-sweep payoff (cluster files clear)**

The sweep is parse-only (`huck -n`/`bash -n`), side-effect-free. Run:

```bash
tools/parse_sweep.sh tools/scripts.tsv tools/parse_results.tsv 2>&1 | tail -15
for F in \
  /usr/src/linux-headers-6.8.0-110/tools/testing/selftests/net/ioam6.sh \
  /usr/src/linux-headers-6.8.0-110/tools/testing/selftests/sysctl/sysctl.sh \
  /home/john/projects/shuck/tests/scripts/param_cmdsub_split_diff_check.sh; do
  echo "=== $F ==="
  ./target/debug/huck -n "$F"; echo "huck-n rc=$?"
  bash -n "$F"; echo "bash-n rc=$?"
done
echo "=== remaining 'expected a command' gaps? ==="
awk -F'\t' '$3=="HUCK_GAP" && $7 ~ /expected a command/{print $6}' tools/parse_results.tsv || echo "(none)"
```

Expected: ioam6, sysctl, and `param_cmdsub_split_diff_check.sh` `huck -n` SILENT, rc 0 = bash. Report which "expected a command" rows remain (the 110/124 ioam6+sysctl and the param_cmdsub harness should clear; report whether the two `file-history` snapshots cleared and whether any remaining "expected a command" row is a DIFFERENT construct — a derail/follow-on). Report the new HUCK_GAP vs the 20 baseline; `HUCK_LENIENT`/`HUCK_CRASH` stay 0.

- [ ] **Step 5: Commit (only if Step 1/2 changed a file)**

If the up-front grep / test run produced a legitimate test update:

```bash
git add -A
git commit -m "$(cat <<'EOF'
v183: regression + parse-sweep pass; update array-literal-comment test

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

If nothing changed, state that no commit was needed (Steps 2-4 are verification only).

## Report back
STATUS, commit SHA (or "no commit needed"), the Step 1 grep classification, Step 2 cargo test summary, Step 3 result, Step 4 per-file rc + new HUCK_GAP vs 20 + which "expected a command" rows remain (and whether any are a different construct).

---

## Final review (whole-branch, before merge)

- [ ] **Step 1: Review the full diff** — `git diff main...HEAD`. Confirm only the `scan_array_literal` `#`-skip + the two lexer unit tests, plus the new harness (and any Task-3 residual), changed.
- [ ] **Step 2: Scope boundary** — `scan_array_element_word`, `skip_array_literal_whitespace`, subscript parsing, and the runtime unchanged; no `bash-divergences.md` change.
- [ ] **Step 3: Hand off** — project merge ritual (AskUserQuestion before merging to main): merge `--no-ff`, push, delete the local branch, record v183 in `project_huck_iterations.md` + `MEMORY.md`, and update the backlog note (re-survey "expected a command" — some rows were cascades of THIS bug).

---

## Self-Review (plan vs spec)

**1. Spec coverage:**
- `#`-comment skip in `scan_array_literal` loop → Task 1 Step 2. ✓
- Lexer unit tests (`a=(\n# c )\n1\n)` → 1 element; `a=(x#y z)` → 2 elements) → Task 1 Step 5. ✓
- New harness `array_comment_diff_check.sh` (comment-with-`)`, comment after `(`, trailing comment, multiple comments; controls mid-word `#`/`$#`/subscript) → Task 2. ✓
- Parse-sweep: ioam6/sysctl/param_cmdsub clear + HUCK_GAP report + file-history note → Task 3 Step 4. ✓
- Full `cargo test` + up-front grep for old array-`#` behavior → Task 3 Steps 1-2. ✓
- All `*_diff_check.sh` green + clippy → Task 3 Step 3 + Task 1 Step 7. ✓
- No `bash-divergences.md` change; record + update backlog note → Final review Step 3. ✓

**2. Placeholder scan:** No TBD/TODO. Every code step shows complete before/after code; every run step shows the command + expected output.

**3. Type consistency:** `scan_array_literal(chars, opts) -> Result<Vec<ArrayLiteralElement>, LexError>` signature unchanged (loop body only). Unit tests use the existing `parse_assignments` + `array_lit` helpers (verified present at `:7433`); `ArrayLiteralElement.subscript` is `Option<Word>` (verified at `:206`). `scan_array_element_word`, `skip_array_literal_whitespace` untouched.

**Resolved during planning:** verified all 6 contract fragments' bash results; confirmed the bug is one root cause across the cluster (ioam6/sysctl/param_cmdsub all comment-with-`)`-in-array, sysctl's line-323 report is a cascade from its line-330 array); confirmed the test helpers and `ArrayLiteralElement` shape.
