# v325 — `$LINENO` fidelity cluster Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix three `$LINENO` divergences from bash 5.2.21 — piped-stdin per-line reset (#266), multi-line eval body offset (#258), and compound-header DEBUG-fire line (#261).

**Architecture:** Three independent fixes in three subsystems — the eval line base (`eval_in_sink`), the piped-stdin REPL loop (`repl.rs`), and compound-clause source lines (huck-syntax parser + command structs, consumed by the v324 executor DEBUG fires). All build on the existing `eval_frame`/`line_base()` mechanism.

**Tech Stack:** Rust; huck-engine, huck-cli, huck-syntax; bash-diff harness.

Spec: `docs/superpowers/specs/2026-07-22-lineno-cluster-design.md`
Issues: #266, #258, #261

## Global Constraints

- bash 5.2.21 `$LINENO` fidelity, tested via FILE and STDIN (not just `-c`).
- Reuse the `eval_frame`/`line_base()` carrier (`line_base() = eval_frame.saturating_sub(1)`; a command stamps `current_lineno = line_base() + cmd.line`). Do NOT change `line_base()`'s formula.
- Commit trailer on every commit; `cargo fmt --all` before committing. NEVER put `Closes #N` in a spec/plan commit (auto-closes early) — bare `#N` only; the closing keyword goes in the PR body.
- Per repo memory: build with `cargo build -p huck`; per-crate tests single-threaded (`cargo test -p huck-engine|huck-syntax --lib --jobs 1 -- --test-threads 1`); NEVER `cargo test --workspace` (OOMs); guard sweeps with `ulimit -v 1500000` + `timeout`; run the `-p huck` lineno/eval/trap integration binaries single-threaded before push; NO GPL bash text.

## Shared harness

`tests/scripts/lineno_fidelity_diff_check.sh` (created in Task 1, extended in Tasks 2–3). Model on `trap_zero_diff_check.sh` but add TWO comparison helpers:
```sh
check_file()  { # writes $2 to a temp file, runs bash vs huck as a FILE ARG
  ... b=$(bash --norc "$f"); h=$("$HUCK_BIN" "$f") ... }
check_stdin() { # same fragment via `< file` (piped stdin)
  ... b=$(bash --norc < "$f"); h=$("$HUCK_BIN" < "$f") ... }
```
Both assert byte-identical (incl. exit). Every check must PASS.

---

### Task 1: B — multi-line eval body `$LINENO` offset (#258)

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (`eval_in_sink`, ~line 6841)
- Create: `tests/scripts/lineno_fidelity_diff_check.sh`

**Model:** `LINENO(eval body line K) = E + N + K - 1` where `E` = eval command line, `N` = newlines in the eval body. huck currently gives `E - 1 + K` (low by `N`).

- [ ] **Step 1: Create the harness with `check_file`/`check_stdin` + the eval cases**

Build the harness skeleton (both helpers). Eval cases (write the fragment so the eval sits at a known line):
```sh
check_file "eval@1 1-line"  'eval '\''echo $LINENO'\'''
check_file "eval@3 2-line"  ':
:
eval '\''echo $LINENO
echo $LINENO'\'''
check_file "eval@2 3-line"  ':
eval '\''echo $LINENO
echo $LINENO
echo $LINENO'\'''
```
(Also add `check_stdin` variants of each — they exercise A too, and must pass after Task 2; for now they may differ, note it.)

- [ ] **Step 2: Confirm red** — build, run the harness; the eval `check_file` cases FAIL (huck low by `N`).

- [ ] **Step 3: Implement**

In `eval_in_sink`, replace:
```rust
shell.eval_frame = Some(shell.current_lineno.max(1));
```
with:
```rust
let body_newlines = joined.bytes().filter(|&b| b == b'\n').count() as u32;
shell.eval_frame = Some(shell.current_lineno.max(1) + body_newlines);
```

- [ ] **Step 4: Confirm the eval `check_file` cases pass** vs bash. Add a unit test (or a pure helper test) asserting `E-1+N+K == E+N+K-1` for a couple of (E,N,K); or a `-p huck` integration-level check via the harness suffices — keep it simple.

- [ ] **Step 5: Regression** — `eval_integration`, `eval_source_sink_integration`, `lineno` integration bins green; huck-engine lib green; `dbg-support2` category unaffected.

