# v182: backslash-escapes in `${var/pat/repl}` operand splitting — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make huck parse `${var/pat/repl}` substitutions whose pattern/replacement contain backslash runs like `\\\"` (escaped-backslash + escaped-quote) and produce bash-identical results — fixing the "unterminated quote" crash on the kernel `scripts/config` (line 209, `V="${V//\\\"/\"}"`).

**Architecture:** The bug is in `split_modifier_operand` (`src/lexer.rs:3221`), the second of three phases for a `${var/pat/repl}` operand (extract body → split on `/` → parse each segment). Its backslash arm un-escapes (`\\`→`\`, `\delim`→`delim`) and, in its `_` arm, fails to consume the escaped char — so `\"` reprocesses the `"` as a quote opener, swallowing the `/` delimiter and corrupting the segment into an unbalanced quote that `parse_braced_operand_opts` rejects. The fix makes `split_modifier_operand` a pure splitter that preserves every `\x` verbatim (consume both chars), leaving all un-escaping to the single downstream pass in `parse_braced_operand_opts`. The final parsed Words are unchanged for working cases; only `split_modifier_operand`'s intermediate output (and two unit-test assertions + the doc comment) change.

**Tech Stack:** Rust (huck shell). Verification via `cargo test`, `tests/scripts/*_diff_check.sh` bash-diff harnesses, and `tools/parse_sweep.sh`.

**Spec:** `docs/superpowers/specs/2026-06-17-subst-operand-escapes-design.md`

**Branch:** `v182-subst-operand-escapes` (create from `main`; do NOT implement on `main`).

---

## Confirmed bash behavior (the contract; results, not just parse)

Verified against bash 5.x:

| Fragment | bash stdout | current huck |
|---|---|---|
| `V='a\"b'; echo "${V//\\\"/\"}"` | `a"b` | ★ unterminated quote (rc 2) |
| `V='a\"b\"c'; echo "${V//\\\"/X}"` | `aXbXc` | ★ unterminated quote |
| `V=a/b/c; echo "${V//\//_}"` | `a_b_c` | `a_b_c` (unchanged control) |
| `V='x\y'; echo "${V//\\/Z}"` | `xZy` | `xZy` (unchanged control) |
| `V=foobar; echo "${V/o/O}"` | `fOobar` | `fOobar` (unchanged control) |

`★` = currently broken. Minimal parse trigger: `${V//\\\"/Z}` (huck "unterminated quote", bash OK).

---

### Task 1: Make `split_modifier_operand` preserve backslash-escapes verbatim

**Files:**
- Modify: `src/lexer.rs:3229-3242` (the `'\\'` arm) + the doc comment at `:3217-3220`
- Test: `src/lexer.rs` (`split_modifier_operand_quotes_and_escapes` ~`:3518`, plus a new regression test)

**Context:** `split_modifier_operand(body, delim)` splits a `${…}` modifier operand body (already extracted verbatim by `scan_braced_operand`) on the first top-level `delim`, skipping `$(…)`/backticks/`{…}`/quoted spans. It is called via `split_substitution_body` (delim `/`) and `split_substring_body` (delim `:`). Downstream, `parse_braced_operand_opts` (`:2314`) un-escapes `\x`→`x` for every `x`. The current `'\\'` arm un-escapes here too (double-processing) and its `_` branch leaves the escaped char unconsumed.

- [ ] **Step 1: Create the branch**

```bash
cd /home/john/projects/shuck
git checkout main && git checkout -b v182-subst-operand-escapes
```

- [ ] **Step 2: Rewrite the `'\\'` arm to preserve escapes verbatim**

In `src/lexer.rs`, the arm currently reads:

```rust
            '\\' => {
                let dst = if delim_seen { &mut second } else { &mut first };
                match chars.peek().copied() {
                    Some(d) if d == delim => {
                        chars.next();
                        dst.push(delim);
                    }
                    Some('\\') => {
                        chars.next();
                        dst.push('\\');
                    }
                    _ => dst.push('\\'),
                }
            }
```

Replace it with:

```rust
            '\\' => {
                // Preserve an escaped char VERBATIM (backslash + the char) and
                // CONSUME the char so it cannot act as a delimiter or open a
                // quote/backtick span. The real un-escaping happens once,
                // downstream, in parse_braced_operand_opts; pre-un-escaping here
                // would double-process backslashes (corrupting runs like `\\\"`).
                // An escaped delimiter (`\/`) is thus preserved AND not seen as a
                // split point. A trailing `\` at end of body pushes just `\`.
                let dst = if delim_seen { &mut second } else { &mut first };
                dst.push('\\');
                if let Some(nc) = chars.next() {
                    dst.push(nc);
                }
            }
```

- [ ] **Step 3: Update the doc comment**

In `src/lexer.rs`, the `split_modifier_operand` doc comment currently contains this sentence (around `:3217-3220`):

```rust
/// appended VERBATIM so the segments re-parse exactly as written. At the top
/// level only, `\delim` un-escapes to `delim` and `\\` to `\`; any other `\x`
/// keeps the backslash. Inside a command substitution escapes are verbatim
/// (they belong to the command), mirroring `scan_paren_substitution`.
```

Replace those four lines with:

```rust
/// appended VERBATIM so the segments re-parse exactly as written. A backslash
/// escape `\x` is ALSO preserved verbatim (and the escaped char consumed, so an
/// escaped delimiter `\delim` does not split and an escaped quote `\"` does not
/// open a span); all un-escaping is done once, downstream, by
/// `parse_braced_operand_opts`. Inside a command substitution escapes are
/// verbatim too (they belong to the command), mirroring `scan_paren_substitution`.
```

- [ ] **Step 4: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: compiles clean.

- [ ] **Step 5: Manual behavior check against bash**

Run:

```bash
B=target/debug/huck
chk() { printf '%s\n' "$2" > /tmp/v182.sh
  printf '%-30s huck=<%s> bash=<%s>\n' "$1" \
    "$($B /tmp/v182.sh 2>&1 | tr '\n' '/')" "$(bash --norc /tmp/v182.sh 2>&1 | tr '\n' '/')"; }
chk 'unescape idiom'    'V='\''a\"b'\''; echo "${V//\\\"/\"}"'
chk 'replace esc-quote' 'V='\''a\"b\"c'\''; echo "${V//\\\"/X}"'
chk 'escaped delim'     'V=a/b/c; echo "${V//\//_}"'
chk 'escaped backslash' 'V='\''x\y'\''; echo "${V//\\/Z}"'
chk 'plain control'     'V=foobar; echo "${V/o/O}"'
chk 'substring control' 'V=abcdef; echo "${V:1:3}"'
```

Expected (every line huck byte-equal to bash): `a"b`, `aXbXc`, `a_b_c`, `xZy`, `fOobar`, `bcd`. (Shell quoting in `chk` calls is fiddly; if a call is mis-quoted, fix the call — the target outputs are the bash column. The authoritative cases live in the Task 2 harness.)

- [ ] **Step 6: Update the unit-test assertions to the verbatim forms**

In `src/lexer.rs`, `split_modifier_operand_quotes_and_escapes` currently reads:

```rust
    fn split_modifier_operand_quotes_and_escapes() {
        // A quoted delimiter is kept verbatim and does not split.
        assert_eq!(
            split_modifier_operand("\"a/b\"/x", '/'),
            ("\"a/b\"".into(), Some("x".into()))
        );
        // An escaped delimiter un-escapes to the literal char and does not split.
        assert_eq!(split_modifier_operand("a\\/b/x", '/'), ("a/b".into(), Some("x".into())));
        // \\ un-escapes to a single backslash.
        assert_eq!(split_modifier_operand("a\\\\b", '/'), ("a\\b".into(), None));
    }
```

Replace the body with (quoted-span assertion unchanged; the two escape assertions become verbatim; add the `\\\"` regression):

```rust
    fn split_modifier_operand_quotes_and_escapes() {
        // A quoted delimiter is kept verbatim and does not split.
        assert_eq!(
            split_modifier_operand("\"a/b\"/x", '/'),
            ("\"a/b\"".into(), Some("x".into()))
        );
        // An escaped delimiter is preserved VERBATIM and does not split
        // (downstream parse_braced_operand_opts un-escapes `\/`→`/`).
        assert_eq!(split_modifier_operand("a\\/b/x", '/'), ("a\\/b".into(), Some("x".into())));
        // `\\` is preserved verbatim (un-escaped once, downstream).
        assert_eq!(split_modifier_operand("a\\\\b", '/'), ("a\\\\b".into(), None));
        // Regression: `\\\"` (escaped backslash + escaped quote) must not let the
        // `"` open a span that swallows the delimiter. Body `\\\"/Z` (Rust
        // literal `"\\\\\\\"/Z"`) splits to pattern `\\\"` and replacement `Z`.
        assert_eq!(
            split_modifier_operand("\\\\\\\"/Z", '/'),
            ("\\\\\\\"".into(), Some("Z".into()))
        );
    }
```

- [ ] **Step 7: Run the affected unit tests**

Run: `cargo test --lib split_modifier_operand 2>&1 | tail -20`
Expected: all `split_modifier_operand_*` tests pass (basic_split, skips_command_sub, skips_backtick, quotes_and_escapes, brace_nesting) — 0 failed.

- [ ] **Step 8: clippy**

Run: `cargo clippy 2>&1 | tail -5`
Expected: no new warnings.

- [ ] **Step 9: Commit**

```bash
git add src/lexer.rs
git commit -m "$(cat <<'EOF'
v182: split_modifier_operand preserves backslash-escapes verbatim

The ${var/pat/repl} operand splitter un-escaped \\ and \delim and, in its
fallback arm, left the escaped char unconsumed — so `\"` reprocessed the `"`
as a quote opener, swallowing the `/` delimiter and corrupting the segment
into an unbalanced quote (UnterminatedQuote on e.g. ${V//\\\"/Z}). Now the
splitter preserves every \x verbatim (consuming the char); the single
downstream un-escape in parse_braced_operand_opts does the rest, so final
pattern/replacement Words are unchanged for working cases and \\\" parses.
Also fixes a latent empty-pattern bug (${x/\\/Z}).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS (DONE / DONE_WITH_CONCERNS / NEEDS_CONTEXT / BLOCKED), commit SHA, the Step 5 huck-vs-bash table (all must match), the Step 7 result, any concerns. If a code site doesn't match the line numbers above, find it by the surrounding code shown and report the discrepancy.

---

### Task 2: Bash-diff harness `dollar_subst_escape_diff_check.sh`

**Files:**
- Create: `tests/scripts/dollar_subst_escape_diff_check.sh`

**Context:** Harnesses under `tests/scripts/*_diff_check.sh` run fragments through bash AND huck and assert byte-identical output. All v182 cases print clean stdout with rc 0 in bash, so compare full stdout+exit. These assert the substitution RESULT (not merely that the fragment parses). Model the house style on `tests/scripts/dollar_quote_forms_diff_check.sh` (HUCK_BIN discovery, a `check` helper, PASS/FAIL tally, `exit (FAIL>0)`).

- [ ] **Step 1: Write the harness**

Create `tests/scripts/dollar_subst_escape_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v182: backslash-escaped chars in a
# ${var/pat/repl} substitution operand (pattern/replacement). The `\\\"`
# (escaped-backslash + escaped-quote) run used to crash huck with "unterminated
# quote" (kernel scripts/config line 209: V="${V//\\\"/\"}"). All cases assert
# the substitution RESULT, not just that the fragment parses. rc 0 in bash →
# compare full stdout+exit.
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

# The scripts/config un-escape idiom and friends.
check "unescape backslash-quote"   'V='\''a\"b'\''; echo "${V//\\\"/\"}"'
check "replace esc-quote global"   'V='\''a\"b\"c'\''; echo "${V//\\\"/X}"'
check "anchored prefix esc-quote"  'V='\''\"x'\''; echo "${V/#\\\"/Q}"'

# Escaped-delimiter / escaped-backslash controls (results unchanged by the fix).
check "escaped delimiter"          'V=a/b/c; echo "${V//\//_}"'
check "escaped backslash"          'V='\''x\y'\''; echo "${V//\\/Z}"'
check "single-slash escaped delim" 'V=a/b; echo "${V/\//_}"'

# Plain substitution + substring controls (path unaffected).
check "plain single subst"         'V=foobar; echo "${V/o/O}"'
check "plain global subst"         'V=foobar; echo "${V//o/O}"'
check "substring offset:length"    'V=abcdef; echo "${V:1:3}"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make executable, build, run**

Run:

```bash
chmod +x tests/scripts/dollar_subst_escape_diff_check.sh
cargo build 2>&1 | tail -2
bash tests/scripts/dollar_subst_escape_diff_check.sh
```

Expected: `Total: 9, Pass: 9, Fail: 0`, exit 0. If any case FAILs, STOP and report the diff — investigate whether the implementation regressed or the harness fragment's shell-quoting is wrong (do NOT weaken a case). The `unescape backslash-quote` case must produce bash output `a"b`.

- [ ] **Step 3: Prove the harness is non-tautological**

Build the pre-fix binary in a throwaway worktree and confirm the harness FAILS against it:

```bash
git worktree add /tmp/huck-prefix main 2>&1 | tail -1
( cd /tmp/huck-prefix && cargo build 2>&1 | tail -1 )
HUCK_BIN=/tmp/huck-prefix/target/debug/huck bash tests/scripts/dollar_subst_escape_diff_check.sh; echo "prefix-exit=$?"
git worktree remove --force /tmp/huck-prefix
```

Expected: against the pre-fix binary the harness FAILS — the 3 `\\\"` cases (unescape idiom, replace global, anchored) crash with "unterminated quote"; prefix-exit=1. The control cases pass pre-fix (unchanged results). If it PASSES pre-fix, the harness is tautological — STOP and report. (`main` is the pre-v182 baseline.)

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/dollar_subst_escape_diff_check.sh
git commit -m "$(cat <<'EOF'
v182: bash-diff harness for backslash-escapes in ${var/pat/repl} operands

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

**Context — UP-FRONT grep (v178/v180/v181 lesson):** Before any regression run, grep ALL of `tests/` and `src/` for tests encoding the OLD `split_modifier_operand` un-escaping. The intermediate-string assertions in `split_modifier_operand_quotes_and_escapes` were already updated in Task 1; this grep catches any OTHER assertion of `split_modifier_operand`'s output or any `${var/.../...}` RESULT test that the fix might disturb. Result-level tests (the actual substitution output) MUST stay identical — if one changes, that's a real regression to STOP on, not to update.

- [ ] **Step 1: Up-front grep**

Run:

```bash
grep -rn "split_modifier_operand\|split_substitution\|split_substring" tests/ src/ | grep -v 'fn split_modifier_operand\|fn split_substitution\|fn split_substring'
grep -rn '//\\|/\\\\|\${.*/.*/' tests/scripts/ | head -20
```

Read each hit and classify UPDATE (asserts the old intermediate split output) vs LEAVE (a result-level test that must stay identical, or unrelated). Report the classification of every hit. The only expected UPDATE was already done in Task 1; if another intermediate-output assertion exists, update it to the verbatim form and report it.

- [ ] **Step 2: Full test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: 0 failures across all unit + integration binaries. A result-level substitution test failing is a RED FLAG (the fix should preserve results) — STOP and report rather than "fixing" the assertion. Only an intermediate `split_modifier_operand` output assertion legitimately changes.

- [ ] **Step 3: Whole bash-diff harness suite**

Run:

```bash
cargo build 2>&1 | tail -1
for s in tests/scripts/*_diff_check.sh; do
  out=$(bash "$s" 2>&1); rc=$?
  printf '%s rc=%s %s\n' "$s" "$rc" "$(echo "$out" | tail -1)"
done | grep -v 'Fail: 0' || echo "ALL HARNESSES GREEN"
```

Expected: `ALL HARNESSES GREEN`. The existing `param_*`/`braced_*` substitution harnesses passing is the key proof that results are unchanged.

- [ ] **Step 4: Parse-sweep payoff (scripts/config clears)**

The sweep is parse-only (`huck -n`/`bash -n`), side-effect-free. Run:

```bash
tools/parse_sweep.sh tools/scripts.tsv tools/parse_results.tsv 2>&1 | tail -15
for F in /usr/src/linux-headers-6.8.0-110/scripts/config /usr/src/linux-headers-6.8.0-124/scripts/config; do
  echo "=== $F (expect SILENT, rc 0) ==="
  ./target/debug/huck -n "$F"; echo "huck-n rc=$?"
  bash -n "$F"; echo "bash-n rc=$?"
done
echo "=== any remaining literal 'unterminated quote' gaps? ==="
awk -F'\t' '$3=="HUCK_GAP" && tolower($7) ~ /unterminated quote/{print $6}' tools/parse_results.tsv || true
```

Expected: both `scripts/config` copies `huck -n` SILENT, rc 0 = bash; no `HUCK_GAP` row with a literal "unterminated quote" message remains. Report the new HUCK_GAP vs the 22 baseline (v181 left it at 22); `HUCK_LENIENT`/`HUCK_CRASH` stay 0. The gap is script-level — report the `scripts/config` clearing explicitly.

- [ ] **Step 5: Commit (only if Step 1 changed any file beyond Task 1)**

If the up-front grep produced an additional UPDATE, commit it:

```bash
git add -A
git commit -m "$(cat <<'EOF'
v182: regression + parse-sweep pass; update residual split-output assertion

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

If nothing changed, state that no commit was needed (the regression + sweep are verification only).

## Report back
STATUS, commit SHA (or "no commit needed"), the Step 1 grep classification, Step 2 cargo test summary, Step 3 result, Step 4 scripts/config rc + new HUCK_GAP vs 22 + confirmation no literal "unterminated quote" gap remains.

---

## Final review (whole-branch, before merge)

- [ ] **Step 1: Review the full diff** — `git diff main...HEAD`. Confirm only `split_modifier_operand`'s `'\\'` arm + its doc comment + the `quotes_and_escapes` test (and any Task-3 residual), plus the new harness, changed.
- [ ] **Step 2: Scope boundary** — `scan_braced_operand`, `parse_braced_operand_opts`, and the `$(…)`/backtick/`{…}`/quoted-span skipping in `split_modifier_operand` unchanged; no runtime pattern-matching change; no `bash-divergences.md` change (sweep-found, no tracked entry).
- [ ] **Step 3: Hand off** — project merge ritual (AskUserQuestion before merging to main): merge `--no-ff`, push, delete the local branch, record v182 in `project_huck_iterations.md` + `MEMORY.md`, and correct the backlog note (the literal "unterminated quote" cluster was only `scripts/config`; the `unterminated '((' arith` / `unterminated 'case` gaps are separate clusters).

---

## Self-Review (plan vs spec)

**1. Spec coverage:**
- `split_modifier_operand` `'\\'` arm → verbatim preservation → Task 1 Step 2. ✓
- Doc comment update → Task 1 Step 3. ✓
- Two `quotes_and_escapes` assertions → verbatim; `\\\"` regression unit test → Task 1 Step 6. ✓
- New harness `dollar_subst_escape_diff_check.sh` (un-escape idiom asserting RESULT, controls, substring control) → Task 2. ✓
- Parse-sweep: both `scripts/config` clear + HUCK_GAP report + no literal "unterminated quote" remains → Task 3 Step 4. ✓
- Full `cargo test` + up-front grep for old split-output assertions, result-tests must stay identical → Task 3 Steps 1-3. ✓
- All `*_diff_check.sh` green + clippy → Task 3 Step 3 + Task 1 Step 8. ✓
- No `bash-divergences.md` change; record + correct backlog note → Final review Step 3. ✓

**2. Placeholder scan:** No TBD/TODO. Every code step shows complete before/after code; every run step shows the command + expected output. The Step 5 manual-check quoting is acknowledged fiddly with authoritative cases pinned to the bash column + Task 2 harness.

**3. Type consistency:** `split_modifier_operand(body: &str, delim: char) -> (String, Option<String>)` signature unchanged (arm body only). The regression assertion's Rust literals verified: body `"\\\\\\\"/Z"` = `\\\"/Z` (6 chars), pattern `"\\\\\\\""` = `\\\"` (4 chars), repl `"Z"`. `parse_braced_operand_opts` (the downstream un-escaper) and `scan_braced_operand` (the verbatim extractor) untouched.

**Resolved during planning:** verified all 6 contract fragments' bash results (incl. the `a"b` un-escape idiom and the `bcd` substring control); confirmed the two existing assertions' exact text and that the quoted-span assertion stays unchanged; confirmed the Rust string literals for the `\\\"` regression test.
