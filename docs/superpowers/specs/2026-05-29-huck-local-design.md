# huck v52 — `local` builtin (M-67)

## Goal

Add the bash `local` builtin: inside a function, declare variables
scoped to that function call. On function return, the variables are
restored to their pre-call state (or unset if they didn't exist
before).

After v52:

- `local NAME=value` inside a function: declare NAME local; sets to
  `value`. Restored on return.
- `local NAME` (no value): declare NAME local; sets to empty string.
- `local A=1 B C=3`: multiple at once.
- `local` outside a function: error + status 1 (bash-faithful).

New tracked divergence: **M-67: `local`**.

## Scope decisions (locked)

This is unambiguous bash-compat. No questions surfaced.

1. **`local NAME` (no value)**: sets NAME to empty string in the
   local scope (bash-compat).
2. **Already-local in same scope**: `local X=1; local X=2` does NOT
   re-snapshot; the second call just updates X's value. On return,
   X is restored to the pre-first-`local` state.
3. **`local -p` / `local -i` / `local -a` / `local -A`** (bash
   attribute extensions): deferred.
4. **Outside a function**: error "local: can only be used in a
   function" + status 1.

## Out of scope (deferred)

- `local -p` listing.
- `local -i`/`-a`/`-A` typed attributes (integer, array, assoc).
- Dynamic-scope lookup chain through nested function frames. huck's
  `lookup_var` is a flat HashMap; the v52 implementation gives the
  observable bash behavior (inner local shadows outer; restore on
  return) without making lookup walk a frame stack. Nested functions
  with separately-`local`-shadowed names work because each frame's
  snapshot is restored on its own return.

## Architecture

Three coordinated pieces:

### 1. `Shell.local_scopes` field (`src/shell_state.rs`)

```rust
/// Stack of "to-restore-on-function-exit" snapshots. Each frame
/// records, for every `local NAME` invocation in that function,
/// the pre-`local` state of NAME (`None` if NAME was unset, `Some`
/// if it had a Variable). Pushed in `call_function`, popped + applied
/// on function exit.
pub local_scopes: Vec<std::collections::HashMap<String, Option<Variable>>>,
```

Initialize to `Vec::new()` in `Shell::new`.

`Variable` is currently private to `shell_state.rs`. Make it `pub`
since it's now exposed via this field.

### 2. `Shell` helper methods (`src/shell_state.rs`)

```rust
/// Returns a clone of the named variable's current state, or None
/// if unset. Used by `local` to snapshot pre-local state.
pub fn snapshot_var(&self, name: &str) -> Option<Variable> {
    self.vars.get(name).cloned()
}

/// Restores `name` to `snapshot`: Some → reinstall; None → remove.
/// Used by call_function on exit to undo a `local` frame.
pub fn restore_var(&mut self, name: &str, snapshot: Option<Variable>) {
    match snapshot {
        Some(v) => { self.vars.insert(name.to_string(), v); }
        None => { self.vars.remove(name); }
    }
}
```

### 3. `call_function` integration (`src/executor.rs`)

The current `call_function` (`src/executor.rs:1378-1406`) already
saves/restores `positional_args` and `function_arg0`. Add a parallel
push+restore for `local_scopes`:

```rust
fn call_function(...) -> ExecOutcome {
    let saved_positional = std::mem::take(&mut shell.positional_args);
    shell.positional_args = args;
    shell.function_arg0.push(name.to_string());
    shell.local_scopes.push(std::collections::HashMap::new());  // NEW

    let result = run_command(&body, shell, sink);

    // (existing trap firing — unchanged)
    let status_for_trap = match &result { ... };
    shell.set_last_status(status_for_trap);
    crate::traps::fire_return_trap(shell);

    // NEW: pop local scope and restore each saved var.
    if let Some(frame) = shell.local_scopes.pop() {
        for (var_name, snapshot) in frame {
            shell.restore_var(&var_name, snapshot);
        }
    }

    shell.function_arg0.pop();
    shell.positional_args = saved_positional;

    match result {
        ExecOutcome::FunctionReturn(n) => ExecOutcome::Continue(n),
        other => other,
    }
}
```

### 4. `builtin_local` (`src/builtins.rs`)

