# huck v127 — copy-on-write `Shell` clone (fix per-command-substitution deep-copy) Design

**Status:** approved design, ready for implementation plan.
**Implements:** make `Shell::clone()` O(1) instead of O(total-data-size), by
wrapping the large, read-mostly tables in `Rc` with copy-on-write. Eliminates
the ~90× per-`$(…)` overhead that makes `nvm ls` 4-20× slower than bash.
**Branch (impl):** `v127-cow-shell-clone`.

## Background — measured root cause (this session)

`run_substitution` (`src/expand.rs:1174`) runs every `$(…)` on a CLONED shell to
isolate the substitution's state mutations (bash runs `$(…)` in a subshell):
```rust
pub fn run_substitution(seq: &Sequence, shell: &mut Shell) -> String {
    let mut cloned = shell.clone();          // <-- deep-copies the ENTIRE Shell
    let (output, status) = executor::execute_capturing(seq, &mut cloned);
    …
}
```
`Shell` is `#[derive(Clone)]`, so `clone()` deep-copies `functions:
HashMap<String, Box<Command>>` — every function's full parsed AST — plus `vars`,
`command_hash`, `history`, `completion_specs`, etc. Measured:

| scenario (2000× `$(true)`) | huck | note |
|---|---|---|
| empty shell | **0.52s** | clone is cheap |
| nvm-loaded (114 large functions) | **46.4s** (22s user) | ~90× slower — pure clone cost |
| bash, nvm-loaded | 6.1s | bash forks (copy-on-write), no deep copy |

