# Test-Module Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Relocate the inline `#[cfg(test)] mod …` blocks out of the six largest source files into per-module direct-child files, with zero behavior change and an unchanged test set.

**Architecture:** Each *top-level* `#[cfg(test)] mod <name> { … }` block in a source file `foo.rs` is replaced by a one-line `#[cfg(test)] mod <name>;` and its body is moved verbatim into `foo/<name>.rs`. Because each relocated module remains a **direct child of its original parent**, the `use super::*;` at the top of every block resolves unchanged — the move needs no import edits. `#[cfg(test)]`-gated *non-module* items (helper fns / test-only `impl` blocks) stay in the code file.

**Tech Stack:** Rust (2021), cargo. Crates: `huck-syntax` (parser), `huck-engine` (builtins/executor/expand/shell_state/arith).

**Issue:** [#104](https://github.com/jdstanhope/huck/issues/104). **Spec:** `docs/superpowers/specs/2026-07-10-test-module-extraction-design.md`.

## Global Constraints

- **Pure refactor.** No production code, no test code, no symbol name changes. Test bodies move byte-for-byte.
- **Only `#[cfg(test)] mod …` blocks move.** These `#[cfg(test)]`-gated *non-module* root items STAY in their code file: `builtins.rs::run_declaration_builtin_strs`, `expand.rs::glob_expand_fields` + test-only `impl Field`, `shell_state.rs` test-only `impl Shell`.
- **No `--workspace`** — it OOM-kills this box. Test per-crate with `--jobs 1 --lib -- --test-threads 1`.
- **Acceptance oracle:** the per-crate test count and pass/fail breakdown must be **identical before and after** every task. A delta means a module was dropped or a `cfg`/import broke.
- **Formatting:** `cargo fmt --all` before every commit; `cargo fmt --all --check` must be clean (CI enforces it).
- **Commit trailer** on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Branch:** all work on `v278-test-module-extraction` (do not commit to `main`).

## The extraction recipe (applies to every module in Tasks 1–6)

For a top-level block in `crates/<crate>/src/<file>.rs`:

```rust
#[cfg(test)]
mod <name> {
    use super::*;
    …body…
}
```

1. Create `crates/<crate>/src/<file>/<name>.rs` containing exactly the **body** (every line strictly between the module's opening `{` and its matching closing `}` at column 0). The body already begins with `use super::*;` — leave it as-is.
2. In `<file>.rs`, replace the entire block (from the `#[cfg(test)]` line through the closing `}` at column 0) with:
   ```rust
   #[cfg(test)]
   mod <name>;
   ```
3. The `mod <name>;` declarations may sit wherever the original blocks were, or be grouped together — placement inside `<file>.rs` is irrelevant to resolution. Keep them where the blocks were to minimize diff noise.

No edit to any crate `lib.rs` is required: these are child modules of `<file>`, declared inside `<file>.rs`, not new top-level crate modules.

---

### Task 1: Extract `arith.rs` tests (recipe shakedown)

Smallest engine file, single module — proves the recipe and captures the `huck-engine` baseline.

**Files:**
- Modify: `crates/huck-engine/src/arith.rs` (remove `mod tests` body @ ~1296)
- Create: `crates/huck-engine/src/arith/tests.rs`

**Interfaces:**
- Consumes: nothing. Produces: no API change; establishes the recorded `huck-engine` lib test count reused by Tasks 3–6.

- [ ] **Step 1: Record the huck-engine baseline test count**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -5`
Record the exact summary line, e.g. `test result: ok. 1773 passed; 0 failed; …`. Note the passed count — call it `ENGINE_N`. This is the invariant for Tasks 1, 3, 4, 5, 6.

- [ ] **Step 2: Move the module**

Apply the recipe to `mod tests` in `crates/huck-engine/src/arith.rs` → `crates/huck-engine/src/arith/tests.rs`. In `arith.rs` the block becomes:
```rust
#[cfg(test)]
mod tests;
```

- [ ] **Step 3: Format**

Run: `cargo fmt --all`

- [ ] **Step 4: Verify test count unchanged**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -5`
Expected: `passed` count equals `ENGINE_N`; `0 failed`.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/arith.rs crates/huck-engine/src/arith/tests.rs
git commit -m "$(printf 'refactor: extract arith.rs unit tests into arith/tests.rs (#104)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: Extract `parser.rs` tests

Single module; captures the `huck-syntax` baseline.

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` (remove `mod tests` body @ ~5258)
- Create: `crates/huck-syntax/src/parser/tests.rs`

**Interfaces:**
- Consumes: nothing. Produces: no API change; establishes `SYNTAX_N`.

- [ ] **Step 1: Record the huck-syntax baseline test count**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 2>&1 | tail -5`
Record the passed count — call it `SYNTAX_N` (~441).

- [ ] **Step 2: Move the module**

Apply the recipe to `mod tests` in `crates/huck-syntax/src/parser.rs` → `crates/huck-syntax/src/parser/tests.rs`. Replace the block with `#[cfg(test)]\nmod tests;`.

- [ ] **Step 3: Format**

Run: `cargo fmt --all`

- [ ] **Step 4: Verify test count unchanged**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 2>&1 | tail -5`
Expected: `passed` equals `SYNTAX_N`; `0 failed`.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-syntax/src/parser.rs crates/huck-syntax/src/parser/tests.rs
git commit -m "$(printf 'refactor: extract parser.rs unit tests into parser/tests.rs (#104)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 3: Extract `shell_state.rs` tests (5 modules)

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs`
- Create (5): `crates/huck-engine/src/shell_state/{tests,array_value_tests,assoc_value_tests,ifs_helper_tests,shopt_tests}.rs`

**Do NOT move:** the `#[cfg(test)] impl Shell` block @ ~3247 — it stays in `shell_state.rs`.

**Interfaces:** Consumes `ENGINE_N` (Task 1). Produces: no API change.

- [ ] **Step 1: Move all 5 modules**

Apply the recipe to each top-level test module in this order: `tests` (@~3271), `array_value_tests` (@~3906), `assoc_value_tests` (@~4033), `ifs_helper_tests` (@~4243), `shopt_tests` (@~4268). Leave the `impl Shell` gated block untouched.

- [ ] **Step 2: Format**

Run: `cargo fmt --all`

- [ ] **Step 3: Verify test count unchanged**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -5`
Expected: `passed` equals `ENGINE_N`; `0 failed`.

- [ ] **Step 4: Commit**

```bash
git add crates/huck-engine/src/shell_state.rs crates/huck-engine/src/shell_state/
git commit -m "$(printf 'refactor: extract shell_state.rs unit tests into shell_state/*.rs (#104)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 4: Extract `expand.rs` tests (5 modules)

**Files:**
- Modify: `crates/huck-engine/src/expand.rs`
- Create (5): `crates/huck-engine/src/expand/{tests,array_expansion_tests,positional_slicing_tests,assoc_expansion_tests,ifs_splitter_tests}.rs`

**Do NOT move:** the `#[cfg(test)] pub fn glob_expand_fields` @ ~2350 and the `#[cfg(test)] impl Field` @ ~2418 — both stay in `expand.rs`.

**Interfaces:** Consumes `ENGINE_N`. Produces: no API change.

- [ ] **Step 1: Move all 5 modules**

Apply the recipe to each top-level test module: `tests` (@~2437), `array_expansion_tests` (@~4049), `positional_slicing_tests` (@~4280), `assoc_expansion_tests` (@~4366), `ifs_splitter_tests` (@~4546). Leave `glob_expand_fields` and `impl Field` untouched.

- [ ] **Step 2: Format**

Run: `cargo fmt --all`

- [ ] **Step 3: Verify test count unchanged**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -5`
Expected: `passed` equals `ENGINE_N`; `0 failed`.

- [ ] **Step 4: Commit**

```bash
git add crates/huck-engine/src/expand.rs crates/huck-engine/src/expand/
git commit -m "$(printf 'refactor: extract expand.rs unit tests into expand/*.rs (#104)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 5: Extract `executor.rs` tests (9 modules)

**Files:**
- Modify: `crates/huck-engine/src/executor.rs`
- Create (9): `crates/huck-engine/src/executor/{tests,array_assign_tests,assoc_assign_tests,coproc_name_tests,arith_for_tests,loop_levels_executor_tests,select_menu_tests,g3_dbracket_extglob_noshopt_tests,errexit_andor_tests}.rs`

**Interfaces:** Consumes `ENGINE_N`. Produces: no API change.

- [ ] **Step 1: Move all 9 modules**

Apply the recipe to each: `tests` (@~8884), `array_assign_tests` (@~11058), `assoc_assign_tests` (@~11303), `coproc_name_tests` (@~11463), `arith_for_tests` (@~11500), `loop_levels_executor_tests` (@~11564), `select_menu_tests` (@~11736), `g3_dbracket_extglob_noshopt_tests` (@~11835), `errexit_andor_tests` (@~11879).

- [ ] **Step 2: Format**

Run: `cargo fmt --all`

- [ ] **Step 3: Verify test count unchanged**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -5`
Expected: `passed` equals `ENGINE_N`; `0 failed`.

- [ ] **Step 4: Commit**

```bash
git add crates/huck-engine/src/executor.rs crates/huck-engine/src/executor/
git commit -m "$(printf 'refactor: extract executor.rs unit tests into executor/*.rs (#104)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 6: Extract `builtins.rs` tests (36 modules)

The largest file. 36 top-level test modules move to `builtins/<name>.rs`.

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs`
- Create (36): `crates/huck-engine/src/builtins/<name>.rs` for each name below.

**Do NOT move:** the `#[cfg(test)] pub(crate) fn run_declaration_builtin_strs` @ ~352 — it stays in `builtins.rs`.

**Interfaces:** Consumes `ENGINE_N`. Produces: no API change.

- [ ] **Step 1: Move all 36 modules**

Apply the recipe to each top-level test module, keeping its name:
`tests`, `fg_bg_tests`, `kill_tests`, `cd_pwd_tests`, `disown_tests`, `history_tests`, `special_builtin_tests`, `alias_tests`, `shift_tests`, `set_tests`, `source_tests`, `local_tests`, `colon_tests`, `true_false_tests`, `command_tests`, `readonly_tests`, `read_tests`, `printf_tests`, `exit_tests`, `type_tests`, `hash_tests`, `dirstack_tests`, `declare_tests`, `integer_attr_tests`, `eval_tests`, `help_tests`, `set_options_tests`, `array_declare_tests`, `assoc_declare_tests`, `loop_levels_tests`, `pipefail_option_tests`, `getopts_step_tests`, `umask_tests`, `ulimit_tests`, `enable_tests`, `normalize_logical_tests`.

Note: `getopts_step_tests` uses `use super::getopts_step;` and `normalize_logical_tests` uses `use super::normalize_logical;` — these `super::`-relative imports resolve to `builtins` unchanged after the move; leave them as written.

- [ ] **Step 2: Format**

Run: `cargo fmt --all`

- [ ] **Step 3: Verify test count unchanged**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -5`
Expected: `passed` equals `ENGINE_N`; `0 failed`.

- [ ] **Step 4: Sanity-check the file shrank and holds only code + `mod X;` lines**

Run: `wc -l crates/huck-engine/src/builtins.rs`
Expected: ~9,300 lines (down from 16,107). No `#[cfg(test)] mod <name> {` block bodies remain — only `#[cfg(test)]\nmod <name>;` declarations plus the retained `run_declaration_builtin_strs` helper.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/builtins.rs crates/huck-engine/src/builtins/
git commit -m "$(printf 'refactor: extract builtins.rs unit tests into builtins/*.rs (#104)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 7: Docs note + final whole-tree verification

**Files:**
- Modify: `docs/architecture.md`

**Interfaces:** Consumes `SYNTAX_N`, `ENGINE_N`. Produces: none.

- [ ] **Step 1: Add a one-line note to architecture.md**

In `docs/architecture.md`, near the Module map (around the `builtins.rs`/`executor.rs`/`parser.rs` rows, ~line 100), add a sentence noting the convention, e.g.:

```markdown
> Unit tests live in per-module sibling files: `<file>/tests.rs` (and, for
> files with several test modules such as `builtins.rs`/`executor.rs`, one
> `<file>/<name>_tests.rs` per module). Production symbols named in this map
> are unaffected.
```

- [ ] **Step 2: Full fmt check**

Run: `cargo fmt --all --check`
Expected: no output (clean).

- [ ] **Step 3: Build the binary**

Run: `cargo build -p huck`
Expected: builds green.

- [ ] **Step 4: Both crates' tests, final confirmation**

Run:
```bash
cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
```
Expected: `passed` equals `SYNTAX_N` and `ENGINE_N` respectively; `0 failed` both.

- [ ] **Step 5: Commit**

```bash
git add docs/architecture.md
git commit -m "$(printf 'docs: note per-module test-file layout in architecture.md (#104)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Self-review notes

- **Spec coverage:** every source file in the spec's inventory table has a task (arith→T1, parser→T2, shell_state→T3, expand→T4, executor→T5, builtins→T6); the "what stays put" gated items are called out as *Do NOT move* in T3/T4/T6; the docs note is T7. The 57-file total = 1+1+5+5+9+36.
- **Line numbers are advisory** — they will drift as blocks are removed; locate modules by name (`#[cfg(test)]\nmod <name>`), not by line.
- **Order rationale:** simplest-first (single-module arith/parser establish the recipe and the two crate baselines) before the multi-module files, with the 36-module builtins last.
- **Risk:** the only failure modes are (a) an off-by-one on a closing brace when cutting a body, or (b) dropping a module — both caught immediately by the per-task test-count assertion and `cargo build`.
