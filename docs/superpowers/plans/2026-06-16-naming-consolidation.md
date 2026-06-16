# v170: naming consolidation (low-risk batch) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Consolidate a handful of inconsistent function names onto the project's conventions, and document those conventions — pure refactor, no behavior change.

**Architecture:** A sequence of compiler-verified renames (each scoped by word-boundary / unique-full-name `replace_all` so it never touches the unrelated `resolve_spec_or_error` or builtin-dispatch strings), plus a "Naming conventions" doc section. Verification is the Rust compiler (every code reference) + the full bash-diff suite (behavior unchanged).

**Tech Stack:** Rust (edition 2024). No new deps, no new tests.

**Spec:** `docs/superpowers/specs/2026-06-16-naming-consolidation-design.md`

**Branch:** `v170-naming-consolidation`

**Note on method:** there is no TDD red/green here — these are pure renames with no behavior change. The verification at each step is `cargo build` (the compiler flags any missed reference) and, at the end, the full unit+integration suite + all 93 harnesses + clippy (behavior unchanged). rust-analyzer's semantic rename is unavailable (its LSP server crashes in this env), so renames use exact-string `replace_all` constrained to safe targets as detailed below.

---

### Task 1: `read_*` → `scan_*` (lexer.rs)

All 8 are private to `src/lexer.rs` (verified: no references in other files), and each full name is unique (none is a prefix of another), so a per-name `replace_all` within `lexer.rs` is precise.

**Files:** Modify `src/lexer.rs`.

- [ ] **Step 1: Apply the 8 renames**

In `src/lexer.rs`, do these 8 exact-string `replace_all` substitutions (full identifier each time, so no over-match):
- `read_dollar_expansion` → `scan_dollar_expansion`
- `read_ansi_c_quoted` → `scan_ansi_c_quoted`
- `read_var_name` → `scan_var_name`
- `read_braced_param_expansion` → `scan_braced_param_expansion`
- `read_subscript` → `scan_subscript`
- `read_array_literal` → `scan_array_literal`
- `read_array_element_word` → `scan_array_element_word`
- `read_braced_name` → `scan_braced_name`

- [ ] **Step 2: Confirm no `read_*` span-scanner names remain and it builds**

Run: `grep -nE '\bread_(dollar_expansion|ansi_c_quoted|var_name|braced_param_expansion|subscript|array_literal|array_element_word|braced_name)\b' src/lexer.rs || echo "none remain"`
Expected: `none remain`.
Run: `cargo build 2>&1 | tail -2`
Expected: `Finished` (the compiler confirms every reference — incl. the doc-comments that already said "Scans" now match the name).

- [ ] **Step 3: Tidy the two doc-comments that said "Scans …"**

`scan_subscript` and `scan_array_literal` have doc-comments beginning "Scans …" — now consistent with the name; no edit needed unless a comment still says "read". Quickly check:
Run: `grep -nE 'fn scan_(subscript|array_literal)' src/lexer.rs`
(Informational — confirm the new names; doc bodies are already fine.)

- [ ] **Step 4: Commit**

