# v169: fix L-24 — inherit extglob in arith-nested command substitutions — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Thread the active `LexerOptions` into `arith_string_to_word` so a command substitution nested inside `$(( ))` / `(( ))` / `for ((;;))` inherits the parent's extglob state (fix L-24).

**Architecture:** Add an `opts: LexerOptions` parameter to `arith_string_to_word` (replacing its four `LexerOptions::default()` calls). The `$(( ))` lexer caller passes its in-scope `opts` (path A). For the parser-side `(( ))`/C-for callers (path B), store the options on the token: `Token::ArithBlock(String, LexerOptions)`, captured at lex time and forwarded by the parser. `LexerOptions` is `{ extglob: bool }` (Copy).

**Tech Stack:** Rust (edition 2024). `LexerOptions` (lexer.rs:306). `Token::ArithBlock` (lexer.rs:286). `arith_string_to_word` (lexer.rs:1652).

**Spec:** `docs/superpowers/specs/2026-06-16-arith-extglob-opts-design.md`

**Branch:** `v169-arith-extglob-opts`

---

## Background the implementer needs

- `arith_string_to_word` (lexer.rs:1652) has four `LexerOptions::default()` calls: lexer.rs:1668 & 1710 (`read_dollar_expansion(&mut chars, &mut parts, true, LexerOptions::default())`) and lexer.rs:1673 & 1715 (`scan_backtick_substitution(&mut chars, LexerOptions::default())`). The two pairs are byte-identical lines.
- Callers: lexer.rs:1741 (path A, `$(( ))`, `opts` in scope); command.rs:1040 (`((expr))` at command pos), command.rs:2249 (`((x++))` in pipeline pos), command.rs:1407 (C-for section, inside `parse_arith_for_header`). The latter three are reached from a `Token::ArithBlock(text)`.
- `Token::ArithBlock(String)` is defined at lexer.rs:286 and constructed only at lexer.rs:713 (`tokens.push(Token::ArithBlock(body))`), where `opts` is in scope (the enclosing tokenizer fn has `opts: LexerOptions` at lexer.rs:385).
- Every `Token::ArithBlock` pattern site (the compiler will list them after the variant changes): command.rs 1036, 1037, 1428, 1462, 1610, 2045, 2170, 2245, 2246; lexer.rs test sites 3646, 3654, 3663, 3682, 3706, 7007, 7017, 7030, 7040, 7050, 7081. Note: `command.rs:740` is `ParseError::ArithBlock(String)` (a *different* enum) — **do not** change it.
- L-24 reproduces now (bash succeeds, huck errors): `shopt -s extglob; echo $(( $( [[ foo == @(foo|bar) ]] && echo 1 || echo 0 ) ))` → bash `1`, huck `unterminated '[[ ]]'`.

---

### Task 1: Thread `opts` through `arith_string_to_word` + the `ArithBlock` token

A single atomic change (signature + token variant + all callers/match sites must compile together). Unit test first (TDD).

**Files:**
- Modify: `src/lexer.rs` (function signature, 4 default() calls, token variant + construction, path-A caller, test-site patterns, + new unit test).
- Modify: `src/command.rs` (3 parser callers + match-site patterns).

- [ ] **Step 1: Write the failing unit test**

Add inside the `#[cfg(test)] mod tests { … }` block in `src/lexer.rs` (near the `arith` tests):

```rust
    #[test]
    fn arith_string_to_word_inherits_extglob() {
        // A command substitution inside arithmetic whose body uses an extglob
        // pattern lexes only when extglob is enabled (L-24).
        let body = "$( [[ foo == @(foo|bar) ]] && echo 1 )";
        assert!(arith_string_to_word(body, LexerOptions { extglob: true }).is_ok());
        assert!(arith_string_to_word(body, LexerOptions { extglob: false }).is_err());
    }
```

- [ ] **Step 2: Run it to confirm it fails (wrong arity)**

Run: `cargo test --lib arith_string_to_word_inherits_extglob 2>&1 | grep -E 'error\[|this function takes' | head`
Expected: a compile error — `arith_string_to_word` takes 1 argument but 2 were supplied.

- [ ] **Step 3: Add the `opts` parameter to `arith_string_to_word`**

In `src/lexer.rs:1652`, change:
```rust
pub(crate) fn arith_string_to_word(s: &str) -> Result<Word, LexError> {
```
to:
```rust
pub(crate) fn arith_string_to_word(s: &str, opts: LexerOptions) -> Result<Word, LexError> {
```

