# v362 — EOF Diagnostic Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make an unexpected-EOF diagnostic name the innermost still-open matched pair, and report it at that pair's line, the way bash does.

**Architecture:** `reported_pair()` walks the frame stack v361 already built — every frame carries the offset its construct opened at — applying a measured suppression table. The error variant keeps deciding the message *shape*; the pair decides the *delimiter*. No new storage.

**Tech Stack:** Rust (`huck-syntax`, `huck-engine`), bash 5.2.21 as the differential oracle.

**Spec:** [`docs/superpowers/specs/2026-08-16-eof-diagnostic-model-design.md`](../specs/2026-08-16-eof-diagnostic-model-design.md) — read it before Task 0, and v360's spec beside it for the measured table. **Issue:** [#643](https://github.com/jdstanhope/huck/issues/643).

## How to use this plan

This plan gives you **interfaces, gates and evidence to collect** — not finished implementations. That is deliberate: v359's plan pasted complete function bodies, they were copied without scrutiny, and a `hash -:` panic shipped past a green clippy run, 2490 lib tests and a 275-harness sweep.

If a gate does not come out as this plan says it will, **stop and report it** rather than adjusting the gate. That rule has already earned its place three times: it caught a panic introduced two tasks earlier, and it refuted two "surely this is derivable" simplifications that would have silently broken the pattern-operand family.

## Global Constraints

- **This work stays on `v362-eof-diagnostic-model`. Do NOT merge to `main` without the user's explicit approval.** A `vNN` PR is handed over, not merged.
- **Oracle:** bash 5.2.21 (`bash --norc --noprofile`). Never assert bash's behaviour from memory — measure it.
- **Commit trailer:** `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`
- **Format before commit:** `cargo fmt --all`. **Lint with the pinned toolchain:** `cargo +1.97.1 clippy --workspace --all-targets --locked -- -D warnings`, and check its EXIT STATUS — a trailing `echo CLIPPY_CLEAN` prints either way and has already hidden a real error once.
- **This box is 1 core / 1.9 GB.** Per-crate tests, never `--workspace`; wrap in `ulimit -v`; `--jobs 1`; the engine lib suite needs `-- --test-threads 4`. Runs over ~1 minute must be detached (`setsid nohup … &` + poll).
- **Nothing may JOIN the divergent set.** `tools/eof_matrix.sh --check` runs on every task; rows leaving is the point, rows joining is a regression and a stop.
- **Shape 2 must not move.** `if`, `while`, `case`, `{ }`, `( )` and function bodies keep `syntax error: unexpected end of file`.

## The gates

Referenced by number throughout.

1. **Matrix** — `tools/eof_matrix.sh --check`: the rows this task set out to fix have LEFT the divergent set, and none joined.
2. **Corpus** — `tools/param_corpus.sh <binary>`, 250 `${…}` forms. Byte-identical for the inert tasks; for behavioural tasks, every changed row is explained in the commit.
3. **Parse sweep** — `tools/parse_sweep.sh`, 3103 real scripts. Identical for the inert tasks; `HUCK_CRASH 0` always.
4. **Suites and sweep** — both `--lib` suites, every `-p huck` integration binary, full `tests/scripts/run_diff_checks.sh`, pinned clippy.
5. **Shape 2 controls** — the six grammar constructs, byte-identical including exit status.

## File Structure

| file | responsibility |
| --- | --- |
| `crates/huck-syntax/src/lexer/pairs.rs` *(new)* | `reported_pair()` and the suppression table — the model, unit-testable without running a scanner. |
| `crates/huck-syntax/src/lexer.rs` | records the reported pair at raise time; loses `err_open_hint`. |
| `crates/huck-syntax/src/parser.rs` | single-exit `parse_param_expansion` + the drain; the `$((` re-read marker. |
| `crates/huck-syntax/src/command.rs` | `Delim::ArrayParen`. |
| `crates/huck-syntax/src/spell.rs` | spells it `)`. |
| `crates/huck-engine/src/error_emit.rs` | `lex_is_shape3` keeps the shape split, stops naming delimiters; `render_syntax_diag` takes the delimiter. |
| `tests/scripts/eof_delimiter_matrix_diff_check.sh` *(new, Task 7)* | the generated matrix, once green. |
| `tests/scripts/eof_pair_lines_diff_check.sh` *(new, Task 7)* | what 813 single-line cells cannot see. |

