# v360 — EOF Delimiter Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make huck decide which delimiter an unexpected EOF names, and at which line, from the innermost still-open matched pair — the way bash does — instead of from whichever scanner happened to raise.

**Architecture:** One `PairStack` on the `Lexer` holding `(Delim, opening offset, line rule)`, pushed and popped with the modes and gated by a suppression table. It replaces `mode_open_offs`, `err_open_off`, `err_open_hint` and `Mode::Arith::quote_open_off`. The stack is read **only** for diagnostics, so a push/pop bug degrades a message and cannot change what the shell executes.

**Tech Stack:** Rust (workspace crates `huck-syntax`, `huck-engine`), bash 5.2.21 as the differential oracle, `tests/scripts/*_diff_check.sh` harnesses.

**Spec:** [`docs/superpowers/specs/2026-08-13-eof-delimiter-model-design.md`](2026-08-13-eof-delimiter-model-design.md) — read it before Task 1. **Issue:** [#635](https://github.com/jdstanhope/huck/issues/635).

## How to use this plan

This plan gives you **interfaces, tests, and gates** — not finished implementations. That is deliberate: v359's plan pasted complete function bodies, they were copied without scrutiny, and a `hash -:` panic shipped past a green clippy run, 2490 lib tests and a 275-harness sweep. Write the code yourself against the stated interface, and treat every gate as a thing you must *see* pass, not assume.

If a gate does not come out as this plan says it will, **stop and report it** rather than adjusting the gate. A surprising measurement is the most valuable output this work can produce; three of v360's own rules came from measurements that contradicted an earlier guess.

## Global Constraints

- **Oracle:** bash 5.2.21 (`bash --norc --noprofile`). Never assert bash's behaviour from memory — measure it.
- **Commit trailer** on every commit: `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`
- **Format before commit:** `cargo fmt --all` (CI enforces `--check`).
- **Lint with the pinned toolchain:** `cargo +1.97.1 clippy --workspace --all-targets --locked -- -D warnings`. A newer local stable misses warnings CI raises.
- **This box is 1 core / 1.9 GB.** Run tests per-crate, never `--workspace`; wrap in `ulimit -v`; use `--jobs 1`. The engine lib suite needs `-- --test-threads 4`.
- **Runs over ~1 minute must be detached** (`setsid nohup … &` + poll the log); a long command started as a normal background task gets killed at ~1 minute.
- **Every harness is run RED first**, against a binary built at the parent commit in a throwaway worktree (`git worktree add <tmp> <sha>`), before its green run is believed.
- **The pair stack is diagnostics-only.** No task may make execution depend on it.
- **Shape 2 must not move.** `if`, `while`, `case`, `{ }`, `( )` and function bodies keep `syntax error: unexpected end of file`, byte-identical.
- Branch: `v360-eof-delimiter-model`, cut from `main`. Do **not** merge it — a `vNN` PR is handed to the user.

## File Structure

| file | responsibility |
| --- | --- |
| `crates/huck-syntax/src/lexer/pairs.rs` *(new)* | `Pair`, `LineRule`, `PairStack`, and the `opens_pair` suppression table. The model, isolated and unit-testable without a lexer. |
| `crates/huck-syntax/src/lexer.rs` | owns a `PairStack`; pushes/pops it in `push_mode`/`pop_mode` and at the two frame-less pair sites; snapshots it in `mark`/`rewind`; exposes `error_pair()`. |
| `crates/huck-syntax/src/parser.rs` | `parse_param_expansion` single-exit conversion + drain hook; `unterminated_cmdsub`/`unterminated_backtick` read the reported pair; the `$((` re-read keeps its pair. |
| `crates/huck-syntax/src/command.rs` | `Delim::ArrayParen` (spells `)`, Shape 3). |
| `crates/huck-syntax/src/spell.rs` | spells the new variant. |
| `crates/huck-engine/src/error_emit.rs` | renders from the reported pair; `lex_is_shape3`'s variant→`Delim` mapping is deleted; the `$(`-uses-EOF-line special case becomes the pair's `LineRule`. |
| `tools/eof_matrix.sh` *(new)* | the measuring instrument: generates the 813 cells, compares bash vs huck, prints a TSV. |
| `tools/eof_matrix_baseline.tsv` *(new)* | today's divergent rows, committed so each task can show exactly which ones it fixed. |
| `tests/scripts/eof_delimiter_matrix_diff_check.sh` *(new, Task 9)* | the generated harness, once green. |
| `tests/scripts/eof_pair_lines_diff_check.sh` *(new, Task 9)* | hand-written rows for what the matrix cannot reach: multi-line line rules, `$((1+2)`, piped stdin, escaped quotes. |

---

### Task 1: Phase 0 — single-exit `parse_param_expansion`, proved inert

`parse_param_expansion` (`crates/huck-syntax/src/parser.rs:971-1583`) is 612 lines with **eleven exits** — ten `return Err` and one `return Ok` — and **21 `pop_mode` calls** among them, each manually re-paired with `restore_dq!()`, some popping two modes. Task 7's drain hook needs a single exit, and Task 3's pair pop wants one too. This task changes **no behaviour**.

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs:971-1583`

**Interfaces:**
- Consumes: nothing.
- Produces: `parse_param_expansion(iter: &mut Lexer, quoted: bool) -> Result<WordPart, ParseError>` — same signature, now shaped as `push_mode(…); let result = (|| { … })(); restore_dq!(); iter.pop_mode(); result`, mirroring `parse_arith_expansion` at `parser.rs:2255`. Inner modes (`ParamSubscriptOperand`, operand modes) are still pushed and popped inside the closure; only the `ParamExpansion` frame's pop moves to the single exit.

- [ ] **Step 1: Capture the pre-refactor baseline**

Build the current `main` binary into a worktree and record two references:

```bash
S=/tmp/claude-1000/-home-john-projects-huck/scratch     # or your scratch dir
git worktree add "$S/base" main && (cd "$S/base" && cargo build --locked --bin huck)
HUCK_BIN="$S/base/target/debug/huck" tools/parse_sweep.sh tools/scripts.tsv "$S/parse_base.tsv"
```

`tools/parse_sweep.sh` runs `bash -n` and `huck -n` over 4632 real scripts. It is parse-only, which is exactly the surface this refactor touches.

- [ ] **Step 2: Write the `${…}` execution corpus**

`parse_sweep` is `-n` only, so it cannot see a change in what an expansion *evaluates to*. Write `tools/param_corpus.sh` generating at least 200 `${…}` forms and running each through a given binary, printing `fragment<TAB>stdout<TAB>stderr<TAB>rc`. Cover, with and without an enclosing `"…"`: plain name, `${#x}`, `${!x}`, every operator (`:-` `:=` `:?` `:+` `#` `##` `%` `%%` `/` `//` `^` `^^` `,` `,,`), subscripts (`${a[0]}`, `${a[@]}`, `${a[*]}`), nested expansions in operands, `$'…'` in operands, and malformed forms (`${}`, `${1x}`, `${${x}`, `${x:-`) so the error paths are exercised too. Wrap each run in `ulimit -v 500000` and `timeout 5`.

- [ ] **Step 3: Record the corpus baseline**

```bash
tools/param_corpus.sh "$S/base/target/debug/huck" > "$S/corpus_base.tsv"
wc -l "$S/corpus_base.tsv"     # expect >= 200 rows
```

- [ ] **Step 4: Do the conversion**

Restructure to the closure shape. The mechanical hazards, all present in the current code: `restore_dq!()` must still run on every path (it moves to the single exit); the two-`pop_mode` sites are popping an *inner* mode plus `ParamExpansion` — only the latter moves out; and `?` inside the closure must not skip the inner pops. Do not change any condition, message, or returned value.

- [ ] **Step 5: Prove it inert — parse sweep**

```bash
cargo build --locked --bin huck
tools/parse_sweep.sh tools/scripts.tsv "$S/parse_new.tsv"
diff "$S/parse_base.tsv" "$S/parse_new.tsv" && echo "PARSE SWEEP IDENTICAL"
```

Expected: identical, zero lines of diff. Print the row count next to the word "identical" — a diff of two empty files also prints nothing, and that has produced a false pass in this repo before.

- [ ] **Step 6: Prove it inert — expansion corpus**

```bash
tools/param_corpus.sh target/debug/huck > "$S/corpus_new.tsv"
diff "$S/corpus_base.tsv" "$S/corpus_new.tsv" && echo "CORPUS IDENTICAL ($(wc -l < "$S/corpus_new.tsv") rows)"
```

Expected: identical.

- [ ] **Step 7: Prove it inert — zero expectation edits**

```bash
git diff --stat main -- 'tests/' 'crates/*/tests/' 'crates/*/src/**/tests.rs'
```

Expected: **empty**. A behaviour-preserving refactor that needed a test edited is not behaviour-preserving; if this is non-empty, stop and report.

- [ ] **Step 8: Full verification**

```bash
cargo fmt --all
cargo +1.97.1 clippy --workspace --all-targets --locked --jobs 1 -- -D warnings
(ulimit -v 6000000; cargo test -p huck-syntax --lib --locked --jobs 1)         # 485 pass
(ulimit -v 6000000; cargo test -p huck-engine --lib --locked --jobs 1 -- --test-threads 4)  # 2020 pass
(ulimit -v 8000000; cargo test -p huck --locked --jobs 1 -- --test-threads 4)  # 161 suites, 1317 tests
cargo build --release --locked --bin huck && tests/scripts/run_diff_checks.sh  # 303 passed, 0 failed
```

Run the last two detached; the sweep takes ~13 minutes.

- [ ] **Step 9: Commit**

```bash
git add crates/huck-syntax/src/parser.rs tools/param_corpus.sh
git commit -m "refactor(#635): parse_param_expansion gets a single exit"
```

The message must record the inertness evidence: parse sweep identical over N rows, corpus identical over M rows, zero expectation edits.

---

### Task 2: The measuring instrument

Every later task proves itself against this. It touches no production code.

**Files:**
- Create: `tools/eof_matrix.sh`, `tools/eof_matrix_baseline.tsv`

**Interfaces:**
- Produces: `tools/eof_matrix.sh [--tsv]` → one row per cell, tab-separated: `DEPTH CONTEXT MIDDLE INNER FRAGMENT BASH HUCK VERDICT`, where `BASH`/`HUCK` are `"<line> <delimiter>"` for a Shape 3 message or `"<line> ~<message prefix>"` otherwise, and `VERDICT` is `OK` or `DIFF`. Honours `HUCK_BIN`. Later tasks call it and diff against the baseline.

- [ ] **Step 1: Write the generator**

Two sweeps, both placing the fragment on **line 3 of a 4-line script** (`echo a`, `echo b`, fragment, `echo c`) so a line number that is right for the wrong reason still shows:

- depth 1 — 15 contexts × 11 openers = 165 cells. Contexts: `echo `, `echo "`, `echo '`, `echo $(`, ``echo ` ``, `echo ${x:-`, `echo $((1+`, `echo $[1+`, `((1+`, `for ((i=0;i<`, `[[ a == `, `echo ${a[`, `v=(`, `( `, `{ `. Openers: none, `"`, `'`, `` ` ``, `$(`, `${x`, `$((1+`, `$[1+`, `(`, `\"`, `\'`.
- depth 2 — 8 outers × 9 middles × 9 inners = 648 cells. Outers: none, `"`, `'`, `$((1+`, `$[1+`, `$(`, `` ` ``, `v=(`. Middles: none, `"`, `'`, `${x:-`, `${`, `$(`, `` ` ``, `$((1+`, `$[1+`. Inners: none, `"`, `'`, `` ` ``, `$(`, `${x`, `$((1+`, `\"`, `\'`.

Each cell runs `bash --norc --noprofile f.sh` and `$HUCK_BIN f.sh`, both under `ulimit -v 500000` and `timeout 5`. Extract the line number and, when the message is Shape 3, the delimiter between `` matching ` `` and `` ' ``; otherwise the first 22 characters of the message after `line N: `.

- [ ] **Step 2: Verify the instrument against known answers**

Run it and check three cells whose answers this plan states, so an extraction bug cannot silently classify everything as OK:

```bash
tools/eof_matrix.sh | awk -F'\t' '$5=="echo $((1+${x" || $5=="echo $((1+\"" || $5=="v=("'
```

Expected: `echo $((1+${x` → bash `3 )`, huck `3 }`, DIFF. `echo $((1+"` → `3 "` both, OK. `v=(` → bash `3 )`, huck `5 ~syntax error: unterminated`, DIFF.

- [ ] **Step 3: Record the baseline**

```bash
tools/eof_matrix.sh --tsv > tools/eof_matrix_baseline.tsv
awk -F'\t' 'NR>1{c[$8]++} END{for(k in c) print k, c[k]}' tools/eof_matrix_baseline.tsv
```

Expected: `OK 735`, `DIFF 78` (16 divergent of 165 at depth 1, 62 of 648 at depth 2).

If the counts differ from this plan, **stop and report** — either the instrument or this plan's measurement is wrong, and the difference must be understood before any production code changes.

- [ ] **Step 4: Commit**

```bash
git add tools/eof_matrix.sh tools/eof_matrix_baseline.tsv
git commit -m "tools(#635): the EOF-delimiter matrix and today's baseline"
```

---

### Task 3: `PairStack` mirroring the mode stack — inert

Introduce the structure and make it the source of the *offset*, with no rule changes. After this task the matrix baseline must be **unchanged**.

**Files:**
- Create: `crates/huck-syntax/src/lexer/pairs.rs`
- Modify: `crates/huck-syntax/src/lexer.rs` (module decl; the `Lexer` field; `push_mode`/`pop_mode`; `mark`/`rewind`; `scan_step_guarded`; `error_open_start`)

**Interfaces:**
- Consumes: `Delim` from `crate::command`.
- Produces:

```rust
pub(crate) enum LineRule { Open, Eof }

pub(crate) struct Pair {
    pub delim: Delim,
    pub open_off: usize,
    pub line: LineRule,
}

pub(crate) struct PairStack { /* Vec<Pair> */ }

impl PairStack {
    pub(crate) fn push(&mut self, delim: Delim, open_off: usize, line: LineRule);
    pub(crate) fn pop(&mut self, expect: Delim) -> Option<Pair>;  // debug_assert on mismatch
    pub(crate) fn top(&self) -> Option<&Pair>;
    pub(crate) fn depth(&self) -> usize;
}
```

`Lexer::error_pair(&self) -> Option<Pair>` replaces `error_open_start()`; keep `error_open_start()` as a thin wrapper returning `self.error_pair().map(|p| p.open_off)` so this task changes no call sites.

- [ ] **Step 1: Write the failing unit test**

In `pairs.rs`, a test that the stack reports its top and that `pop` is order-checked:

```rust
#[test]
fn stack_reports_the_innermost_pair() {
    let mut s = PairStack::default();
    s.push(Delim::DQuote, 4, LineRule::Open);
    s.push(Delim::DollarParen, 9, LineRule::Eof);
    let top = s.top().expect("a pair is open");
    assert_eq!(top.delim, Delim::DollarParen);
    assert_eq!(top.open_off, 9);
    assert!(matches!(top.line, LineRule::Eof));
    s.pop(Delim::DollarParen);
    assert_eq!(s.top().map(|p| p.delim), Some(Delim::DQuote));
}
```

- [ ] **Step 2: Run it and watch it fail**

`cargo test -p huck-syntax --lib --locked --jobs 1 stack_reports` → FAIL, module does not exist.

- [ ] **Step 3: Implement `pairs.rs` and wire it into the lexer**

Map each pair-bearing `Mode` to its `Delim` and `LineRule` (`$(` → `Eof`; everything else → `Open`), push in `push_mode`, pop in `pop_mode`, and delete `mode_open_offs` entirely — `mark`/`rewind` snapshot the pair stack in its place. `scan_step_guarded` records the top on error. Modes with no pair (`Command`, operand modes, `Regex`, `Extglob`) push nothing.

- [ ] **Step 4: Run the unit tests**

`cargo test -p huck-syntax --lib --locked --jobs 1` → 486+ pass.

- [ ] **Step 5: Prove inertness against the baseline**

```bash
cargo build --locked --bin huck
tools/eof_matrix.sh --tsv > /tmp/m.tsv
diff tools/eof_matrix_baseline.tsv /tmp/m.tsv && echo "MATRIX UNCHANGED (813 cells)"
```

Expected: identical. This task is a refactor; a changed cell means the mode→pair mapping is wrong somewhere, and you should find out which cell and why before continuing.

- [ ] **Step 6: Full sweep + suites, then commit**

Sweep 303/303, all suites green, clippy clean.

```bash
git add crates/huck-syntax/src/lexer/pairs.rs crates/huck-syntax/src/lexer.rs
git commit -m "refactor(#635): a pair stack replaces mode_open_offs"
```

---

### Task 4: The stack becomes the authority — fixes #631

Switch the *delimiter* and *line* source from the error variant to the stack top, and push the two pairs that have no mode of their own.

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (arith quote spans; `${…}` operand `'` spans; delete `err_open_hint` and `Mode::Arith::quote_open_off`)
- Modify: `crates/huck-engine/src/error_emit.rs` (`lex_is_shape3`, `emit_matching`)
- Modify: `crates/huck-engine/src/builtins.rs:8800` (the other `error_open_start` consumer)

**Interfaces:**
- Consumes: `PairStack`, `Pair`, `LineRule` from Task 3.
- Produces: `emit_matching` takes a `Pair` and reads `pair.line` instead of matching on `Delim::DollarParen`. `lex_is_shape3` no longer maps variants to delimiters; it answers only *is this error an open-delimiter EOF*.

- [ ] **Step 1: Write the failing harness rows**

Add to `tests/scripts/eof_pair_lines_diff_check.sh` (create it here; it grows again in Task 9) rows for #631, verified against bash 5.2.21 first:

```
echo "${x:-'          ->  line 3: … matching `''
echo "${'             ->  line 3: … matching `''
echo "a${x:-'         ->  line 3: … matching `''
echo $((1+${x:-'      ->  line 3: … matching `''
```

Plus controls that must not move: `echo ${x:-'` (unquoted, already `'`), `echo "${x#'` (already `'`), `echo "abc'` (a `'` inside `"` is literal → names `"`).

- [ ] **Step 2: Run it red**

Against the Task 3 binary: the four #631 rows FAIL (huck says `}`), the controls PASS.

- [ ] **Step 3: Implement**

Push a quote pair when an arith body opens a quote span (replacing `Mode::Arith::quote_open_off`), and when a `${…}` operand opens a `'` span in a quoted context. Point the renderer at `error_pair()`. Delete `err_open_hint`.

- [ ] **Step 4: Run the harness green, and the matrix**

```bash
tools/eof_matrix.sh --tsv > /tmp/m.tsv
diff <(awk -F'\t' '$8=="DIFF"' tools/eof_matrix_baseline.tsv) <(awk -F'\t' '$8=="DIFF"' /tmp/m.tsv)
```

Expected: the 6 `'`-in-`${}` rows leave the DIFF set; **nothing joins it**. A new DIFF row is a regression — investigate before continuing.

- [ ] **Step 5: Existing harnesses must stay green**

`arith_eof_quote_diff_check.sh` (41 rows) and `unterminated_eof_diff_check.sh` are the regression net for #621's quote-span behaviour, which this task re-implements on the stack. Both must pass unchanged.

- [ ] **Step 6: Full verification and commit**

```bash
git commit -m "fix(#631): the pair stack decides which delimiter an EOF names"
```

---

### Task 5: The suppression table — fixes #627

**Files:**
- Modify: `crates/huck-syntax/src/lexer/pairs.rs` (add `opens_pair`)
- Modify: `crates/huck-syntax/src/lexer.rs` (gate the pushes; delete `span_opener_off`)

**Interfaces:**
- Produces: `pub(crate) fn opens_pair(enclosing: Option<Delim>, opener: Delim, escaped: bool) -> bool` — the spec's rules 1–5. `escaped` is true when the opener is preceded by an odd run of backslashes.

- [ ] **Step 1: Write the failing unit tests**

The table is testable without a lexer, which is the payoff for isolating it:

```rust
#[test]
fn arith_swallows_brace_and_legacy_but_not_nested_arith() {
    // Measured: `$[1+$((2+` names the inner `)`, `$((1+$[2+` names the OUTER `)`.
    assert!(!opens_pair(Some(Delim::DollarDParen), Delim::DollarBrace, false));
    assert!(!opens_pair(Some(Delim::DollarDParen), Delim::DollarBracket, false));
    assert!(opens_pair(Some(Delim::DollarDParen), Delim::DollarDParen, false));
    assert!(opens_pair(Some(Delim::DollarBracket), Delim::DollarDParen, false));
}

#[test]
fn single_quote_is_literal_in_dquote_but_a_pair_in_a_brace_operand() {
    assert!(!opens_pair(Some(Delim::DQuote), Delim::SQuote, false));
    assert!(opens_pair(Some(Delim::DollarBrace), Delim::SQuote, false));
}

#[test]
fn nothing_opens_inside_a_single_quote_and_an_escaped_quote_never_opens() {
    assert!(!opens_pair(Some(Delim::SQuote), Delim::DQuote, false));
    assert!(!opens_pair(Some(Delim::SQuote), Delim::DollarParen, false));
    assert!(!opens_pair(None, Delim::DQuote, true));
    assert!(!opens_pair(Some(Delim::DollarDParen), Delim::SQuote, true));
}
```

- [ ] **Step 2: Run them and watch them fail** — `opens_pair` does not exist.

- [ ] **Step 3: Implement the table and gate the pushes**

`span_opener_off` (the #621-round guard that declined to record an escaped quote) is deleted; its job is now rule 5 in the table.

- [ ] **Step 4: Matrix gate**

Expected: the 37 `${`/`$[`-inside-arith rows leave the DIFF set, plus the 3 `$[`-inside-`$((` rows. Nothing joins it. `arith_eof_quote_diff_check.sh`'s escaped-quote rows must still pass — they are what pins rule 5.

- [ ] **Step 5: Full verification and commit** — `git commit -m "fix(#627): an arith body opens no pair for \${ or \$["`

---

### Task 6: The array literal is a pair — fixes #633

Also the one construct whose message **shape** and **exit status** change.

**Files:**
- Modify: `crates/huck-syntax/src/command.rs` (add `Delim::ArrayParen`), `crates/huck-syntax/src/spell.rs` (spells `)`), `crates/huck-engine/src/error_emit.rs` (`is_matching_delim` must return **true** for it — unlike `Delim::Paren`, which is Shape 2), `crates/huck-syntax/src/lexer.rs` (`Mode::ArrayLiteral` pushes it)

**Interfaces:**
- Consumes: `opens_pair` from Task 5.
- Produces: `Delim::ArrayParen`, Shape 3, spells `)`, `LineRule::Open`.

- [ ] **Step 1: Write the failing harness rows**

In `eof_pair_lines_diff_check.sh`, measured against bash first — note the **exit status** column, which differs:

```
v=(                 ->  line 3: … matching `)'    rc 1   (huck today: line 5, own message, rc 2)
v=(a b              ->  line 3: … matching `)'    rc 1
v=(""               ->  line 3: … matching `)'    rc 1
v=([0]=x            ->  line 3: … matching `)'    rc 1
declare -a v=(a     ->  line 3: … matching `)'    rc 1
```

Plus a multi-line row: `v=(a` on line 2 of a 4-line file → bash `line 2`.

- [ ] **Step 2: Run red** — all rows fail, on both message and status.

- [ ] **Step 3: Implement**

`LexError::UnterminatedArrayLiteral` keeps its identity for `is_unterminated_lex` (the REPL still asks for continuation), but the diagnostic now comes from the pair. The status change needs the error's fatality classification checked against `docs/architecture.md`'s error-model notes: bash exits **1** here where huck exits 2. If making the status match turns out to need more than a classification change, **stop and report** — the message half is the model's job and the status half may deserve its own issue.

- [ ] **Step 4: Matrix gate** — the 7 array-literal rows leave the DIFF set; nothing joins.

- [ ] **Step 5: Full verification and commit** — `git commit -m "fix(#633): an unterminated array literal names \`)\` at its opening line"`

---

### Task 7: EOF beats validation — fixes #634

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` (`parse_param_expansion`'s single exit from Task 1)
- Modify: `crates/huck-syntax/src/lexer.rs` (the drain entry point)

**Interfaces:**
- Consumes: the single exit (Task 1), `opens_pair` (Task 5).
- Produces: `Lexer::drain_to_pair_close(&mut self) -> Result<(), LexError>` — a character skip driven by the same suppression table, consuming until the pair that was open at entry closes, and returning the lex error if input runs out first.

- [ ] **Step 1: Write the failing harness rows** (measured first)

```
echo ${$(       ->  line 5: … matching `)'     (huck today: line 3, `}')
echo ${$((1+    ->  line 3: … matching `)'     (huck today: line 3, `}')
echo ${${x      ->  line 3: … matching `}'     (huck today: syntax error: unsupported expansion)
echo ${${x}     ->  line 3: … matching `}'     (huck today: syntax error: unsupported expansion)
```

Controls that already agree and must not move: `` echo ${`cmd `` → `` ` ``; `echo ${!` and `echo ${1x` → `}`.

- [ ] **Step 2: Run red.**

- [ ] **Step 3: Implement**

At the single exit, when the result is a *validation* error (not already a lex EOF), drain to the pair's close; if the drain runs out of input, the lex error wins. The drain must itself open pairs — that is what makes `${$(` report `)` at the EOF line rather than `}`.

- [ ] **Step 4: Guard against over-reach**

A validation error where input does **not** run out must still surface unchanged. Add rows for `echo ${${x}}; echo after` and `echo ${1x}; echo after` — both must keep today's message and still run the following command.

- [ ] **Step 5: Matrix gate** — the 12 `${`-name-position rows leave the DIFF set; nothing joins.

- [ ] **Step 6: Full verification and commit** — `git commit -m "fix(#634): an EOF inside \${ reports the pair, not the validation"`

---

### Task 8: The `$((` re-read keeps its pair — fixes #629

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` (`parse_arith_expansion`'s `ArithBail` arm, ~`parser.rs:2286`)

**Interfaces:**
- Consumes: `PairStack` (Task 3).
- Produces: no new API — the rewind re-pushes an arith pair carrying the `$((`'s offset and `LineRule::Open` before re-driving as a command substitution.

- [ ] **Step 1: Write the failing harness rows** (measured first)

```
echo a / echo $((1+2) / echo c / echo d    ->  bash line 2: … matching `)'   (huck today: line 5)
echo a / echo $((1+2)                      ->  bash line 2                    (huck today: line 3)
```

Controls that already agree: `echo $( (1+2)` and `echo $(cmd` both report the EOF line; `echo $(( (1+2)` reports the `$((` line.

- [ ] **Step 2: Run red.**

- [ ] **Step 3: Implement.** The `mark`/`rewind` restores the pair stack to its pre-`$((` state (Task 3), so the arith entry must be re-pushed deliberately — with a comment saying bash never re-reads, which is why its answer keeps the arith's line.

- [ ] **Step 4: Verify and commit** — `git commit -m "fix(#629): a re-read \$(( keeps its opening line"`

---

### Task 9: Wire the matrix into the sweep

**Files:**
- Create: `tests/scripts/eof_delimiter_matrix_diff_check.sh`
- Modify: `tests/scripts/eof_pair_lines_diff_check.sh` (final rows), `tools/eof_matrix_baseline.tsv` (now all-OK)

- [ ] **Step 1: Confirm every in-scope cell is green**

Of the 78 cells divergent at the baseline, **72 are in v360's scope and 6 are not**. The six are out of scope by the spec, not by convenience, and every one is a message shape that is neither Shape 2 nor a matched pair:

| cell | why excluded |
| --- | --- |
| `[[ a == ` × {none, `(`, `\"`, `\'`} | `[[ … ]]` conditional-expression wording — bash says `unexpected argument …` / `syntax error in conditional expression` |
| `echo (` | huck reads it as a function definition; bash gives a near-token error |
| `v=(` + `(` | bash gives a near-token error, not a matched-pair one |

```bash
tools/eof_matrix.sh --tsv | awk -F'\t' 'NR>1{c[$8]++} END{for(k in c) print k, c[k]}'
```

Expected: `OK 807`, `DIFF 6`, and the six are exactly the cells above. If a *seventh* cell diverges, it belongs to a family this plan did not scope — **stop and report it** rather than adding it to the skip list.

- [ ] **Step 2: Write the harness**

It sources `tests/scripts/lib/harness.sh` for `HUCK_BIN`, the executable guard, `compare` and `harness_summary`, and generates the same cells minus a **skip list of exactly those six**, each carrying an inline comment saying which shape it is and why it is not a matched pair. Do not skip a cell any other way — a silent filter is how a harness comes to be green for the wrong reason.

The header comment must state what the model is, that the cells are generated rather than listed, and what is deliberately not covered, with issue links: #624's semantic half, #628, the `[[ … ]]` wording, `echo (`, and `v[` (#75's family).

- [ ] **Step 3: Add the line-rule and driver rows to `eof_pair_lines_diff_check.sh`**

The matrix is all single-line, so the line rules are invisible in it. Add, each measured first: a pair opening on an earlier line than the EOF for every pair type; the piped-stdin driver (normalising only the program name, as `unterminated_eof_diff_check.sh` does); and the escaped-quote rows pinning #624's diagnostic half.

- [ ] **Step 4: Run both red at the parent commit**

Build `main` in a worktree and run both harnesses against it. Expected: the matrix harness fails exactly **72** of its 807 rows (the in-scope divergences; the other six never reach it), and `eof_pair_lines` fails its model rows while passing its controls. A harness that is green against the pre-fix binary is not testing the fix.

- [ ] **Step 5: Refresh the baseline and commit**

```bash
tools/eof_matrix.sh --tsv > tools/eof_matrix_baseline.tsv    # 807 OK, the 6 excluded cells still DIFF
git add tests/scripts/eof_delimiter_matrix_diff_check.sh tests/scripts/eof_pair_lines_diff_check.sh tools/eof_matrix_baseline.tsv
git commit -m "test(#635): the generated EOF-delimiter matrix joins the sweep"
```

- [ ] **Step 6: Full sweep**

Expected: **305 passed, 0 failed** (303 + the two new harnesses), and the sweep about 70 s longer.

---

### Task 10: Shape 2 proof, docs, and hand-off

**Files:**
- Modify: `docs/architecture.md` (the pair model in the cross-cutting conventions section), `docs/superpowers/plans/2026-08-13-eof-delimiter-model.md` (check the boxes)
- Create: `site/content/blog/<slug>.mdx`

- [ ] **Step 1: Prove Shape 2 did not move**

Run every Shape 2 construct (`if true`, `while true; do :`, `case x in`, `{ echo hi`, `( echo hi`, `f() {`) through the parent-commit binary and the branch binary and diff. Expected: byte-identical, including exit status. This is the design's boundary condition and deserves its own recorded evidence.

- [ ] **Step 2: Update `docs/architecture.md`**

A short subsection: the two EOF shapes, the pair stack as the single authority for Shape 3, the suppression table's rules, and the fact that the stack is diagnostics-only. Note the four mechanisms it replaced so nobody reintroduces one.

- [ ] **Step 3: Issue bookkeeping**

Comment on [#624](https://github.com/jdstanhope/huck/issues/624) recording that its diagnostic half is now covered by the model and only the semantic half remains open — the handling #606 got. Leave #628 open.

- [ ] **Step 4: Write the blog entry**

Audience: people who use shells. Lead with the symptom — an error message that named the wrong thing and sent you to the wrong line. Every `# before` / `# after` pair must be **real output**, the before side from a binary built at `main` in a throwaway worktree. Validate with velite:

```bash
cd site && export NVM_DIR="$HOME/.nvm" && . "$NVM_DIR/nvm.sh" && nvm use node >/dev/null \
  && ( ulimit -v 12000000; node_modules/.bin/velite --strict )
```

`--strict` is mandatory; plain `velite` prints schema issues and still exits 0.

- [ ] **Step 5: Record the iteration in memory**

`project_huck_iterations.md` + a one-line `MEMORY.md` hook, in the established style: what shipped, and the ⚠️ lessons — especially any measurement that contradicted this plan.

- [ ] **Step 6: Open the PR and hand it over**

```bash
git push -u origin v360-eof-delimiter-model
gh pr create --base main --title "v360: one model for which delimiter an EOF names" --body "…Closes #635, #627, #629, #631, #633, #634"
```

**Do not merge it.** A `vNN` iteration PR is handed to the user to review and merge. Wait for the GitHub run to finish and pass before calling it ready — local green is not CI green.

---

## Self-Review

**Spec coverage.** Pair inventory → Tasks 3, 6. Suppression rules 1–5 → Task 5 (rules 1–3, 5) and Task 4 (rule 4). Rule 6, EOF-beats-validation → Task 7. `$(`'s line rule → Task 3 (`LineRule::Eof`). #629 → Task 8. #633's exit status → Task 6 Step 3. Parser's two hardcoded Shape 3 sites → Task 4 Step 3. Replacing all four ad-hoc mechanisms → Tasks 3 (`mode_open_offs`), 4 (`err_open_hint`, `quote_open_off`), 5 (`span_opener_off`). Phase 0 → Task 1. Verification plan → Tasks 2, 9, 10. Every spec section maps to a task.

**Placeholders.** None: every step names its files, its command, and its expected result.

**Type consistency.** `Pair`/`LineRule`/`PairStack`/`opens_pair`/`error_pair`/`drain_to_pair_close` are used with the same names and signatures in Tasks 3–8. `Delim::ArrayParen` is introduced in Task 6 and used only there.

**Known gap, deliberate.** Task 6 Step 3 may discover that the array literal's exit-status change needs more than a fatality reclassification. The step says to stop and report rather than push through, because that half may deserve its own issue.
