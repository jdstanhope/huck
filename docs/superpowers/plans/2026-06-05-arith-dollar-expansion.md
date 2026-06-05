# `$`-expansion in Arithmetic + `declare -f/-F` Silent Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `(( ))`, `$(( ))`, and C-style `for ((;;))` headers expand `$`-forms (`$#`, `${…}`, `$(…)`, `$@`, `$1`) before arithmetic evaluation, and make `declare -f`/`-F` on a missing function silent (rc 1, no output) like bash.

**Architecture:** Defer arithmetic parsing for those three sites from lex/parse time to **eval** time, mirroring the existing `eval_subscript` (`src/expand.rs:120`): expand the arithmetic body to a string via the normal word-expansion machinery, then `arith::parse` + `arith::eval`. Arith bodies are carried as an expandable `Word` instead of a pre-parsed `ArithExpr`. A single helper `arith_string_to_word` builds those Words (reusing `read_dollar_expansion`), and a single helper `eval_arith_word` does expand→parse→eval.

**Tech Stack:** Rust (binary crate `huck`). Unit tests `cargo test --bin huck`; integration `cargo test --test <name>`; bash-diff harness under `tests/scripts/`.

---

## Implementation refinement vs spec

The spec's table shows `Token::ArithBlock(Word)`. We instead **keep `Token::ArithBlock(String)` as raw text** and convert to a `Word` at parse time, because the arith-`for` header is lexed as one `ArithBlock` and `parse_arith_for_header` must split it on top-level `;` *before* conversion (splitting a `Word` on a literal `;` is awkward). Only `$(( ))` builds its `Word` in the lexer (where `read_dollar_expansion` is in scope). Behavior is identical to the spec.

## File Structure

- `src/lexer.rs` — new `pub(crate) fn arith_string_to_word`; `WordPart::Arith { body: Word, quoted }`; `$(( ))` path builds a `Word`. `Token::ArithBlock(String)` unchanged. Existing lexer unit tests that match `WordPart::Arith { expr }` updated.
- `src/command.rs` — `Command::Arith(Word)`; `ArithForClause` init/cond/step `Option<Word>`; `parse_arith_for_header` + the standalone-`((` parse convert raw text to `Word`(s). Dead `ParseError::ArithBlock`/`ArithForHeader` removed if unused.
- `src/expand.rs` — new `pub(crate) fn eval_arith_word`; the two `WordPart::Arith` eval sites (`:665`, `:795`) call it; existing expand unit tests building `WordPart::Arith { expr }` updated.
- `src/executor.rs` — `run_arith` (`:476`) and `run_arith_for_inner` (init/cond/step) call `eval_arith_word`.
- `src/arith.rs` — the now-unreachable `$`-branch in `tokenize` (`:144`) documented or removed.
- `src/builtins.rs` — `declare_list_functions` (`:853`) silent on missing.
- `tests/arith_dollar_integration.rs`, `tests/scripts/arith_dollar_diff_check.sh` — NEW.
- `tests/declare_silent_integration.rs` (or extend an existing declare test file) + harness fragments — NEW.
- `docs/bash-divergences.md`, `README.md` — M-88 fixed; M-90 deferred; changelog; README row.

---

### Task 1: Core refactor — expand-then-parse for `(( ))`, `$(( ))`, arith-`for`

This is a type-propagation refactor that must compile as a unit. Implement in this order; lean on `cargo build` to surface every call/test site that needs updating.

**Files:** `src/lexer.rs`, `src/command.rs`, `src/expand.rs`, `src/executor.rs`, `src/arith.rs` (+ their existing unit tests)

- [ ] **Step 1: Write the failing integration test (TDD driver)**

Create `tests/arith_dollar_integration.rs`:

```rust
//! v93: $-forms inside arithmetic contexts (M-88).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn dollar_hash_in_dbl_paren() {
    assert_eq!(run("set -- a b\n(($# == 2)) && echo Y || echo N\n").0, "Y\n");
}

#[test]
fn arr_len_in_dbl_paren() {
    assert_eq!(run("a=(x y z)\n((${#a[@]} == 3)) && echo Y || echo N\n").0, "Y\n");
}

#[test]
fn param_expansion_in_arith_expansion() {
    assert_eq!(run("set -- -a5\necho $((${1#-a} + 2))\n").0, "7\n");
}

#[test]
fn command_sub_in_arith_expansion() {
    assert_eq!(run("echo $(( $(echo 3) * 4 ))\n").0, "12\n");
}

#[test]
fn dollar_in_arith_for_header() {
    assert_eq!(run("a=(x y z)\nfor ((i=0; i<${#a[@]}; i++)); do printf '%s' \"$i\"; done\necho\n").0, "012\n");
}

#[test]
fn bare_identifier_still_works() {
    assert_eq!(run("n=5\necho $((n + 1))\n").0, "6\n");
}

#[test]
fn quote_removal_in_arith() {
    assert_eq!(run("x=5\n(( x == \"5\" )) && echo Y || echo N\n").0, "Y\n");
}

#[test]
fn empty_arith_expansion_is_zero() {
    assert_eq!(run("e=\necho $(( e ))\n").0, "0\n");
}
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test --test arith_dollar_integration 2>&1 | tail -20`
Expected: failures (current huck errors `expected identifier after '$'` on the `$#`/`${…}`/`$(…)` cases).

- [ ] **Step 3: Add `arith_string_to_word` to `src/lexer.rs`**

Place near `read_dollar_expansion`. It converts a raw arithmetic body string into an expandable `Word`, treating it as double-quoted content (so `$@`/`$*`/command-sub follow bash's in-arith "as if within double quotes" rule). Reuse `read_dollar_expansion` for `$`-forms and the existing backtick scanner for `` `…` ``.

```rust
/// Converts a raw arithmetic body string into an expandable `Word`, treating
/// it as if within double quotes (bash's rule for arithmetic expressions).
/// `$`-forms become ParamExpansion/Var/CommandSub/Arith parts; backticks
/// become CommandSub; everything else is literal text. Used by `$(( ))`
/// (lexer) and, via `command.rs`, by `(( ))` and arith-`for` headers.
pub(crate) fn arith_string_to_word(s: &str) -> Result<Word, LexError> {
    let mut chars = s.chars().peekable();
    let mut parts: Vec<WordPart> = Vec::new();
    let mut lit = String::new();
    while let Some(&c) = chars.peek() {
        match c {
            '$' => {
                if !lit.is_empty() {
                    parts.push(WordPart::Literal { text: std::mem::take(&mut lit), quoted: true });
                }
                chars.next();
                read_dollar_expansion(&mut chars, &mut parts, true)?;
            }
            '`' => {
                if !lit.is_empty() {
                    parts.push(WordPart::Literal { text: std::mem::take(&mut lit), quoted: true });
                }
                chars.next();
                let sequence = scan_backtick_substitution(&mut chars)?;
                parts.push(WordPart::CommandSub { sequence, quoted: true });
            }
            _ => { lit.push(c); chars.next(); }
        }
    }
    if !lit.is_empty() {
        parts.push(WordPart::Literal { text: lit, quoted: true });
    }
    Ok(Word(parts))
}
```

Note: confirm the exact name of the backtick scanner via `grep -n 'fn scan_backtick' src/lexer.rs`; if it differs, use the actual name. If it requires the opening `` ` `` already consumed, the `chars.next()` above handles that — verify against an existing caller.

- [ ] **Step 4: Change `WordPart::Arith` to carry a `Word`**

In `src/lexer.rs`, change the variant (`:138`) from `Arith { expr: crate::arith::ArithExpr, quoted: bool }` to:

```rust
    Arith { body: Word, quoted: bool },
```

In `read_dollar_expansion`'s `$((` arm (`:1116`), replace the eager parse:

```rust
                chars.next(); // consume second '(' — this is `$((`
                let inner = scan_arith_body(chars)?;
                let body = arith_string_to_word(&inner)?;
                parts.push(WordPart::Arith { body, quoted });
```

- [ ] **Step 5: Change the AST carriers in `src/command.rs`**

- `Command::Arith(crate::arith::ArithExpr)` (`:443`) → `Command::Arith(crate::lexer::Word)`.
- `ArithForClause` (`:492`): `init`/`cond`/`step` from `Option<crate::arith::ArithExpr>` to `Option<crate::lexer::Word>`.
- Standalone `((expr))` parse (`:733`): replace
  ```rust
  let expr = crate::arith::parse(&text).map_err(|e| ParseError::ArithBlock(e.to_string()))?;
  return Ok(Command::Arith(expr));
  ```
  with
  ```rust
  let body = crate::lexer::arith_string_to_word(&text)
      .map_err(|e| ParseError::ArithBlock(e.to_string()))?;
  return Ok(Command::Arith(body));
  ```
- `parse_arith_for_header` (`:1070`): keep `split_top_level_semi(text)` and the "exactly 3 sections" check; change `ArithForHeaderTriple` to `Option<crate::lexer::Word>` ×3, and change `parse_section` to:
  ```rust
  let parse_section = |s: &str| -> Result<Option<crate::lexer::Word>, ParseError> {
      let trimmed = s.trim();
      if trimmed.is_empty() {
          Ok(None)
      } else {
          crate::lexer::arith_string_to_word(trimmed)
              .map(Some)
              .map_err(|e| ParseError::ArithBlock(e.to_string()))
      }
  };
  ```

- [ ] **Step 6: Add `eval_arith_word` to `src/expand.rs`**

Place beside `eval_subscript`:

```rust
/// Bash-faithful arithmetic evaluation of an arith body `Word`: expand all
/// `$`-forms + quotes first (as `eval_subscript` does for subscripts), then
/// parse and evaluate. Empty/all-whitespace expansion is `0` (bash: `$(())`==0).
pub(crate) fn eval_arith_word(
    body: &Word,
    shell: &mut Shell,
) -> Result<i64, crate::arith::ArithError> {
    let s = crate::param_expansion::expand_word_to_string(body, shell);
    let t = s.trim();
    if t.is_empty() {
        return Ok(0);
    }
    let expr = crate::arith::parse(t)?;
    crate::arith::eval(&expr, shell)
}
```

Confirm `crate::arith::parse` returns `Result<ArithExpr, ArithError>` and `eval` returns `Result<i64, ArithError>` (check `eval_subscript`'s usage at `:122-124`; match the actual error type — adjust the return type if it differs).

- [ ] **Step 7: Update the two `$(( ))` eval sites in `src/expand.rs`**

Both `WordPart::Arith { expr, quoted: _ }` arms (`:665`, `:795`) change to `WordPart::Arith { body, quoted: _ }` and call `eval_arith_word(body, shell)` instead of `crate::arith::eval(expr, shell)`. Keep the surrounding `Ok(n) => …` / `Err(e) => { eprintln!("huck: arithmetic: {e}"); shell.set_last_status(1); … }` behavior unchanged. The `is_quoted`-style site (`:863`) matching `WordPart::Arith { quoted, .. }` is field-name-compatible (`..`) and needs no change.

- [ ] **Step 8: Update the eval sites in `src/executor.rs`**

- `run_arith` (`:476`): signature changes from `expr: &crate::arith::ArithExpr` to `body: &crate::lexer::Word`; body becomes `match crate::expand::eval_arith_word(body, shell) { Ok(0) => Continue(1), Ok(_) => Continue(0), Err(e) => { eprintln!("huck: ((: {e}"); Continue(1) } }`. Update its caller (the `Command::Arith` match arm) to pass the `Word`.
- `run_arith_for_inner`: the `init` eval (`:~510`), `cond` eval (`:529`), and `step` eval (`:570`) switch from `crate::arith::eval(x, shell)` to `crate::expand::eval_arith_word(x, shell)` where `x: &Word`. `None` segments keep current meaning (init/step skipped; `cond: None` ⇒ value 1).

- [ ] **Step 9: Clean up the arith `$`-branch + dead ParseErrors**

- In `src/arith.rs`, the `'$' =>` branch in `tokenize` (`:144`) is now unreachable from all callers (subscripts and the three arith sites all pass `$`-free strings post-expansion). Either delete it or add a doc comment: `// Unreachable: all callers expand $-forms before arith::parse (v93). Kept defensive.` Decide by `grep -n 'arith::parse' src/*.rs` — if no caller can pass a raw `$`, prefer deleting for clarity.
- `ParseError::ArithBlock` (`:560`) is still produced (Steps 5) so keep it. `ParseError::ArithForHeader` (`:563`): if no longer produced after Step 5, remove it and any `continuation::classify` arm referencing it; otherwise keep. Let the compiler's dead-code/unused warnings guide this.

- [ ] **Step 10: Fix existing unit tests that construct/match `WordPart::Arith`**

`cargo build --bin huck 2>&1` will flag them. Sites: `src/lexer.rs:3826, 3841, 3855, 3875, 3885, 3896-3897` (match `{ expr }`) and `src/expand.rs:1884, 1901, 1918` (build `{ expr: … }`). Convert each to the `body: Word` shape — e.g. a test that built `WordPart::Arith { expr: ArithExpr::Number(2), quoted: false }` becomes `WordPart::Arith { body: Word(vec![WordPart::Literal { text: "2".into(), quoted: true }]), quoted: false }`, and matches on `body` instead of `expr`. Preserve each test's intent (a lexer test asserting `$((1+2))` produces an Arith part now asserts the part is `Arith { body, .. }` whose expansion/literal text is `1+2`).

- [ ] **Step 11: Build + run the new integration test + full suite**

Run: `cargo build --bin huck && cargo test --test arith_dollar_integration 2>&1 | tail -20`
Expected: all 8 tests PASS.
Run: `cargo test --bin huck 2>&1 | tail -8 && cargo test 2>&1 | grep -E 'test result|error' | tail -40`
Expected: full suite green (no regressions in existing arith/lexer/for tests).

- [ ] **Step 12: Clippy + commit**

Run: `cargo clippy --all-targets 2>&1 | tail -5`
```bash
git add src/lexer.rs src/command.rs src/expand.rs src/executor.rs src/arith.rs tests/arith_dollar_integration.rs
git commit -m "feat: expand \$-forms in (( )) / \$(( )) / arith-for before eval (M-88)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer is mandatory and canonical — include exactly, with "(1M context)".

---

### Task 2: `declare -f`/`-F` silent on missing function

**Files:** `src/builtins.rs` (`declare_list_functions`, `:853`); unit test in the `#[cfg(test)] mod tests` block

- [ ] **Step 1: Write the failing unit test**

In the builtins test module (near `declare_f_lists_functions`, `:9536`):

```rust
#[test]
fn declare_f_missing_is_silent() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    // -f on a missing function: rc 1, no stdout (and, per fix, no error text).
    let oc = declare_list_functions(&["nope".to_string()], false, &mut buf, &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(1)));
    assert_eq!(String::from_utf8_lossy(&buf), "");
}
```

(If `declare_list_functions` isn't reachable from the test module, drive it through `run`/`builtin_declare` with `-F nope` and assert `Continue(1)` + empty stdout instead — match the surrounding tests' calling convention.)

- [ ] **Step 2: Run to confirm it passes for stdout but the stderr line still prints**

Run: `cargo test --bin huck declare_f_missing_is_silent 2>&1 | tail -10`
Expected: the assertion passes for the return/stdout, but manually confirm via `printf 'declare -F nope\n' | ./target/debug/huck 2>&1` that the `huck: declare: nope: not found` line currently prints. That stderr line is what we remove.

- [ ] **Step 3: Remove the missing-function diagnostic**

In `declare_list_functions` (`:869-873`), change:

```rust
        if shell.functions.contains_key(name) {
            let _ = writeln!(out, "declare -f {name}");
        } else {
            eprintln!("huck: declare: {name}: not found");
            exit = 1;
        }
```
to:
```rust
        if shell.functions.contains_key(name) {
            let _ = writeln!(out, "declare -f {name}");
        } else {
            // bash: `declare -f`/`-F` on a missing function is silent (rc 1).
            exit = 1;
        }
```

Leave the `-p`-on-unset-variable diagnostics (`:976`, `:1618`) UNCHANGED — bash prints those.

- [ ] **Step 4: Verify**

Run: `cargo test --bin huck declare 2>&1 | tail -10 && printf 'declare -F nope; echo rc=$?\n' | ./target/debug/huck 2>&1`
Expected: declare tests pass; the manual run prints only `rc=1` (no "not found" line).

- [ ] **Step 5: Commit**

```bash
git add src/builtins.rs
git commit -m "fix: declare -f/-F on a missing function is silent like bash

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Arith bash-diff harness

**Files:** `tests/scripts/arith_dollar_diff_check.sh` (NEW, huck's 19th harness)

- [ ] **Step 1: Create the harness**

Mirror the structure of `tests/scripts/dbracket_multiline_diff_check.sh` (the `check "label" 'frag'` helper that runs a fragment through both bash and huck and asserts byte-identical combined stdout+stderr+exit). Use real bash_completion idioms:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v93: $-forms inside arithmetic (M-88).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[ -x "$HUCK_BIN" ] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [ "$b" = "$h" ]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "dollar-hash cmp"   'set -- a b; (($# == 2)) && echo yes || echo no'
check "arr-len cmp"       'a=(x y z); ((${#a[@]} == 3)) && echo yes || echo no'
check "arr-len minus"     'a=(x y z); i=1; ((i < ${#a[@]} - 1)) && echo yes || echo no'
check "param-strip arith" 'set -- -a5; echo $((${1#-a} + 2))'
check "cmdsub in arith"   'echo $(( $(echo 3) * 4 ))'
check "arith-for dollar"  'a=(x y z); for ((i=0; i<${#a[@]}; i++)); do printf %s "$i"; done; echo'
check "bare ident"        'n=5; echo $((n + 1))'
check "quote removal"     'x=5; (( x == "5" )) && echo yes || echo no'
check "empty is zero"     'e=; echo $(( e ))'
check "positional arith"  'set -- 10 20; echo $(( $1 + $2 ))'
check "nested arr index"  'a=(5 6 7); j=2; echo $(( a[j] + ${#a[@]} ))'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Run the harness**

Run: `cargo build --bin huck && bash tests/scripts/arith_dollar_diff_check.sh 2>&1 | tail -20`
Expected: every line PASS, `Fail: 0`. If any FAIL, the diff shows the divergence — investigate (bash is the oracle); do not edit expected output to match huck. If `a[j]` array-index-in-arith is itself a separate unsupported form, drop that one fragment and note it (it is not part of M-88's `$`-expansion) rather than masking a real bug.

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/arith_dollar_diff_check.sh
git commit -m "test: bash-diff harness for \$-forms in arithmetic (M-88)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: `declare` silent integration + harness fragments

**Files:** `tests/declare_silent_integration.rs` (NEW)

- [ ] **Step 1: Write the integration tests**

```rust
//! v93: declare -f/-F on a missing function is silent (rc 1, no output).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Returns (stdout, stderr, exit_code).
fn run(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn declare_f_missing_silent() {
    let (so, se, rc) = run("declare -f no_such_fn; echo rc=$?\n");
    assert_eq!(so, "rc=1\n");
    assert_eq!(se, "");
}

#[test]
fn declare_cap_f_missing_silent() {
    let (so, se, _) = run("declare -F no_such_fn; echo rc=$?\n");
    assert_eq!(so, "rc=1\n");
    assert_eq!(se, "");
}

#[test]
fn declare_cap_f_existing_prints() {
    // A defined function: -F prints its name (declare -f <name>), rc 0.
    let (so, _se, _) = run("f() { :; }\ndeclare -F f >/dev/null; echo rc=$?\n");
    assert_eq!(so, "rc=0\n");
}

#[test]
fn mise_style_probe_no_leak() {
    // The exact mise idiom: stderr is NOT redirected, only stdout.
    let (_so, se, _) = run("declare -F _mise_hook >/dev/null; echo done\n");
    assert_eq!(se, "", "missing-function probe must not leak to stderr");
}
```

- [ ] **Step 2: Run**

Run: `cargo test --test declare_silent_integration 2>&1 | tail -15`
Expected: all PASS (Task 2 already implemented the fix).

- [ ] **Step 3: Commit**

```bash
git add tests/declare_silent_integration.rs
git commit -m "test: declare -f/-F silent-on-missing integration (mise probe)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`

- [ ] **Step 1: Read the M-88 entry + change log + counts**

Run: `grep -n 'M-88\|M-89\|^## Change log\|2026-06-0\|Missing features (Tier 2)' docs/bash-divergences.md | head -20` and read the M-88 deferred entry (added in v92) plus the v92 change-log entry to match style.

- [ ] **Step 2: Flip M-88 to fixed + re-prioritize**

Change the M-88 entry from `[deferred]` (low) to `[fixed v93]` and note the re-prioritization (it was the dominant bash-completion blocker). Describe: `(( ))`/`$(( ))`/arith-`for` now expand `$`-forms (`$#`, `${…}`, `$(…)`, `$@`, `$1`, `arr[i]`) before evaluation via expand-then-parse (mirrors `eval_subscript`); arith bodies carried as `Word`; `eval_arith_word` helper; quote removal honored; empty expansion ⇒ 0.

- [ ] **Step 3: Add the M-90 deferred entry**

Add to the Missing-features (Tier 2) section:

```markdown
- **M-90: builtin error output ignores `2>` redirection** — `[deferred]`
  (high). Every huck builtin writes diagnostics via `eprintln!` to the
  process's real stderr rather than a redirectable error sink, so
  `cmd … 2>/dev/null` does not suppress builtin error messages
  (e.g. `declare -p UNSET 2>/dev/null` still prints). bash routes builtin
  stderr through the command's fd 2. Fixing this means threading an error sink
  through all builtins — a broad refactor. Surfaced while diagnosing
  interactive `source ~/.bashrc` (mise's `declare -p PROMPT_COMMAND
  2>/dev/null` leaked).
```

- [ ] **Step 4: Note the `declare -f/-F` fix**

In the M-66/declare entry (find via `grep -n 'declare' docs/bash-divergences.md`), add a sentence that `declare -f`/`-F` on a missing function is now silent (rc 1) like bash `[fixed v93]`.

- [ ] **Step 5: Change-log entry + counts**

Add a `2026-06-05` v93 change-log entry mirroring the v92 style (M-88 fixed: expand-then-parse arithmetic; `declare -f/-F` silent; M-90 logged deferred; new 19th harness `arith_dollar_diff_check.sh`; new integration files). Update the Tier-2 roster count (M-88 stays counted, M-90 adds one → bump by 1).

- [ ] **Step 6: README v93 row**

Add a v93 row after v92: "`$`-forms (`$#`/`${…}`/`$(…)`) inside `(( ))`/`$(( ))`/arith-`for` (M-88, expand-then-parse); `declare -f/-F` silent-on-missing".

- [ ] **Step 7: Verify + commit**

Run: `grep -n 'M-88\|M-90\|v93' docs/bash-divergences.md README.md` (confirm M-88 `[fixed v93]`, M-90 `[deferred]`, README row, no placeholders).
```bash
git add docs/bash-divergences.md README.md
git commit -m "docs: M-88 fixed v93 (arith \$-expansion); M-90 deferred; declare note; README

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

- **Spec coverage:** §1 architecture + §2 carriers + §3 eval → Task 1. §4 declare → Task 2. Testing §1-5 → Tasks 1/3/4. M-90 deferral + M-88 flip → Task 5. Covered.
- **Placeholder scan:** none — all code shown. The only "decide via compiler" instructions (Steps 9, 10) are inherent to a type-propagation refactor and name the exact sites from grep.
- **Type consistency:** `WordPart::Arith { body: Word, quoted }`, `Command::Arith(Word)`, `ArithForClause { init/cond/step: Option<Word> }`, `arith_string_to_word(&str) -> Result<Word, LexError>`, `eval_arith_word(&Word, &mut Shell) -> Result<i64, ArithError>` — names consistent across all tasks. `Token::ArithBlock(String)` intentionally unchanged (see refinement note).
- **Edge cases:** empty expansion ⇒ 0 (Step 6); quote removal via `expand_word_to_string` (Task 1 test `quote_removal_in_arith`); `$`-free arithmetic unchanged (no `$` → `arith_string_to_word` yields a single literal Word → same result); arith-for `;`-split happens on raw text before conversion.