- [ ] **Step 4: Replace the four `LexerOptions::default()` inside the function with `opts`**

Apply both replacements across `src/lexer.rs` (these exact lines occur only inside `arith_string_to_word`):
```rust
read_dollar_expansion(&mut chars, &mut parts, true, LexerOptions::default())?;
```
→
```rust
read_dollar_expansion(&mut chars, &mut parts, true, opts)?;
```
and
```rust
let sequence = scan_backtick_substitution(&mut chars, LexerOptions::default())?;
```
→
```rust
let sequence = scan_backtick_substitution(&mut chars, opts)?;
```
Then verify none remain in the function:
Run: `awk 'NR>=1652 && NR<=1728 && /LexerOptions::default/{print NR": "$0}' src/lexer.rs`
Expected: no output.

- [ ] **Step 5: Change the `ArithBlock` token to carry `LexerOptions`**

In `src/lexer.rs:286`, change:
```rust
    ArithBlock(String),
```
to:
```rust
    ArithBlock(String, LexerOptions),
```

- [ ] **Step 6: Store `opts` at the construction site**

In `src/lexer.rs:713`, change:
```rust
                    tokens.push(Token::ArithBlock(body));
```
to:
```rust
                    tokens.push(Token::ArithBlock(body, opts));
```

- [ ] **Step 7: Path A — pass `opts` at the `$(( ))` caller**

In `src/lexer.rs:1741`, change:
```rust
                let body = arith_string_to_word(&inner)?;
```
to:
```rust
                let body = arith_string_to_word(&inner, opts)?;
```

- [ ] **Step 8: Path B — forward `opts` from the three parser callers**

In `src/command.rs`:

(a) `((expr))` at command position (~lines 1036–1040): change the destructure + call so `opts` flows through. Replace:
```rust
        let Some(Token::ArithBlock(text)) = iter.next() else {
            unreachable!("matches! guard above guarantees ArithBlock")
        };
        let body = crate::lexer::arith_string_to_word(&text)
```
with:
```rust
        let Some(Token::ArithBlock(text, opts)) = iter.next() else {
            unreachable!("matches! guard above guarantees ArithBlock")
        };
        let body = crate::lexer::arith_string_to_word(&text, opts)
```

(b) `((x++))` in pipeline position (~lines 2246–2249): replace:
```rust
        let Some(Token::ArithBlock(text)) = iter.next() else {
            unreachable!("matches! guard above guarantees ArithBlock")
        };
        let body = crate::lexer::arith_string_to_word(&text)
```
with:
```rust
        let Some(Token::ArithBlock(text, opts)) = iter.next() else {
            unreachable!("matches! guard above guarantees ArithBlock")
        };
        let body = crate::lexer::arith_string_to_word(&text, opts)
```

(c) C-for header. At the destructure (~line 1428), change:
```rust
        Some(Token::ArithBlock(text)) => text,
```
to capture opts and thread it. Replace the surrounding `let header_text = match … ;` so both are bound:
```rust
    let (header_text, arith_opts) = match iter.next() {
        Some(Token::ArithBlock(text, opts)) => (text, opts),
        _ => unreachable!("caller verified peek"),
    };
    let (init, cond, step) = parse_arith_for_header(&header_text, arith_opts)?;
```
Then change `parse_arith_for_header`'s signature (~line 1394):
```rust
fn parse_arith_for_header(text: &str) -> Result<ArithForHeaderTriple, ParseError> {
```
to:
```rust
fn parse_arith_for_header(text: &str, opts: crate::lexer::LexerOptions) -> Result<ArithForHeaderTriple, ParseError> {
```
and its inner `parse_section` closure call (~line 1407):
```rust
            crate::lexer::arith_string_to_word(trimmed)
```
to:
```rust
            crate::lexer::arith_string_to_word(trimmed, opts)
```
(The closure captures `opts` by copy — `LexerOptions` is `Copy`.)

- [ ] **Step 9: Build; the compiler lists every remaining `ArithBlock` pattern — update each mechanically**

Run: `cargo build 2>&1 | grep -E 'error' | head -40`
For each remaining `Token::ArithBlock` pattern error, apply the mechanical rule:
- `matches!(…, Token::ArithBlock(_))` / `Token::ArithBlock(_) =>` → `Token::ArithBlock(..)`.
  Sites (non-test): command.rs 1036, 1462, 1610, 2045, 2170, 2245.
- `Token::ArithBlock(s) => assert_eq!(s, …)` → `Token::ArithBlock(s, _) => assert_eq!(s, …)`.
  Sites (lexer.rs tests): 7007, 7017, 7030, 7040, 7050.
