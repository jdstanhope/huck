# v171: array→indexed noun consolidation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename the six public indexed-array methods from `*_array*` to `*_indexed*` so the indexed/associative method pairs are symmetric and match the internal `Indexed` vocabulary. Pure rename — no behavior change.

**Architecture:** Six word-boundary-anchored `sed` renames across `src/*.rs` (word boundaries make the `set_array_element` ⊂ `unset_array_element` collision a non-issue, so order is irrelevant and no other identifier is affected), then update the `architecture.md` naming-conventions note (which references `get_array`). Verification is the Rust compiler (every reference) + the full bash-diff suite (behavior unchanged).

**Tech Stack:** Rust (edition 2024). No new deps, no new tests.

**Spec:** `docs/superpowers/specs/2026-06-16-array-indexed-noun-design.md`

**Branch:** `v171-array-indexed-noun`

**Method note:** rust-analyzer's semantic rename is unavailable (LSP server crashes in this env). `Edit`'s `replace_all` is exact-substring/per-file (would require careful unset-before-set ordering across ~8 files); GNU `sed`'s `\b…\b` is word-boundary-aware and repo-wide, so it is the correct tool here. `\bset_array_element\b` does NOT match inside `unset_array_element` (the `set` there is preceded by the word char `n`), so each of the six renames is independent and safe. The compiler + harnesses are the verification regardless.

---

### Task 1: Rename the six `*_array*` methods → `*_indexed*`

**Files:** Modify `src/*.rs` (the six methods are referenced across `shell_state.rs`, `executor.rs`, `builtins.rs`, `expand.rs`, `param_expansion.rs`, `arith.rs`, `completion_spec.rs`, `shell.rs`).

- [ ] **Step 1: Confirm none of the six names appear inside a string literal (sed-corruption guard)**

Run:
```bash
grep -rnoE '"[^"]*\b(replace_array|set_array_element|append_array_element|unset_array_element|get_array|lookup_array_element)\b[^"]*"' src/ || echo "no string-literal occurrences — safe to sed"
```
Expected: `no string-literal occurrences — safe to sed`. (If any line prints, STOP — one of these names is in a string and a blanket sed would corrupt it; switch to per-site edits for that case.)

- [ ] **Step 2: Apply the six word-boundary renames**

Run (GNU sed; `\b` word boundaries make the set/unset collision a non-issue and protect any longer identifier):
```bash
cd /home/john/projects/shuck
sed -i 's/\breplace_array\b/replace_indexed/g'              src/*.rs
sed -i 's/\bset_array_element\b/set_indexed_element/g'      src/*.rs
sed -i 's/\bappend_array_element\b/append_indexed_element/g' src/*.rs
sed -i 's/\bunset_array_element\b/unset_indexed_element/g'  src/*.rs
sed -i 's/\bget_array\b/get_indexed/g'                      src/*.rs
sed -i 's/\blookup_array_element\b/lookup_indexed_element/g' src/*.rs
```

- [ ] **Step 3: Confirm no old names remain and the new ones exist**

Run:
```bash
grep -rnE '\b(replace_array|set_array_element|append_array_element|unset_array_element|get_array|lookup_array_element)\b' src/ || echo "no old *_array* method names remain"
```
Expected: `no old *_array* method names remain`.
Run: `grep -rcE '\b(replace_indexed|set_indexed_element|append_indexed_element|unset_indexed_element|get_indexed|lookup_indexed_element)\b' src/shell_state.rs`
Expected: a non-zero count (the new definitions + refs are present).
Also confirm untouched siblings/canonicals are intact:
```bash
grep -rcE '\b(get_associative|set_associative_element|replace_associative|extend_indexed|set_indexed_var)\b' src/shell_state.rs
```
Expected: non-zero (these were not renamed).

- [ ] **Step 4: Build**

Run: `cargo build 2>&1 | tail -2`
Expected: `Finished`. (The compiler confirms every reference across all files was renamed consistently. A leftover or wrong reference would be a build error.)

- [ ] **Step 5: Commit**

