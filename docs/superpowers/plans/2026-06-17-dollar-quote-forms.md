# v181: `$`-quote forms (`$'…'` ANSI-C, `$"…"` locale) in the lexer — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make huck's lexer match bash for both `$`-quote forms — `$'…'` (ANSI-C) is special only outside double quotes, and `$"…"` (locale translation) is implemented as the identity (huck has no message catalog), fixing a crash (`$'` inside double quotes → "unterminated quote") and a wrong-output bug (`$"hello"` → `$hello`).

**Architecture:** Both fixes are in one shared kernel, `scan_dollar_expansion` (`src/lexer.rs:1747`), which every word scanner routes through. (1) Gate the existing `$'` ANSI-C arm with `if !quoted`; inside double quotes the `$` then falls to the `_` literal-`$` arm and the caller's double-quote loop handles the trailing `'` as a literal. (2) Add a `Some('"') if !quoted` arm that drops the `$` and leaves the `"` unconsumed, so the caller's existing double-quote handler scans the body as a plain double-quoted string (identity translation).

**Tech Stack:** Rust (huck shell). Verification via `cargo test`, `tests/scripts/*_diff_check.sh` bash-diff harnesses, and `tools/parse_sweep.sh`.

**Spec:** `docs/superpowers/specs/2026-06-17-dollar-quote-forms-design.md`

**Branch:** `v181-dollar-quote-forms` (create from `main`; do NOT implement on `main`).

---

## Confirmed bash behavior (the contract; C locale)

Verified directly against bash 5.x. `★` = currently broken in huck.

| Fragment | bash stdout | current huck |
|---|---|---|
| `echo "$'"` | `$'` | ★ unterminated quote (rc 2) |
| `echo "a$'b"` | `a$'b` | ★ unterminated quote |
| `echo "ping6)$'"` | `ping6)$'` | ★ unterminated quote |
| `x="cost $'n"; echo "$x"` | `cost $'n` | ★ unterminated quote |
| `echo 'a "b'"'c)$'"'d'` | `a "b'c)$'d` | ★ unterminated quote |
| `echo $"hello"` | `hello` | ★ `$hello` |
| `x=Z; echo $"a $x b"` | `a Z b` | ★ `$a Z b` |
| `echo $""` | (empty) | ★ `$` |
| `echo $"with \"escaped\" and $x"` | `with "escaped" and ` | ★ `$with "escaped" and ` |
| `echo $'a\tb\nc'` | `a⇥b`⏎`c` | `a⇥b`⏎`c` (already correct — control) |

---

### Task 1: Fix both `$`-quote arms in `scan_dollar_expansion`

**Files:**
- Modify: `src/lexer.rs:1785-1789` (the `$'` arm)
- Modify: `src/lexer.rs` (add a `$"` arm adjacent to it)
- Test: `src/lexer.rs` (unit tests in the existing `mod tests`)

**Context:** `scan_dollar_expansion(chars, parts, quoted, opts)` (`src/lexer.rs:1747`) is called with `quoted=true` from the double-quote loop (`src/lexer.rs:560`) and `quoted=false` from the main word loop (`:604`) and the no-brace-expansion loop (`:1159`). The `$` has already been consumed; the function matches on `chars.peek()`. The current `$'` arm (lines 1785-1789) ignores `quoted`. The `_` arm (lines 1826-1828) pushes a literal `$` **without consuming** the peeked char. The double-quote loop's `Some(ch) => quoted_current.push(ch)` (`:568`) turns a leftover `'` into a literal; its `Some('"') => break` (`:542`) closes the string. The main and no-brace loops each have a `'"'` arm (`:536`, `:1178`) that scans a double-quoted body.

- [ ] **Step 1: Create the branch**

```bash
cd /home/john/projects/shuck
git checkout main && git checkout -b v181-dollar-quote-forms
```

- [ ] **Step 2: Gate the `$'` arm with `if !quoted`**

In `src/lexer.rs`, the arm currently reads:

```rust
        Some('\'') => {
            chars.next();
            let text = scan_ansi_c_quoted(chars)?;
            parts.push(WordPart::Literal { text, quoted: true });
        }
```

Replace it with (add the `if !quoted` guard + an explanatory comment):