- `matches!(t, … Token::ArithBlock(_) …)` in tests → `Token::ArithBlock(..)`.
  Sites (lexer.rs tests): 3646, 3654, 3663, 3682, 3706, 7081.
Re-run `cargo build` until clean. (None of these patterns use the string-or-opts beyond the already-bound `s`; the second field is always `_`/`..`.)

- [ ] **Step 10: Build clean + run the new unit test**

Run: `cargo build 2>&1 | tail -2`
Expected: `Finished`.
Run: `cargo test --lib arith_string_to_word_inherits_extglob`
Expected: `test result: ok. 1 passed`.

- [ ] **Step 11: Full lib suite + clippy**

Run: `cargo test --lib 2>&1 | grep -E 'test result: ok|test result: FAILED' | tail -1`
Expected: `test result: ok.` 0 failed (≈2211 passed).
Run: `cargo clippy --lib --bins --quiet 2>&1 | grep -E 'warning|error' || echo CLEAN`
Expected: `CLEAN`.

- [ ] **Step 12: Build the binary + spot-check the L-24 fix end-to-end**

Run:
```bash
cargo build --quiet && H=./target/debug/huck
for f in \
  'shopt -s extglob; echo $(( $( [[ foo == @(foo|bar) ]] && echo 1 || echo 0 ) ))' \
  'shopt -s extglob; echo $(( `[[ ab == @(ab|cd) ]] && echo 3 || echo 4` ))' \
  'shopt -s extglob; (( $( [[ x == @(x|y) ]] && echo 1 || echo 0 ) )); echo $?' \
  'shopt -s extglob; for (( i=$( [[ a == @(a|b) ]] && echo 0 || echo 9 ); i<2; i++ )); do echo $i; done' \
  'echo $(( $(echo 3) + 4 ))'; do
  b=$(bash -c "$f" 2>&1); h=$($H -c "$f" 2>&1); [ "$b" = "$h" ] && echo "OK   $f -> [$h]" || echo "DIFF $f -> bash[$b] huck[$h]"; done
```
Expected: five `OK` lines (`[1]`, `[3]`, `[0]`, `[0\n1]`, `[7]`).

- [ ] **Step 13: Commit**

```bash
git add src/lexer.rs src/command.rs
git commit -m "v169: inherit extglob in arith-nested command substitutions (fix L-24)

Thread LexerOptions through arith_string_to_word (4 default() sites -> opts).
The \$(( )) lexer caller passes its in-scope opts; the (( ))/for-((;;)) parser
callers get opts from Token::ArithBlock, now ArithBlock(String, LexerOptions)
captured at lex time. A command substitution nested in arithmetic now lexes
with the parent's extglob state instead of OFF.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Bash-diff harness + full regression

**Files:**
- Create: `tests/scripts/arith_extglob_diff_check.sh`

- [ ] **Step 1: Create the harness**

Create `tests/scripts/arith_extglob_diff_check.sh`:

```bash
#!/usr/bin/env bash
# v169 (L-24): a command substitution nested inside arithmetic must inherit the
# shell's extglob state. Run fragments through bash and huck; assert identical.
set -u

HUCK="$(dirname "$0")/../../target/debug/huck"
if [ ! -x "$HUCK" ]; then
    echo "build huck first: cargo build" >&2
    exit 1
fi
if ! command -v bash >/dev/null 2>&1; then
    echo "bash not found on PATH; this differential harness requires bash" >&2
    exit 1
fi

fragments=(
    # --- path A: $(( $(...extglob...) )) and backtick ---
    'shopt -s extglob; echo $(( $( [[ foo == @(foo|bar) ]] && echo 1 || echo 0 ) ))'
    'shopt -s extglob; echo $(( $( [[ z == !(a|b) ]] && echo 7 || echo 9 ) ))'
    'shopt -s extglob; v=$(( $( [[ ab == @(ab|cd) ]] && echo 5 || echo 6 ) )); echo $v'
    'shopt -s extglob; echo $(( `[[ ab == @(ab|cd) ]] && echo 3 || echo 4` ))'
    # --- path B: (( )) standalone and for ((;;)) header ---
    'shopt -s extglob; (( $( [[ x == @(x|y) ]] && echo 1 || echo 0 ) )); echo $?'
    'shopt -s extglob; for (( i=$( [[ a == @(a|b) ]] && echo 0 || echo 9 ); i<2; i++ )); do echo $i; done'
    # --- control: plain arith cmdsub (no extglob) is unchanged ---
    'echo $(( $(echo 3) + 4 ))'
    '(( $(echo 1) )); echo $?'
)