```bash
git add -A src/
git commit -m "v171: rename indexed-array methods array -> indexed

replace_array->replace_indexed, set_array_element->set_indexed_element,
append_array_element->append_indexed_element, unset_array_element->
unset_indexed_element, get_array->get_indexed, lookup_array_element->
lookup_indexed_element. Makes the indexed/associative method pairs symmetric and
matches the internal Indexed vocabulary (VarValue::Indexed, store_indexed_*,
extend_indexed). Pure rename; verbs and the assign()/store_ layering untouched.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Update the naming-conventions doc

The `architecture.md` "Naming conventions" section (added v170) uses `get_array` as an example — now stale — and should state the indexed/associative adjective rule.

**Files:** Modify `docs/architecture.md`.

- [ ] **Step 1: Fix the stale `get_array` example and add the adjective rule**

In `docs/architecture.md`, in the "## Naming conventions" section, change the Retrieval bullet's example from `get_array` to `get_indexed`:
- Find: `borrows a stored container (`&T`, e.g. `get_array`);`
- Replace with: `borrows a stored container (`&T`, e.g. `get_indexed`);`

Then append this bullet to the same "Naming conventions" list (after the "Options" bullet):

```markdown
- **Array types** — the two array kinds use `indexed` / `associative` as the
  type adjective (matching `VarValue::Indexed`/`Associative`), e.g.
  `replace_indexed`/`replace_associative`, `set_indexed_element`/
  `set_associative_element`, `get_indexed`/`get_associative`. Avoid the bare
  noun `array` (ambiguous — both kinds are arrays).
```

- [ ] **Step 2: Commit**

```bash
git add docs/architecture.md
git commit -m "docs: note indexed/associative array-type adjective; fix get_array example (v171)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Full regression (gate)

Pure rename — the existing suite + harnesses prove it's behavior-neutral.

**Files:** none.

- [ ] **Step 1: Clippy**

Run: `cargo clippy --lib --bins --quiet 2>&1 | grep -E 'warning|error' || echo CLEAN`
Expected: `CLEAN`.

- [ ] **Step 2: Full test suite**

Run: `cargo test >/tmp/v171.log 2>&1; echo "exit: $?"; grep -cE 'test result: FAILED' /tmp/v171.log`
Expected: `exit: 0`, FAILED count `0`.

- [ ] **Step 3: All bash-diff harnesses**

Run: `p=0; f=0; for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 && p=$((p+1)) || { f=$((f+1)); echo "FAIL $s"; }; done; echo "$p passed, $f failed"`
Expected: `93 passed, 0 failed`.

(No commit — regression gate. Any failure ⇒ STOP and investigate before merge.)

---

## Final review (orchestrator, after all tasks)

- Whole-branch diff: confirm only identifier renames in `src/*.rs` + the `architecture.md` note; no behavior logic changed; `set`/`unset`/`assign`/`get_associative`/`extend_indexed`/`set_indexed_var` and all dispatch strings intact (`grep -c '"cd"' src/builtins.rs` unchanged).
- Re-grep: `grep -rnE '\b(replace_array|set_array_element|append_array_element|unset_array_element|get_array|lookup_array_element)\b' src/ docs/` returns nothing.
- Merge `v171-array-indexed-noun` to main `--no-ff` after user confirmation (AskUserQuestion); push; delete the local branch.
- Record the iteration in `project_huck_iterations.md` + `MEMORY.md`; note the mutation VERBS were confirmed principled (only the noun was inconsistent), so no further mutation-area naming work is warranted beyond the deferred cosmetic nits.

---

## Self-review (plan vs spec)

- **Spec coverage:** the six array→indexed renames (Task 1) ✓; substring-collision safety via `\b` sed (Task 1 method note + Step 2) ✓; target-collision-free (verified in spec; re-confirmed Step 3) ✓; verbs/layering/`set_indexed_var`/associative methods untouched (Step 3 confirmation) ✓; architecture.md adjective note + stale `get_array` example fix (Task 2) ✓; verification compiler + clippy + suite + 93 harnesses (Task 1 Step 4, Task 3) ✓; no behavior change, no divergence-doc edit ✓.
- **Placeholder scan:** none — exact sed commands, exact grep verifications with expected output, exact doc edit.
- **Type consistency:** the six targets (`replace_indexed`/`set_indexed_element`/`append_indexed_element`/`unset_indexed_element`/`get_indexed`/`lookup_indexed_element`) are used consistently across tasks; the untouched set (`set_indexed_var`, `extend_indexed`, `*_associative_*`, `assign`, `store_*`) is explicitly preserved.