```rust
fn builtin_local(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if shell.local_scopes.is_empty() {
        eprintln!("huck: local: can only be used in a function");
        return ExecOutcome::Continue(1);
    }
    for arg in args {
        let (name, value): (&str, Option<String>) = match arg.find('=') {
            Some(eq) => (&arg[..eq], Some(arg[eq + 1..].to_string())),
            None => (arg.as_str(), None),
        };
        if !is_valid_name(name) {
            eprintln!("huck: local: `{arg}': not a valid identifier");
            return ExecOutcome::Continue(1);
        }
        // Snapshot only if NAME is not already saved in this frame.
        // Do the snapshot before any mutable borrows of local_scopes.
        let needs_snapshot = !shell
            .local_scopes
            .last()
            .map(|f| f.contains_key(name))
            .unwrap_or(false);
        if needs_snapshot {
            let snap = shell.snapshot_var(name);
            shell
                .local_scopes
                .last_mut()
                .unwrap()
                .insert(name.to_string(), snap);
        }
        shell.set(name, value.unwrap_or_default());
    }
    ExecOutcome::Continue(0)
}
```

### 5. Dispatch + `BUILTIN_NAMES`

- Add `"local"` to `BUILTIN_NAMES`.
- Add `"local" => builtin_local(args, shell)` to `run_builtin`.
- NOT added to `is_special_builtin` — bash classifies `local` as a
  regular builtin (it can fail when called outside a function).

## Behavior table

| Input | Behavior |
|---|---|
| (outside function) `local X=1` | error + status 1 |
| (in function) `local X=1` | X becomes "1" locally |
| (in function, X was "outer") `local X=in` | X="in" inside; X="outer" after return |
| (in function, X was unset) `local X=in` | X="in" inside; X unset after return |
| (in function) `local X` | X="" inside; X restored after return |
| (in function) `local X=1 Y Z=3` | X="1", Y="", Z="3" all local |
| (in function) `local 1foo=bar` | error "not a valid identifier" + status 1 |
| (in function) `local X=1; local X=2` | X="2" inside; restored to pre-first-`local` state after return |
| nested f→g, both `local X=...` | each frame restored on its own return |

## Test plan

### Unit tests in `src/builtins.rs#[cfg(test)] mod local_tests`

6 tests:

1. `local_outside_function_errors_status_1` — empty `local_scopes`
   → `Continue(1)`.
2. `local_with_value_sets_and_records_snapshot` — push empty frame,
   call `local X=hi`, assert `lookup_var("X") == Some("hi")` AND
   frame has `("X", None)` (pre-state was unset).
3. `local_without_value_sets_empty` — push empty frame; `local X` →
   `lookup_var("X") == Some("")`.
4. `local_snapshots_existing_var` — pre-set X="outer"; push frame;
   `local X=in`; assert frame snapshot is `Some(Variable{value:"outer",...})`.
5. `local_idempotent_in_same_frame` — `local X=1; local X=2`; assert
   frame snapshot taken ONCE (still the original pre-local state).
6. `local_invalid_identifier_errors` — push frame; `local 1foo=bar`
   → `Continue(1)`.

### Unit tests in `src/executor.rs#[cfg(test)]`

3 tests via `exec_script` helper (already exists):

7. `function_with_local_does_not_leak_var` — define `f() { local X=in; }`;
   call f; `shell.lookup_var("X")` is None.
8. `function_local_restores_outer_var` — set X="outer"; define
   `f() { local X=in; echo $X; }`; call f; `shell.lookup_var("X") ==
   Some("outer")`.
9. `nested_function_calls_have_isolated_locals` — outer f does
   `local X=outer`, then calls inner g which does `local X=inner`;
   after both return, X is back to the initial state.

### Integration tests at `tests/local_integration.rs`

2 binary-driven tests:

1. `local_scopes_to_function` — script:
   `X=outer\nf() { local X=in; echo "in=$X"; }\nf\necho "out=$X"\nexit\n`;
   stdout has `in=in` and `out=outer`.
2. `local_outside_function_errors` — script:
   `local X=1\necho $?\nexit\n`; stdout includes a non-zero status
   line; stderr contains "can only be used in a function".

### Smoke

`cargo test --all-targets` must pass. PTY flake tolerated.

## Implementation tasks

1. **Foundation + builtin + 9 unit tests**:
   - `src/shell_state.rs`: add `pub local_scopes: Vec<HashMap<String,
     Option<Variable>>>` field (init `Vec::new()` in `Shell::new`);
     make `Variable` `pub`; add `snapshot_var` and `restore_var`
     methods.
   - `src/executor.rs::call_function`: push `HashMap::new()` onto
     `local_scopes` before running body; pop + apply restore after
     body.
   - `src/builtins.rs`: add `builtin_local`; add `"local"` to
     `BUILTIN_NAMES`; add dispatch arm; append `mod local_tests`
     with 6 tests; add 3 executor tests in `src/executor.rs::mod tests`
     (the existing function-scope test mod).

2. **Integration tests**: create `tests/local_integration.rs` with
   the 2 scenarios.

3. **Docs**: add M-67 entry; change-log entry; README v52 row.

Three tasks. TDD per task.

## Acceptance criteria

- All 9 unit tests pass (6 in builtins, 3 in executor).
- All 2 integration tests pass.
- `cargo test --all-targets` passes (modulo PTY flake).
- `cargo clippy --all-targets -- -D warnings` passes.
- `docs/bash-divergences.md` has the new M-67 entry as
  `[fixed v52]`.
- `local` inside a function declares scoped vars.
- `local` outside a function errors with status 1.
- Locals are correctly restored to caller's state on function exit,
  including the case where the caller had the var unset (var becomes
  unset again).
- Pre-existing tests still pass after the `Variable` visibility
  change.
