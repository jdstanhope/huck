# v127 — Copy-on-Write `Shell` Clone Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `Shell::clone()` (run on every command substitution) O(1) instead of O(total-data-size), eliminating the ~90× per-`$(…)` deep-copy overhead that makes `nvm ls` 4-20× slower than bash.

**Architecture:** Wrap the large, read-mostly `Shell` tables (`functions`, `command_hash`, `completion_specs`, `history`) in `Rc<…>`. `#[derive(Clone)]` then clones them by refcount bump (O(1)); the rare write inside a substitution uses `Rc::make_mut` (copy-on-write — copies once, only when shared). Substitution isolation is preserved exactly; only the cost drops.

**Tech Stack:** Rust `std::rc::Rc` (huck is single-threaded — only `thread::sleep`, no `thread::spawn` — so `Rc` is safe; fall back to `Arc` only if the compiler demands `Send`).

Spec: `docs/superpowers/specs/2026-06-10-cow-shell-clone-design.md`.

**Conventions:**
- Build/test: `cargo build`/`cargo build --release`; `cargo test`; `cargo clippy --all-targets`.
- Commit trailer EXACTLY (keep "(1M context)"): `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
- Branch: `v127-cow-shell-clone` (from `main` before Task 1).

---

## File Structure

| File | Responsibility | Tasks |
|------|----------------|-------|
| `src/shell_state.rs` | `Rc`-wrap the 4 fields; `Rc::new` in `new()`; `define_function`/`remove_function` helpers; COW guard test | 1, 2 |
| `src/executor.rs` | `Command::FunctionDef` → `define_function` | 1 |
| `src/builtins.rs` | `unset -f` → `remove_function`; convert ~11 test `functions.insert`; `hash` builtin → `make_mut(command_hash)`; `history.clear` → `make_mut` | 1, 2 |
| `src/shell.rs` | `history.add`/`.load` → `make_mut(history)` | 2 |
| `src/completion_builtins.rs`, `src/completion_spec.rs` | `completion_specs` mutations → `make_mut`; test `functions.insert` → `define_function` | 1, 2 |
| `README.md` | optional perf note | 3 |

---

### Task 1: `Rc`-wrap `functions` (the dominant win) + helpers + COW guard

**Files:**
- Modify: `src/shell_state.rs` (`functions` field `:253`; `new()` `:389`; add helpers + test)
- Modify: `src/executor.rs` (`FunctionDef` `:431`)
- Modify: `src/builtins.rs` (`unset -f` `:565`; test inserts)
- Modify: `src/completion_spec.rs` (test inserts `:695-696`)

- [ ] **Step 1: Wrap the `functions` field**

In `src/shell_state.rs`, change the field (`:253`):
```rust
    pub functions: std::rc::Rc<HashMap<String, Box<crate::command::Command>>>,
```
In `new()`'s `Self { … }` literal (`:389`, `functions: HashMap::new(),`):
```rust
            functions: std::rc::Rc::new(HashMap::new()),
```
(If a `use std::rc::Rc;` at the top of the file reads cleaner than fully-qualifying, add it and use `Rc<…>`/`Rc::new`. Either is fine — be consistent.)

- [ ] **Step 2: Add the mutator helpers**

In `src/shell_state.rs`, in an `impl Shell` block (near the other var/array helpers), add:
```rust
    /// Defines (or replaces) a shell function. Copy-on-write: if the function
    /// table is shared (e.g. with a command-substitution clone), this copies it
    /// first so the mutation does not leak across the isolation boundary.
    pub(crate) fn define_function(&mut self, name: String, body: Box<crate::command::Command>) {
        std::rc::Rc::make_mut(&mut self.functions).insert(name, body);
    }

    /// Removes a shell function. Returns true if it existed. Copy-on-write.
    pub(crate) fn remove_function(&mut self, name: &str) -> bool {
        std::rc::Rc::make_mut(&mut self.functions).remove(name).is_some()
    }
```

- [ ] **Step 3: Convert the production write sites**

`src/executor.rs:431` — `Command::FunctionDef`:
```rust
        Command::FunctionDef { name, body } => {
            shell.define_function(name.clone(), body.clone());
            ExecOutcome::Continue(0)
        }
```
`src/builtins.rs:565` — `unset -f` (currently `shell.functions.remove(arg);`):
```rust
            shell.remove_function(arg);
