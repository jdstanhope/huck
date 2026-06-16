# v164: Rc-COW vars table — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wrap `Shell::vars` in `Rc` with copy-on-write so `Shell::clone()` (run for every `$(...)`) stops deep-copying the entire variable table.

**Architecture:** Change the field `vars: HashMap<String, Variable>` to `vars: Rc<HashMap<String, Variable>>`, mirroring the four tables already wrapped (`functions`/`history`/`command_hash`/`completion_specs`). Reads pass through `Rc`'s `Deref` unchanged; the ~41 mutating sites go through a new private `vars_mut()` helper that calls `Rc::make_mut` (O(1) when uniquely owned, copies only when shared — i.e. lazily on the first write inside an active `$()`). Copy-on-write preserves `$()` subshell isolation: the clone's first var write copies the map, leaving the parent's untouched.

**Tech Stack:** Rust (edition 2024), `std::rc::Rc`, `std::collections::HashMap`. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-06-16-rc-cow-vars-design.md`

**Branch:** `v164-rc-cow-vars`

---

## Background the implementer needs

- `vars` is a **private** field of `Shell` (`src/shell_state.rs:381`). Every access is inside `src/shell_state.rs` (verified: no `self.vars.` mutating sites exist in any other file). So this change is fully contained to one file plus one new test.
- `Shell` derives `Clone` (`#[derive(Debug, Clone)]`, `src/shell_state.rs:379`). The hot clone site is `let mut cloned = shell.clone();` in `run_substitution` (`src/expand.rs:1256`), run for every command substitution.
- The four existing Rc-COW tables show the exact pattern. Example field doc (`functions`, ~`src/shell_state.rs:400`):
  ```rust
  /// Wrapped in `Rc` for copy-on-write: `clone()` (used for every
  /// `$(…)` subshell isolation) is O(1) — just a refcount bump. A
  /// write (`define_function`/`remove_function`) calls `Rc::make_mut`,
  /// which copies the map only when the `Rc` is shared. huck is
  /// single-threaded so `Rc` (not `Arc`) is correct here.
  pub functions: Rc<HashMap<String, Box<crate::command::Command>>>,
  ```
  Example write through `Rc::make_mut` (`src/shell_state.rs:2057`): `Rc::make_mut(&mut self.functions).insert(name, body);`
- `Rc` is already imported in `src/shell_state.rs` (used by the four existing tables — `Rc::make_mut`/`Rc::new` appear throughout). No new `use` needed; if in doubt, fully-qualify as `std::rc::Rc`.
- The constructor `Shell::new()` (`src/shell_state.rs:673`) builds a **local** `let mut vars = HashMap::new();` (line 674), populates it from `std::env::vars()` in a loop (`vars.insert(...)` at ~line 695 — these are on the **local** binding, not `self.vars`), then moves it into the struct via field-shorthand `vars,` (in the `let mut shell = Self { vars, ... }` literal at ~line 712). `impl Default for Shell` (line 2454) just calls `Self::new()`, so it needs no separate change.

---

### Task 1: COW-isolation guard test

This characterization test asserts that a cloned `Shell` does not share mutable variable state with its parent — the invariant the whole refactor must preserve. It **passes on the current (eager-clone) code** and must keep passing after the refactor; its job is to fail loudly if the COW logic is ever wrong.

**Files:**
- Modify (add a test): `src/shell_state.rs` — inside the existing `#[cfg(test)] mod tests { ... }` block (tests start after line ~2540; add the new `#[test]` fn among them).

- [ ] **Step 1: Add the guard test**

Add this `#[test]` function inside the existing `#[cfg(test)] mod tests` module in `src/shell_state.rs` (the module already has `use super::*;`, so `Shell` is in scope):

```rust
    #[test]
    fn cloned_shell_var_writes_do_not_leak_to_parent() {
        // COW isolation: Shell::clone() (run for every `$()` subshell) must not
        // share mutable variable state with its parent. This holds for both the
        // eager-clone and the Rc::make_mut implementations and guards the v164
        // refactor — it covers the insert, overwrite (get_mut), and remove paths.
        let mut parent = Shell::new();
        parent.set("x", "outer".to_string()); // will be overwritten in the child
        parent.set("keep", "kept".to_string()); // will be removed in the child

        let mut child = parent.clone();
        child.set("x", "inner".to_string()); // overwrite an existing var
        child.set("y", "new".to_string()); // insert a brand-new var
        child.unset("keep"); // remove an existing var

        // The parent is completely untouched by the child's writes.
        assert_eq!(parent.get("x"), Some("outer"));
        assert_eq!(parent.get("keep"), Some("kept"));
        assert_eq!(parent.get("y"), None);

        // The child sees exactly its own writes.
        assert_eq!(child.get("x"), Some("inner"));
        assert_eq!(child.get("y"), Some("new"));
        assert_eq!(child.get("keep"), None);
    }
```