- [ ] **Step 6: fmt + commit**
```bash
cargo fmt --all
git add crates/huck-engine/src/builtins.rs tests/scripts/lineno_fidelity_diff_check.sh
git commit -m "$(cat <<'EOF'
v325: multi-line eval body $LINENO offset by body newline count (#258)

bash's $LINENO inside an eval body is E + N + K - 1 (E=eval line, N=body
newlines, K=body line); huck was low by N. Add the body's newline count to
eval_frame in eval_in_sink. Single-line evals unchanged.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: A — piped-stdin cumulative `$LINENO` (#266)

**Files:**
- Modify: `crates/huck-cli/src/repl.rs` (the non-interactive stdin loop, ~line 237–260)
- Modify: `tests/scripts/lineno_fidelity_diff_check.sh` (add stdin cases)

**Model:** the non-interactive stdin REPL processes one logical command per `process_line`, each re-stamping from line 1. Maintain a cumulative physical-line counter; set the line base before each `process_line` so command line-K → `lines_before + K`.

- [ ] **Step 1: Add stdin `check_stdin` cases (red)**
```sh
check_stdin "stdin 3-line"     'echo $LINENO
echo $LINENO
echo $LINENO'
check_stdin "stdin for body"   'for x in 1
do
echo $LINENO
done'
```
Confirm they FAIL (huck reports `1 1 1` / wrong absolute line via stdin).

- [ ] **Step 2: Implement the cumulative base**

In the non-interactive stdin REPL loop (where `read_logical_command` returns `ReadResult::Ready { buffer, history }` and calls `process_line(&buffer, …)`), gate on `!shell.is_interactive`. Track a `let mut lines_before: u32 = 0;` outside the loop. Before `process_line`:
```rust
if !shell.is_interactive {
    shell.eval_frame = Some(lines_before + 1); // line_base() = lines_before
}
```
After `process_line` (regardless of outcome), advance:
```rust
if !shell.is_interactive {
    let physical = buffer.bytes().filter(|&b| b == b'\n').count() as u32
        + if buffer.ends_with('\n') { 0 } else { 1 };
    lines_before += physical;
    shell.eval_frame = None; // restore top-level default between commands
}
```
Notes: `eval_frame` is `None` at top level otherwise, so reusing it as the stdin base is safe; an `eval`/`source` in the script saves/restores it around its own frame (composes). Determine the exact `buffer`/`history` newline semantics by inspection (a logical command may span several physical lines) — the counter must advance by the physical lines this command consumed. Interactive mode is unchanged.

- [ ] **Step 3: Confirm stdin cases pass** — the `check_stdin` cases (and the Task-1 eval `check_stdin` variants) now match bash. Re-run the whole harness.

- [ ] **Step 4: Regression** — file-arg and `-c` `$LINENO` unchanged; interactive unaffected (can't easily test in the harness — reason about it); `script_line_numbers_integration` / `lineno` / `bash_source_lineno` bins green; huck-engine + huck-cli lib green.

- [ ] **Step 5: fmt + commit**
```bash
cargo fmt --all
git add crates/huck-cli/src/repl.rs tests/scripts/lineno_fidelity_diff_check.sh
git commit -m "$(cat <<'EOF'
v325: cumulative $LINENO for a piped-stdin script (#266)

The non-interactive stdin REPL processed one command per process_line, each
re-stamping $LINENO from line 1. Track a cumulative physical-line counter and
set the eval_frame line base before each process_line so a stdin script's
$LINENO matches a file/`-c` script. Interactive mode unchanged.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: C — compound-header line for DEBUG fires (#261)

**Files:**
- Modify: `crates/huck-syntax/src/command.rs` (add `line` to 4 clauses)
- Modify: `crates/huck-syntax/src/parser.rs` (capture line in 4 parsers; zero in `zero_lines_in_command`)
- Modify: `crates/huck-engine/src/executor.rs` (stamp before the v324 header fires)
- Modify: `tests/scripts/lineno_fidelity_diff_check.sh` (add C cases)

- [ ] **Step 1: Add C `check_file` cases (red)**
```sh
check_file "for header lineno"  'trap '\''echo L$LINENO'\'' DEBUG
for x in 1 2
do
echo hi
done'
check_file "case header lineno" 'trap '\''echo L$LINENO'\'' DEBUG
case a in
a) echo m;;
esac'
```
Confirm the header-fire lines DIFFER from bash (huck reports 1 / a stale line).

- [ ] **Step 2: Add `line: u32` to the clause structs**

In `command.rs`, add `pub line: u32,` to `ForClause`, `ArithForClause`, `CaseClause`, `SelectClause`. This breaks the parser constructors + `zero_lines_in_command` — fix them in the next steps (the compiler lists every site).

- [ ] **Step 3: Capture the line in the four parsers**

In `parse_for` (~4647), `parse_arith_for_clause` (~4605), `parse_select` (~4754), `parse_case` (~4832): at the very top, BEFORE `expect_keyword(iter, …)`, add `let line = iter.line();`, and pass `line` into the constructed clause (`ForClause { …, line }`, etc.). `iter.line()` (Lexer, lexer.rs:200) is the 1-based line of the next char = the keyword's line.

- [ ] **Step 4: Zero the new fields in `zero_lines_in_command`**

In `parser.rs` `zero_lines_in_command` (~1553), add arms/lines that set the four clauses' `.line = 0` (mirroring the `ExecCommand::Exec(e) => e.line = 0` pattern), so the `-c`/comsub line-stripping path stays consistent. (Search for where `Command::For`/`Case`/`Select`/`ArithFor` are handled — add the zero there.)

- [ ] **Step 5: Stamp the header line before the v324 fires**

In `executor.rs`, immediately BEFORE each compound-header `let _ = crate::traps::fire_debug_trap(shell);` added in v324 — the per-iteration fire in `run_for_inner`, the per-iteration fire in `run_select_inner`, the entry fire in `run_case_inner`, and the init/cond/step fires in `run_arith_for_inner` — insert:
```rust
if clause.line != 0 {
    shell.current_lineno = shell.line_base() + clause.line;
}
```
(The four arith-for `for ((…))` fires all use the same `clause.line`.) The body simple commands re-stamp their own line via the normal path, so the next iteration's header fire re-stamps correctly.

- [ ] **Step 6: Confirm C cases pass** vs bash (header fires report the header line, per iteration). Add a huck-syntax unit test: a parsed `for`/`case`/`select`/arith-for clause has a non-zero `line`; `zero_lines_in_command` zeros it.

- [ ] **Step 7: Regression — everything**
```bash
cargo test -p huck-syntax --lib --jobs 1 -- --test-threads 1     # green
cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1     # green
cargo build -p huck && cargo build --release -p huck
ulimit -v 1500000; bash tests/scripts/lineno_fidelity_diff_check.sh        # all PASS
ulimit -v 1500000; bash tests/scripts/debug_firing_points_diff_check.sh    # firing COUNTS unchanged
ulimit -v 1500000; timeout 300 bash tests/scripts/run_diff_checks.sh       # green (coproc flake is pre-existing)
ulimit -v 2000000; HUCK_BASH_TEST_CATEGORY=dbg-support2 HUCK_TEST_TIMEOUT=60 \
  BASH_SOURCE_DIR=/tmp/bash-5.2.21 timeout 150 bash tests/bash-test-suite/runner.sh 2>&1 | grep -iE "dbg-support2 \|"   # PASS
for t in lineno bash_source_lineno script_line_numbers_integration trap_integration eval_integration; do
  cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 2>&1 | grep "test result"
done
```

- [ ] **Step 8: fmt + commit**
```bash
cargo fmt --all
git add crates/huck-syntax/src/command.rs crates/huck-syntax/src/parser.rs crates/huck-engine/src/executor.rs tests/scripts/lineno_fidelity_diff_check.sh
git commit -m "$(cat <<'EOF'
v325: compound clauses carry a source line for DEBUG header fires (#261)

Add a `line` field to For/ArithFor/Case/Select clauses (captured from the
keyword token in the parser, zeroed on the line-stripping path), and stamp it
before the v324 compound-header DEBUG fires so $LINENO reports the header line
(per iteration), matching bash.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

- **Spec coverage:** Task 1 = B (spec §B), Task 2 = A (spec §A), Task 3 = C (spec §C). The shared harness (spec §Testing 1) is built in T1 and extended per task; unit tests (§Testing 2) live with each part; regressions (§Testing 3) in T3's Step 7.
- **Placeholders:** none; each code change is concrete. The stdin buffer newline-count is specified with a fallback ("determine exact semantics by inspection") because the exact `buffer`/`history` shape must be read — the counter's contract (advance by physical lines consumed) is exact.
- **Type consistency:** `eval_frame: Option<u32>`, `line_base() = eval_frame-1`, new `line: u32` on 4 clauses, `iter.line() -> u32`. All verified.
- **Scope:** the `read`-consumes-a-line stdin edge and extdebug-decision-at-header-fires are excluded; `line_base()` formula unchanged.
