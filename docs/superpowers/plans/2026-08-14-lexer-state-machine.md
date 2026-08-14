# v361 — Lexer State Machine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the lexer's state into the automaton it already has — one state at a time, nesting by stack, transitions enforced by a method — and delete the flags, parallel structures and dead state that accumulated beside it. No detectable behaviour change.

**Architecture:** `modes: Vec<Mode>` stays the pushdown automaton. Each mode's sub-states become variants or parameters of that mode instead of parallel booleans on `Lexer`; the one lockstep-pushed vector merges into the frames; options and one-shot instructions leave the automaton; inherited context is derived by looking down the stack rather than copied into child frames.

**Tech Stack:** Rust (`huck-syntax`), bash 5.2.21 as the differential oracle for the inertness gates.

**Spec:** [`docs/superpowers/specs/2026-08-14-lexer-state-machine-design.md`](../specs/2026-08-14-lexer-state-machine-design.md) — read it before Task 1. **Issue:** [#641](https://github.com/jdstanhope/huck/issues/641).

## How to use this plan

This plan gives you **interfaces, gates and evidence to collect** — not finished implementations. That is deliberate: v359's plan pasted complete function bodies, they were copied without scrutiny, and a `hash -:` panic shipped past a green clippy run, 2490 lib tests and a 275-harness sweep. Write the code yourself against the stated shape.

If a gate does not come out as this plan says it will, **stop and report it** rather than adjusting the gate. In v360 that rule caught a panic I had introduced two tasks earlier.

## Global Constraints

- **This work stays on `v361-lexer-state-machine`. Do NOT merge to `main` — not even a "safe" docs-only commit — without the user's explicit approval.** No exceptions, and no `git push origin main`.
- **No detectable behaviour change.** Same tokens, same errors, same messages, same exit statuses, on every input. Every task is inert on its own; a task that cannot be shown inert is a behaviour change in disguise, and the answer is to stop, not to relax the gate.
- **Oracle:** bash 5.2.21 (`bash --norc --noprofile`). Never assert bash's behaviour from memory.
- **Commit trailer:** `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`
- **Format before commit:** `cargo fmt --all`. **Lint with the pinned toolchain:** `cargo +1.97.1 clippy --workspace --all-targets --locked -- -D warnings`.
- **This box is 1 core / 1.9 GB.** Per-crate tests, never `--workspace`; wrap in `ulimit -v`; `--jobs 1`; the engine lib suite needs `-- --test-threads 4`. Runs over ~1 minute must be detached (`setsid nohup … &` + poll).
- **Report the line delta** of every task (`git diff --stat` against the branch point). The expectation is that this iteration REMOVES lines; a task that adds them needs a sentence saying why.

## The inertness gates

Every task runs all five. They are listed once here and referenced by number.

1. **Zero expected-value edits** — `git diff --stat main -- '*tests.rs' 'tests/'` is empty. A behaviour-preserving change that needed a test edited is not one.
2. **`${…}` corpus** — `tools/param_corpus.sh <binary>` over 250 forms, byte-identical between a binary built at the branch point and the current one. This corpus exists because a refactor that passed every other gate still panicked on `echo ${x:1`.
3. **Parse sweep** — `tools/parse_sweep.sh`, 3103 real scripts, identical to the branch-point baseline.
4. **EOF matrix** — `tools/eof_matrix.sh --check`: 0 FIXED, 0 REGRESSED across 813 cells.
5. **Suites and sweep** — both `--lib` suites, every `-p huck` integration binary, full `tests/scripts/run_diff_checks.sh`, pinned clippy clean.

Gates 2, 3 and 4 need `tools/param_corpus.sh` and `tools/eof_matrix.sh`, which currently exist only on the parked `v360-eof-delimiter-model` branch. **Task 0 brings them over.**

## File Structure

| file | responsibility |
| --- | --- |
| `crates/huck-syntax/src/lexer.rs` | the automaton: `Mode`, the stack, the per-mode branches. Loses the flags, the parallel vector and the dead state. Baseline **10195 lines**. |
| `crates/huck-syntax/src/lexer/state.rs` *(new)* | `CommandPos` and its transition method — the one place a command-position change is allowed, so illegal moves are unrepresentable or loud. |
| `crates/huck-syntax/src/parser.rs` | loses `set_*` calls as their state becomes a mode parameter. Baseline **6039 lines**. |
| `crates/huck-syntax/src/recover.rs` | recovery hints become parameters rather than lexer fields. Baseline **713 lines**. |
| `tools/param_corpus.sh`, `tools/eof_matrix.sh` | the inertness instruments, brought over in Task 0. |

---

### Task 0: Bring the measurement instruments over

Without these, gates 2–4 cannot run and the whole iteration is unverifiable.

**Files:**
- Create: `tools/param_corpus.sh`, `tools/eof_matrix.sh` (cherry-picked from `v360-eof-delimiter-model`)

- [ ] **Step 1: Cherry-pick the two tool commits**

`a76870a6` (the matrix) and the `tools/param_corpus.sh` half of `c8fa2223`. Take the tools ONLY — not v360's `PairStack` (`83db1f5f`), which is the parallel stack this iteration exists to avoid, and not the `parse_param_expansion` single-exit refactor unless the user has separately approved landing it.

- [ ] **Step 2: Establish the branch-point baselines**

Build a binary at the branch point in a throwaway worktree and record all three references, so every later task diffs against a fixed point rather than against its predecessor:

```bash
S=<scratch>; git worktree add "$S/base" d14dcc87 && (cd "$S/base" && cargo build --locked --bin huck)
tools/param_corpus.sh "$S/base/target/debug/huck" > "$S/corpus_base.tsv"     # expect 250 rows
HUCK_BIN="$S/base/target/debug/huck" tools/parse_sweep.sh tools/scripts.tsv "$S/parse_base.tsv"
```

Expect the parse sweep to report 3103 scripts, AGREE_OK 3092, AGREE_FAIL 11, no crashes or timeouts. If it reports anything else, stop — the baseline is wrong and every later gate inherits the error.

- [ ] **Step 3: Confirm the matrix instrument agrees with its own expectations**

`tools/eof_matrix.sh --check` → 813 cells, 78 DIFF, 0 FIXED, 0 REGRESSED. That is the pre-existing EOF divergence set; this iteration must not move it in either direction.

- [ ] **Step 4: Commit** — `git commit -m "tools(#641): bring the inertness instruments onto the v361 branch"`

---

### Task 1: Delete the vestigial state

Three pieces of state are already dead. Measurement, on `main`:

- **`expect_regex`** — zero reads, zero writes. The only mention outside the field declaration is `mark`/`rewind` copying it.
- **`has_token`** — written only `= false` (inside a branch that tests it), never `= true`. Its `if self.has_token { … }` block in `finish()` is therefore unreachable, and the two `debug_assert!`s in `mark`/`rewind` carry a vacuous `!self.has_token` conjunct.
- **`pending_heredocs`** — never written; the only non-`atom_` mutation site left is an `.iter()` read. Its `if !self.pending_heredocs.is_empty()` check in `finish()` never fires. `atom_pending_heredocs` is the live queue.

`finish()` reduces to `Ok(Step::Eof)`.

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (fields, `Mark` fields and their restores, `finish()`, the two debug asserts)

**Interfaces:**
- Consumes: nothing.
- Produces: no API change. `finish(&mut self) -> Result<Step, LexError>` keeps its signature.

- [ ] **Step 1: Prove unreachable BEFORE deleting**

Do not delete on grep evidence. Replace each dead read with a loud failure — `unreachable!("v361: has_token was believed always false")` in the `finish()` branch, likewise for the heredoc check — and leave the fields in place.

- [ ] **Step 2: Run gates 2, 3 and 5 with the panics armed**

If any input reaches one, the field is NOT dead and this task's premise is wrong for it: stop and report which input got there. The `${…}` corpus and the 3103-script sweep are the ones most likely to reach `finish()`.

- [ ] **Step 3: Delete**

Both fields, `pending_heredocs`, their `Mark` fields and restores, the two dead branches in `finish()`, and the now-vacuous `!self.has_token` conjunct in each `debug_assert!`. Leave the doc comments that refer to `!has_token` as a CONCEPT only where they still explain something; delete the ones that describe the field.

- [ ] **Step 4: All five gates, and report the line delta**

Expect a reduction of roughly 30–40 lines. Report the actual number.

- [ ] **Step 5: Commit** — `git commit -m "refactor(#641): delete three pieces of dead lexer state"`

---

### Task 2: `CommandPos` — the command-position flags become one state

**Files:**
- Create: `crates/huck-syntax/src/lexer/state.rs`
- Modify: `crates/huck-syntax/src/lexer.rs`

**Interfaces:**
- Produces:

```rust
pub(crate) enum CommandPos { /* derived in Step 1 — do not guess it */ }

impl CommandPos {
    /// The ONLY way the position changes. Returns the new state, and is where an
    /// illegal move is caught.
    pub(crate) fn advance(self, ev: PosEvent) -> CommandPos;
}
```

`Mode::Command { pos: CommandPos }` replaces the fieldless variant. The position field is private to `state.rs`, so nothing outside can assign it.

- [ ] **Step 1: Derive the states from the write sites, and write them down first**

The flags are `cmd_at_word_start` (12 write sites), `in_assignment_value` (7), `assign_val_tilde_ok` (10, one of them `= boundary`, i.e. data-dependent). Read every write site and record, in the commit message or a comment, which COMBINATIONS actually occur. Three bools is 8 combinations; the enum should have only the reachable ones.

The `= boundary` site is the one to think hardest about: if the tilde rule genuinely depends on a runtime value rather than a position, it is a parameter of the state, not a state.

- [ ] **Step 2: Write the failing unit tests for the transition table**

Test the transitions, not the scanner: from each state, each event, the expected next state — and that an illegal move is rejected. This is the part that makes the transitions *enforced* rather than merely tidier, so it is written before the migration.

- [ ] **Step 3: Migrate the reads and writes**

Every `self.cmd_at_word_start = …` becomes an `advance` call. Every read becomes a `matches!` on the position.

- [ ] **Step 4: All five gates.** Gate 2 (the corpus) is the sharp one here: assignment-value and tilde behaviour is exactly what it exercises.

- [ ] **Step 5: Commit**, reporting the line delta and the reachable-combination count from Step 1.

---

### Task 3: `mode_open_offs` merges into the frames

The one lockstep push in the workspace: `push_mode` pushes `modes` and `mode_open_offs` together, `pop_mode` pops both.

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs`

**Interfaces:**
- Produces: `push_mode(&mut self, m: Mode)` unchanged from the caller's view. Internally the stack element carries both the mode and its opening offset. `error_open_start()` keeps its signature and meaning.

- [ ] **Step 1: Decide where the offset rides**

Either a wrapper struct in the stack (`struct Frame { mode: Mode, open_off: usize }`) or an `open_off` field on every variant. The wrapper is one place; per-variant fields would repeat it 13 times. Note `Mode` is `Copy` and `mark`/`rewind` clone the whole stack, so a wrapper is also the cheaper of the two.

- [ ] **Step 2: Migrate, then check the drift is now impossible**

The point of the task is that the two can no longer disagree — after this, `mode_open_offs` does not exist to drift from. Say so in the commit rather than leaving it implied.

- [ ] **Step 3: All five gates.** Gate 4 (the EOF matrix) is the sharp one: the offset feeds every unterminated-delimiter line number, and all 813 cells must stay exactly as they are.

- [ ] **Step 4: Commit** — report the line delta.

---

### Task 4: Handshake flags become entry states

`body_started` appears on `CommandSub`, `DoubleQuote`, `Arith`, `ArrayLiteral` and `Regex`; `backtick_raw_started` is a lexer field doing the same job for `BacktickRaw`; `seen_name` is `ParamExpansion`'s pre/post-name phase. All answer "have I consumed my opener yet", which is a state.

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs`

- [ ] **Step 1: Convert one mode first — `DoubleQuote`** — the simplest, and it proves the shape before it is repeated. `Mode::DoubleQuote { body_started: bool }` becomes two states.
- [ ] **Step 2: Gates 4 and 5 on that one mode alone**, so a mistake is attributable to one construct.
- [ ] **Step 3: Convert the rest**, including `backtick_raw_started`, which stops being a lexer field entirely — that is the point of doing it here rather than leaving it in Task 6.
- [ ] **Step 4: All five gates. Commit**, reporting the line delta.

---

### Task 5: Inherited context is derived, not copied

`enclosing_dquote` is copied into four operand variants; `opts.in_dquote` is context stored as configuration, written through `set_in_dquote` and read via `in_dquote()`.

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs`, `crates/huck-syntax/src/parser.rs`

- [ ] **Step 1: Establish that the stack can answer the question**

Before deleting anything, add the derivation (walk down for an enclosing `DoubleQuote`, or whatever the measurement shows) and assert in debug builds that it agrees with the copied field on every read. Run gates 2, 3 and 5 with that assert armed.

If it disagrees anywhere, **stop and report**: either the copy is carrying information the stack does not have — which is a finding worth more than the cleanup — or the derivation is wrong.

- [ ] **Step 2: Remove the copies** once the assert has held across all three gates.
- [ ] **Step 3: All five gates. Commit**, reporting the line delta.

**Note:** the M-156 gate (`set_in_dquote`) interacts with `${…}` operand parsing in ways the corpus covers closely. If Step 1's assert fires here, that is the expected place.

---

### Task 6: Options and one-shot instructions leave the automaton

`brace_expand` (a shopt, 11 parser references), `replay` (a construction kind), `retokenize_arith_as_cmdsub` (a one-shot instruction), and the recovery hints `recovery_cmd_word` (11 parser references) / `recovery_redirect_target`.

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs`, `crates/huck-syntax/src/parser.rs`, `crates/huck-syntax/src/recover.rs`

- [ ] **Step 1: Sort each one explicitly** — option, construction kind, or instruction — and move it to the matching home: `LexerOptions`, a constructor parameter, or an explicit argument. Record the sort in the commit message; a field whose bucket is unclear stays and gets a comment saying why.
- [ ] **Step 2: All five gates. Commit**, reporting the line delta.

---

### Task 7: Shrink the parser's `set_*` surface

Whatever remains of `set_regex_body_started`, `set_force_extglob`, `set_param_start_off_from_cursor`, `set_in_dquote`, `set_retokenize_arith_as_cmdsub`, `set_recovery_cmd_word`, `set_recovery_redirect_target` after Tasks 2–6.

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs`, `crates/huck-syntax/src/parser.rs`

- [ ] **Step 1: For each remaining setter, state which it is** — a mode parameter that should be supplied at push time, or something that genuinely cannot be. Convert the first kind; **document the second kind and leave it**. This task is bounded on purpose: it is where the iteration could grow without limit.
- [ ] **Step 2: All five gates. Commit**, listing which setters went and which stayed, with the reason for each survivor.

---

### Task 8: Document the automaton, report, and hand over

**Files:**
- Modify: `docs/architecture.md`
- Modify: this plan (check the boxes)

- [ ] **Step 1: Write the state machine down in `docs/architecture.md`** — the modes, the command-position states and their legal transitions, and the four rules (control state is the automaton; data may ride on a frame; options and instructions live outside it; inherited context is derived). Someone adding a construct next year should be able to tell where their new state belongs without reading this plan.
- [ ] **Step 2: Report the totals** — line delta per file against `d14dcc87`, count of bools removed from `Lexer`, count of fields removed from `Mode` variants, count of `set_*` methods removed. `lexer.rs` starts at 10195 lines, `parser.rs` at 6039, `recover.rs` at 713.
- [ ] **Step 3: Final full verification on the whole branch diff** — all five gates once more, from the branch point rather than from the previous task.
- [ ] **Step 4: Push the branch and open a PR** referencing `Closes #641`.

**Do NOT merge.** Hand the PR to the user. Wait for the GitHub run to finish and pass before calling it ready — local green is not CI green.

---

## Self-Review

**Spec coverage.** Rule 1 (control state is the automaton) → Tasks 2 and 4. Rule 2 (enforced transitions) → Task 2, whose transition tests are written before the migration. Rule 3 (no parallel structures) → Tasks 1 and 3. Rule 4 (data on frames) → Task 3's wrapper decision. Rule 5 (options/instructions leave) → Task 6. Rule 6 (inherited context derived) → Task 5. The parser-poking problem → Task 7. The dead state the spec names → Task 1. Verification → the five gates, run by every task.

**Placeholder scan.** `CommandPos`'s variants are deliberately not listed: Task 2 Step 1 derives them from the write sites and forbids guessing. Every other step names its files, its command and its expected result.

**Type consistency.** `CommandPos`/`PosEvent`/`advance` appear only in Task 2. `Frame { mode, open_off }` appears only in Task 3, as one of two options with the choice justified there.

**Ordering risk.** Task 1 is first because deleting dead state shrinks everything after it. Task 2 is the largest and is deliberately not last, so its fallout is found while there is still budget. Task 7 depends on 2–6 and is bounded to prevent it becoming an open-ended refactor of the parser interface.