```
(If the surrounding code uses the boolean result, `shell.remove_function(arg)` returns it; otherwise call and ignore.)

Reads are UNCHANGED — `shell.functions.get(…)`, `.contains_key(…)`, `.keys()`, `.iter()`, `.get(…).cloned()` all work through `Rc`'s `Deref` (verified sites: executor.rs:2781/2917/2975/4627, builtins.rs:909/918/5690/5739/5766).

- [ ] **Step 4: Build — find the test write sites that no longer compile**

Run: `cargo build 2>&1 | grep -E "error|functions" | head -40`
Expected: errors at the ~11 TEST sites doing `shell.functions.insert(…)` / `sh.functions.insert(…)` (builtins.rs ~9005/9625/9670/9716/10091/10092/10120; completion_spec.rs 695-696) — `Rc<HashMap>` has no `insert`. Convert each:
```rust
        shell.functions.insert("myfn".to_string(), body);
```
→
```rust
        shell.define_function("myfn".to_string(), body);
```
(For the `.clone()` ones like `functions.insert("fn1", body.clone())`, keep the `body.clone()`: `shell.define_function("fn1".to_string(), body.clone());`.)
Re-run `cargo build` until clean.

- [ ] **Step 5: Add the deterministic COW guard test**

In `src/shell_state.rs`'s `#[cfg(test)] mod tests`, add (use whatever minimal `Box<Command>` the neighboring tests build — e.g. parse a snippet via the crate's parser, or a trivial `Command`; mirror an existing test that constructs a function `body`):
```rust
#[test]
fn shell_clone_shares_functions_and_cow_isolates_defines() {
    use std::rc::Rc;
    let mut a = Shell::new();
    let body = /* a minimal Box<Command> — mirror a neighboring test's body */;
    a.define_function("f".to_string(), body.clone());
    assert_eq!(Rc::strong_count(&a.functions), 1);

    let b = a.clone();
    // clone shares the table — O(1), not a deep copy.
    assert_eq!(Rc::strong_count(&a.functions), 2);

    // Defining on `a` after the clone must COW: `a` gets it, `b` does NOT
    // (this is the $()-isolation guarantee).
    a.define_function("g".to_string(), body);
    assert!(a.functions.contains_key("g"));
    assert!(!b.functions.contains_key("g"));
    assert_eq!(Rc::strong_count(&a.functions), 1); // `a` now owns its copy
}
```
Run: `cargo test --lib shell_clone_shares_functions 2>&1 | tail -8` — PASS.

- [ ] **Step 6: Behavioral check — function-def in `$()` must not leak**

```bash
cargo build 2>&1 | tail -1
printf 'x=$(myf(){ echo hi; }; myf); echo "x=$x"; type myf 2>&1 | head -1\n' > /tmp/leak.sh
echo "--- HUCK ---"; ./target/debug/huck /tmp/leak.sh
echo "--- BASH ---"; bash --norc /tmp/leak.sh
```
Expected (both): `x=hi` then `myf` NOT found / "not found" — the function defined inside `$()` does not leak to the parent. (huck must match bash.)

- [ ] **Step 7: Measure the win (functions alone)**

```bash
cargo build --release 2>&1 | tail -1
printf '. "$HOME/.nvm/nvm.sh"\ni=0; while [ $i -lt 2000 ]; do x=$(true); i=$((i+1)); done; echo done\n' > /tmp/bench_nvm.sh
echo "huck nvm-loaded 2000x \$(true):"; /usr/bin/time -f "  %e s (%U user)" ./target/release/huck /tmp/bench_nvm.sh 2>&1
```
Expected: drops from the pre-fix ~46s toward ~1-3s (functions are the dominant term). Capture the number. (Do NOT source `~/.bashrc` — creds; `~/.nvm/nvm.sh` is fine.)

- [ ] **Step 8: clippy + commit**