- [ ] **Step 2: Run the test — it must PASS on the current code**

Run: `cargo test --lib cloned_shell_var_writes_do_not_leak_to_parent`
Expected: `test result: ok. 1 passed`. (It passes because the current eager `clone()` already isolates; this is a guard, not a red-green test.)

- [ ] **Step 3: Commit**

```bash
git add src/shell_state.rs
git commit -m "test: guard $() clone variable isolation (pre-v164-refactor)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Wrap `vars` in `Rc` with copy-on-write

This is a single atomic change: the field type, the constructor, the `vars_mut()` helper, and every mutating call site must all change together for the crate to compile (changing the field type alone breaks every `self.vars.insert/get_mut/remove`).

**Files:**
- Modify: `src/shell_state.rs` — field decl (line 381), constructor struct literal (~line 712), add `vars_mut()` helper, and the ~41 mutating sites between lines ~1181 and ~2483.

- [ ] **Step 1: Change the field type + add the doc comment**

Replace (`src/shell_state.rs:381`):
```rust
    vars: HashMap<String, Variable>,
```
with:
```rust
    /// The shell variable table. Wrapped in `Rc` for copy-on-write:
    /// `clone()` (run for every `$(…)` subshell isolation in
    /// `run_substitution`) is O(1) — a refcount bump — instead of a deep
    /// copy of every `Variable`/`VarValue`. Mutating access goes through
    /// `vars_mut()` (`Rc::make_mut`), which copies the map only when the
    /// `Rc` is shared — i.e. lazily, on the first write inside an active
    /// `$()`. huck is single-threaded so `Rc` (not `Arc`) is correct.
    vars: std::rc::Rc<HashMap<String, Variable>>,