---

### Task 0: Baselines

**Files:** none — measurement only.

- [ ] **Step 1: Build a branch-point binary**

```bash
S=<scratch>; git worktree add "$S/base" a8a7169b && (cd "$S/base" && cargo build --locked --bin huck)
```

- [ ] **Step 2: Record the three references**

```bash
tools/param_corpus.sh "$S/base/target/debug/huck" > "$S/corpus_base.tsv"     # expect 250 rows
HUCK_BIN="$S/base/target/debug/huck" tools/parse_sweep.sh tools/scripts.tsv "$S/parse_base.tsv"
tools/eof_matrix.sh --check
```

Expect the parse sweep to report 3103 scripts, AGREE_OK 3092, AGREE_FAIL 11, no crashes; and the matrix 813 cells, 78 DIFF, 0 FIXED, 0 REGRESSED. Anything else means the baseline is wrong and every later gate inherits the error — stop.

- [ ] **Step 3: Record which coordinates belong to which issue**

```bash
sed -n '/^EXPECTED_DIFF=/,/^COORDS$/p' tools/eof_matrix.sh | grep -E "^[12]/" > "$S/coords.txt"
```

Later tasks quote the coordinates they expect to remove. Attribution is approximate where a cell could be owned two ways (an array literal *containing* a `${` in arith); say which reading you used.

---

### Task 1: Single-exit `parse_param_expansion` — inert