`cargo clippy --all-targets 2>&1 | tail -5` (clean — watch for `clippy::rc_buffer` or similar; if it fires on `Rc<HashMap>`, `#[allow]` it with a comment, since the COW is intentional).
```bash
git add src/shell_state.rs src/executor.rs src/builtins.rs src/completion_spec.rs
git commit -m "perf(v127): Rc + copy-on-write for Shell.functions (O(1) clone per \$())

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `Rc`-wrap `command_hash`, `history`, `completion_specs`

**Files:**
- Modify: `src/shell_state.rs` (3 fields + `new()`)
- Modify: `src/builtins.rs` (`hash` builtin; `history.clear`)
- Modify: `src/shell.rs` (`history.add`/`.load`)
- Modify: `src/completion_builtins.rs` (`completion_specs` mutations)

- [ ] **Step 1: Wrap the three fields**

In `src/shell_state.rs`:
- `command_hash` (`:354`):
  ```rust
      pub command_hash: std::rc::Rc<std::collections::HashMap<String, (std::path::PathBuf, u32)>>,
  ```
- `completion_specs` (`:362`):
  ```rust
      pub completion_specs: std::rc::Rc<CompletionSpecs>,
  ```
- `history` (`:263`):
  ```rust
      pub history: std::rc::Rc<crate::history::History>,
  ```
In `new()`: `command_hash: Rc::new(std::collections::HashMap::new()),`, `completion_specs: Rc::new(CompletionSpecs::default()),`, `history: Rc::new(crate::history::History::new()),`.
(`CompletionSpecs` and `History` already derive `Clone`, so `Rc::make_mut` works.)

- [ ] **Step 2: Build — locate write sites**

`cargo build 2>&1 | grep -E "error" | head -40`. Convert each non-compiling mutation to `Rc::make_mut(&mut shell.<field>)…`:

`src/builtins.rs` `hash` builtin:
- `:5964` `shell.command_hash.clear();` → `std::rc::Rc::make_mut(&mut shell.command_hash).clear();`
- `:5975` `shell.command_hash.remove(name)` → `std::rc::Rc::make_mut(&mut shell.command_hash).remove(name)`
- `:5995` / `:6062` `shell.command_hash.insert(…)` → `std::rc::Rc::make_mut(&mut shell.command_hash).insert(…)`

`src/builtins.rs:3855` `shell.history.clear();` → `std::rc::Rc::make_mut(&mut shell.history).clear();`
`src/shell.rs:284` `shell.history.load();` → `std::rc::Rc::make_mut(&mut shell.history).load();`
`src/shell.rs:322` `shell.history.add(history.clone());` → `std::rc::Rc::make_mut(&mut shell.history).add(history.clone());`
(Plus any test sites for history `.add` — e.g. builtins.rs:8270/8271/8284 — convert the same way: `Rc::make_mut(&mut shell.history).add(…)`.)

`src/completion_builtins.rs` — the sites mutating `shell.completion_specs.…` (`:325` `.default_spec =`, `:328` `.empty_spec =`, `:539` `.by_command.get_mut`, and any `.by_command.insert`): change `shell.completion_specs.X` (mutation) to `std::rc::Rc::make_mut(&mut shell.completion_specs).X`. NOTE: `shell.current_completion_spec` is a SEPARATE field (not wrapped) — leave its `.take()`/assignment sites unchanged.

Reads (`shell.command_hash.get`/`.iter`, `shell.history.<immutable>`, `shell.completion_specs.by_command[...]` reads) are unchanged via `Deref`.

- [ ] **Step 3: Build clean + add guard assertions**

`cargo build 2>&1 | tail -3` (clean).
Extend the COW guard test (or add siblings) asserting `Rc::strong_count` for `command_hash`, `completion_specs`, `history` is 2 after `shell.clone()` and that a `hash`/history mutation on one shell doesn't leak to the clone. (Mirror Task 1's pattern.)

- [ ] **Step 4: Behavioral regression for these tables**

```bash
cargo build 2>&1 | tail -1
# hash isolation: a `hash` inside $() doesn't change the parent's hash table
printf 'hash -r; x=$(hash -p /bin/ls myls 2>/dev/null; echo done); hash | grep -c myls || echo "0 (not leaked)"\n' > /tmp/h.sh
./target/debug/huck /tmp/h.sh; echo "(expect 0/not leaked)"
cargo test 2>&1 | grep -E "test result: FAILED|error\[" | head; echo "(none=clean)"
```

- [ ] **Step 5: clippy + commit**

`cargo clippy --all-targets 2>&1 | tail -5` (clean).
```bash
git add src/shell_state.rs src/builtins.rs src/shell.rs src/completion_builtins.rs
git commit -m "perf(v127): Rc + COW for command_hash, history, completion_specs

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Full regression + nvm perf payoff + docs

**Files:**
- Modify: `README.md` (optional perf note)

- [ ] **Step 1: Full behavioral regression**

