# v175: permissive function-name charset Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Accept bash's function-name character set (`foo-bar`, `foo.bar`, `a+b`, `2foo`, …) so real scripts parse, without loosening for-loop / coproc identifier validation.

**Architecture:** Function-name validation currently reuses the strict identifier validator `valid_identifier_text`, shared with for-loop variables and coproc names. Add a dedicated permissive `valid_function_name_text` (single unquoted non-empty `Literal`, not a reserved keyword) and use it at only the two function-definition name sites.

**Tech Stack:** Rust (the huck parser, `src/command.rs`); bash-diff harness; the parse-compat sweep (`tools/parse_sweep.sh`).

**Spec:** `docs/superpowers/specs/2026-06-17-function-name-charset-design.md`

**Branch:** `v175-function-name-charset`

**Background facts (verified):**
- huck rejects `foo-bar() {…}`, `foo.bar`, `foo:bar`, `a+b`, `2foo`, `function foo-bar {…}` with "invalid function name"; bash accepts all. 11 real scripts (fzf, completion functions) hit this.
- `function foo()` and plain identifier names already work; the ONLY defect is the charset.
- `valid_identifier_text` (`command.rs:1326`) is used at exactly three places: the two function-def name sites (`1183`, `1218`) and `for_variable_name`/coproc (`1357`, `1589`). Only the two function-def sites change.
- Function-def *detection* does NOT depend on the charset (`foo-bar()` already reaches `parse_function_def` and errors inside it), so no detection/routing code needs changing.
- `huck -n <file>` is parse-only (verified side-effect-free), rc 0 on a clean parse.

---

### Task 1: Add `valid_function_name_text` and use it at the function-def sites

**Files:** Modify `src/command.rs`.

- [ ] **Step 1: Add the permissive validator**

In `src/command.rs`, immediately AFTER the closing `}` of `valid_identifier_text` (the function ending at ~line 1350, just before `/// Returns the loop-variable name …` / `fn for_variable_name`), add:
```rust
/// Returns the function name if `word` is a single, unquoted, non-empty
/// `Literal` that is not a reserved keyword. Unlike `valid_identifier_text`, this
/// does NOT restrict the character set: bash accepts almost any single word as a
/// function name (`foo-bar`, `a.b`, `2foo`, …), and the lexer already guarantees a
/// single `Literal` has no metacharacters or whitespace, so the trailing `()`
/// (or the `function` keyword) — not the name's spelling — is what makes it a
/// definition. The keyword guard keeps `if() { :; }` a syntax error like bash.
fn valid_function_name_text(word: &Word) -> Option<String> {
    if word.0.len() != 1 {
        return None;
    }
    let WordPart::Literal { text, quoted: false } = &word.0[0] else {
        return None;
    };
    if text.is_empty() {
        return None;
    }
    let tok = Token::Word(Word(vec![WordPart::Literal {
        text: text.clone(),
        quoted: false,
    }]));
    if keyword_of(&tok).is_some() {
        return None;
    }
    Some(text.clone())
}
```

- [ ] **Step 2: Swap the two function-def call sites**

Both `parse_function_def` (line 1183) and `parse_function_keyword_def` (line 1218) have the identical line:
```rust
    let name = valid_identifier_text(&name_word).ok_or(ParseError::FunctionName)?;
```
Change BOTH to:
```rust
    let name = valid_function_name_text(&name_word).ok_or(ParseError::FunctionName)?;
```
(Leave `for_variable_name` at line 1357 and the coproc check at line 1589 on `valid_identifier_text` — unchanged.)

- [ ] **Step 3: Build**

Run: `cargo build 2>&1 | tail -2`
Expected: `Finished`. (Note: `valid_identifier_text` is still referenced by `for_variable_name`/coproc, so it does not become dead code.)

- [ ] **Step 4: Verify the fix + regressions (vs bash)**

