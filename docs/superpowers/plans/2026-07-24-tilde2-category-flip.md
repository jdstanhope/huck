# tilde2 Category Flip Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Flip the `tilde2` bash-suite category to PASS by fixing two lexer-side tilde-recognition bugs (Summary PASS 23→24, FAIL 59→58).

**Architecture:** huck's lexer only *recognizes* whether a `~` is a tilde-prefix (emits `TokenKind::Tilde`) vs a literal; the expander does the HOME substitution later. Both fixes adjust recognition only, in `crates/huck-syntax/src/lexer.rs`. Root A tightens assignment-value tilde eligibility; Root B recognizes a word-start `~` inside an unquoted value operand. The broader "move recognition into the expander" refactor is out of scope (issue #295).

**Tech Stack:** Rust (crates `huck-syntax` lexer, `huck-engine`); bash-diff harnesses under `tests/scripts/`; the official bash test-suite runner.

Issue: [#294](https://github.com/jdstanhope/huck/issues/294). Spec: `docs/superpowers/specs/2026-07-24-tilde2-category-flip-design.md`.

## Global Constraints

- Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- `cargo fmt --all` before every commit (CI enforces `--check`).
- Build the binary with `cargo build -p huck` (debug) — the harness uses `target/debug/huck`.
- NEVER `cargo test --workspace` (OOM-kills this 1-core/1.9GB box). Per-crate, single-threaded: `cargo test -p <crate> --jobs 1 -- --test-threads 1`.
- Guard the bash-suite runner / diff sweeps with `ulimit -v 2000000` + `timeout`.
- Run the `-p huck` integration binaries single-threaded before any push (a `--lib`-only run once passed locally but failed CI).
- Do NOT copy GPL bash test text into the repo; harnesses author their own fragments.
- Bash source for the runner: `BASH_SOURCE_DIR=/tmp/bash-5.2.21`. Baseline scratch for regression comparison: `/tmp/huck-bash-tests-20260724T171608Z.S6lc5T` (commit 8999d79 baseline; `tilde2` diff was 25 lines).
- Work on branch `v335-tilde2-category-flip` off `main`. Do NOT merge to main or push to main.

---

### Task 1: Root A — assignment-value tilde eligibility re-enables on `:` only

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs:5265` (in `scan_command_word_atom`)
- Test: `tests/scripts/tilde_diff_check.sh` (extend)

**Interfaces:**
- Consumes: nothing new.
- Produces: no API change. Behavior: in an assignment value, a `~` after an embedded `=` is now literal; after an unquoted `:` still a tilde-prefix.

**Background:** At `lexer.rs:5265`, the per-char eligibility update inside the assignment-value literal run is `boundary = self.in_assignment_value && matches!(ch, '=' | ':')`. This re-enables tilde eligibility after *any* `=` in the value, so `h=HOME=~` and `ADDPATH=PATH=~/bin` wrongly expand the second-segment `~`. bash only continues tilde eligibility after an unquoted `:` (the assigning `=`'s word-start eligibility is already seeded by `begin_assignment_value`). Fix: drop `=`.

- [ ] **Step 1: Write failing harness cases**

Append to `tests/scripts/tilde_diff_check.sh` (before the final summary/exit), in the existing `checkf "label" 'body'` style:

```bash
# ---- Root A (#294): tilde after an EMBEDDED `=` in an assignment value is
# literal (only the assigning `=` word-start and unquoted `:` are tilde-prefixes).
checkf "assign inner-eq literal"   'HOME=/h; h=HOME=~; echo "$h"'          # HOME=~
checkf "assign path inner-eq"      'HOME=/h; X=/b:/u; A=PATH=~/bin:$X; echo "$A"'  # PATH=~/bin:/b:/u
checkf "export inner-eq literal"   'HOME=/h; export h=HOME=~; echo "$h"'   # HOME=~
checkf "assign after-colon expands" 'HOME=/h; foo=a:~; echo "$foo"'       # a:/h  (regression guard)
checkf "assign both tildes ~:~"    'HOME=/h; foo=~:~; echo "$foo"'         # /h:/h (regression guard)
checkf "assign inner-eq then colon" 'HOME=/h; foo=x=~:~; echo "$foo"'      # x=~:/h
# The posix / non-posix eval tail from tilde2.tests (guards the Root A cascade:
# once `h=HOME=~` keeps the literal tilde, the eval chain matches bash).
checkf "eval tail cascade" $'HOME=/h\nh=HOME=~\nset -o posix\neval echo $h\nset +o posix\neval echo $h'  # HOME=~ \n HOME=/h
```

- [ ] **Step 2: Run harness to verify the new cases fail**

Run: `cargo build -p huck && HUCK_BIN=$(pwd)/target/debug/huck bash tests/scripts/tilde_diff_check.sh`
Expected: the pre-existing cases PASS; the new "assign inner-eq …" cases FAIL (huck expands where bash keeps literal). "after-colon"/"~:~" already PASS.

- [ ] **Step 3: Apply the Root A fix**

In `crates/huck-syntax/src/lexer.rs`, the line (~5265):

```rust
boundary = self.in_assignment_value && matches!(ch, '=' | ':');
```

becomes:

```rust
// bash tilde-expands the prefix after the ASSIGNING `=` (word start, seeded by
// begin_assignment_value) and after each unquoted `:`, but NOT after a second,
// embedded `=` (`h=HOME=~` → literal). Only `:` re-enables eligibility. (#294)
boundary = self.in_assignment_value && ch == ':';
```

- [ ] **Step 4: Run harness to verify pass**

Run: `cargo fmt --all && cargo build -p huck && HUCK_BIN=$(pwd)/target/debug/huck bash tests/scripts/tilde_diff_check.sh`
Expected: all cases PASS (including the new Root A cases and the colon regression guards).

- [ ] **Step 5: Run the lexer lib tests**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1`
Expected: PASS (no tilde/assignment lexer unit test regressions).

- [ ] **Step 6: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs tests/scripts/tilde_diff_check.sh
git commit -m "v335: tilde after embedded = in assignment value is literal (#294 Root A)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Root B — word-start `~` in an unquoted value operand `${x:-~}`/`${x:=~}`

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (`scan_step_param_operand`, unquoted `~` handling)
- Test: `tests/scripts/tilde_diff_check.sh` (extend)

**Interfaces:**
- Consumes: `TokenKind::ParamOp(ParamOpKind::{UseDefault,AssignDefault,ErrorIfUnset,UseAlternate})` as the preceding history token (the word-start signal); the `is_pattern`/`in_dquote`/`enclosing_dquote` params already passed to `scan_step_param_operand`; `try_parse_tilde(&mut self.cursor, false)`.
- Produces: `TokenKind::Tilde { spec, assign_ctx: false }` for a word-start `~` in an unquoted value operand. The parser already converts `TokenKind::Tilde` → `WordPart::Tilde` (parser.rs:359), and `expand_word_to_string` already resolves it via `expand_assignment` — no parser or engine change.

**Background:** `${x:-~}` unquoted yields literal `~` in huck (bash: expands the leading tilde). The default word is already expanded through the tilde-resolving `expand_assignment`; the only gap is that `scan_step_param_operand` emits a literal `~` instead of a `TokenKind::Tilde`. bash expands ONLY the word-start tilde (not one after `:`), ONLY for value operands (not patterns), and ONLY when unquoted. Word-start is detected by a backward history read: at operand-word-start the most recently emitted token is the value `ParamOp` (no operand atom emitted yet). This is a backward look at already-emitted tokens — not a forward scan and not a parser dependency.

- [ ] **Step 1: Write failing harness cases**

Append to `tests/scripts/tilde_diff_check.sh`:

```bash
# ---- Root B (#294): word-start tilde in an UNQUOTED value operand expands;
# quoted or pattern operands stay literal; a tilde after `:` in the operand is literal.
checkf "op use-default unq"     'HOME=/h; unset t; echo ${t:-~}'          # /h
checkf "op use-default noco"    'HOME=/h; unset t; echo ${t-~}'           # /h
checkf "op assign-default unq"  'HOME=/h; unset t; echo ${t:=~}; echo "$t"'  # /h \n /h
checkf "op use-alt unq"         'HOME=/h; t=x; echo ${t:+~}'              # /h
checkf "op default quoted lit"  'HOME=/h; unset t; echo "${t:-~}"'        # ~
checkf "op default inq lit"     'HOME=/h; unset t; echo "${t:-"~"}"'      # ~
checkf "op default midcolon"    'HOME=/h; unset t; echo ${t:=~/a:~/b}; echo "$t"'  # /h/a:~/b (x2)
checkf "op default not-start"   'HOME=/h; t=x; echo ${t:-$t~}'            # x~  (tilde not word-start)
checkf "op removeprefix lit"    'HOME=/h; t=~; echo ${t#~}'               # (pattern operand: ~ literal)
checkf "op removesuffix lit"    'HOME=/h; t=a~; echo ${t%~}'              # a   (pattern operand)
```

- [ ] **Step 2: Run harness to verify the new cases fail**

Run: `cargo build -p huck && HUCK_BIN=$(pwd)/target/debug/huck bash tests/scripts/tilde_diff_check.sh`
Expected: the unquoted value-operand cases FAIL (huck emits `~`); the quoted/pattern/mid-colon/not-start cases already PASS (huck keeps `~` literal there, matching bash).

- [ ] **Step 3: Add a lexer unit test (recognition)**

In the `scan_step_param_operand` tests module (near the existing `operand_*` tests, e.g. `operand_deferred_cmdsub`), add:

```rust
#[test]
fn operand_word_start_tilde_unquoted_value() {
    // `${x:-~}` value operand, unquoted: a word-start `~` is a Tilde token.
    let a = operand_atoms(
        "~}",
        Mode::ParamWordOperand { in_dquote: false, enclosing_dquote: false, is_pattern: false },
    );
    assert!(matches!(a[0], TokenKind::Tilde { .. }),
        "word-start ~ in an unquoted value operand must be a Tilde token, got {:?}", a[0]);
}
```

Note: `operand_atoms` seeds the operand mode directly (no preceding `ParamOp` in `history`). If the chosen word-start signal is the backward `ParamOp` history read, adjust the test helper to push a `ParamOp` token first, OR gate the unit test to assert via the harness instead. Prefer making `operand_atoms` prepend a `TokenKind::ParamOp(ParamOpKind::UseDefault(true))` to `history` so the unit test exercises the real signal.

- [ ] **Step 4: Run the unit test to verify it fails**

Run: `cargo test -p huck-syntax --jobs 1 --lib operand_word_start_tilde -- --test-threads 1`
Expected: FAIL (a[0] is a `Lit`, not `Tilde`).

- [ ] **Step 5: Implement Root B in `scan_step_param_operand`**

In the UNQUOTED branch of `scan_step_param_operand` (the `else`/non-`in_dquote` path), BEFORE the unquoted literal-run arm that would swallow `~`, add an arm:

```rust
// Root B (#294): a WORD-START `~` in an UNQUOTED VALUE operand is a tilde-prefix
// (`${x:-~}` → HOME). Gates: unquoted (`!in_dquote && !enclosing_dquote`); value
// operand (`!is_pattern` — patterns `#`/`%`/`/` are not tilde-expanded); word
// start — detected by a BACKWARD read of the last emitted token being the value
// `ParamOp` (no operand atom emitted yet). bash expands only the leading tilde,
// not one after `:`, so this fires solely at operand start.
Some('~')
    if !in_dquote
        && !enclosing_dquote
        && !is_pattern
        && matches!(
            self.history.last().map(|t| &t.kind),
            Some(TokenKind::ParamOp(
                ParamOpKind::UseDefault(_)
                    | ParamOpKind::AssignDefault(_)
                    | ParamOpKind::ErrorIfUnset(_)
                    | ParamOpKind::UseAlternate(_)
            )),
        ) =>
{
    self.cursor.next(); // consume `~`
    match try_parse_tilde(&mut self.cursor, false) {
        Some(spec) => self.history.push(Token::new(
            TokenKind::Tilde { spec, assign_ctx: false },
            Span::new(off, l, c),
        )),
        None => self.history.push(Token::new(
            TokenKind::Lit { text: "~".into(), quoted: false },
            Span::new(off, l, c),
        )),
    }
    return Ok(Step::Produced);
}
```

Place this arm inside the same `match self.cursor.peek()` that handles the unquoted operand chars, ahead of the general literal-run arm. Import `ParamOpKind` / `try_parse_tilde` if not already in scope (they live in the same module).

`try_parse_tilde`'s second arg is the assignment-value flag (whether `:` terminates a `~user` name). Pass `false` (normal-word semantics): all tested cases use a bare `~`/`~/path`, which terminate at `/` regardless, so `false` is correct and conservative; `~user:` inside an operand is untested and out of scope (cf. #72).

- [ ] **Step 6: Run the unit test + harness to verify pass**

Run: `cargo fmt --all && cargo build -p huck && cargo test -p huck-syntax --jobs 1 --lib operand_word_start_tilde -- --test-threads 1 && HUCK_BIN=$(pwd)/target/debug/huck bash tests/scripts/tilde_diff_check.sh`
Expected: unit test PASS; every harness case PASS (Root B cases now expand, quoted/pattern/mid-colon/not-start stay literal).

- [ ] **Step 7: Run the syntax lib test suite (no regressions)**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1`
Expected: PASS (~441 tests; no operand/param-expansion regressions).

- [ ] **Step 8: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs tests/scripts/tilde_diff_check.sh
git commit -m "v335: word-start tilde in an unquoted value operand \${x:-~} (#294 Root B)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Category flip + full regression sweep + docs/memory

**Files:**
- Modify: `docs/bash-test-suite-baseline.md` (if it records per-category status)
- Modify: memory `project_huck_iterations.md` + `MEMORY.md` (outside the repo tree — update in the memory dir)

**Interfaces:** none (verification + docs only).

- [ ] **Step 1: Build both binaries**

Run: `cargo build --locked --bin huck && cargo build --release --locked --bin huck`
Expected: both succeed.

- [ ] **Step 2: Confirm `tilde2` flips to PASS at 0 diff**

Run:
```bash
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
ulimit -v 2000000
HUCK_BASH_TEST_CATEGORY=tilde2 timeout 120 bash tests/bash-test-suite/runner.sh 2>/dev/null | grep -i tilde2
```
Expected: `| tilde2 | PASS |`. If still FAIL, read the new scratch dir's `tilde2.diff` and reconcile (left = bash expected `.right`, right = huck `.out`).

- [ ] **Step 3: Full official runner — flip confirmed, no regressions**

Run:
```bash
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
ulimit -v 2000000
timeout 900 bash tests/bash-test-suite/runner.sh > /tmp/v335-after.md 2>/dev/null
grep -E "PASS:|FAIL:|TIMEOUT:" /tmp/v335-after.md
```
Expected: `PASS: 24`, `FAIL: 58`, `TIMEOUT: 0`. Note the new scratch dir printed at the top of `/tmp/v335-after.md`.

- [ ] **Step 4: Per-category diff-line regression check vs baseline**

Compare per-category diff-LINE counts against the saved baseline (the PASS table alone hides within-category regressions — binding rule from prior iterations):
```bash
OLD=/tmp/huck-bash-tests-20260724T171608Z.S6lc5T
NEW=$(grep -oE '/tmp/huck-bash-tests-[^ ]+' /tmp/v335-after.md | head -1)
for f in "$NEW"/*.diff; do c=$(basename "$f" .diff);
  o=$( [ -f "$OLD/$c.diff" ] && wc -l < "$OLD/$c.diff" || echo 0 ); n=$(wc -l < "$f");
  [ "$o" != "$n" ] && echo "$c: $o -> $n"; done
```
Expected: only `tilde2` changes (25 → 0). No other category's diff-line count increases. Watch especially `array`, `assoc`, `new-exp`, `exp-tests`, `quote`, `posixexp`. If any increased, STOP and investigate before proceeding.

- [ ] **Step 5: Integration binaries single-threaded (pre-push CI parity)**

Run each tilde/assignment/expansion-adjacent `-p huck` integration binary single-threaded, e.g.:
```bash
for t in $(ls crates/huck-engine/tests/*.rs tests/*.rs 2>/dev/null | xargs -n1 basename | sed 's/\.rs$//' | sort -u); do
  echo "== $t =="; ulimit -v 2000000; cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 2>&1 | tail -3; done
```
Expected: all green. (At minimum run any tilde/param-expansion/assignment-named bins plus the full set huck's CI runs.)

- [ ] **Step 6: Full bash-diff sweep**

Run: `ulimit -v 2000000; timeout 600 bash tests/scripts/run_diff_checks.sh 2>&1 | tail -20`
Expected: green (including the extended `tilde_diff_check.sh`).

- [ ] **Step 7: Update baseline doc + memory**

- If `docs/bash-test-suite-baseline.md` records per-category status/counts, update `tilde2` → PASS and Summary PASS 23→24 / FAIL 59→58 (note "Updated by v335").
- Update memory `project_huck_iterations.md` (new v335 entry, full detail) and `MEMORY.md` (one-line hook): tilde2 flip, the two roots, and the durable lesson (diff-direction pitfall; downstream-cascade verification; the #295 refactor deferral).

- [ ] **Step 8: Commit**

```bash
git add docs/bash-test-suite-baseline.md 2>/dev/null; git add -A
git commit -m "v335: flip tilde2 to PASS (23->24); baseline + docs (#294)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final review + PR (after Task 3)

- Whole-branch diff review: both edits are recognition-only; confirm no forward-scan, no lexer→parser dependency, no engine change.
- Push `v335-tilde2-category-flip`; open a PR targeting `main` with body `Closes #294` (leave #295 open). Wait for CI to finish and pass before handing off to the user (local green ≠ CI green on this 1-core box). Do NOT merge — the user merges.