```rust
        // `$'…'` is ANSI-C quoting ONLY outside double quotes. Inside `"…"`
        // (`quoted`) bash treats the `$` as a literal char, so skip this arm and
        // fall through to the `_` arm (literal `$`, the `'` left for the caller's
        // double-quote loop to take as a literal) — matching bash `echo "$'"` → `$'`.
        Some('\'') if !quoted => {
            chars.next();
            let text = scan_ansi_c_quoted(chars)?;
            parts.push(WordPart::Literal { text, quoted: true });
        }
```

- [ ] **Step 3: Add the `$"` locale-translation arm**

Immediately AFTER the `$'` arm from Step 2, add:

```rust
        // `$"…"` is bash's locale-translation quoting, special only outside double
        // quotes. huck has no message catalog, so the translation is the identity:
        // `$"…"` ≡ `"…"`. Drop the `$` and leave the `"` unconsumed so the caller's
        // existing double-quote handler scans the body (with its normal
        // expansions/escapes). Inside double quotes (`quoted`) `$"` is a literal `$`
        // via the `_` arm, after which the `"` closes the surrounding string.
        Some('"') if !quoted => {}
```

(The empty body is intentional: consume nothing, push nothing — the caller reprocesses the `"`.)

- [ ] **Step 4: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: compiles clean.

- [ ] **Step 5: Manual behavior check against bash**

Run:

```bash
B=target/debug/huck
chk() { printf '%s\n' "$2" > /tmp/v181.sh
  printf '%-34s huck=<%s> bash=<%s>\n' "$1" \
    "$($B /tmp/v181.sh 2>&1 | tr '\n' '/')" "$(bash --norc /tmp/v181.sh 2>&1 | tr '\n' '/')"; }
chk 'dq $quote'            'echo "$'"'"'"'
chk 'dq a$quote b'         'echo "a$'"'"'b"'
chk 'assign dq $quote'     'x="cost $'"'"'n"; echo "$x"'
chk 'nested quote-switch'  'echo '"'"'a "b'"'"'"'"'"'"'"'"'c)$'"'"'"'"'"'"'"'"'"'d'"'"''
chk 'locale $"hello"'      'echo $"hello"'
chk 'locale $"a $x b"'     'x=Z; echo $"a $x b"'
chk 'locale $""'           'echo $""'
chk 'unquoted ANSI-C'      'echo $'"'"'a\tb\nc'"'"''
```

Expected: every line shows `huck=<…>` byte-equal to `bash=<…>` — `$'`, `a$'b`, `cost $'n`, `a "b'c)$'d`, `hello`, `a Z b`, empty, `a⇥b/c`. (The shell-quoting in `chk` calls is fiddly; if a call is mis-quoted, fix the call — the fragments must match the table above. The authoritative cases live in the Task 2 harness.)

- [ ] **Step 6: Add lexer unit tests**

In `src/lexer.rs`, inside `mod tests`, add (near `tokenize_dollar_var_in_double_quotes_is_quoted`, ~line 4429):

```rust
    #[test]
    fn tokenize_dollar_squote_inside_double_quotes_is_literal() {
        // v181: `$'` inside double quotes is a literal `$` + `'`, NOT ANSI-C
        // quoting; it must tokenize (pre-fix this was Err(UnterminatedQuote)).
        let toks = tokenize("\"$'\"").unwrap();
        assert_eq!(toks.len(), 1);
        let Token::Word(Word(parts)) = &toks[0] else { panic!("not a word: {toks:?}") };
        let joined: String = parts.iter().map(|p| match p {
            WordPart::Literal { text, .. } => text.clone(),
            other => panic!("unexpected part {other:?}"),
        }).collect();
        assert_eq!(joined, "$'");
    }

    #[test]
    fn tokenize_dollar_dquote_locale_drops_dollar() {
        // v181: `$"x"` is locale-translation quoting = identity; the `$` is
        // dropped and the body is a plain double-quoted literal `x`.
        assert_eq!(tokenize("$\"x\"").unwrap(), vec![wq("x")]);
    }

    #[test]
    fn tokenize_unquoted_ansi_c_still_decodes() {
        // v181 regression: unquoted `$'…'` ANSI-C escapes still decode (the
        // `!quoted` guard must not disturb the outside-double-quotes path).
        assert_eq!(tokenize("$'a\\tb'").unwrap(), vec![wq("a\tb")]);
    }
```

- [ ] **Step 7: Run the new unit tests**