```bash
cargo test 2>&1 | grep -E "test result: FAILED|error\[" | head    # none
cargo clippy --all-targets 2>&1 | tail -3                         # clean
for h in tests/scripts/*_diff_check.sh; do bash "$h" >/dev/null 2>&1 || echo "FAIL $h"; done  # silent
```
Specifically confirm these suites pass (the COW-isolation-sensitive ones): anything touching function definitions, `unset -f`, `hash`, `complete`/`compgen`, `history`, command substitution.

- [ ] **Step 2: Behavioral isolation spot-checks vs bash**

```bash
# function defined in $() does not leak
printf 'x=$(g(){ echo G; }; g); type g 2>&1|head -1\n' > /tmp/i1.sh
diff <(./target/debug/huck /tmp/i1.sh 2>&1) <(bash --norc /tmp/i1.sh 2>&1) && echo "i1 OK"
# function defined normally IS visible + callable
printf 'f(){ echo F; }; f; f\n' > /tmp/i2.sh
diff <(./target/debug/huck /tmp/i2.sh 2>&1) <(bash --norc /tmp/i2.sh 2>&1) && echo "i2 OK"
```

- [ ] **Step 3: nvm perf payoff (the headline number)**

```bash
cargo build --release 2>&1 | tail -1
printf '. "$HOME/.nvm/nvm.sh"\nnvm ls >/dev/null 2>&1\n' > /tmp/perf.sh
echo "HUCK nvm ls:"; for i in 1 2; do /usr/bin/time -f "  %e s (%U user %S sys)" ./target/release/huck /tmp/perf.sh 2>&1; done
echo "BASH nvm ls:"; for i in 1 2; do /usr/bin/time -f "  %e s (%U user %S sys)" bash --norc /tmp/perf.sh 2>&1; done
```
Expected: huck wall-clock drops from ~26s toward bash's ~6.5s (target: within ~2×; user-CPU no longer dominant). Also re-run the `2000× $(true)` nvm-loaded micro-bench (`/tmp/bench_nvm.sh` from Task 1) — should be ~1-3s now (was ~46s). Capture before/after.

- [ ] **Step 4: README note (optional)**

In `README.md` Status section, you MAY add a brief line that command-substitution-heavy scripts (e.g. `nvm`) now run at near-bash speed (the per-`$()` `Shell` clone is O(1) via copy-on-write). Keep it short; no harness-count change (no new harness). Skip if it doesn't fit cleanly.

- [ ] **Step 5: Commit**

```bash
git add README.md   # only if changed
git commit -m "docs(v127): note COW shell-clone perf improvement

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
(If README unchanged, skip this commit.)

---

## Final verification (after all tasks)

- [ ] `cargo build` + `cargo clippy --all-targets` clean.
- [ ] `cargo test 2>&1 | grep -E "test result: FAILED|error\["` → none.
- [ ] all bash-diff harnesses pass.
- [ ] COW guard test green; `$()`-defined function does not leak (matches bash).
- [ ] `nvm ls` within ~2× of bash; the `2000× $(true)` nvm-loaded micro-bench ≈ 1-3s (was ~46s).

## Self-review notes (plan author)
- **Spec coverage:** `functions` Rc + helpers + sites + guard → Task 1; `command_hash`/`history`/`completion_specs` Rc + sites → Task 2; full regression + perf payoff + docs → Task 3. The deterministic `Rc::strong_count` guard (Task 1 Step 5, extended Task 2 Step 3) and the `$()`-no-leak behavioral check (Task 1 Step 6, Task 3 Step 2) cover the spec's correctness requirements.
- **Type consistency:** `functions: Rc<HashMap<String, Box<Command>>>`; helpers `define_function(String, Box<Command>)` / `remove_function(&str) -> bool`; `command_hash: Rc<HashMap<String,(PathBuf,u32)>>`; `completion_specs: Rc<CompletionSpecs>`; `history: Rc<History>`. Writes via `Rc::make_mut`; reads via `Deref` (unchanged).
- **Why Task 1 = functions only first:** it's the dominant cost (nvm's 114 large ASTs) and lets us measure the bulk of the win before touching the other 3 (smaller) tables — and bounds blast radius (functions has the most write/test sites). Tasks 2/3 complete the spec's "full COW" scope.
- **Risk hinge:** isolation. `Rc::make_mut` copies iff `strong_count > 1`, so a `$()` clone that mutates one of these tables gets its own copy and the parent is untouched — identical to the old deep-clone semantics. Guarded by the strong_count test + the `$()`-no-leak checks.
