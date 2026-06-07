# Extglob inside command substitutions (M-101) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make extglob patterns (`!(…)`/`@(…)`/`+(…)`/`*(…)`/`?(…)`) work inside `$(…)` / `` `…` `` command substitutions, array-literal elements, and `${…}` operands — not just at top level — by threading the lexer's `LexerOptions` (extglob) into recursive body re-tokenization.

**Architecture:** Lexer-only. Add `opts: LexerOptions` (a `Copy` struct) to the 10 private `src/lexer.rs` helpers on the recursive-tokenize paths; replace `tokenize(body)` with `tokenize_with_opts(body, opts)` at the recursive sites. `arith_string_to_word` (pub(crate), external callers) keeps its signature and passes `LexerOptions::default()`. No parser/AST/evaluator change.

**Tech Stack:** Rust, `src/lexer.rs` only (+ new test files). Tests: `cargo test --bin huck`, `cargo test --test <name>`, `bash tests/scripts/<name>_diff_check.sh`.

**Spec:** `docs/superpowers/specs/2026-06-07-extglob-in-command-sub-design.md`. Read it first.

**Commit trailer (MANDATORY, canonical — every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Task 1: Thread `opts` into recursive body lexing

**Files:** Modify `src/lexer.rs` only.

- [ ] **Step 1: Write failing lexer unit tests**

Add to the `#[cfg(test)] mod tests` in `src/lexer.rs` (the `Token`/`WordPart`/`LexerOptions` imports are already in scope):

```rust
// Helper: does the token stream parse with NO leftover bare LParen/ArithBlock
// from a mis-lexed extglob inside a command sub? We assert the inner CommandSub
// sequence built successfully (tokenize_with_opts(..extglob..) returns Ok and the
// body did not error).
#[test]
fn extglob_inside_command_sub_lexes() {
    let opts = LexerOptions { extglob: true };
    // `$(echo !(x))` — the inner `!(x)` must be extglob, not negation+subshell.
    let toks = tokenize_with_opts("echo $(echo !(x))", opts).unwrap();
    // The outer word `$(...)` is one CommandSub WordPart; if the body had errored,
    // tokenize_with_opts would have returned Err.
    assert!(toks.iter().any(|t| matches!(
        t, Token::Word(Word(parts)) if parts.iter().any(|p| matches!(p, WordPart::CommandSub { .. }))
    )));
}

#[test]
fn extglob_inside_backtick_sub_lexes() {
    let opts = LexerOptions { extglob: true };
    tokenize_with_opts("echo `echo !(x)`", opts).unwrap(); // must not Err
}

#[test]
fn extglob_inside_array_literal_command_sub_lexes() {
    let opts = LexerOptions { extglob: true };
    // line-1232 shape: array literal whose element is a $() containing !(...)
    tokenize_with_opts("a=($(printf '%s\\n' /tmp/!(x)))", opts).unwrap(); // must not Err
}

#[test]
fn command_sub_without_extglob_still_errors_on_bare_extglob() {
    // With extglob OFF, `!(x)` inside $() is negation+subshell -> the body errors,
    // exactly as before this change (no behavior change when extglob is off).
    let opts = LexerOptions { extglob: false };
    assert!(tokenize_with_opts("echo $(echo !(x))", opts).is_err());
}

#[test]
fn plain_command_sub_unchanged() {
    // Normal command subs unaffected with extglob on or off.
    for eg in [false, true] {
        let opts = LexerOptions { extglob: eg };
        tokenize_with_opts("echo $(echo hi) $((1+1))", opts).unwrap();
    }
}
```
Run `cargo test --bin huck extglob_inside 2>&1 | tail` → the first three FAIL (body re-tokenized with extglob off), the off-control passes.

- [ ] **Step 2: `parse_substitution_body` — the chokepoint**

Change (`src/lexer.rs:1997`):
```rust
fn parse_substitution_body(body: &str, opts: LexerOptions) -> Result<crate::command::Sequence, LexError> {
    let tokens = crate::lexer::tokenize_with_opts(body, opts)
        .map_err(|e| LexError::Substitution(Box::new(e)))?;
    let parsed = crate::command::parse(tokens).map_err(LexError::SubstitutionParseError)?;
    Ok(parsed.unwrap_or_else(empty_sequence))
}
```
(Use the local function name — likely just `tokenize_with_opts(body, opts)` since it's in the same module.)

- [ ] **Step 3: Add `opts` to the command-sub capture helpers**

`scan_paren_substitution` (1915) and `scan_backtick_substitution` (2010):
```rust
fn scan_paren_substitution(chars: &mut CharCursor<'_>, opts: LexerOptions)
    -> Result<crate::command::Sequence, LexError> {
    ...
    return parse_substitution_body(&body, opts); // both return sites (1923 + the depth-0 one)
}

fn scan_backtick_substitution(chars: &mut CharCursor<'_>, opts: LexerOptions)
    -> Result<crate::command::Sequence, LexError> {
    ...
    return parse_substitution_body(&body, opts); // the return at ~2017
}
```

- [ ] **Step 4: Add `opts` to `read_dollar_expansion` + forward it**

`read_dollar_expansion` (1483) signature becomes:
```rust
fn read_dollar_expansion(
    chars: &mut CharCursor<'_>,
    parts: &mut Vec<WordPart>,
    quoted: bool,
    opts: LexerOptions,
) -> Result<(), LexError> {
```
Inside it: pass `opts` to `scan_paren_substitution(chars, opts)`. The `$((`
arithmetic arm calls `arith_string_to_word(&inner)` — leave that call unchanged
(Section 2 of the spec). Any `${…}` handling that calls `parse_braced_operand`
must pass `opts` (Step 5).

- [ ] **Step 5: Add `opts` to the remaining helpers that call `read_dollar_expansion` / re-tokenize**

Add `opts: LexerOptions` (last param) to each of these and forward it to every
`read_dollar_expansion(...)` / `scan_*` / `parse_*` call inside them, and replace
their recursive `tokenize(...)` with `tokenize_with_opts(..., opts)`:

- `scan_extglob_group` (986) — forward `opts` to its `read_dollar_expansion` calls (sites 1006, 1044).
- `scan_regex_operand` (893) — forward `opts` to its `read_dollar_expansion` calls (914, 950).
- `scan_expanding_body_line` (1351) — forward `opts` (1375).
- `parse_braced_operand` (1837) — forward `opts` to its `read_dollar_expansion` calls (1854, 1891); if it has its own `tokenize(...)`, make it `tokenize_with_opts(..., opts)`.
- `read_array_element_word` (2391) — forward `opts`; the `tokenize(&buf)` at ~2501 → `tokenize_with_opts(&buf, opts)`.
- `parse_subscript_body` (2316) — add `opts`; the `tokenize(src)` at 2317 → `tokenize_with_opts(src, opts)`.

Then update ALL call sites of these helpers to pass `opts`:
- In `tokenize_core` (300): every `read_dollar_expansion(&mut chars, …)`, `scan_extglob_group(…)`, `scan_regex_operand(…)`, `parse_subscript_body(…)`, `read_array_element_word(…)`, `scan_expanding_body_line(…)`, `parse_braced_operand(…)`, `scan_backtick_substitution(…)` call passes `opts` (which `tokenize_core` already owns).
- Inside helpers that now have `opts` and call other now-`opts`-taking helpers, forward `opts`.
- `parse_subscript_body`'s caller at 809 is inside a helper — thread `opts` to that helper too if needed (follow the compiler errors; every private helper on these paths gets `opts`).

Strategy: make the signature changes, then **let the compiler list every call site that now needs `opts`** and pass the in-scope `opts` at each. All these helpers are private to `src/lexer.rs`, so the threading is closed within the file.

- [ ] **Step 6: `arith_string_to_word` — keep signature, pass default**

`arith_string_to_word` (1406) is `pub(crate)` with external callers — do NOT add a
param. At its two internal `read_dollar_expansion(...)` calls (1422, 1464), pass
`LexerOptions::default()`:
```rust
read_dollar_expansion(&mut chars, &mut parts, true, LexerOptions::default())?;
```

- [ ] **Step 7: Build + run**

- `cargo build 2>&1 | tail -8` → fix any remaining "this function takes N arguments" call sites by passing the in-scope `opts` (or `LexerOptions::default()` only inside `arith_string_to_word`). It compiles when every path is threaded.
- `cargo test --bin huck extglob_inside 2>&1 | tail` → the 5 new tests pass.
- `cargo test --bin huck 2>&1 | tail -15` → ALL unit tests pass (no regression to normal command subs, arith, subscripts, array literals, v105 `=~`).
- `cargo clippy --bin huck 2>&1 | tail -3` → clean.

- [ ] **Step 8: Commit**

```bash
git add src/lexer.rs
git commit -m "feat(lexer): inherit extglob in recursive body lexing (M-101)

Thread LexerOptions through read_dollar_expansion and the command-sub / array-
element / subscript / braced-operand helpers, so parse_substitution_body and the
other recursive re-tokenize sites use tokenize_with_opts(body, opts) instead of
the default extglob-off tokenize(). Fixes !(...)/@(...) inside \$()/backtick/array
literals. arith_string_to_word keeps its signature (passes default opts). No
parser/AST change.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer MANDATORY/canonical.

## Rules (Task 1)
- src/lexer.rs only. Do not change the parser/evaluator or the public `tokenize`/`tokenize_with_opts` API.
- Do not edit existing tests to pass; if a normal-command-sub / arith / subscript test regresses, you threaded `opts` somewhere it changes behavior — re-check (extglob-off paths must stay byte-identical).
- `arith_string_to_word` MUST keep its current signature (external callers).

## Task 1 report
**DONE/BLOCKED**, commit SHA, the list of functions that gained `opts`, confirmation `arith_string_to_word` is unchanged, the new-test + full-lexer-suite pass lines, clippy status.

---

## Task 2: Reader fix — execute the clean prefix before a failing unit

**Why:** Task 1 threads extglob into recursive lexing, but the v104 reader
(`run_sourced_contents`) tokenizes a whole chunk with the extglob value at chunk
start. bash_completion enables extglob at line 45, but the initial chunk fails to
tokenize at line 1232 (a `$()`-extglob body errors under extglob-off) *before*
line 45's `shopt` runs — so the reader discards everything up to the failure
(huck defines 20/81 functions). Fix: the lexer returns the partial tokens it
produced before the error; the reader executes the complete units (running line
45's `shopt`), then re-lexes the truncated trailing unit with the now-current
extglob. See spec Section 3.5.

**Files:** Modify `src/lexer.rs` (add `tokenize_partial`), `src/builtins.rs`
(`run_sourced_contents`).

- [ ] **Step 1: Failing integration test (the reader symptom)**

Add to `tests/extglob_command_sub_integration.rs` (created in Task 3 — if not yet,
create it now with the `run` helper copied from `tests/set_x_integration.rs`):

```rust
#[test]
fn shopt_extglob_then_later_command_sub_in_same_chunk() {
    // 3-line: shopt on line 1, a marker on line 2, an extglob-in-$() on line 3.
    // Before the reader fix, the whole chunk fails to tokenize and MARKER is
    // skipped. After: MARKER prints and the $() runs.
    let (out, _e, _c) = run("shopt -s extglob\necho MARKER\necho $(echo ok)\n");
    assert!(out.contains("MARKER"));
}

#[test]
fn shopt_extglob_then_function_with_extglob_sub() {
    // The bash_completion _xinetd_services shape: shopt, then a function whose
    // body has a $()-extglob; defining + calling it must work.
    let script = "shopt -s extglob\nf() { echo $(printf '%s\\n' /nonexist/!(x)); }\nf\necho done\n";
    let (out, _e, _c) = run(script);
    assert!(out.contains("done"));
}

#[test]
fn malformed_line_reports_once_no_loop() {
    // A genuinely un-lexable line must report once and continue (no hang).
    let (out, err, _c) = run("echo a\n$(\necho b\n");
    assert!(out.contains('a') || out.contains('b'));
    assert!(err.contains("syntax error"));
}
```
Run `cargo test --test extglob_command_sub_integration shopt_extglob 2>&1 | tail` → `shopt_extglob_then_*` FAIL (MARKER/done skipped), `malformed_line` must not hang. (Use a per-test timeout mindset; if a test hangs, the progress guard is missing.)

- [ ] **Step 2: Add `tokenize_partial` to `src/lexer.rs`**

`tokenize_core` currently returns `Result<(Vec<Token>, Vec<usize>), (LexError, usize)>` via an IIFE closure that returns `Result<(), LexError>`. Change the core to ALSO expose the partial tokens on error, and add a public `tokenize_partial`:

```rust
/// Like `tokenize_with_offsets`, but on a lex error returns the tokens
/// successfully produced BEFORE the error plus `Some((error, byte_offset))`.
/// `offsets.len() == tokens.len() + 1`; the trailing offset is the error offset
/// (or `input.len()` on success).
pub fn tokenize_partial(
    input: &str,
    opts: LexerOptions,
) -> (Vec<Token>, Vec<usize>, Option<(LexError, usize)>) {
    // same body as tokenize_core, but the final match becomes:
    //   match result {
    //     Ok(()) => { offsets.push(input.len()); (tokens, offsets, None) }
    //     Err(e) => { offsets.push(chars.offset()); (tokens, offsets, Some((e, chars.offset()))) }
    //   }
}
```
Refactor so `tokenize_core`/`tokenize_with_offsets`/`tokenize_with_opts` delegate to ONE implementation (e.g. `tokenize_partial` is the core; `tokenize_with_offsets` wraps it: `let (t,o,err) = tokenize_partial(...); match err { None => Ok((t,o)), Some(e) => Err(e) }`; `tokenize_with_opts` drops offsets the same way). Existing `tokenize_with_offsets`/`tokenize_with_opts` behavior + their tests are UNCHANGED.

Build + run the existing offset tests: `cargo test --bin huck offsets_ tokenize 2>&1 | tail` → still pass.

- [ ] **Step 3: Rewrite the tokenize call in `run_sourced_contents`**

In `src/builtins.rs`, replace the `match tokenize_with_offsets(...) { Ok ... Err ... }`
block with `tokenize_partial` + the truncation handling. Sketch (preserve all
existing unit-execution + extglob-flip + `set -v` + ExecOutcome logic):

```rust
let extglob = shell.shopt_options.get("extglob").unwrap_or(false);
let (tokens, offsets, terr) = crate::lexer::tokenize_partial(
    &contents[start..], crate::lexer::LexerOptions { extglob },
);
let total = tokens.len();
if total == 0 {
    // nothing lexed. If there was an error, report+skip; else done.
    if let Some((e, foff)) = terr {
        eprintln!("huck: {}: line {}: syntax error{}", path.display(),
            line_of(start + foff), crate::shell::lex_error_message(e));
        last_status = 2;
        start = next_line_start(start + foff);
        prev_end = start;
        continue 'outer;
    }
    break;
}
let mut iter = tokens.into_iter().peekable();
loop {
    while matches!(iter.peek(), Some(crate::lexer::Token::Newline)) { iter.next(); }
    let unit_start_idx = total - iter.len();
    if iter.peek().is_none() {
        // Consumed every token in this (possibly partial) batch.
        if let Some((e, foff)) = &terr {
            // The batch was truncated by a lex error after the last complete unit.
            let resume = start + offsets[total]; // = error offset (sentinel)
            if resume > start {
                start = resume; prev_end = start; continue 'outer; // re-lex remainder
            }
            // No progress possible -> report + skip.
            eprintln!("huck: {}: line {}: syntax error{}", path.display(),
                line_of(start + foff), crate::shell::lex_error_message(e.clone()));
            last_status = 2;
            start = next_line_start(start + foff);
            prev_end = start;
            continue 'outer;
        }
        break 'outer; // clean end of input
    }
    match crate::command::parse_one_unit(&mut iter) {
        Ok(None) => { /* same as today */ }
        Ok(Some(seq)) => { /* execute exactly as today: set -v echo, execute,
                              ExecOutcome match, extglob-flip re-lex */ }
        Err(e) if terr.is_some() && is_unterminated(&e) => {
            // Truncated trailing unit: re-lex it with the now-current extglob.
            let resume = start + offsets[unit_start_idx];
            if resume > start {
                start = resume; prev_end = start; continue 'outer;
            }
            // Truncated unit is the FIRST unit and extglob did not change ->
            // genuinely un-lexable. Report the lex error + skip its line.
            let (le, foff) = terr.clone().unwrap();
            eprintln!("huck: {}: line {}: syntax error{}", path.display(),
                line_of(start + foff), crate::shell::lex_error_message(le));
            last_status = 2;
            start = next_line_start(start + foff);
            prev_end = start;
            continue 'outer;
        }
        Err(e) => { /* genuine parse error: existing report + resync path */ }
    }
}
```
Add a small helper:
```rust
fn is_unterminated(e: &crate::command::ParseError) -> bool {
    use crate::command::ParseError::*;
    matches!(e, UnterminatedFunction | UnterminatedLoop | UnterminatedIf
        | UnterminatedCase | UnterminatedBrace | UnterminatedSubshell
        | UnterminatedDoubleBracket)
}
```
(Confirm the exact `ParseError` variant names by reading `src/command.rs`; include every "unterminated/incomplete compound" variant. `LexError` may need `Clone` — it likely already derives it; if not, clone the message string instead.)

IMPORTANT subtleties:
- The progress guard (`resume > start`) is what prevents an infinite loop on a
  genuinely malformed first unit. Make sure BOTH the iter-empty and the
  unterminated-Err branches enforce it.
- Keep the existing extglob-flip re-lex inside the `Ok(Some(seq))` arm — when the
  clean prefix's `shopt -s extglob` executes, that flip path ALSO triggers a
  re-lex; the truncation path is the fallback when the flip happens to land mid
  un-lexable construct.
- `LexError` doesn't implement `Display` directly — reuse `crate::shell::lex_error_message` exactly as the current code does.

- [ ] **Step 4: Run tests**
- `cargo test --test extglob_command_sub_integration 2>&1 | tail -20` → Step-1 tests pass (MARKER/done print; malformed reports once, no hang).
- `cargo test 2>&1 | tail -20` → FULL suite green (the reader change must not regress v104's linear-source-reader tests, `set -v`, errexit, etc.).
- `cargo clippy --all-targets 2>&1 | tail -3` → clean.

- [ ] **Step 5: bash_completion smoke**
```bash
printf 'source /usr/share/bash-completion/bash_completion\ndeclare -F 2>/dev/null | wc -l\n' > /tmp/bcf.sh
./target/debug/huck /tmp/bcf.sh 2>/tmp/bce.txt | tail -1   # function count (target: ~81)
echo "errors: $(grep -c 'syntax error' /tmp/bce.txt)"; grep 'syntax error' /tmp/bce.txt | head -3
```
Report the function count (should jump from 20 toward ~81) and the remaining errors (the next gaps). Lines 1232/1249 `command substitution` errors should be GONE.

- [ ] **Step 6: Commit**
```bash
git add src/lexer.rs src/builtins.rs
git commit -m "feat: reader executes clean prefix before a lex-failing unit (M-101)

tokenize_partial returns tokens produced before a lex error; run_sourced_contents
executes the complete units (so an earlier shopt -s extglob takes effect) then
re-lexes the truncated trailing unit with the now-current extglob. Fixes the v104
batch-reader interaction where an early shopt couldn't affect a later extglob-in-
command-sub in the same chunk. Progress guard prevents loops on malformed lines.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 2 report
**DONE/BLOCKED**, commit SHA, the `tokenize_partial` approach, confirmation existing `tokenize_with_offsets`/`tokenize_with_opts` are unchanged, the integration + full-suite pass lines, the bash_completion function count (before/after), clippy.

---

## Task 3: Integration tests + bash_completion smoke

**Files:** Create `tests/extglob_command_sub_integration.rs`.

- [ ] **Step 1: Write match-semantics tests (vs bash)**

Copy the `run(script) -> (stdout, stderr, code)` helper from `tests/set_x_integration.rs`. Use a per-test temp dir with known files so glob results are deterministic. Verify each expected value under `bash` first.

```rust
use std::fs;

fn in_tmp(files: &[&str], script: &str) -> (String, String, i32) {
    // create a unique temp dir, touch `files`, cd into it in the script
    let dir = std::env::temp_dir().join(format!("huck_eg_{}", std::process::id()));
    let _ = fs::create_dir_all(&dir);
    for f in files { let _ = fs::write(dir.join(f), ""); }
    let full = format!("cd '{}'\nshopt -s extglob\n{}", dir.display(), script);
    let r = run(&full);
    let _ = fs::remove_dir_all(&dir);
    r
}

#[test]
fn extglob_in_command_sub_literal() {
    // assignment from a command sub whose body has !(...) — no glob match needed
    let (out, _e, _c) = run("shopt -s extglob\nx=$(echo a/!(b)); echo \"$x\"\n");
    assert_eq!(out, "a/!(b)\n"); // !(b) doesn't match a path here -> literal (nullglob off)
}

#[test]
fn extglob_in_command_sub_globs() {
    let (out, _e, _c) = in_tmp(&["keep", "skip"], "echo $(printf '%s\\n' !(skip))\n");
    assert_eq!(out, "keep\n");
}

#[test]
fn extglob_in_backtick_sub() {
    let (out, _e, _c) = in_tmp(&["keep", "skip"], "echo `printf '%s\\n' !(skip)`\n");
    assert_eq!(out, "keep\n");
}

#[test]
fn extglob_in_array_literal_command_sub() {
    // the line-1232 shape: array literal element is a $() with !(...)
    let (out, _e, _c) = in_tmp(&["keep", "skip"], "a=($(printf '%s\\n' !(skip))); echo \"${a[0]}\"\n");
    assert_eq!(out, "keep\n");
}

#[test]
fn extglob_off_command_sub_unchanged() {
    let (out, _e, _c) = run("echo $(echo hi)\n");
    assert_eq!(out, "hi\n");
}
```
Confirm each asserted value with `bash` first (the comments reflect bash behavior — `a/!(b)` stays literal because nullglob is off and `!(b)` matches no path). Adjust assertions to match real bash if any differ; if huck disagrees with bash, STOP and report.

Run: `cargo build && cargo test --test extglob_command_sub_integration 2>&1 | tail -20` → all pass. Then `cargo test 2>&1 | tail -5` (no regressions).

- [ ] **Step 2: bash_completion smoke (not committed)**

```bash
printf 'source /usr/share/bash-completion/bash_completion\necho HUCK_END\n' > /tmp/bc.sh
./target/debug/huck /tmp/bc.sh 2>&1 | grep -nE "line 1232|line 1249|command substitution" | head
echo "--- next distinct errors ---"
./target/debug/huck /tmp/bc.sh 2>&1 | grep -iE 'error' | sort -u | head -5
```
Expected: lines 1232/1249 `command substitution` errors GONE. Report the FIRST remaining error (the next gap — do NOT fix it). If the file is absent, skip and say so.

- [ ] **Step 3: Commit**
```bash
git add tests/extglob_command_sub_integration.rs
git commit -m "test: extglob inside command subs / array literals (M-101)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: bash-diff harness (31st)

**Files:** Create `tests/scripts/extglob_command_sub_diff_check.sh`.

- [ ] **Step 1: Write the harness**

Copy `tests/scripts/dbracket_regex_diff_check.sh`'s structure verbatim (HUCK_BIN, `check()`, `Total/Pass/Fail` footer, non-zero exit). `cargo build` first. Each fragment must set up a deterministic temp dir + files inline and print stable output. Fragments:

```
mkdir -p /tmp/hkeg$$; cd /tmp/hkeg$$; : > keep; : > skip; shopt -s extglob; echo $(printf '%s\n' !(skip)); cd /; rm -rf /tmp/hkeg$$
mkdir -p /tmp/hkeg$$; cd /tmp/hkeg$$; : > keep; : > skip; shopt -s extglob; echo `printf '%s\n' !(skip)`; cd /; rm -rf /tmp/hkeg$$
mkdir -p /tmp/hkeg$$; cd /tmp/hkeg$$; : > keep; : > skip; shopt -s extglob; a=($(printf '%s\n' !(skip))); echo "${a[0]}"; cd /; rm -rf /tmp/hkeg$$
shopt -s extglob; x=$(echo a/!(b)); echo "$x"
echo $(echo plain)
```
(Use a fixed dir name with `$$` so bash and huck each make their own; both create+clean it, so output is just the globbed filenames.) Run each under `bash --norc --noprofile` first to confirm huck agrees.

- [ ] **Step 2: Run**
`bash tests/scripts/extglob_command_sub_diff_check.sh 2>&1 | tail` → `Total: 5, Pass: 5, Fail: 0`. If a fragment legitimately diverges (confirm by running both shells), drop with a `# dropped:` comment and report; do NOT mask an M-101 bug.

- [ ] **Step 3: Commit**
```bash
git add tests/scripts/extglob_command_sub_diff_check.sh
git commit -m "test: bash-diff harness for extglob in command subs (31st)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Documentation

**Files:** Modify `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Read structure**
`grep -n '^## Change log\|Tier 1\|Last updated\|^- \*\*L-23\|^### M-100\|2026-06-07' docs/bash-divergences.md | head` and `grep -n 'v105' README.md`. Read the M-100 entry, the v105 change-log entry + README row, and the L-23 note to match formatting. Confirm next free is **M-101** and **L-24**.

- [ ] **Step 2: Add M-101 `[fixed v106]`**
Tier-1 entry: recursive body re-tokenization dropped the parent's `LexerOptions`, so extglob `!(…)`/`@(…)` inside `$()`/backtick/array-literal/`${}` lexed as negation/subshell (→ `syntax error in command substitution: unexpected token after command`); fix = thread `opts` through the 10 private lexer helpers, `tokenize_with_opts(body, opts)` at recursive sites; `arith_string_to_word` unchanged (default opts); reached via v104/v105; bash_completion lines 1232/1249 payoff. Bump Tier-1 count.

- [ ] **Step 3: Add L-24 `[intentional]`**
Tier-4: a command substitution nested inside `$(( … ))` arithmetic does not inherit extglob (`arith_string_to_word` passes default opts to keep its pub(crate) signature) — negligible edge. Bump Tier-4 count / "Last updated".

- [ ] **Step 4: Change-log + README row**
`2026-06-07` v106 change-log entry (style of v105): mechanism, the bash_completion 1232/1249 payoff, 31st harness, test count, L-24, next gap. Add the v106 README iteration row after v105.

- [ ] **Step 5: Verify + commit**
`grep -n 'M-101\|fixed v106\|L-24\|v106' docs/bash-divergences.md README.md` → real numbers, no placeholders.
```bash
git add docs/bash-divergences.md README.md
git commit -m "docs: v106 extglob in command subs (M-101) — changelog, README, L-24

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Final (after all tasks)
- [ ] Whole-branch review: `git log --oneline main..HEAD`, `git diff --stat main..HEAD`.
- [ ] `cargo test 2>&1 | tail -5` (green), `cargo clippy --all-targets 2>&1 | tail -2` (clean).
- [ ] All harnesses: `for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done` (silent = pass).
- [ ] AskUserQuestion merge gate, then `git merge --no-ff` + push + delete branch, then update memory files.