```bash
git add src/lexer.rs
git commit -m "v170: rename lexer read_* span scanners to scan_*

8 cursor-advancing collectors (read_dollar_expansion, read_ansi_c_quoted,
read_var_name, read_braced_param_expansion, read_subscript, read_array_literal,
read_array_element_word, read_braced_name) renamed to scan_* for one consistent
verb. All private to lexer.rs; pure rename.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `resolve_spec` → `run_spec` and `enumerate_action` → `complete_action` (completion subsystem)

`resolve_spec` is referenced in `src/completion.rs`, `src/completion_spec.rs`, `src/completion_builtins.rs` (calls, an `use` import, and comments). The trap is the unrelated `resolve_spec_or_error` (job-spec) — but that lives in `builtins.rs`/job code, NOT the completion files, so a per-file `replace_all` of `resolve_spec` in just the three completion files is safe. Confirm that first.

**Files:** Modify `src/completion.rs`, `src/completion_spec.rs`, `src/completion_builtins.rs`.

- [ ] **Step 1: Confirm `resolve_spec_or_error` is absent from the three completion files**

Run: `grep -ln 'resolve_spec_or_error' src/completion.rs src/completion_spec.rs src/completion_builtins.rs || echo "absent from all three — safe to replace_all resolve_spec there"`
Expected: `absent from all three …`. (If it unexpectedly appears, STOP and switch to paren+import-anchored edits instead.)

- [ ] **Step 2: Rename `resolve_spec` → `run_spec` in the three completion files**

In each of `src/completion_spec.rs`, `src/completion.rs`, `src/completion_builtins.rs`, `replace_all` `resolve_spec` → `run_spec`. This catches the `fn` definition, all calls, the `use crate::completion_spec::{resolve_spec, …}` import (completion.rs), and the doc/comment mentions. It does NOT collide with the existing `run_spec_with_empty_fallback` (different identifier) or with `run_spec` (none exists yet).

- [ ] **Step 3: Rename `enumerate_action` → `complete_action` in completion_spec.rs**

In `src/completion_spec.rs`, `replace_all` `enumerate_action` → `complete_action` (the only file containing it; unique name).

- [ ] **Step 4: Build**

Run: `grep -rn '\bresolve_spec\b' src/ | grep -v 'resolve_spec_or_error' || echo "no resolve_spec left"`
Expected: `no resolve_spec left`.
Run: `cargo build 2>&1 | tail -2`
Expected: `Finished`. (Confirms the import + every call were renamed; `resolve_spec_or_error` is untouched — verify: `grep -c 'resolve_spec_or_error' src/builtins.rs` is unchanged/nonzero.)

- [ ] **Step 5: Commit**

```bash
git add src/completion.rs src/completion_spec.rs src/completion_builtins.rs
git commit -m "v170: rename resolve_spec -> run_spec, enumerate_action -> complete_action

resolve_spec (the compspec evaluator) joins the existing run_spec_with_empty_fallback
root, freeing resolve_* to mean 'follow indirection'. enumerate_action joins the
complete_* candidate-producer family. \\b-scoped so the unrelated
resolve_spec_or_error (job-spec) is untouched. Pure rename.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: `eval_arith_word` → `eval_substring_index` (param_expansion.rs only)

There are TWO `eval_arith_word`: `expand::eval_arith_word` (the `$(())` evaluator — KEEP) and `param_expansion::eval_arith_word` (the `${var:off:len}` index evaluator that prints `huck: arithmetic:` + sets `$?` — RENAME). They are different operations sharing a name. `param_expansion.rs` references only its own local one (verified: no `crate::expand::eval_arith_word` there), so a `replace_all` scoped to `param_expansion.rs` renames exactly the 3 occurrences (def at ~345 + calls at ~207, ~212) and leaves `expand.rs` untouched.

**Files:** Modify `src/param_expansion.rs`.

- [ ] **Step 1: Rename within param_expansion.rs**

In `src/param_expansion.rs`, `replace_all` `eval_arith_word` → `eval_substring_index`.

- [ ] **Step 2: Build + confirm expand.rs's one is untouched**

Run: `grep -c 'eval_arith_word' src/param_expansion.rs` → expected `0`.
Run: `grep -c 'eval_arith_word' src/expand.rs` → expected unchanged (3: def + 2 calls).
Run: `cargo build 2>&1 | tail -2`
Expected: `Finished`.

- [ ] **Step 3: Commit**

```bash
git add src/param_expansion.rs
git commit -m "v170: disambiguate param_expansion eval_arith_word -> eval_substring_index

The substring \${var:off:len} index evaluator (prints huck: arithmetic: + sets
\$? on error) shared a name with expand::eval_arith_word (the \$(()) evaluator).
They are different operations; rename the param_expansion one to reflect its
role. Not a dedup. Pure rename.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Document the naming conventions (architecture.md)

**Files:** Modify `docs/architecture.md`.

- [ ] **Step 1: Add a "Naming conventions" subsection**

In `docs/architecture.md`, add this subsection immediately after the "Cross-cutting conventions" section (find it with `grep -n '## Cross-cutting conventions' docs/architecture.md` and insert before the next `##`/`---`):