```

- [ ] **Step 2: Wrap the table when moving it into the struct in `Shell::new`**

The constructor builds a local `vars: HashMap` and moves it in via field shorthand. Keep the local construction (the `std::env::vars()` loop) exactly as-is, and change only the struct-literal field. In `Shell::new()` (`src/shell_state.rs`, the `let mut shell = Self {` literal at ~line 712), replace the shorthand line:
```rust
            vars,
```
with:
```rust
            vars: std::rc::Rc::new(vars),
```
(The local `let mut vars = HashMap::new();` at line 674 and the `vars.insert(...)` loop populating it stay unchanged — they operate on the local plain `HashMap` before it is wrapped.)

- [ ] **Step 3: Add the private `vars_mut()` helper**

Add this method inside `impl Shell { ... }`, immediately **after** the `get` accessor (`src/shell_state.rs:779-781`, the `pub fn get(&self, name: &str) -> Option<&str>` method), so reads and the COW writer sit together:

```rust
    /// Copy-on-write mutable access to the variable table. `Rc::make_mut` is
    /// O(1) when the `Rc` is uniquely owned (the normal case) and clones the
    /// map only when it is shared — i.e. lazily, on the first write inside an
    /// active `$(…)` substitution, which is exactly the subshell-isolation
    /// boundary. All in-file `self.vars.{insert,get_mut,remove}` writes route
    /// through this; reads use `self.vars` directly (via `Rc` `Deref`).
    fn vars_mut(&mut self) -> &mut HashMap<String, Variable> {
        std::rc::Rc::make_mut(&mut self.vars)
    }
```

- [ ] **Step 4: Convert every mutating site to `vars_mut()`**

Mechanical transform across `src/shell_state.rs`: every `self.vars.insert(`, `self.vars.get_mut(`, and `self.vars.remove(` becomes `self.vars_mut().insert(`, `self.vars_mut().get_mut(`, `self.vars_mut().remove(`. Leave **read** sites (`self.vars.get(`, `self.vars.keys(`, `self.vars.contains_key(`, `self.vars.iter(`) unchanged.

Representative before → after (apply the same rule to all ~41 sites):
```rust
// ~line 1187
self.vars.insert(                  →  self.vars_mut().insert(
// ~line 1181
match self.vars.get_mut(name) {    →  match self.vars_mut().get_mut(name) {
// ~line 1217
self.vars.remove(name);            →  self.vars_mut().remove(name);
// ~line 1430 (inside an `if let ... && let Some(v) = ...` chain)
&& let Some(v) = self.vars.get_mut(name)  →  && let Some(v) = self.vars_mut().get_mut(name)
// ~line 2019-2021
self.vars.remove("FUNCNAME");      →  self.vars_mut().remove("FUNCNAME");
```
The complete set of lines to convert (current line numbers; all are `self.vars.{insert,get_mut,remove}`): 1181, 1187, 1205, 1217, 1260, 1263, 1271, 1287, 1290, 1371, 1374, 1382, 1403, 1419, 1430, 1438, 1443, 1457, 1486, 1667, 1670, 1693, 1696, 1713, 1727, 1730, 1753, 1756, 1851, 1864, 1922, 1988, 2003, 2019, 2020, 2021, 2046, 2166, 2247, 2278, 2483.

- [ ] **Step 5: Verify no mutating site was missed**

Run: `grep -nE 'self\.vars\.(insert|get_mut|remove)\b' src/shell_state.rs`
Expected: **no output** (every mutating site now goes through `self.vars_mut()`). If any line prints, convert it and re-run.

Also confirm the reads were left alone:
Run: `grep -cE 'self\.vars\.(get|keys|contains_key|iter)\b' src/shell_state.rs`
Expected: a non-zero count (≈24) — these are correct as-is.

- [ ] **Step 6: Build**

Run: `cargo build`
Expected: `Finished` with no errors. (If a borrow error appears at a `get_mut` chain, it is because `vars_mut()` takes `&mut self` for the duration of the returned borrow — the existing `self.vars.get_mut` sites had the same borrow shape, so a clean conversion compiles unchanged. Do not restructure logic; just ensure the `self.vars.X` → `self.vars_mut().X` substitution is exact.)

- [ ] **Step 7: Run the guard test + variable/subshell tests**

Run: `cargo test --lib cloned_shell_var_writes_do_not_leak_to_parent`
Expected: `test result: ok. 1 passed`.

- [ ] **Step 8: Run the full unit + integration suite**

Run: `cargo test`
Expected: exit 0, no `test result: FAILED`, no `panicked at`. (The lib unit-test binary should report ≈2192 passed, plus the integration binaries.)

- [ ] **Step 9: Run clippy**

Run: `cargo clippy --lib --bins --quiet`
Expected: exit 0, no warnings. (In particular no `clippy::clone_on_ref_ptr` or borrow lints from the new helper.)

- [ ] **Step 10: Run the bash-diff harnesses**

Run: `p=0; f=0; for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 && p=$((p+1)) || { f=$((f+1)); echo "FAIL $s"; }; done; echo "$p passed, $f failed"`
Expected: `91 passed, 0 failed`.

- [ ] **Step 11: Perf sanity check (record the number; not a committed test)**

Confirm a command-substitution-heavy fragment over a populated variable table runs fast (the structural win is O(1) clone vs deep copy):

Run:
```bash
time ./target/debug/huck -c 'for i in $(seq 1 300); do v$i=$i; done; for i in $(seq 1 5000); do x=$(:); done; echo done'
```
Expected: prints `done` and completes quickly (well under a second on a debug build). Record the wall-clock in the commit message. (A rigorous before/after against the parent commit is done at final branch review by the orchestrator; this step only confirms no accidental pathological slowdown.)

- [ ] **Step 12: Commit**

```bash
git add src/shell_state.rs
git commit -m "v164: Rc-COW the vars table

Wrap Shell.vars in Rc<HashMap<String, Variable>> with copy-on-write via a
private vars_mut() (Rc::make_mut) helper, mirroring the functions/history/
command_hash/completion_specs tables. Shell::clone() (run for every \$())
no longer deep-copies the variable table; the copy happens lazily only on
the first var write inside an active \$() substitution, which is exactly the
subshell-isolation boundary. Reads pass through Rc Deref unchanged.

Behavior-preserving (guard test + full suite + 91 harnesses + clippy green).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final review (orchestrator, after both tasks)

- Whole-branch diff review: confirm only `src/shell_state.rs` changed (field type, constructor wrap, `vars_mut()` helper, mutating-site conversions, guard test); no behavior logic was altered.
- Re-run `grep -nE 'self\.vars\.(insert|get_mut|remove)\b' src/shell_state.rs` → must be empty.
- Rigorous perf confirmation: build the parent-commit binary in a throwaway worktree and time the Step-11 fragment on both, recording the before/after wall-clock (expected: clear reduction scaling with the variable-table size). For the record/iteration notes only.
- Merge `v164-rc-cow-vars` to main `--no-ff` after user confirmation (AskUserQuestion); push; delete the local branch.
- Update `project_huck_iterations.md` + `MEMORY.md` and the architecture-review memory (mark improvement #2 of the sequence done).

---

## Self-review (plan vs spec)

- **Spec coverage:** field-type change + `Rc::new` in constructor (Task 2 Steps 1-2) ✓; `vars_mut()` helper + write-site conversion + reads-unchanged (Task 2 Steps 3-5) ✓; COW-isolation correctness guarded (Task 1) ✓; full suite + harnesses + clippy (Task 2 Steps 7-10) ✓; perf measurement (Task 2 Step 11 + final review) ✓; out-of-scope fields untouched (only `vars` is converted) ✓.
- **Placeholder scan:** none — every code step shows exact code; the conversion lists all line numbers and a zero-result verification grep.
- **Type consistency:** `vars: std::rc::Rc<HashMap<String, Variable>>`, `vars_mut(&mut self) -> &mut HashMap<String, Variable>`, and `Rc::make_mut(&mut self.vars)` are consistent across all steps; the guard test uses the confirmed `set`/`get`/`unset` signatures.