fail=0
for f in "${fragments[@]}"; do
    b_out=$(bash -c "$f" 2>&1)
    h_out=$("$HUCK" -c "$f" 2>&1)
    if [ "$b_out" != "$h_out" ]; then
        echo "DIFF on: $f"
        diff <(printf '%s\n' "$b_out") <(printf '%s\n' "$h_out") || true
        echo "---"
        fail=1
    fi
done

if [ "$fail" -eq 0 ]; then
    echo "all ${#fragments[@]} arith-extglob fragments produce identical output to bash"
fi
exit "$fail"
```

- [ ] **Step 2: Make it executable and run it**

Run: `chmod +x tests/scripts/arith_extglob_diff_check.sh && bash tests/scripts/arith_extglob_diff_check.sh`
Expected: `all 8 arith-extglob fragments produce identical output to bash`, exit 0.

- [ ] **Step 3: Run ALL bash-diff harnesses (now 93)**

Run: `p=0; f=0; for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 && p=$((p+1)) || { f=$((f+1)); echo "FAIL $s"; }; done; echo "$p passed, $f failed"`
Expected: `93 passed, 0 failed`.

- [ ] **Step 4: Run the full test suite**

Run: `cargo test >/tmp/v169.log 2>&1; echo "exit: $?"; grep -cE 'test result: FAILED' /tmp/v169.log`
Expected: `exit: 0` and a FAILED count of `0`.

- [ ] **Step 5: Commit**

```bash
git add tests/scripts/arith_extglob_diff_check.sh
git commit -m "test: bash-diff harness for extglob in arith-nested cmdsubs (v169)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Resolve the L-24 divergence doc

**Files:**
- Modify: `docs/bash-divergences.md`

- [ ] **Step 1: Delete the resolved L-24 entry**

Remove the entire `### L-24: a command substitution nested inside `$(( … ))` does not inherit extglob` block (heading at ~line 327 through its `- **Workaround**:` line, inclusive, plus the trailing blank line).

- [ ] **Step 2: Decrement the Tier-4 count**

In the summary table, change the Tier-4 row from `41` to `40`:
```markdown
| Low-impact (Tier 4) | 40 | Open edge cases / cosmetic divergences (`[low]`/`[intentional]`/`[deferred]`). |
```

- [ ] **Step 3: Commit**

```bash
git add docs/bash-divergences.md
git commit -m "docs: resolve L-24 (v169 — extglob inherited in arith-nested cmdsubs)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final review (orchestrator, after all tasks)

- Whole-branch diff: `src/lexer.rs`, `src/command.rs`, the new harness, `docs/bash-divergences.md`. Confirm `arith_string_to_word` callers all pass `opts`, no `LexerOptions::default()` remains in it, and `command.rs:740` (`ParseError::ArithBlock`) was NOT touched.
- Re-verify the path-A and path-B repros by hand against bash; confirm a plain non-extglob script (`echo $(( $(echo 5) ))`) still works.
- Merge `v169-arith-extglob-opts` to main `--no-ff` after user confirmation (AskUserQuestion); push; delete the local branch.
- Record the iteration in `project_huck_iterations.md` + `MEMORY.md`; note the architecture-review scanner-area follow-ons are now all done.

---

## Self-review (plan vs spec)

- **Spec coverage:** `arith_string_to_word(s, opts)` + 4 default→opts (Task 1 Steps 3–4) ✓; path A caller (Step 7) ✓; `ArithBlock(String, LexerOptions)` + construction + 3 parser callers + C-for threading (Steps 5,6,8) ✓; compiler-guided match-site updates (Step 9) ✓; unit test on/off (Step 1) ✓; new harness path A + path B + control (Task 2) ✓; full regression incl. clippy + 93 harnesses (Steps 11, Task 2) ✓; delete L-24, Tier-4 41→40 (Task 3) ✓; scope — arith.rs Pratt tokenizer untouched, ParseError::ArithBlock untouched ✓.
- **Placeholder scan:** none — exact line numbers, exact before/after code, exact expected output; the only compiler-driven step (9) enumerates the known sites.
- **Type consistency:** `arith_string_to_word(&str, LexerOptions) -> Result<Word, LexError>`, `Token::ArithBlock(String, LexerOptions)`, `parse_arith_for_header(&str, LexerOptions)` used consistently across all callers; `LexerOptions { extglob: bool }` constructed in the unit test.
