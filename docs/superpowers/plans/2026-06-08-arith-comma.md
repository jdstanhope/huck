# v112 — arithmetic comma operator Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the comma (`,`) operator to shell arithmetic so `(( a=1, b=2 ))`, `$((1,2,3))`, `(1,2)+3`, and C-style `for ((i=0,j=0; …; i++,j++))` work — fixing the `mise ` + `<TAB>` `((: unexpected character: ','` error (and its downstream `_upvars` cascade).

**Architecture:** A thin `parse_comma_expr` wrapper above the existing Pratt parser (`src/arith.rs`) — no binding-power renumbering. Comma is the lowest-precedence operator (`L , R` → eval L for side effects, return R). All arithmetic funnels through `arith::parse`, so wiring `parse_comma_expr` into `parse` + the parenthesized-group prefix covers `(( ))`, `$(( ))`, C-style `for` clauses, and parenthesized commas.

**Tech Stack:** Rust, single file `src/arith.rs`. Tests: `cargo test --bin huck`, `cargo test --test arith_comma_integration`, `bash tests/scripts/arith_comma_diff_check.sh`.

**Spec:** `docs/superpowers/specs/2026-06-08-arith-comma-design.md`. Read it first.

**Commit trailer (MANDATORY, canonical — every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

Anchors (verify exact lines — code shifts):
- `enum ArithToken` (`src/arith.rs`, has `LParen, RParen, … Question, Colon, …`).
- lexer single-char arms (`'(' => { chars.next(); out.push(ArithToken::LParen); }` ~`:171`).
- `pub enum ArithExpr` (`~:320`); `Ternary(Box, Box, Box)` variant present.
- `pub fn parse(input)` (`~:384`) → `let expr = p.parse_expr(0)?;` (`~:387`).
- `parse_prefix` `Some(ArithToken::LParen) => { let inner = self.parse_expr(0)?; … }` (`~:564-565`).
- `pub fn eval(expr, shell)` match (`~:602`).
- test module helper `fn eval_str(s, shell)` + `Shell::new()` (`~:1191`).

**Verified bash contract:** `(( a=1, b=2 ))`→`a=1 b=2`; `$((1,2,3))`→`3`; `$(( (1,2)+3 ))`→`5`; `for ((i=0,j=0;i<3;i++,j++))`→`0:0/1:1/2:2`; `$(( a=1, 2 ))`→`2` with `a==1` (comma below assignment); `$(( 1, ))` and `$(( ,1 ))` → bash syntax error (operand expected); `$(( (1,2),3 ))`→`3`.

---

## Task 1: comma token, AST, parser wrapper, eval + unit tests

**Files:**
- Modify: `src/arith.rs` (token, lexer, AST, `parse_comma_expr`, wire into `parse` + `LParen`, eval arm, unit tests)

- [ ] **Step 1: Write the failing unit tests**

In `src/arith.rs`, inside the existing `#[cfg(test)] mod tests { … }` (which has `eval_str` + uses `Shell::new()`), add:
```rust
    #[test]
    fn comma_value_is_last_operand() {
        let mut s = Shell::new();
        assert_eq!(eval_str("1, 2, 3", &mut s).unwrap(), 3);
    }

    #[test]
    fn comma_keeps_side_effects_of_all_operands() {
        let mut s = Shell::new();
        // a=1 then b=2; value is the last (2); both vars set.
        assert_eq!(eval_str("a=1, b=2", &mut s).unwrap(), 2);
        assert_eq!(s.lookup_var("a").as_deref(), Some("1"));
        assert_eq!(s.lookup_var("b").as_deref(), Some("2"));
    }

    #[test]
    fn comma_is_lower_precedence_than_assignment() {
        // `a = 1, 2` is `(a=1), 2`: value 2, a==1 (NOT a=(1,2)=2).
        let mut s = Shell::new();
        assert_eq!(eval_str("a = 1, 2", &mut s).unwrap(), 2);
        assert_eq!(s.lookup_var("a").as_deref(), Some("1"));
    }

    #[test]
    fn comma_inside_parens() {
        let mut s = Shell::new();
        assert_eq!(eval_str("(1, 2) + 3", &mut s).unwrap(), 5);
    }

    #[test]
    fn comma_side_effect_ordering() {
        // i=0 then i++ : value of i++ is 0, i becomes 1.
        let mut s = Shell::new();
        assert_eq!(eval_str("i=0, i++", &mut s).unwrap(), 0);
        assert_eq!(s.lookup_var("i").as_deref(), Some("1"));
    }

    #[test]
    fn comma_nested_left_fold() {
        let mut s = Shell::new();
        assert_eq!(eval_str("(1,2),3", &mut s).unwrap(), 3);
    }

    #[test]
    fn trailing_comma_is_error() {
        let mut s = Shell::new();
        assert!(eval_str("1,", &mut s).is_err());
    }

    #[test]
    fn leading_comma_is_error() {
        let mut s = Shell::new();
        assert!(eval_str(",1", &mut s).is_err());
    }
```

- [ ] **Step 2: Run the unit tests — confirm they fail**

Run: `cargo test --bin huck arith 2>&1 | tail -20`
Expected: the new `comma_*`/`*_comma_*` tests FAIL — `eval_str("1, 2, 3", …)` errors `unexpected character: ','` (no comma support yet).

- [ ] **Step 3: Add the `Comma` token + lex `,`**

In `enum ArithToken` (`src/arith.rs`), add a `Comma` variant (e.g. on the `Question, Colon,` line or its own):
```rust
    Question, Colon, Comma,
```
In the lexer, next to the `'('`/`')'` arms (~`:171`), add:
```rust
            ',' => { chars.next(); out.push(ArithToken::Comma); }
```

- [ ] **Step 4: Add the `Comma` AST node**

In `pub enum ArithExpr` (`~:320`), add (e.g. right after the `Ternary(...)` variant):
```rust
    /// `L , R` — evaluate L (for side effects), then R; value is R. Lowest
    /// precedence. (M-108)
    Comma(Box<ArithExpr>, Box<ArithExpr>),
```

- [ ] **Step 5: Add `parse_comma_expr` and wire it into the two entry points**

In `src/arith.rs`, add the wrapper method on the parser (near `parse_expr`):
```rust
    /// Parse a comma-separated sequence of full expressions (each
    /// `parse_expr(0)`); left-associative, value is the last. Comma is the
    /// lowest-precedence arithmetic operator, so it lives ABOVE the Pratt loop
    /// — this keeps every existing binding power untouched and makes
    /// `a = 1, 2` parse as `(a=1), 2`. (M-108)
    fn parse_comma_expr(&mut self) -> Result<ArithExpr, ArithError> {
        let mut lhs = self.parse_expr(0)?;
        while self.peek() == Some(&ArithToken::Comma) {
            self.bump();
            let rhs = self.parse_expr(0)?;
            lhs = ArithExpr::Comma(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }
```
Then change the two `parse_expr(0)` call sites that should accept a comma-list:
- In `pub fn parse` (`~:387`): `let expr = p.parse_expr(0)?;` → `let expr = p.parse_comma_expr()?;`
- In `parse_prefix`'s `LParen` arm (`~:565`): `let inner = self.parse_expr(0)?;` → `let inner = self.parse_comma_expr()?;`

(Do NOT change the ternary branches' `parse_expr(0)` — comma in a ternary middle is the documented out-of-scope edge.)

- [ ] **Step 6: Add the eval arm**

In `pub fn eval` (`~:602`), add a match arm (e.g. near the `Ternary` arm):
```rust
        ArithExpr::Comma(l, r) => {
            eval(l, shell)?; // evaluate L for its side effects; discard value
            eval(r, shell)   // value of a comma sequence is the last operand
        }
```

- [ ] **Step 7: Run the unit tests — confirm they pass**

Run: `cargo test --bin huck arith 2>&1 | tail -20`
Expected: all `comma_*` tests pass, and the pre-existing arith tests still pass.

- [ ] **Step 8: Verify byte-identical to bash**

```bash
cargo build --bin huck
for f in '(( a=1, b=2 )); echo "$a $b"' \
         'echo $((1,2,3))' \
         'echo $(( (1,2)+3 ))' \
         'a=9; echo $(( a=1, 2 )); echo "$a"' \
         'echo $(( (1,2),3 ))'; do
  b=$(printf '%s\n' "$f" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
  h=$(printf '%s\n' "$f" | ./target/debug/huck 2>&1; echo "EXIT:$?")
  [ "$b" = "$h" ] && echo "MATCH: $f" || { echo "DIFF: $f"; echo " b=[$b]"; echo " h=[$h]"; }
done
```
Expected: five `MATCH` lines.

- [ ] **Step 9: Regression + clippy**

Run: `cargo test --bin huck 2>&1 | tail -3 && cargo clippy --bin huck 2>&1 | tail -3`
Expected: unit suite green (incl. existing `arith` tests); clippy clean (no new warnings — a new enum variant matched in `eval` shouldn't trigger non-exhaustive warnings; if `ArithExpr` is matched anywhere else exhaustively, add the `Comma` arm there too — grep `match .*ArithExpr` / `ArithExpr::` to be sure).

- [ ] **Step 10: Commit**

```bash
git add src/arith.rs
git commit -m "$(cat <<'EOF'
feat: arithmetic comma operator (M-108)

`L , R` evaluates L (for side effects), then R, yielding R — the lowest-
precedence arithmetic operator. Added as a parse_comma_expr wrapper above the
Pratt parser (no binding-power renumbering), wired into arith::parse and the
parenthesized-group prefix, so `(( a=1,b=2 ))`, `$((1,2,3))`, `(1,2)+3`, and
C-style `for ((i=0,j=0; ...; i++,j++))` all work. `a=1,2` correctly parses as
`(a=1),2`. Fixes bash_completion's __reassemble_comp_words_by_ref (mise<TAB>).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1 report
DONE/BLOCKED, commit SHA, the token/AST/parser/eval changes, the 8 unit-test pass line, the five bash MATCH lines, unit-suite + clippy status (note any other `ArithExpr` match site you had to extend).

---

## Task 2: integration tests + 36th harness + payoff smoke

**Files:**
- Create: `tests/arith_comma_integration.rs`
- Create: `tests/scripts/arith_comma_diff_check.sh`

- [ ] **Step 1: Write the integration tests**

Create `tests/arith_comma_integration.rs` (the `run` helper returns `(stdout, stderr, exit_code)`):
```rust
//! v112: arithmetic comma operator integration tests (M-108).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
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
fn double_paren_comma_sets_both() {
    let (out, _e, _c) = run("(( a=1, b=2 ))\necho \"$a $b\"\n");
    assert_eq!(out, "1 2\n", "out: {out}");
}

#[test]
fn dollar_arith_comma_value_is_last() {
    let (out, _e, _c) = run("echo $((1, 2, 3))\n");
    assert_eq!(out, "3\n", "out: {out}");
}

#[test]
fn comma_inside_parens_in_dollar_arith() {
    let (out, _e, _c) = run("echo $(( (1,2) + 3 ))\n");
    assert_eq!(out, "5\n", "out: {out}");
}

#[test]
fn comma_below_assignment() {
    let (out, _e, _c) = run("a=9\necho $(( a=1, 2 ))\necho \"$a\"\n");
    assert_eq!(out, "2\n1\n", "out: {out}");
}

#[test]
fn c_style_for_comma_in_init_and_update() {
    let (out, _e, _c) = run("for ((i=0,j=0; i<3; i++,j++)); do echo \"$i:$j\"; done\n");
    assert_eq!(out, "0:0\n1:1\n2:2\n", "out: {out}");
}

#[test]
fn reassemble_comp_words_shape() {
    // The bash_completion __reassemble loop shape that started this: a C-style
    // for with comma over a COMP_WORDS-like array must run without the
    // `((: unexpected character: ','` error.
    let (out, err, _c) = run(
        "COMP_WORDS=(mise \"\")\n\
         for ((i=0,j=0; i<${#COMP_WORDS[@]}; i++,j++)); do echo \"w$i=${COMP_WORDS[i]}\"; done\n\
         echo done\n");
    assert!(out.contains("done"), "loop did not complete: {out} / {err}");
    assert!(!err.contains("unexpected character"), "comma error leaked: {err}");
}
```
Verify each `assert_eq!` against the system bash first.

- [ ] **Step 2: Run the integration tests — confirm green**

Run: `cargo build --bin huck && cargo test --test arith_comma_integration 2>&1 | tail -10`
Expected: all 6 tests PASS (Task 1 already implemented the feature).

- [ ] **Step 3: Write the 36th bash-diff harness**

Create `tests/scripts/arith_comma_diff_check.sh`, modeled on `tests/scripts/getopts_diff_check.sh` (same `set -u`, `HUCK_BIN`, `check()` combined-output pattern):
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v112: the arithmetic comma operator
# (M-108). `L , R` -> eval L (side effects) then R; value is R; lowest precedence.
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

check "(( a=1,b=2 ))"        '(( a=1, b=2 )); echo "$a $b"'
check "value is last"        'echo $((1, 2, 3))'
check "comma in parens"      'echo $(( (1,2) + 3 ))'
check "comma below assign"   'a=9; echo $(( a=1, 2 )); echo "$a"'
check "nested comma"         'echo $(( (1,2),3 ))'
check "c-for comma"          'for ((i=0,j=0; i<3; i++,j++)); do echo "$i:$j"; done'
check "comma side effects"   'echo $(( x=5, x+1 )); echo "$x"'
check "spaces around comma"  'echo $(( 1 , 2 , 3 ))'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 4: Make it executable and run it + all harnesses**

Run:
```bash
chmod +x tests/scripts/arith_comma_diff_check.sh && cargo build --bin huck
bash tests/scripts/arith_comma_diff_check.sh
export HUCK_BIN="$(pwd)/target/debug/huck"
echo "count: $(ls tests/scripts/*_diff_check.sh | wc -l)"
for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done
echo done
```
Expected: `Total: 8, Pass: 8, Fail: 0`; `count: 36`; no `FAIL` lines.

- [ ] **Step 5: Payoff smoke**

Run:
```bash
cargo build --bin huck
echo "=== the comma error is gone ==="
./target/debug/huck -c 'for ((i=0,j=0; i<3; i++,j++)); do :; done; echo SMOKE_OK' 2>&1
echo "=== mise<TAB> reassemble shape ==="
printf '%s\n' 'COMP_WORDS=(mise ""); for ((i=0,j=0; i<${#COMP_WORDS[@]}; i++,j++)); do :; done; echo REASSEMBLE_OK' | ./target/debug/huck 2>&1
```
Expected: `SMOKE_OK` and `REASSEMBLE_OK`, with NO `((: unexpected character: ','`.

- [ ] **Step 6: Commit**

```bash
git add tests/arith_comma_integration.rs tests/scripts/arith_comma_diff_check.sh
git commit -m "$(cat <<'EOF'
test: 36th bash-diff harness + integration for arith comma (M-108)

8 byte-identical fragments ((( )), $(( )), nested parens, comma-below-assign,
C-style for, side effects, spaces) + 6 integration tests incl. the
bash_completion __reassemble_comp_words_by_ref shape (mise<TAB>).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2 report
DONE/BLOCKED, commit SHA, the 6 integration-test pass line, the `Total: 8, Pass: 8` line, the `count: 36` + no-FAIL line, and the payoff-smoke output (`SMOKE_OK` / `REASSEMBLE_OK`).

---

## Task 3: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Read the structures to update**

```bash
grep -n 'Last updated:\|Missing features (Tier 2) |\|^## Change log\|2026-06-08.*v111' docs/bash-divergences.md | head
grep -n '| v111 ' README.md
```
Confirm next free Tier-2 number is **M-108**.

- [ ] **Step 2: Add the M-108 entry**

In `docs/bash-divergences.md` Tier-2 (Missing features) section (e.g. after the M-107 entry added in v111), add an **M-108** entry `[fixed v112]`: the arithmetic comma operator `,` — `L , R` evaluates L (side effects kept) then R, value R; lowest precedence (below assignment, so `a=1,2` is `(a=1),2`); works in `(( ))`, `$(( ))`, parenthesized sub-expressions, and every C-style `for` clause. Mechanism: a `parse_comma_expr` wrapper above the Pratt parser (`src/arith.rs`) + `ArithToken::Comma` + `ArithExpr::Comma` + an eval arm; wired into `arith::parse` and the parenthesized-group prefix. Driver: bash_completion's `__reassemble_comp_words_by_ref` (`for ((i=0,j=0; …; i++,j++))`, reached by `mise<TAB>`) — fixed `((: unexpected character: ','` and the downstream `_upvars` `invalid option` cascade (with the for-loop aborting, `words`/`cword` were unpopulated). Out of scope (deferred sub-divergence): comma inside a ternary middle branch (`1 ? 2,3 : 4`). 8 unit + 6 integration tests + the 36th harness.

- [ ] **Step 3: Bump the Tier-2 count + summary note**

In the Summary table **Missing features (Tier 2)** row: increment the count by 1 (M-108) and append to the note: `; M-108 arithmetic comma operator fixed by v112`. Update the **Last updated** line to mention v112 (the arithmetic comma operator).

- [ ] **Step 4: Change-log entry + README row**

`docs/bash-divergences.md` change log (after the v111 entry): a `2026-06-08` v112 entry — the comma operator (the `parse_comma_expr`-wrapper mechanism, the two entry points), the `mise<TAB>` payoff (the `((: …` error + `_upvars` cascade cleared), the verified semantics (value = last, side effects of all, comma below assignment), the deferred ternary-middle edge, the 36th harness + the test count from Task 2's full-suite run. Add a v112 README iteration row after v111 in the same compact style. Use the REAL test count: `cargo test 2>&1 | awk '/test result:/{s+=$4} END{print s}'`.

- [ ] **Step 5: Verify (no placeholders) + commit**

```bash
grep -n 'M-108\|fixed v112\|v112' docs/bash-divergences.md README.md | head
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: v112 — arithmetic comma operator (M-108)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 3 report
DONE/BLOCKED, commit SHA, the grep output proving real M-number/version, the test count used.

---

## Final (after all tasks)
- [ ] Whole-branch review: `git log --oneline main..HEAD`, `git diff --stat main..HEAD`.
- [ ] `cargo test 2>&1 | grep -cE 'test result: ok'` (green, no FAILED), `cargo clippy --all-targets 2>&1 | tail -2` (clean).
- [ ] All harnesses: `export HUCK_BIN="$(pwd)/target/debug/huck"; for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done` (silent = pass; 36 files).
- [ ] **Payoff**: `for ((i=0,j=0; i<3; i++,j++))` runs; the `__reassemble_comp_words_by_ref` shape completes with no comma error (Task 2 Step 5).
- [ ] AskUserQuestion merge gate, then `git merge --no-ff` + push + delete branch, then update memory files (`project_huck_iterations.md` + `MEMORY.md`).