Run:
```bash
H="$(pwd)/target/debug/huck"
echo "--- now parse + define + call (must match bash) ---"
for frag in \
  'foo-bar() { echo "[$1]"; }; foo-bar X' \
  'foo.bar() { echo dot; }; foo.bar' \
  'ns:fn() { echo colon; }; ns:fn' \
  'a+b() { echo plus; }; a+b' \
  '2foo() { echo digit; }; 2foo' \
  'function fzf-widget { echo kw; }; fzf-widget' \
  'function f-g() { echo kwparen; }; f-g'; do
  b=$(printf '%s\n' "$frag" | bash 2>&1); h=$(printf '%s\n' "$frag" | "$H" 2>&1)
  [ "$b" = "$h" ] && echo "  OK: [$h]  <= $frag" || echo "  MISMATCH bash=[$b] huck=[$h]  <= $frag"
done
echo "--- regressions: both shells must still REJECT these (nonzero exit) ---"
for frag in 'if() { :; }' 'for a-b in 1; do :; done'; do
  printf '%s\n' "$frag" | bash  -n /dev/stdin >/dev/null 2>&1; bn=$?
  printf '%s\n' "$frag" | "$H" -n /dev/stdin >/dev/null 2>&1; hn=$?
  { [ "$bn" != 0 ] && [ "$hn" != 0 ]; } && echo "  OK both reject (bash=$bn huck=$hn): $frag" || echo "  BAD bash=$bn huck=$hn: $frag"
done
```
Expected: every positive case prints `OK: [...]` (the seven function names define and call, output identical to bash); both regression cases print `OK both reject`.

- [ ] **Step 5: Commit**