Run: `cargo test --lib tokenize_dollar_squote_inside_double_quotes_is_literal tokenize_dollar_dquote_locale_drops_dollar tokenize_unquoted_ansi_c_still_decodes 2>&1 | tail -15`
Expected: 3 passed, 0 failed.

- [ ] **Step 8: clippy**

Run: `cargo clippy 2>&1 | tail -5`
Expected: no new warnings. (An empty match-arm body `Some('"') if !quoted => {}` is fine; clippy does not flag it given the explanatory comment.)

- [ ] **Step 9: Commit**

```bash
git add src/lexer.rs
git commit -m "$(cat <<'EOF'
v181: $-quote forms — $'…' ANSI-C only outside dquotes, $"…" locale identity

scan_dollar_expansion gated both $-quote arms on !quoted: `$'` inside double
quotes is now a literal `$`+`'` (fixes the "unterminated quote" crash, e.g.
`echo "$'"`); a new `$"` arm drops the `$` and lets the existing double-quote
handler scan the body (identity translation — huck has no message catalog —
so `$"hello"` → `hello`). Inside double quotes both fall to the literal-`$`
arm, matching bash. Lexer unit tests added.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS (DONE / DONE_WITH_CONCERNS / NEEDS_CONTEXT / BLOCKED), commit SHA, the Step 5 huck-vs-bash table (all must match), and any concerns. If a code site doesn't match the line numbers above, find it by the surrounding code shown and report the discrepancy.

---

### Task 2: Bash-diff harness `dollar_quote_forms_diff_check.sh`

**Files:**
- Create: `tests/scripts/dollar_quote_forms_diff_check.sh`

**Context:** The harness suite under `tests/scripts/*_diff_check.sh` runs fragments through bash AND huck and asserts byte-identical output. All v181 cases print clean stdout with rc 0 in bash (no intentional-stderr cases), so compare full stdout + exit. Model the house style on `tests/scripts/for_var_name_diff_check.sh` (HUCK_BIN discovery, a `check` helper, PASS/FAIL tally, `exit (FAIL>0)`).

- [ ] **Step 1: Write the harness**

Create `tests/scripts/dollar_quote_forms_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v181: the two bash `$`-quote forms.
#   `$'…'`  ANSI-C quoting — special ONLY outside double quotes; inside `"…"`
#           the `$` is a literal char (huck used to crash: "unterminated quote").
#   `$"…"`  locale-translation quoting — identity here (no message catalog), so
#           `$"…"` ≡ `"…"` (huck used to leak a leading `$`).
# All cases print clean stdout, rc 0 in bash → compare full stdout+exit.
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

# $' inside double quotes — literal `$` + `'` (was a crash).
check "dq dollar-squote"        'echo "$'\''"'
check "dq a dollar-squote b"    'echo "a$'\''b"'
check "dq regex-anchor end"     'echo "ping6)$'\''"'
check "assign dq dollar-squote" 'x="cost $'\''n"; echo "$x"'
check "nested quote-switch"     'echo '\''a "b'\''"'\''c)$'\''"'\''d'\'''

# $"…" locale translation = identity (drop the leading `$`).
check "locale plain"            'echo $"hello"'
check "locale with expansion"   'x=Z; echo $"a $x b"'
check "locale empty"            'echo $""'
check "locale escaped dquote"   'echo $"with \"escaped\" and $x"'

# Controls — unquoted ANSI-C still decodes; plain quotes unaffected.
check "unquoted ANSI-C escapes" 'echo $'\''a\tb\nc'\'''
check "plain dquote"            'echo "x"'
check "plain squote"            'echo '\''y'\'''

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make executable, build, run**

Run:

```bash
chmod +x tests/scripts/dollar_quote_forms_diff_check.sh
cargo build 2>&1 | tail -2
bash tests/scripts/dollar_quote_forms_diff_check.sh
```

Expected: `Total: 12, Pass: 12, Fail: 0`, exit 0. If any case FAILs, STOP and report the diff — investigate whether the implementation or the harness fragment's shell-quoting is wrong (do NOT weaken a case). The `nested quote-switch` fragment must produce bash `a "b'c)$'d`; if the harness's own quoting is off, fix the harness line, not the assertion.

- [ ] **Step 3: Prove the harness is non-tautological**

Build the pre-fix binary in a throwaway worktree and confirm the harness FAILS against it:

```bash
git worktree add /tmp/huck-prefix main 2>&1 | tail -1
( cd /tmp/huck-prefix && cargo build 2>&1 | tail -1 )
HUCK_BIN=/tmp/huck-prefix/target/debug/huck bash tests/scripts/dollar_quote_forms_diff_check.sh; echo "prefix-exit=$?"
git worktree remove --force /tmp/huck-prefix
```

Expected: against the pre-fix binary the harness FAILS (the 5 `$'`-in-dq cases mismatch on the crash, the 4 `$"…"` cases mismatch on the leading `$`; ≥9 FAIL lines, prefix-exit=1). If it PASSES pre-fix, the harness is tautological — STOP and report. (`main` is the pre-v181 baseline; v181 lives only on the branch.)

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/dollar_quote_forms_diff_check.sh
git commit -m "$(cat <<'EOF'
v181: bash-diff harness for $'…' and $"…" $-quote forms

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS, commit SHA, Step 2 Total/Pass/Fail line, Step 3 prefix-exit and which cases failed pre-fix (proving non-tautology).

---

### Task 3: Parse-sweep payoff, regression, and L-48 resolution

**Files:**
- Modify (likely): old-behavior tests surfaced by the up-front grep
- Modify: `docs/bash-divergences.md` (DELETE the L-48 entry)
- Run: full `cargo test`, all `tests/scripts/*_diff_check.sh`, `tools/parse_sweep.sh`

**Context — UP-FRONT grep (v178/v180 lesson):** Before any regression run, grep ALL of `tests/` and `src/` for tests that encode the OLD buggy behavior. Likely sites: any test asserting `$"…"` produces a leading `$` (search `$"` and `\$"`), or any lexer test feeding `$'` inside double quotes. Integration tests are separate binaries that only surface on a full `cargo test`.

- [ ] **Step 1: Up-front grep**

Run:

```bash
grep -rn '\$"' tests/ src/ | grep -v 'dollar_quote_forms_diff_check' | head -40
grep -rn "ansi_c\|ANSI\|\$'" src/lexer.rs | grep -i test
```

Read each hit and classify UPDATE (encodes the old `$"`→`$…` leak or an old `$'`-in-dq error) vs LEAVE (unrelated — e.g. a `$"` that is genuinely inside single quotes, or a comment). Report the classification of every hit. Do NOT change anything that doesn't encode the old broken behavior.

- [ ] **Step 2: Update any old-behavior tests**

For each UPDATE hit, change the expectation to the corrected behavior (`$"…"` yields the body with no leading `$`; `$'` inside double quotes is the literal `$'`). Show exactly what you changed. If there are NO such hits, state that explicitly.

- [ ] **Step 3: Full test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: 0 failures across all unit + integration binaries. If a binary fails on a `$"`/`$'` assertion not caught in Step 1, classify and update it the same way, then re-run. Do NOT weaken unrelated tests — if a failure is unexpected, STOP and report.

- [ ] **Step 4: Whole bash-diff harness suite**

Run:

```bash
cargo build 2>&1 | tail -1
for s in tests/scripts/*_diff_check.sh; do
  out=$(bash "$s" 2>&1); rc=$?
  printf '%s rc=%s %s\n' "$s" "$rc" "$(echo "$out" | tail -1)"
done | grep -v 'Fail: 0' || echo "ALL HARNESSES GREEN"
```

Expected: `ALL HARNESSES GREEN`.

- [ ] **Step 5: Parse-sweep payoff (fcnal-test clears)**

The sweep is parse-only (`huck -n`/`bash -n`), side-effect-free. Run:

```bash
tools/parse_sweep.sh tools/scripts.tsv 2>&1 | tail -15
F=/usr/src/linux-headers-6.8.0-110/tools/testing/selftests/net/fcnal-test.sh
echo "=== fcnal-test huck -n (expect SILENT, rc 0) ==="
./target/debug/huck -n "$F"; echo "huck-n rc=$?"
bash -n "$F"; echo "bash-n rc=$?"
```

Expected: `fcnal-test.sh` `huck -n` emits NO diagnostics, rc 0 = bash (all three former cascade errors — lines 184/348/712 — cleared). Report `HUCK_GAP` vs the 26 baseline (`/tmp/v179_sweep_out.txt` is stale at 29; v180 left it at 26); `HUCK_LENIENT`/`HUCK_CRASH` stay 0. The gap is script-level, so report the fcnal-test clearing explicitly, not just the headline number; note any other `$`-quote users that move.