```markdown
## Naming conventions

Function-name verbs follow these conventions (codified v170; see the 2026-06-16
naming review). New code should match them:

- **Retrieval** — `get_*` borrows a stored container (`&T`, e.g. `get_array`);
  `lookup_*` computes one resolved value (owned `Option<String>`, e.g.
  `lookup_var`); `resolve_*` follows indirection to a concrete target
  (namerefs, paths — e.g. `resolve_nameref`, `resolve_dir`).
- **Lexing/scanning** — `scan_*` advances a `CharCursor` and collects a span
  (e.g. `scan_cmdsub_body`, `scan_subscript`); `split_*` partitions an
  already-collected `&str` (e.g. `split_modifier_operand`); `parse_*` produces
  AST/structure from tokens; `tokenize` turns source into tokens. The thin
  `consume_…_verbatim` wrappers re-emit a closing delimiter around a `scan_*`.
- **Execution** — `run_*` executes an AST node/construct (`run_command`,
  `run_pipeline`); `execute*` are the public crate entry points
  (`execute`/`execute_with_sink`/`execute_capturing`); `eval_*` computes a value
  from an expression; `fire_*_trap` runs a trap.
- **Completion** — `complete_*` produces candidates; `run_spec` evaluates a
  registered compspec; `dispatch::resolve` is the top-level completion entry.
- **Options** — option structs are `*Options` (`LexerOptions`, `ShellOptions`,
  `CompOptions`); their bindings/params are abbreviated `opts`.
```

- [ ] **Step 2: Commit**

```bash
git add docs/architecture.md
git commit -m "docs: codify naming conventions in architecture.md (v170)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Full regression (gate)

No behavior change, no new tests — the existing suite + harnesses are the proof that the renames are behavior-neutral.

**Files:** none.

- [ ] **Step 1: Clippy**

Run: `cargo clippy --lib --bins --quiet 2>&1 | grep -E 'warning|error' || echo CLEAN`
Expected: `CLEAN`.

- [ ] **Step 2: Full test suite**

Run: `cargo test >/tmp/v170.log 2>&1; echo "exit: $?"; grep -cE 'test result: FAILED' /tmp/v170.log`
Expected: `exit: 0`, FAILED count `0`.

- [ ] **Step 3: All bash-diff harnesses**

Run: `p=0; f=0; for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 && p=$((p+1)) || { f=$((f+1)); echo "FAIL $s"; }; done; echo "$p passed, $f failed"`
Expected: `93 passed, 0 failed`.

(No commit — regression gate. Any failure ⇒ STOP and investigate before merge.)

---

## Final review (orchestrator, after all tasks)

- Whole-branch diff: `src/lexer.rs`, `src/completion.rs`, `src/completion_spec.rs`, `src/completion_builtins.rs`, `src/param_expansion.rs`, `docs/architecture.md`. Confirm: no `read_<scanner>` / `resolve_spec` / `enumerate_action` remain; `resolve_spec_or_error` and `expand::eval_arith_word` are intact; no builtin-dispatch string was touched (`grep -c '"cd"' src/builtins.rs` unchanged).
- Merge `v170-naming-consolidation` to main `--no-ff` after user confirmation (AskUserQuestion); push; delete the local branch.
- Record the iteration in `project_huck_iterations.md` + `MEMORY.md`; note the deferred mutation-verb consolidation + cosmetic nits remain as future naming work.

---

## Self-review (plan vs spec)

- **Spec coverage:** convention doc (Task 4) ✓; 8 read_*→scan_* (Task 1) ✓; resolve_spec→run_spec with `resolve_spec_or_error` guard (Task 2 Steps 1–2) ✓; enumerate_action→complete_action (Task 2 Step 3) ✓; eval_arith_word→eval_substring_index disambiguation, expand.rs untouched (Task 3) ✓; verification via compiler + clippy + suite + 93 harnesses (per-task builds + Task 5) ✓; scope — mutation verbs + cosmetics deferred, no behavior change, no divergence-doc edit ✓.
- **Placeholder scan:** none — every rename lists exact old→new strings and exact verification greps with expected output.
- **Type consistency:** the rename targets (`scan_*`, `run_spec`, `complete_action`, `eval_substring_index`) are used consistently; the two `eval_arith_word` are explicitly kept distinct (only param_expansion's renamed).
