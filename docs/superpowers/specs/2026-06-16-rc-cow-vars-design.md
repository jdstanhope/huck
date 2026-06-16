# v164: Rc-COW the `vars` table — Design

**Status:** approved 2026-06-16
**Iteration:** v164
**Origin:** the 2026-06-16 architecture review identified `Shell::vars` as the
one large per-`$()`-deep-cloned table not yet wrapped in `Rc` (the other four —
`functions`/`history`/`command_hash`/`completion_specs` — already use the
Rc-COW pattern from v127). This is improvement #2 of the agreed post-review
structural sequence (lib.rs → **Rc-COW vars** → CharCursor scanner consolidation).

## Goal

Make `Shell::clone()` O(1) with respect to the variable table by wrapping
`vars` in `Rc` and writing through `Rc::make_mut`, eliminating the full deep
copy of every `Variable`/`VarValue` performed on each command substitution
`$(...)`.

## Problem

`Shell` derives `Clone`. `run_substitution` (`src/expand.rs:1256`) does
`let mut cloned = shell.clone()` for **every** `$(...)`. The struct
(`src/shell_state.rs:380`) already wraps `functions`, `history`,
`command_hash`, and `completion_specs` in `Rc` for O(1) clone, but
`vars: HashMap<String, Variable>` (`src/shell_state.rs:381`) is **not** wrapped,
so the largest and most-mutated table is fully deep-copied on every command
substitution — including every `Variable`'s `VarValue` (which may be an indexed
or associative array of `String`s). A command-substitution-heavy script (e.g.
`x=$(...); y=$(...)` in a loop, or a framework like nvm/bash-completion that
runs many `$()` with a large environment loaded) pays a full env copy each time.

This is the same class of catastrophe v127 fixed for the other four tables
(`nvm ls` 26s → 5.9s after wrapping `functions`/etc.); `vars` was simply left
out of that pass.

## Design

### The change

Change the field type and initializer:

```rust
// src/shell_state.rs (struct field, ~line 381)
vars: std::rc::Rc<HashMap<String, Variable>>,
```

```rust
// src/shell_state.rs (Shell::new / constructor, ~line 713 region)
vars: std::rc::Rc::new(HashMap::new()),
```

`Shell::clone()` (derived) then clones `vars` by bumping the `Rc` refcount
instead of deep-copying the map. `Rc` (not `Arc`) — huck is single-threaded and
all four existing wrapped tables use `Rc`; command-substitution children are
separate **processes**, not threads.

### Write mechanics — a private `vars_mut()` helper

Add a private accessor that performs the copy-on-write:

```rust
/// Mutable access to the variable table, copy-on-write. `Rc::make_mut` is
/// O(1) when the `Rc` is uniquely owned (the normal case) and clones the map
/// only when it is shared — i.e. lazily, on the first write inside an active
/// `$(...)` substitution (where parent and clone transiently share the Rc).
fn vars_mut(&mut self) -> &mut HashMap<String, Variable> {
    std::rc::Rc::make_mut(&mut self.vars)
}
```

Rewrite the mutating call sites (all within `src/shell_state.rs`, since `vars`
is private) to go through it:

- `self.vars.insert(...)` → `self.vars_mut().insert(...)` (~19 sites)
- `self.vars.get_mut(...)` → `self.vars_mut().get_mut(...)` (~14 sites)
- `self.vars.remove(...)` → `self.vars_mut().remove(...)` (~8 sites)

The read sites stay **unchanged** — they resolve through `Rc`'s `Deref<Target =
HashMap<...>>`:

- `self.vars.get(...)` (~19), `self.vars.keys()` (~2),
  `self.vars.contains_key(...)` (~2), `self.vars.iter()` (~1)

Exact counts are approximate; the implementer converts every mutating site and
leaves every read site, verified by the field being private (no external
access) and by a clean compile.

### Why this is correct — COW gives subshell isolation for free

A command substitution `$(...)` runs in a subshell environment: variable
assignments inside it must **not** leak to the parent. Today the eager deep
clone in `run_substitution` provides that isolation. With COW the isolation is
preserved, lazily:

1. `run_substitution` clones the `Shell`; parent and clone now share the `vars`
   `Rc` (refcount 2).
2. The substitution runs on the clone. Its first variable write calls
   `vars_mut()` → `Rc::make_mut` sees refcount > 1 → clones the map into a
   private copy owned by the clone.
3. The clone mutates its private copy; the parent's table is untouched.
4. The clone is dropped at the end of `run_substitution`; the parent retains
   its original (now uniquely-owned) `Rc`.

The observable behavior is byte-identical to the eager-clone implementation.
`$()` is synchronous (the parent is blocked while the substitution runs on the
clone), so there is no window in which the parent mutates the shared table.

### Unaffected machinery

These all clone **individual `Variable`s**, independent of the table's `Rc`
wrapper, and require no change:

- `local`-scope unwinding (`snapshot_for_local_scope`, `local_scopes:
  Vec<HashMap<String, Option<Variable>>>`) — snapshots per-name `Variable`
  values for restore on function return.
- Inline-assignment snapshot/restore (`snapshot_var`/`restore_var`,
  `apply_inline_assignments`/`restore_inline_assignments`) — `FOO=v cmd`
  duration scoping.
- Readonly / integer-attribute write paths (`assign`, `try_set`, `mark_*`).

`get_mut` keeps its `Option<&mut Variable>` signature; internally it becomes
`self.vars_mut().get_mut(name)`, which borrows from the `make_mut`'d map exactly
as the prior `self.vars.get_mut(name)` borrowed from the plain map (same borrow
shape → compiles identically).

## Verification

### Correctness

- Full unit suite (~2192 tests) + integration tests (~145 binaries) + all 91
  bash-diff harnesses must pass; `cargo clippy --lib --bins` clean. These
  already exercise variable get/set/unset, `local` scoping, inline-assignment
  scoping, and `$()` subshell isolation heavily.
- Add one focused regression test (in `src/shell_state.rs` or an integration
  test) asserting that a command substitution which assigns a variable does
  **not** mutate the parent shell's value — e.g. a script equivalent to
  `x=outer; echo $(x=inner; echo in)$x` must print `inouter` (the parent's `x`
  stays `outer`). This pins the lazy-COW isolation explicitly so a future
  regression in `vars_mut`/`make_mut` is caught directly rather than only via
  broad harness fallout.

### Performance

A one-off, before/after wall-clock measurement (the project's established style
— cf. v127's `nvm ls` 26s → 5.9s — rather than a committed, flaky benchmark),
documented in the iteration's plan/notes:

- Construct a large variable table (e.g. assign several hundred scalar/array
  variables, or source a representative environment) and run a loop performing
  many command substitutions (e.g. `for i in $(seq 1 2000); do :; x=$(:); done`,
  or a tighter `$(:)`-in-a-loop fragment).
- Measure wall-clock on the parent commit (eager deep clone) vs the v164 binary
  (Rc-COW). Expect a clear reduction that scales with the variable-table size;
  the win is proportional to (table size × number of `$()` calls).

The measurement is for validation/record only; it is not committed as a test.

## Out of scope

Per the approved scope decision, this iteration wraps **only** `vars`. The other
deep-cloned fields — `aliases` (`HashMap<String, String>`), `exported_functions`
(`HashSet<String>`), `local_scopes`, `positional_args` — are left as-is: they
are typically small, and `aliases`/`exported_functions` are `pub` fields whose
writes are scattered across multiple files (more surface, lower payoff). They
are noted as a possible follow-on only if a future measurement shows they matter.

No behavior change is intended anywhere; this is a pure internal-representation /
performance refactor.