- [ ] **Step 6: Delete the L-48 entry**

L-48 misidentified the root cause (it blamed awk-in-cmdsub; the real cause is `$'` inside double quotes, now fixed) and is fully resolved. In `docs/bash-divergences.md`, delete the entire L-48 bullet (the line starting `- **L-48: spurious parse diagnostics on a single-quoted \`awk\` C-style-for body inside \`$(…)\`**`). Verify nothing else references `L-48`:

```bash
grep -n "L-48" docs/bash-divergences.md || echo "no L-48 references remain"
```

- [ ] **Step 7: Commit**

```bash
git add docs/bash-divergences.md
# add any test files updated in Step 2
git commit -m "$(cat <<'EOF'
v181: resolve L-48 (delete entry); regression + parse-sweep pass

L-48's root cause was misidentified (awk-in-cmdsub); the real bug was `$'`
inside double quotes derailing the lexer, fixed in v181. fcnal-test.sh now
parses silently (huck -n rc 0 = bash). Full cargo test + all bash-diff
harnesses green.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS, commit SHA, the Step 1 grep classification, exactly which test files changed (or "none"), Step 3 cargo test summary, Step 4 result, Step 5 fcnal-test rc + HUCK_GAP vs 26, and confirmation L-48 is gone.

---

## Final review (whole-branch, before merge)

- [ ] **Step 1: Review the full diff** — `git diff main...HEAD`. Confirm only `scan_dollar_expansion`'s two arms + unit tests, the new harness, the L-48 deletion, and any genuinely-old-behavior test updates changed.
- [ ] **Step 2: Scope boundary** — `scan_ansi_c_quoted`/`decode_ansi_c_escape` unchanged; no command-sub/arith/`${…}` arm changes; no real gettext (identity only); error-message wording untouched.
- [ ] **Step 3: Hand off** — project merge ritual (AskUserQuestion before merging to main): merge `--no-ff`, push, delete the local branch, record v181 in `project_huck_iterations.md` + `MEMORY.md`, and correct the sweep-memory note that attributed the fcnal-test gap to awk-in-cmdsub. L-48 already deleted on the branch (Task 3).

---

## Self-Review (plan vs spec)

**1. Spec coverage:**
- `$'` `if !quoted` guard → Task 1 Step 2. ✓
- `$"` drop-arm (identity translation) → Task 1 Step 3. ✓
- `quoted == true` needs no new code (falls to `_`) → covered by Steps 2-3 design + Step 5 cases. ✓
- New harness `dollar_quote_forms_diff_check.sh` (the listed `$'`-in-dq, `$"…"`, and control cases) → Task 2. ✓
- Lexer unit tests (`"$'"` tokenizes; `$"x"` drops `$`) → Task 1 Step 6 (plus an unquoted-ANSI-C regression test). ✓
- Parse-sweep fcnal-test clears + HUCK_GAP report → Task 3 Step 5. ✓
- Full `cargo test` + up-front grep for old `$"`/`$'` behavior → Task 3 Steps 1-3. ✓
- All `*_diff_check.sh` green + clippy → Task 3 Step 4 + Task 1 Step 8. ✓
- Delete L-48; record in iterations + MEMORY + correct sweep note → Task 3 Step 6 + Final review Step 3. ✓

**2. Placeholder scan:** No TBD/TODO. Every code step shows complete code; every run step shows the command + expected output. The Step 5 manual-check fragment quoting is acknowledged as fiddly with the authoritative cases pinned to the table and the Task 2 harness.

**3. Type consistency:** `scan_dollar_expansion(chars, parts, quoted, opts)` signature unchanged (arms only). `wq(s)` builds a single quoted `Literal` word — matches the `tokenize("$\"x\"") == vec![wq("x")]` assertion. The `$'`-in-dq unit test joins `WordPart::Literal` texts rather than asserting an exact part count (robust to part-merging). `WordPart`, `Word`, `Token` all already in scope in `lexer.rs`'s `mod tests`.

**Resolved during planning:** verified all 10 contract fragments against bash and current huck (5 `$'`-in-dq crashes, 4 `$"…"` leaks, 1 control already-correct); confirmed `wq` shape for the Fix-2 unit test; confirmed `tokenize("\"$'\"")` yields two literal parts (hence the join-based assertion, not `wq`).
