# v348 — flip `exportfunc` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Flip the bash-suite `exportfunc` category to PASS by fixing four real behavioral roots (hyphen-function import, `eval` line-number, HEREDOC_MAX limit, export-name validation).

**Architecture:** Roots are in separate areas: import (`shell_state.rs`), export builtin, heredoc lexer (`lexer.rs`), and the `eval` site.

**Tech Stack:** Rust (huck-engine + huck-syntax crates), bash-diff harness.

**Spec:** `docs/superpowers/specs/2026-07-30-exportfunc-design.md`. **Issue:** [#339](https://github.com/jdstanhope/huck/issues/339).

## Global Constraints

- Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- `cargo fmt --all` before every commit (CI enforces `--check`).
- Branch `v348-exportfunc`; never push to main, never self-merge.
- Build the binary with `cargo build -p huck`. Per-crate lib tests: `cargo test -p huck-engine --lib` / `-p huck-syntax --lib` (`--jobs 1 -- --test-threads 1`). NEVER `cargo test --workspace` (OOM on this 1-core/1.9GB box). Guard sweeps with `ulimit -v 1500000` + `timeout`.
- Bash reference: `bash --norc --noprofile`. `.right` is authoritative (local bash 5.2.21 matches it exactly).
- No behavior change beyond the four roots; the shellshock CVE checks and normal function export/import must stay passing.

---

### Task 1: Root 1 + Root 4 — function-name acceptance on import and export

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` — the BASH_FUNC import loop (~1177) and its `<name>` acceptance; keep `parse_imported_function`'s shellshock body validation (~1004) unchanged.
- Modify: the `export -f` builtin (find it: `grep -rn 'export' crates/huck-engine/src/builtins*.rs | grep -i 'func\|-f'`).
- Test: shell_state tests + engine export tests.

**Interfaces:** `parse_imported_function(name, value) -> Option<Box<Command>>`; `mark_function_exported(name)`; `exported_function_env() -> Vec<(String,String)>`.

- [ ] **Step 1: Reproduce + failing tests.**

```
# Root 1: round-trip a hyphen-named function through the env
foo-a() { echo "ok2"; }; export -f foo-a; $HUCK -c 'foo-a'   # bash: ok2   huck: command not found
# Root 4: reject un-encodable export names
export -f foo=bar    # bash: export: foo=bar: cannot export (rc 1)   huck: accepts (rc 0)
```

Add tests: (a) an exported hyphen function is importable/callable in a child (or unit-test `parse_imported_function`/import-name acceptance directly with `BASH_FUNC_foo-a%%`); (b) `export -f` of a name containing `=` errors `export: <name>: cannot export` rc 1; (c) shellshock body (`() { :; }; touch x`) still rejected on import (regression).

- [ ] **Step 2: Run, verify fail.**

- [ ] **Step 3: Implement.** Root 1: relax the import `<name>` acceptance to allow any valid bash function name (nonempty, no `=`/`/`/NUL — `-` allowed), NOT only strict identifiers; keep the body-validation shellshock guard. Root 4: in the `export -f` builtin, reject a name that can't be env-encoded (contains `=`) with `export: <name>: cannot export` and exit status 1. Verify vs bash: `export -f foo-a` OK, `export -f foo=bar` rejected, and a normal `export -f myfunc` still works and round-trips.

- [ ] **Step 4: Run tests + hand-check vs bash.**

- [ ] **Step 5: Commit.** (`v348: import/export accept hyphen function names, reject un-encodable names (#339)`, trailer.)

---

### Task 2: Root 3 — enforce HEREDOC_MAX (CVE-2014-7186)

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — the heredoc-opener enqueue into `pending_heredocs` (~1295) / `atom_pending_heredocs`.
- Test: syntax lexer/parser tests.

- [ ] **Step 1: Reproduce + failing test.** `printf 'cat <<EOF <<EOF <<EOF <<EOF <<EOF <<EOF <<EOF <<EOF <<EOF <<EOF <<EOF\n' | bash` — find bash's exact threshold (bash `HEREDOC_MAX`=10; the 11th `<<` triggers `maximum here-document count exceeded`). Add a test asserting huck errors at the same count with the same message.

- [ ] **Step 2: Run, verify fail** (huck currently accepts many heredocs).

- [ ] **Step 3: Implement.** When a heredoc opener would push the pending-heredoc count over the bash limit (10), emit `maximum here-document count exceeded` (with the correct source-line prefix) and fail the parse — matching bash's message, line, and exit behavior. Confirm the threshold and the resulting category output line (`./exportfunc1.sub: line 14: maximum here-document count exceeded`).

- [ ] **Step 4: Run tests + hand-check vs bash** (fewer than the limit still work; exactly the limit works; over errors).

- [ ] **Step 5: Commit.** (trailer.)

---

### Task 3: Root 2 — `eval` syntax-error line-number

**Files:**
- Modify: the `eval` builtin site (parse-error line base for the eval'd string).
- Test: engine eval tests.

- [ ] **Step 1: Reproduce + failing test.** `eval 'X() { (a)>\'` → bash `eval: line 44: syntax error: unexpected end of file`, huck `line 43`. Reproduce minimally (the line number is relative to the eval string / current `$LINENO`). Determine the exact off-by-one. Add a test.

- [ ] **Step 2: Run, verify fail.**

- [ ] **Step 3: Implement.** Align the eval error line base with bash. Investigate whether the offset is eval-specific or a shared string-parse base; fix at the eval site so script/`-c` line-numbering is not disturbed. Verify the exportfunc case AND a couple of standalone `eval '<malformed>'` line numbers vs bash, and that a valid multi-line eval reports correct lines.

- [ ] **Step 4: Run tests + hand-check.**

- [ ] **Step 5: Commit.** (trailer.)

---

### Task 4: `exportfunc` diff-check harness + category-flip verification + docs/memory

**Files:**
- Create: `tests/scripts/exportfunc_diff_check.sh`.
- Modify: `docs/bash-test-suite-baseline.md` (PASS 35 → 36); memory files.

- [ ] **Step 1: Write the harness.** `tests/scripts/exportfunc_diff_check.sh` modeled on `appendop_diff_check.sh`. Where a root needs a child shell, use `$HUCK_BIN -c` directly (the `check()` fragment can `export -f` then re-invoke). Cover: Root 1 hyphen round-trip; Root 4 `export -f foo=bar` reject; Root 3 the 18-`<<EOF` line; Root 2 the eval line-number; plus regressions (normal `export -f`/import round-trip; shellshock body rejected; heredoc count under the limit works). `chmod +x`.

- [ ] **Step 2: Harness green.** `cargo build -p huck`; `ulimit -v 1500000; timeout 120 bash tests/scripts/exportfunc_diff_check.sh`.

- [ ] **Step 3: Official `exportfunc` runner (flip signal).** `BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_BASH_TEST_CATEGORY=exportfunc bash tests/bash-test-suite/runner.sh` → `exportfunc | PASS`, zero diff (also confirm huck output == committed `exportfunc.right`).

- [ ] **Step 4: No-regression sweep.** Build release + debug; `run_diff_checks.sh` green (incl. new harness). Full bash-suite runner: PASS-set == v347 baseline (35) **+ `exportfunc`** = 36, nothing regressed (Root 1 touches env-function import, Root 3 touches the shared heredoc lexer — verify by diffing the PASS-set AND spot-check heredoc/function/comsub-eof categories explicitly). Run `export_f_integration`, `declare_func_export_integration`, heredoc `-p huck` integration bins.

- [ ] **Step 5: Update docs + memory.** `docs/bash-test-suite-baseline.md`: v348 block, PASS 35→36, flip the `exportfunc` row + PASS-list. Append v348 to `project_huck_iterations.md` + a hook to `MEMORY.md` (compact if >~17KB). Commit harness + baseline (memory files are outside the repo — update but don't `git add`).

---

## Self-Review

**Spec coverage:** Root 1+4 → Task 1; Root 3 → Task 2; Root 2 → Task 3; harness + flip + no-regression + docs → Task 4. All covered.

**Placeholder scan:** Task 3 (eval line-number) and Task 2 (HEREDOC_MAX threshold) carry "confirm the exact offset/threshold vs bash" — bounded empirical investigation with concrete reproductions, not placeholders. Root 4's export-builtin location is a `grep` away.

**Consistency:** Root 3 touches the SHARED heredoc lexer (`pending_heredocs`) — Task 4's no-regression sweep (esp. heredoc/comsub-eof) is load-bearing. Root 1's import relaxation must not weaken the shellshock body guard in `parse_imported_function`.
