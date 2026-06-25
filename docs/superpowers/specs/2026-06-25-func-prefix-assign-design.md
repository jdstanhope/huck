# v221 — prefix assignment must not persist across a function call

## Status

Design approved 2026-06-25. Small, single-predicate correctness fix.

## Background

A re-measurement of the `func` bash-test-suite category found 6 independent
blockers; this iteration fixes the one with the broadest real-script value:
prefix (inline) assignments leaking across function calls. `func` will NOT flip
to PASS (5 blockers remain) — this clears 2 of its hunks.

bash discards a prefix assignment after a function returns; only POSIX **special
builtins** persist one. huck wrongly persists for user functions too:

```
v=1; f(){ :; }; v=5 f; echo $v   # bash: 1   huck: 5
```

Root cause — `executor.rs:4219-4220` classifies user functions as `persistent`:

```rust
let persistent = builtins::is_special_builtin(&resolved.program)
    || (!bypass_functions && shell.functions.contains_key(&resolved.program));
```

## Fix

Drop the function term:

```rust
let persistent = builtins::is_special_builtin(&resolved.program);
```

(and update the comment block at 4213-4218 to say user functions are temporary,
like regular commands). No other code changes: `apply_inline_assignments` already
snapshots each var's pre-command state and `restore_var` reinstalls it
**unconditionally** after the call, so the restore correctly clobbers any mutation
the function itself made to the prefixed var — matching bash.

## Behavior (all verified against bash 5.2.21)

| case | expected (bash = huck-after-fix) |
| --- | --- |
| `v=1; f(){:;}; v=5 f; echo $v` | `1` (restored to pre-command value) |
| `v=1; f(){ v=99; }; v=5 f; echo $v` | `1` (restore clobbers the function's global set) |
| `v=1; f(){ local v=99; }; v=5 f; echo $v` | `1` |
| `v=1; f(){ unset v; }; v=5 f; echo $v` | `1` (restore clobbers the unset) |
| no prior `v`; `v=5 f`; `echo ${v-UNSET}` | `UNSET` (restore removes) |
| `f(){ printenv V; }; V=x f` | `x` (exported for the call's duration) |
| posix `f(){ v=20 return; }; v=10; f; echo $v` | `20` (special builtin still persists) |

## Testing

- Flip the bug-encoding unit test `run_exec_single_function_call_inline_assignment_persists`
  (`executor.rs:8132`) → rename `..._does_not_persist`, assert `shell.get("FOO") == None`.
- Add unit tests for the global-mod-clobbered / local / unset / value-restored edges.
- New `tests/scripts/func_prefix_assign_diff_check.sh` mirroring the table vs live bash.
- `cargo test --workspace` green; re-run a few bash-test categories (func, dollars,
  varenv) as a regression guard.

## Out of scope

func blockers #2–#6 (declare -xf export filter, FUNCNAME assignment protection,
redirected-brace-body reconstruction, `function`-keyword preservation, FUNCNEST
enforcement) remain deferred. No POSIX-mode special-casing — bash does not persist
prefix assignments for functions in either mode.
