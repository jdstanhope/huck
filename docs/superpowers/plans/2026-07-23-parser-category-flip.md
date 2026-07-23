# v331 — Flip the `parser` bash-suite category Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the four independent divergences that keep the `parser` bash-suite category off PASS, taking parser.diff 13 → 0 (Summary PASS 19→20, FAIL 63→62).

**Architecture:** Four surgical edits — three error-shape/`$LINENO` fixes in the parser + for-loop runtime, and one non-interactive driver-loop abort — each prototype-verified byte-identical to bash 5.2.21. One bash-diff harness covers all four shapes plus regression guards.

**Tech Stack:** Rust; huck-syntax (`parser.rs`) + huck-engine (`executor.rs`, `builtins.rs`); bash-diff harness.

Spec: `docs/superpowers/specs/2026-07-23-parser-category-flip-design.md`
Issues: [#27](https://github.com/jdstanhope/huck/issues/27) (fixes 1–3), [#283](https://github.com/jdstanhope/huck/issues/283) (fix 4)

## Global Constraints

- bash 5.2.21 fidelity — every fragment byte-identical incl. stderr + exit code:
  - `for 1x in a; do :; done` → `<name>: line 1: \`1x': not a valid identifier`, rc 1.
  - `case x in in do do) :; esac` → `syntax error near unexpected token \`do'` + the echoed line, rc 2.
  - `for()` → `syntax error near unexpected token \`('` + the echoed line, rc 2.
  - non-interactive syntax error (`-c` string / script file / sourced-file remainder) ABORTS the parse-context (no later command runs), rc 2; a **sourced** file's parent still continues; interactive `source`/`.`/rc keep recovery; lex errors already abort.
- Do NOT change: the lex-error restart arm; the `recover_at_eof && peek_is_recovery_close` EOF-recovery branch (truncated inner mode); the same-line abort behavior; interactive recovery.
- Commit trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`; `cargo fmt --all` before committing. Per repo memory: build with `cargo build -p huck`; per-crate tests single-threaded (`cargo test -p <crate> --lib --jobs 1 -- --test-threads 1`); NEVER `cargo test --workspace` (OOMs); guard sweeps with `ulimit -v 1500000` + `timeout`; run `-p huck` integration bins single-threaded before push; NO GPL bash text; no `Closes #N` in commits (bare `#N`).

---

### Task 1: Parser near-token error shapes (fixes 2 + 3) + harness

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` (`expect_or_recover` ~4321; `parse_for` ~4682)
- Create: `tests/scripts/parser_syntax_errors_diff_check.sh`

- [ ] **Step 1: Write the harness (red)**

Create `tests/scripts/parser_syntax_errors_diff_check.sh` (model on an existing `-c` bash-diff harness such as `syntax_error_diag_diff_check.sh`). A `check "label" 'fragment'` helper compares `bash --norc --noprofile -c "$frag"` vs `"$HUCK_BIN" -c "$frag"`, byte-identical on stdout, stderr (with the huck binary path normalized to `bash` so the program-name prefix matches — see how `syntax_error_diag_diff_check.sh` normalizes), and `EXIT:$?`. Add these cases (this task's + a guard; later tasks extend the file):
```sh
check "case wrong-token"  'case x in in do do) :; esac'      # near-token `do`, rc 2
check "for single-paren"  'for()'                            # near-token `(`, rc 2
check "for-paren newline"  $'for()\ntrue'                    # near-token `(`, rc 2
check "if-then EOF recover" 'echo $(if true; then echo hi'   # still recovers (EOF guard) — unchanged
```
Build (`cargo build -p huck`) and run — the two near-token cases FAIL (huck emits the unexpected-EOF / invalid-variable-name shape). Confirm each expected output against `bash --norc --noprofile` first.

- [ ] **Step 2: `expect_or_recover` — concrete wrong token → near-token error**

In `crates/huck-syntax/src/parser.rs`, `expect_or_recover` (~4321), the body is:
```rust
    if peek_leading_keyword(iter)? == Some(expected) {
        consume_command_word(iter)?;
        Ok(true)
    } else if iter.recover_at_eof() && iter.peek_is_recovery_close()? {
```
Insert a branch between them:
```rust
    if peek_leading_keyword(iter)? == Some(expected) {
        consume_command_word(iter)?;
        Ok(true)
    } else if iter.peek_kind()?.is_some() && !iter.peek_is_recovery_close()? {
        // A concrete wrong token where `expected` keyword was required — bash
        // reports it as a near-token error (`unexpected token \`X'`), not the
        // unterminated/EOF shape.
        Err(ParseError::Unexpected(iter.unexpected_here(None)?))
    } else if iter.recover_at_eof() && iter.peek_is_recovery_close()? {
```
The `peek_kind().is_some()` guard means at true EOF (`peek_kind` == `None`) control still reaches the existing recovery branch — so `echo $(if true; then echo hi` (truncated inner mode) keeps recovering. `peek_is_recovery_close()` excludes the delimiter that legitimately closes the enclosing construct.

- [ ] **Step 3: `parse_for` — reject a lone `(`**

In `parse_for` (~4682), immediately after the `((`-arith-for check:
```rust
    {
        return parse_arith_for_clause(iter);
    }
```
add:
```rust
    // A single `(` after `for` (not `((`) is a syntax error at the `(` —
    // bash: `syntax error near unexpected token \`('`.
    if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen))) {
        return Err(ParseError::Unexpected(iter.unexpected_here(None)?));
    }
```
(`TokenKind`/`Operator` are already in scope in this file; confirm the import path matches the arith-for check just above, which also matches on `Operator::LParen`.)

- [ ] **Step 4: Confirm the harness passes** for the two near-token cases + the EOF-recover guard, byte-identical to bash.

- [ ] **Step 5: Regression**
```bash
cargo test -p huck-syntax --lib --jobs 1 -- --test-threads 1   # green (475)
cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1   # green
ulimit -v 1500000; HUCK_BIN=./target/debug/huck bash tests/scripts/parser_syntax_errors_diff_check.sh && echo PASS
HUCK_BIN=./target/debug/huck bash tests/scripts/syntax_error_diag_diff_check.sh && echo "sed PASS"
```

- [ ] **Step 6: fmt + commit**
```bash
cargo fmt --all
git add crates/huck-syntax/src/parser.rs tests/scripts/parser_syntax_errors_diff_check.sh
git commit -m "$(cat <<'EOF'
v331: parser near-token error shapes for `for(` and keyword-position wrong tokens (#27)

`expect_or_recover` now reports a concrete wrong token where a keyword was
required as a near-token error (not the unexpected-EOF shape); `parse_for`
rejects a lone `(` (not `((`) at the `(`. Byte-identical to bash. Part of the
parser bash-suite category flip.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: `for` bad-name runtime error carries `line N:` (fix 1)

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (`run_for_inner` ~1783)
- Modify: `tests/scripts/parser_syntax_errors_diff_check.sh` (add the case)

- [ ] **Step 1: Add the harness case (red)**

Append to `tests/scripts/parser_syntax_errors_diff_check.sh`:
```sh
check "for bad-name lineno" 'for 1x in a; do :; done'   # `line 1:` prefix, rc 1
```
Run — huck FAILs (stderr missing the `line 1:` prefix bash emits).

- [ ] **Step 2: Stamp the header line before the error**

In `run_for_inner` (executor.rs ~1783), the current code is:
```rust
    if !crate::builtins::is_valid_name(&clause.var) {
        {
            let mut err = err_writer(err_sink, sink);
            crate::sh_error_to!(
```
Insert the stamp right after the `if` opens, before the inner block:
```rust
    if !crate::builtins::is_valid_name(&clause.var) {
        // Stamp the for-header line so the runtime error carries bash's
        // `line N:` prefix (compound commands don't stamp current_lineno).
        if clause.line != 0 {
            shell.current_lineno = shell.line_base() + clause.line;
        }
        {
            let mut err = err_writer(err_sink, sink);
            crate::sh_error_to!(
```
`clause.line` is the for-header line (v325); `line_base()` folds the eval/stdin base.

- [ ] **Step 3: Confirm the harness passes** (all Task 1 + this case) byte-identical to bash.

- [ ] **Step 4: Regression**
```bash
cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1   # green
ulimit -v 1500000; HUCK_BIN=./target/debug/huck bash tests/scripts/parser_syntax_errors_diff_check.sh && echo PASS
```

- [ ] **Step 5: fmt + commit**
```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs tests/scripts/parser_syntax_errors_diff_check.sh
git commit -m "$(cat <<'EOF'
v331: for-loop bad-name runtime error carries the `line N:` prefix (#27)

Stamp the for-header line into current_lineno before the not-a-valid-identifier
error so it matches bash's `<src>: line N: \`x': not a valid identifier`.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Non-interactive syntax error aborts the parse-context (fix 4) + category flip

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (`run_sourced_contents_in_sinks_inner`, regular parse-error arm ~7910)
- Modify: `tests/scripts/parser_syntax_errors_diff_check.sh` (add abort + parent-continues + regression cases)

- [ ] **Step 1: Add the harness cases (red)**

Extend `tests/scripts/parser_syntax_errors_diff_check.sh`. The `-c` multi-line and file/source cases need helpers beyond the single-fragment `check`; add a small `check_file` (writes `$frag` to a temp file, runs `bash <file>` vs `"$HUCK_BIN" <file>`, normalizes the path to a fixed token) and reuse `check` for the `-c` multi-line strings:
```sh
check "abort multiline -c"  $'echo a\nfor()\necho b'         # a; error; rc 2; NO b
# script file: mid-file syntax error aborts
check_file "abort script-file" $'echo a\nfor()\necho b\n'    # a; error; rc 2; NO b
# sourced file aborts its remainder but the parent continues:
# (build a temp sub-file, source it from a -c string)
check "source parent-continues" "$(printf 'echo before; source %s; echo parent-after' "$SUBFILE")"
# regression guards:
check "valid multi still runs" $'echo x\necho y'             # x; y; rc 0
check "same-line still aborts" 'echo a; for(); echo b'       # a…? — match bash exactly; rc 2
```
For the source case, create `$SUBFILE` in the harness setup (`printf 'echo in-src\nfor()\necho after-err\n' > "$SUBFILE"`), and compare bash vs huck of the whole `-c` string; expect `before`, `in-src`, the error, `parent-after`, rc 0 — NO `after-err`. Confirm every expected output against bash first, then run — the abort/file/source cases FAIL (huck prints the trailing command).

- [ ] **Step 2: Abort the non-interactive parse-context**

In `run_sourced_contents_in_sinks_inner` (builtins.rs), the `Err(e)` arm has (after the lex-error early restart `if is_lex { … continue 'outer; }`):
```rust
                    // Regular parse error: skip tokens to the next newline and
                    // continue within the same lexer (no restart needed).
                    loop {
```
Insert BEFORE that comment/loop:
```rust
                    // bash aborts the entire non-interactive parse context on a
                    // regular syntax error — a `-c` string, a script file, or a
                    // `source`d file's remainder — rather than skipping the
                    // offending line and resuming. Each `-c`/script/`source` runs
                    // its OWN `run_sourced_contents_in_sinks`, so returning here
                    // aborts THIS context while a parent driver loop (for a
                    // `source`d file) still continues — matching bash exactly
                    // (`source bad; echo x` runs `echo x`). Interactive
                    // `source`/`.`/rc keep the skip-and-continue recovery below.
                    if !shell.is_interactive {
                        return ExecOutcome::Continue(2);
                    }
```
`Continue(2)` maps to exit 2 at `run_program_in_sinks` (`Continue(s) => take_pending_fatal_status().unwrap_or(s)`; none pending here → 2). Do NOT touch the lex-error arm (lex errors run to EOF already).

- [ ] **Step 3: Confirm the harness passes** — all cases byte-identical to bash (abort shapes + parent-continues + both regression guards).

- [ ] **Step 4: `parser` category flips to PASS**
```bash
cargo build --release -p huck
HUCK_BASH_TEST_CATEGORY=parser HUCK_TEST_TIMEOUT=90 BASH_SOURCE_DIR=/tmp/bash-5.2.21 \
  timeout 400 bash tests/bash-test-suite/runner.sh > /tmp/parser_run.md 2>&1
grep -iE "parser \|" /tmp/parser_run.md          # expect: | parser | PASS |
SC=$(grep -oE "/tmp/huck-bash-tests[^ ]*" /tmp/parser_run.md | head -1)
echo "parser.diff: $(wc -l < $SC/parser.diff) lines (expect 0)"
```

- [ ] **Step 5: Regression — no other category slips**
```bash
cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1   # green
# `-p huck` integration bins that encode parser/syntax/-c behavior (single-threaded):
for t in parse_sweep_integration cli_syntax_error error_prologue_integration; do
  cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 2>&1 | grep "test result" || echo "(no such bin: $t)"
done
ulimit -v 1500000; timeout 550 bash tests/scripts/run_diff_checks.sh   # green (coproc flake pre-existing)
# confirm no previously-PASS category regressed:
for c in dbg-support2 rhs-exp procsub posix2; do
  HUCK_BASH_TEST_CATEGORY=$c HUCK_TEST_TIMEOUT=90 BASH_SOURCE_DIR=/tmp/bash-5.2.21 \
    timeout 200 bash tests/bash-test-suite/runner.sh 2>&1 | grep -iE "$c \|"
done
```
Note: the exact `-p huck` integration binary names are best-effort — list `crates/huck/tests/*.rs` (or `ls tests/`) and run whichever cover parser / syntax-error / `-c` behavior single-threaded; `--lib` alone is not sufficient (a `-p huck` integration bin failed CI in v289).

- [ ] **Step 6: Docs + memory (part of the branch)**
  - `docs/bash-test-suite-baseline.md`: prepend an "Updated by v331 (#27/#283, 2026-07-23 UTC): `parser` flipped to PASS (0-diff). Summary PASS 19→20, FAIL 63→62." note.
  - `project_huck_iterations.md` + `MEMORY.md`: record v331 (parser flip; the four divergences; the durable lesson — a near-miss category can hide a driver-semantics root behind cosmetic error-shape diffs).

- [ ] **Step 7: fmt + commit**
```bash
cargo fmt --all
git add crates/huck-engine/src/builtins.rs tests/scripts/parser_syntax_errors_diff_check.sh docs/bash-test-suite-baseline.md
git commit -m "$(cat <<'EOF'
v331: non-interactive syntax error aborts the parse-context; flips parser to PASS (#283)

A regular parse error in a `-c` string / script file / sourced-file remainder now
aborts that non-interactive parse-context (rc 2) instead of skipping the bad line
and resuming — matching bash. A sourced file's parent still continues; interactive
recovery is preserved. Completes the parser bash-suite category flip (13 -> 0 diff,
Summary PASS 19->20).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```
(Memory files live outside the repo — update them in the same work session, not this commit.)

---

## Self-Review

- **Spec coverage:** fix 1 (Task 2), fixes 2+3 (Task 1), fix 4 (Task 3); harness (Task 1, extended 2+3); category flip + regressions (Task 3). All four spec divergences map to a task.
- **Placeholders:** none — every edit carries exact code. The `-p huck` integration-binary names in Task 3 Step 5 are flagged best-effort (enumerate from `tests/` — the real names, not guesses, must run).
- **Type consistency:** `ParseError::Unexpected(iter.unexpected_here(None)?)`; `iter.peek_kind()? -> Option<TokenKind>`; `Operator::LParen`; `clause.line: u32`; `shell.line_base() -> u32`; `ExecOutcome::Continue(2)`.
- **Scope:** four surgical edits only; no syntax-error-recovery redesign; the shared `expect_or_recover` change is guarded (`peek_kind().is_some()`) to preserve EOF recovery — the review must confirm the `if`/`while`/`case`/brace callers still behave.