And `nvm ls`: huck **~26s (13s user CPU)** vs bash **~6.5s (1.4s user)**. The
~9× excess is *user CPU* spent deep-cloning nvm's function ASTs on each of the
hundreds of `$(…)` calls. A discriminator showed the cost scales with AST
**size** (200 tiny functions ≈ 3s; nvm's 114 large ones ≈ 46s), i.e. it is the
deep copy of the function-body trees.

huck is single-threaded (only `thread::sleep`, no `thread::spawn`), so `Rc` is
safe (no `Send` requirement).

## Architecture — `Rc` + copy-on-write for the big read-mostly tables

Keep the `shell.clone()` in `run_substitution` (it provides bash's subshell
isolation), but make cloning the large tables O(1) by sharing them via `Rc` and
copying only on write (`Rc::make_mut`). The substitution rarely mutates these
tables; when it does, `make_mut` copies that one table for the clone, leaving
the parent's untouched — identical isolation semantics, far cheaper.

### Fields to wrap in `Rc`

| field | today | new |
|---|---|---|
| `functions` | `HashMap<String, Box<Command>>` | `Rc<HashMap<String, Box<Command>>>` |
| `command_hash` | `HashMap<String, (PathBuf, u32)>` | `Rc<HashMap<String, (PathBuf, u32)>>` |
| `completion_specs` | `CompletionSpecs` | `Rc<CompletionSpecs>` |
| `history` | `crate::history::History` | `Rc<crate::history::History>` |

`functions` is the dominant win (nvm's case). `history` / `completion_specs` /
`command_hash` are the other potentially-large, read-mostly tables (a long
session's history; bash-completion's specs) and are read-mostly inside
substitutions, so COW applies cleanly.

**Left as plain (NOT wrapped):** `vars`, `local_scopes`, `positional_args`
(mutated *inside* substitutions — COW would copy on first write anyway, so no
gain), and the already-tiny `aliases`, `traps`, `dir_stack`, `command_hash`'s
neighbors, etc. (negligible clone cost). `current_completion_spec` (an
`Option<CompletionSpec>`, separate from `completion_specs`) stays plain.

`#[derive(Clone)]` on `Shell` is kept — it now clones the `Rc` fields by
refcount bump (O(1)) automatically.

### Access pattern
- **Reads** are unchanged: `Rc<T>` derefs to `&T`, so `shell.functions.get(n)`,
  `shell.command_hash.get(n)`, `shell.completion_specs.by_command…`,
  `shell.history.…` (immutable methods) all keep compiling as-is.
- **Writes** go through `Rc::make_mut(&mut field)` (copies iff shared). To keep
  call sites clean and contain churn (especially the ~11 test sites that do
  `shell.functions.insert(…)`), add small mutator methods on `Shell`:
  - `pub(crate) fn define_function(&mut self, name: String, body: Box<Command>)`
    → `Rc::make_mut(&mut self.functions).insert(name, body);`
  - `pub(crate) fn remove_function(&mut self, name: &str) -> bool`
    → `Rc::make_mut(&mut self.functions).remove(name).is_some()`
  - For `command_hash` / `history` / `completion_specs`, the production write
    sites are few and localized (the `hash` builtin; history `add`/`load`/
    `clear`; `complete`/`compgen`), so use `Rc::make_mut(&mut shell.<field>)…`
    inline at those sites (or a tiny mutator if it reads cleaner). Match the
    style the implementer finds least churny per table.

### Write sites to convert (production)
- `functions`: `Command::FunctionDef` handler (`src/executor.rs:~431`,
  `shell.functions.insert`) → `shell.define_function(...)`; `unset -f`
  (`src/builtins.rs:565`, `shell.functions.remove`) → `shell.remove_function(...)`.
- `command_hash`: the `hash` builtin (`src/builtins.rs:~5964/5975/5995/6062` —
  clear/remove/insert) → `Rc::make_mut(&mut shell.command_hash)…`.
- `history`: `src/shell.rs:284` (`.load()`), `:322` (`.add(...)`),
  `src/builtins.rs:3855` (`.clear()`) → `Rc::make_mut(&mut shell.history)…`.
- `completion_specs`: `src/completion_builtins.rs` sites that mutate
  `shell.completion_specs.by_command` / `.default_spec` / `.empty_spec` →
  `Rc::make_mut(&mut shell.completion_specs).by_command…` etc.
- Test code that does `shell.functions.insert(...)` (~11 sites in `builtins.rs`,
  `completion_spec.rs`) → `shell.define_function(...)`.

## Correctness / must-not-regress
- **Isolation is preserved.** A `$(…)` that defines a function / hashes a
  command / appends history mutates only the *cloned* shell's table (via
  `make_mut` copying it), so the parent is unaffected — exactly as today (and as
  bash's subshell). Verified by the existing function/cmd-sub/`unset -f`/`hash`/
  completion tests staying green.
- The ONLY observable change is speed.
- No new threads; `Rc` (not `Arc`) is correct. If the compiler unexpectedly
  demands `Send`/`Sync` for `Shell` somewhere, switch those fields to `Arc`
  (same O(1)-clone property; the build will indicate this).

## Files & responsibilities

| File | Change |
|------|--------|
| `src/shell_state.rs` | Wrap the 4 fields in `Rc`; init in `new()` (`Rc::new(…)`); add `define_function`/`remove_function` (+ any small history/hash/completion mutators); update in-module readers if any used field methods that need `&mut`. |
| `src/executor.rs` | `Command::FunctionDef` → `define_function`. |
| `src/builtins.rs` | `unset -f` → `remove_function`; `hash` builtin → `make_mut(command_hash)`; history `clear` → `make_mut`; convert test `functions.insert` → `define_function`. |
| `src/shell.rs` | history `add`/`load` → `make_mut(history)`. |
| `src/completion_builtins.rs` / `src/completion_spec.rs` | `completion_specs` mutations → `make_mut`; test `functions.insert` → `define_function`. |
| `tests/` | NEW perf-COW guard test (deterministic, below). |
| `docs/bash-divergences.md`, `README.md` | no divergence entry (perf, not a behavior gap); optionally note the perf improvement in the README status line. |

## Testing
1. **Deterministic COW guard** (NOT timing-based) — add to `shell_state.rs`
   `#[cfg(test)]` (or a `tests/cow_shell_clone.rs`):
   - After `let c = shell.clone()`, assert
     `Rc::strong_count(&shell.functions) == 2` (sharing, not deep copy). Same for
     `command_hash`, `completion_specs`, `history`.
   - COW correctness: `shell.define_function("g", …)` on the parent AFTER cloning
     must NOT appear in the clone `c` (and vice-versa); `Rc::strong_count` drops
     back to 1 for the mutated field. Confirms `make_mut` isolates.
2. **Behavioral regression:** full unit + integration suites and ALL bash-diff
   harnesses green — especially: defining a function inside `$(f)` does not leak
   to the parent; `unset -f`; `hash`; `complete`/`compgen`; `history`.
3. **Perf check (manual, reported — not a CI assertion, to avoid flakiness):**
   - the nvm-loaded `2000× $(true)` micro-bench (`/tmp` script) drops from ~46s
     toward ~1s;
   - `nvm ls` wall-clock + user-CPU (`/usr/bin/time`) collapses toward bash's
     (target: huck within ~2× of bash, user-CPU no longer dominant).
   Report before/after numbers honestly.

## Edge cases & notes
- `Rc::make_mut` requires the inner type be `Clone` — `HashMap`, `History`,
  `CompletionSpecs` already derive/impl `Clone` (they're cloned today). Confirm
  `CompletionSpecs` and `History` are `Clone` (they must be, since `Shell` is);
  no change needed.
- A function defined at the top-level REPL (not in a substitution) does a
  `make_mut` while `strong_count == 1` (no other clone alive) → no copy, just a
  mutable borrow. So the common path stays allocation-free.
- This does not change `vars` cloning; if a future profile shows `vars` (or
  `local_scopes`) dominating for some workload, wrapping them is a separate,
  later consideration (they're mutated inside substitutions, so COW is less
  clearly a win — out of scope here).
- Not a bash-divergence fix — purely internal performance. No M-/L- entry.