```bash
git add src/command.rs
git commit -m "v175: permissive function-name charset (bash-compatible)

Function-name validation reused the strict identifier validator
valid_identifier_text (shared with for-loop vars + coproc names), wrongly
rejecting bash-legal names like foo-bar, foo.bar, a+b, 2foo. Add a dedicated
valid_function_name_text (single unquoted non-empty Literal, not a keyword; no
charset restriction) and use it at the two function-def sites. for-loop / coproc
identifier validation is unchanged; if() {...} is still a syntax error like bash.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Bash-diff harness for function-name charset

**Files:** Create `tests/scripts/function_name_diff_check.sh`.

- [ ] **Step 1: Write the harness**

Create `tests/scripts/function_name_diff_check.sh` with exactly:
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v175: bash-legal function-name characters
# (-, ., :, +, leading digit) across all three definition forms. Each case DEFINES
# and CALLS the function, asserting identical stdout+exit. Negative regressions
# (reserved-word name, hyphen for-loop var) are asserted by exit code only (the
# error WORDING legitimately differs: `huck:` vs `bash: line N:`).
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

both_reject() {  # label ; fragment — assert BOTH shells reject (nonzero -n exit)
    local label="$1" frag="$2" bn hn
    printf '%s\n' "$frag" | bash --norc --noprofile -n /dev/stdin >/dev/null 2>&1; bn=$?
    printf '%s\n' "$frag" | "$HUCK_BIN" -n /dev/stdin >/dev/null 2>&1; hn=$?
    if [[ "$bn" != 0 && "$hn" != 0 ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s (bash rc=%s huck rc=%s; both should be nonzero)\n' "$label" "$bn" "$hn"; FAIL=$((FAIL+1)); fi
}

check "dash name"        'foo-bar() { echo "[$1]"; }; foo-bar X'
check "dot name"         'foo.bar() { echo dot; }; foo.bar'
check "colon name"       'ns:fn() { echo colon; }; ns:fn'
check "plus name"        'a+b() { echo plus; }; a+b'
check "leading digit"    '2foo() { echo digit; }; 2foo'
check "function kw dash" 'function fzf-widget { echo kw; }; fzf-widget'
check "function kw paren" 'function f-g() { echo kwparen; }; f-g'
check "plain identifier"  'foo_bar() { echo id; }; foo_bar'
both_reject "reserved name rejected" 'if() { :; }'
both_reject "hyphen for-var rejected" 'for a-b in 1; do :; done'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Run it**

Run: `chmod +x tests/scripts/function_name_diff_check.sh && cargo build --quiet && bash tests/scripts/function_name_diff_check.sh`
Expected: `Total: 10, Pass: 10, Fail: 0`.
If a `check` case FAILs, the fix missed that name shape — investigate (do not weaken). If a `both_reject` case FAILs because huck now ACCEPTS `if()` or `for a-b in`, the validator is too loose (it must keep the keyword guard / leave for-loop validation alone) — report it.

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/function_name_diff_check.sh
git commit -m "test: v175 bash-diff harness for permissive function-name charset

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Parse-sweep payoff + full regression

**Files:** none (verification only).

- [ ] **Step 1: Confirm the payoff via the parse sweep**

```bash
cargo build --quiet
PARSE_TIMEOUT=10 bash tools/parse_sweep.sh tools/scripts.tsv /tmp/v175_sweep.tsv | tail -10
echo "--- remaining 'invalid function name' HUCK_GAPs (was 11) ---"
awk -F'\t' '$3=="HUCK_GAP" && index($7,"invalid function name")' /tmp/v175_sweep.tsv | wc -l
echo "--- total HUCK_GAP now (was 60 after v174) ---"
awk -F'\t' '$3=="HUCK_GAP"' /tmp/v175_sweep.tsv | wc -l
echo "--- any new HUCK_LENIENT? (should stay 0) ---"
awk -F'\t' '$3=="HUCK_LENIENT"' /tmp/v175_sweep.tsv | wc -l
```
Expected: "invalid function name" drops to `0`, total `HUCK_GAP` falls by ~11 (≈60 → ≈49), `HUCK_LENIENT` stays `0`, no `HUCK_CRASH`. If "invalid function name" gaps REMAIN, inspect one (`awk -F'\t' '$3=="HUCK_GAP" && index($7,"invalid function name"){print $6; exit}' /tmp/v175_sweep.tsv`, then the failing line) — report it rather than forcing.

- [ ] **Step 2: Full regression**

Run: `cargo clippy --lib --bins --quiet 2>&1 | grep -E 'warning|error' || echo CLEAN` → `CLEAN`.
Run: `cargo test >/tmp/v175.log 2>&1; echo "exit: $?"; grep -cE 'test result: FAILED' /tmp/v175.log` → `exit: 0`, `0`.
Run: `p=0; f=0; for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 && p=$((p+1)) || { f=$((f+1)); echo "FAIL $s"; }; done; echo "$p passed, $f failed"` → `0 failed` (count is now 97 with the new harness).

Note: there is an existing unit test `parse_function_invalid_name` in `src/command.rs` (~line 4379). If it asserts that a *hyphen/dot*-style name is invalid, it is now wrong and should be updated to use a name that is STILL invalid (e.g. a reserved word like `if`, or a quoted/multi-part name); if it already uses such a case, leave it. Check it during this step and fix if needed (then it's part of the Step-2 green run; commit any change with the lexer test).

- [ ] **Step 3: No commit** unless the unit-test tweak above was needed (commit that with `git add src/command.rs` and a message like `test: update parse_function_invalid_name for v175 permissive names`). Report the before/after `HUCK_GAP` numbers and regression results.

---

## Final review (orchestrator, after all tasks)

- Whole-branch diff: `src/command.rs` (new `valid_function_name_text` + two call-site swaps, and any `parse_function_invalid_name` test tweak), the new `tests/scripts/function_name_diff_check.sh`. Confirm `valid_identifier_text`, `for_variable_name`, and the coproc check are untouched.
- Re-run `function_name_diff_check.sh` (10/10) and the full harness suite (97/97); spot-check the fzf script now parses: `./target/debug/huck -n /usr/share/doc/fzf/examples/key-bindings.bash && echo "fzf parses"`.
- Merge `v175-function-name-charset` to main `--no-ff` after user confirmation (AskUserQuestion); push; delete the branch.
- Record in `project_huck_iterations.md` + `MEMORY.md`; update the parse-sweep backlog note (HUCK_GAP ~49 left). No `bash-divergences.md` change.

---

## Self-review (plan vs spec)

- **Spec coverage:** new `valid_function_name_text` (Task 1 Step 1) ✓; both call-site swaps (Task 1 Step 2) ✓; `valid_identifier_text`/for-loop/coproc unchanged (Task 1 Step 2 note + final review) ✓; harness with -/./:/+ /leading-digit across all 3 forms + define-and-call + regressions (Task 2) ✓; parse-sweep payoff 11→0 (Task 3 Step 1) ✓; full regression + the pre-existing `parse_function_invalid_name` unit-test check (Task 3 Step 2) ✓; iteration record, no divergence doc (final review) ✓.
- **Placeholder scan:** none — complete validator code, exact before/after for both call sites, full harness, exact verification commands with expected output. The `parse_function_invalid_name` step gives concrete guidance (use a still-invalid name), not a vague "fix tests".
- **Type/name consistency:** `valid_function_name_text(word: &Word) -> Option<String>` matches `valid_identifier_text`'s signature and the call-site usage `valid_function_name_text(&name_word).ok_or(ParseError::FunctionName)?`; `keyword_of`, `Token`, `Word`, `WordPart::Literal` are all already in scope in `command.rs` (used by `valid_identifier_text` directly above); harness filename and `target/debug/huck` consistent across tasks.