The drain in Task 6 needs one exit. This function has eleven and changes **no behaviour**.

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` (`parse_param_expansion`)

**Interfaces:**
- Produces: `parse_param_expansion(iter: &mut Lexer, quoted: bool) -> Result<WordPart, ParseError>` — unchanged signature, restructured as `push_mode(…); let result = (|| { … })(); iter.set_in_dquote(saved_dq); iter.pop_mode(); result`, mirroring `parse_arith_expansion`.

- [ ] **Step 1: Restructure to one exit**

Inner modes (operand, subscript) are still pushed and popped inside the closure; only the `ParamExpansion` pop moves out.

**Both traps from the v360 attempt, which cost a panic and two tasks to find:**

- Two arms — substitute-pattern and substring-offset — pop with a bare `iter.pop_mode(); iter.pop_mode();` and NO `restore_dq!()`, so a search for the `restore_dq` pattern misses them. Leaving their second pop in place after the frame pop moves takes out the `Command` floor: `echo ${x:1` panics with *"Command is the floor and must never be popped"*.
- A whitespace-ignored diff review will NOT show this. Those two sites are textually unchanged; the bug is their interaction with the new exit.

- [ ] **Step 2: Gate 2 — the corpus is the sharp one**

It carries one unterminated row per operand mode precisely because that class slipped through before. `echo ${x:` and `echo ${x:1` are the rows that catch this.

- [ ] **Step 3: Gates 3, 4** — parse sweep identical, suites and sweep green.

- [ ] **Step 4: Prove zero expectation edits** — `git diff --stat a8a7169b -- '*tests.rs' 'tests/'` empty.

- [ ] **Step 5: Commit** — `git commit -m "refactor(#643): parse_param_expansion gets a single exit"`, recording that the corpus and parse sweep are byte-identical.

---

### Task 2: `reported_pair()` — the walk, inert

Introduce the model returning **today's answers**, so the switch of authority is provable before any rule changes it.

**Files:**
- Create: `crates/huck-syntax/src/lexer/pairs.rs`
- Modify: `crates/huck-syntax/src/lexer.rs`, `crates/huck-engine/src/error_emit.rs`, and the four `render_syntax_diag` callers (`shell.rs:555`, `builtins.rs:8600`, `:8742`, `:8806`)

**Interfaces:**
- Produces:

```rust
// pairs.rs
pub(crate) fn reported_pair(frames: &[ModeFrame]) -> Option<(Delim, usize)>;
```

Returns the delimiter bash would name and the offset to report it at, or `None` when no frame is a pair (then the failing atom reports itself). In THIS task it applies no suppression: the innermost pair-bearing frame wins, except that an `Arith` frame with a live quote span reports the quote, reading the `in_squote`/`in_dquote`/`quote_open_off` the frame already carries.

- `Lexer::error_delim(&self) -> Option<Delim>` — recorded at raise time beside `err_open_off`.
- `render_syntax_diag(shell, err, source, token_line, delim: Option<Delim>)` — one added parameter.

- [ ] **Step 1: Write the failing unit tests**

Test the walk on constructed frame slices — no scanner, no rendering:

```rust
#[test]
fn the_innermost_pair_bearing_frame_wins() {
    let frames = [frame(Mode::Command), frame(Mode::DoubleQuote), frame(Mode::CommandSub)];
    assert_eq!(reported_pair(&frames).map(|(d, _)| d), Some(Delim::DollarParen));
}

#[test]
fn an_arith_frame_with_a_live_quote_span_reports_the_quote() {
    // The span has no frame of its own; the arith frame carries it.
    let frames = [frame(Mode::Command), arith_with_dquote_open_at(7)];
    assert_eq!(reported_pair(&frames), Some((Delim::DQuote, 7)));
}

#[test]
fn no_pair_bearing_frame_means_the_atom_reports_itself() {
    let frames = [frame(Mode::Command)];
    assert_eq!(reported_pair(&frames), None);
}
```

- [ ] **Step 2: Run them and watch them fail** — the module does not exist.

- [ ] **Step 3: Implement, and switch the authority**

`lex_is_shape3` keeps deciding Shape 2 vs Shape 3 and stops returning a `Delim`; the delimiter comes from `error_delim()`, threaded through the four callers. Delete `err_open_hint` — the arith quote span is now read off the frame. Keep `span_opener_off`: Task 3's rule 4 depends on it.

- [ ] **Step 4: Gate 1 — this task must be INERT**

`--check` reports **0 FIXED, 0 REGRESSED**. A row moving here means the walk does not reproduce today's answers, and the difference must be understood before Task 3 builds on it.

- [ ] **Step 5: Gates 2, 3, 4, 5.** Then commit.

---

### Task 3: The suppression table — closes #627

**Files:**
- Modify: `crates/huck-syntax/src/lexer/pairs.rs`

**Interfaces:**
- Consumes: `reported_pair` from Task 2.
- Produces: `fn opens_pair(enclosing: Option<Delim>, opener: Delim, escaped: bool) -> bool` — the table the walk consults.

- [ ] **Step 1: Write the failing unit tests, from the measurements**

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
fn a_single_quote_is_opaque_and_an_escaped_quote_opens_nothing() {
    assert!(!opens_pair(Some(Delim::SQuote), Delim::DollarParen, false));
    assert!(!opens_pair(Some(Delim::DQuote), Delim::SQuote, false));
    assert!(!opens_pair(None, Delim::DQuote, true));
}
```

- [ ] **Step 2: Run them and watch them fail.**

- [ ] **Step 3: Implement and wire into the walk.**

- [ ] **Step 4: Gate 1 — expect ~30 coordinates to LEAVE**

They are the `arith`/`legacy`/`arithcmd`/`forhdr` contexts with a `brace` or `legacy` inner. Quote the list from Task 0's `coords.txt`. **Nothing may join.**

- [ ] **Step 5: Gates 2, 3, 4, 5.** The corpus may legitimately change here — any changed row must be a `${…}` inside arithmetic, and the commit says which.

- [ ] **Step 6: Commit** — `fix(#627): an arith body opens no pair for ${ or $[`

---

### Task 4: The array literal — closes #633

Two halves. Do the message and line first, prove them, then the status.

**Files:**
- Modify: `crates/huck-syntax/src/command.rs`, `crates/huck-syntax/src/spell.rs`, `crates/huck-engine/src/error_emit.rs`, `crates/huck-syntax/src/lexer/pairs.rs`

**Interfaces:**
- Produces: `Delim::ArrayParen` — spells `)`, and `is_matching_delim` returns **true** for it, unlike `Delim::Paren` which is deliberately Shape 2 for subshells.

- [ ] **Step 1: Write the failing harness rows**

Measured against bash first. `v=(`, `v=(a b`, `v=(""`, `v=([0]=x`, `declare -a v=(a` are all ``line 3: … matching `)'`` with **rc 1**; and `v=(a` on line 2 of a 4-line file is reported at **line 2**, not the last line.

- [ ] **Step 2: Run red.** All rows fail on both message and status.

- [ ] **Step 3: Message and line only**

`ArrayLiteral` becomes a pair-bearing frame; `LexError::UnterminatedArrayLiteral` keeps its identity so `is_unterminated_lex` still asks the REPL for continuation.

- [ ] **Step 4: Gate 1** — the ~15 array-literal coordinates leave. Status rows still fail; that is expected at this step.

- [ ] **Step 5: Measure WHY bash exits 1 before implementing it**

Do not guess. Compare against a plain syntax error (`if then` → 2) and an unterminated quote (`echo "a` → 2) to find what makes this one different. **If the answer reaches further into the v358 fatality classifier than a classification change — stop and report.** The spec allows this half to become its own issue.

- [ ] **Step 6: Implement the status, then gates 1–5. Commit.**

---

### Task 5: The re-read `$((` keeps its line — closes #629

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (`Mode::CommandSub`), `crates/huck-syntax/src/parser.rs` (`parse_arith_expansion`'s `ArithBail` arm, ~`parser.rs:2276`)

**Interfaces:**
- Produces: `Mode::CommandSub { from_arith_reread: bool }` — a field on what v361 made a unit variant. It is frame data: the offset is already correct after the rewind, only the line rule is wrong.

- [ ] **Step 1: Write the failing harness rows** (measured first)

`echo a` / `echo $((1+2)` / `echo c` / `echo d` is ``line 2: … matching `)'`` in bash and line 5 in huck. Controls that already agree and must not move: `echo $( (1+2)` and `echo $(cmd` report the EOF line; `echo $(( (1+2)` reports the `$((` line.

- [ ] **Step 2: Run red.**

- [ ] **Step 3: Implement.** The `ArithBail` arm rewinds and re-drives as a command substitution; mark the frame it pushes. Comment that bash never re-reads, which is why its answer keeps the arithmetic's line.

- [ ] **Step 4: Gates 1–5.** No matrix cell covers this, so gate 1 is only "nothing joined". Commit.

---

### Task 6: EOF beats validation — closes #634

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` (the single exit from Task 1), `crates/huck-syntax/src/lexer.rs`

**Interfaces:**
- Consumes: the single exit (Task 1), `opens_pair` (Task 3).
- Produces: `Lexer::drain_to_pair_close(&mut self) -> Result<(), LexError>` — a character skip driven by the same table, consuming until the pair open at entry closes, returning the lex error if input runs out first.

- [ ] **Step 1: Write the failing harness rows** (measured first)

`echo ${$(` is ``line 5: … matching `)'``; `echo ${$((1+` is ``line 3: … matching `)'``; `echo ${${x` and `echo ${${x}` are ``line 3: … matching `}'``. Controls that already agree: `` echo ${`cmd `` names `` ` ``, and `echo ${!` / `echo ${1x` name `}`.

- [ ] **Step 2: Run red.**

- [ ] **Step 3: Implement.** At the single exit, a *validation* error (not already a lex EOF) drains to the pair's close; if input runs out the lex error wins. The drain opens pairs itself — that is what makes `${$(` report `)` at the EOF line rather than `}`.

- [ ] **Step 4: Guard against over-reach**

A validation error where input does NOT run out must surface unchanged. Rows: `echo ${${x}}; echo after` and `echo ${1x}; echo after` keep today's message and still run the following command.

- [ ] **Step 5: Gate 1 — the ~21 `${`-name-position coordinates leave. Gates 2–5. Commit.**

---

### Task 7: Wire the matrix in, document, hand over

**Files:**
- Create: `tests/scripts/eof_delimiter_matrix_diff_check.sh`, `tests/scripts/eof_pair_lines_diff_check.sh`
- Modify: `tools/eof_matrix.sh` (`EXPECTED_DIFF` down to what remains), `docs/architecture.md`

- [ ] **Step 1: Confirm what remains divergent**

Expect **12**: the 6 out-of-model cells (four `[[ a == `, `echo (`, `v=((`) and the 6 belonging to #631/#640. If a thirteenth remains, it belongs to a family this plan did not scope — **stop and report** rather than adding it to a skip list.

- [ ] **Step 2: Write the generated harness**

It sources `tests/scripts/lib/harness.sh` and generates the cells minus a skip list of exactly those 12, each with an inline comment naming its issue. Do not filter any other way — a silent filter is how a harness comes to be green for the wrong reason.

- [ ] **Step 3: Write the hand-written harness**

What 813 single-line cells cannot see: which line each pair reports (multi-line inputs, one per pair type), #629's `$((1+2)`, #633's exit status, the piped-stdin driver, and the Shape 2 controls.

- [ ] **Step 4: Run both RED at the branch point**

Build `a8a7169b` in a worktree and run both against it. A harness green against the pre-fix binary is not testing the fix.

- [ ] **Step 5: Update `docs/architecture.md`** — one paragraph in the cross-cutting section: the two message shapes, that the variant picks the shape and the pair picks the delimiter, and where the suppression table lives.

- [ ] **Step 6: Full verification from the branch point, then push and open the PR** referencing `Closes #643, #627, #629, #633, #634`.

**Do NOT merge.** Wait for the GitHub run to finish and pass before calling it ready — local green is not CI green.

---

## Self-Review

**Spec coverage.** The walk → Task 2. The suppression table → Task 3. The two frame-less pairs → Task 2 (arith span read off the frame; atom fallback as `None`). Deleting `err_open_hint` → Task 2. Variant-decides-shape / pair-decides-delimiter → Task 2. `Delim::ArrayParen` and the status → Task 4. The `CommandSub` marker → Task 5. The single exit and the drain → Tasks 1 and 6. Verification → the five gates on every task, harnesses in Task 7. Every spec section maps to a task.

**Placeholder scan.** None. Task 4 Step 5 deliberately does not say what the status fix is — it says measure it first and stop if it reaches beyond a classification change, which is the spec's instruction, not a gap.

**Type consistency.** `reported_pair(&[ModeFrame]) -> Option<(Delim, usize)>` and `opens_pair(Option<Delim>, Delim, bool) -> bool` are used with those signatures in Tasks 2, 3 and 6. `Delim::ArrayParen` appears only in Task 4. `Mode::CommandSub { from_arith_reread }` only in Task 5. `drain_to_pair_close` only in Task 6.

**Ordering.** Tasks 1 and 2 are inert and come first, so the authority switch is proved before any rule changes it. Task 3 is the largest behavioural win and lands early, while there is budget to investigate a surprise. Task 4's status half is the one item allowed to split off.
