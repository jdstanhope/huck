# huck v151 — `FUNCNAME` inside function bodies Design

**Status:** approved design, ready for implementation plan.
**Fixes:** M-107 — `$FUNCNAME` / `${FUNCNAME[…]}` is always empty in huck; bash exposes a
dynamic call-stack array (`FUNCNAME[0]` = the currently-executing function, `[1]` = its
caller, …), unset outside any function.
**Branch (impl):** `v151-funcname`.
**Scope:** `FUNCNAME` only. The companion arrays `BASH_SOURCE` / `BASH_LINENO` are out of
scope (they need per-function source tracking and a `LINENO` huck lacks — L-29).

## Approach (chosen: stored & synced special array)

huck already tracks the function call stack in `Shell.function_arg0: Vec<String>` —
`call_function` pushes the function name on entry and pops it on return. Maintain
`FUNCNAME` as a real indexed array in `vars`, rebuilt from `function_arg0` whenever that
stack changes. Then every read form resolves through the existing array/scalar machinery
with no new read-path code (mirrors the existing `PIPESTATUS` special-array pattern,
`shell_state.rs:1093`).

The alternative (dynamic compute-on-read) is more bash-faithful for the pathological case
of a user assigning `FUNCNAME`, but spreads `"FUNCNAME"` special-cases across every
array-read site (element / `@` / `*` / `#` / `!`-keys) with a real risk of missing one.
The stored approach reuses the whole array surface for the cost of one sync method.

## Architecture

### 1. `Shell::sync_funcname()` — the only write side

New method on `Shell` (in `src/shell_state.rs`), mirroring `set_pipestatus`:

```rust
/// Rebuild the dynamic `FUNCNAME` array from the live function call stack.
/// `FUNCNAME[0]` is the currently-executing function, `[1]` its caller, etc.
/// (reverse of `function_arg0`'s push order). When the stack is empty (top
/// level) `FUNCNAME` is unset, matching bash. Called by `call_function` after
/// every push/pop of `function_arg0`.
pub(crate) fn sync_funcname(&mut self) {
    if self.function_arg0.is_empty() {
        self.vars.remove("FUNCNAME");
        return;
    }
    let n = self.function_arg0.len();
    let elements: BTreeMap<usize, String> = (0..n)
        .map(|k| (k, self.function_arg0[n - 1 - k].clone()))
        .collect();
    self.vars.insert(
        "FUNCNAME".to_string(),
        Variable {
            value: VarValue::Indexed(elements),
            exported: false,
            readonly: false,
            integer: false,
        },
    );
}
```

### 2. Two sync points in `call_function`

`call_function` (`src/executor.rs:2905`) has a single-exit structure — push at line 2921,
body at 2924, linear cleanup with the pop at 2947, one `match` return. No early returns
between push and pop, so two calls cover all paths:

- after `shell.function_arg0.push(name.to_string());` (2921) → `shell.sync_funcname();`
- after `shell.function_arg0.pop();` (2947) → `shell.sync_funcname();`

Ordering consequences (both match bash):
- The RETURN trap fires at 2936, **before** the pop — so inside a RETURN trap `FUNCNAME`
  still reflects the executing function.
- The local-scope restore (2941) also runs before the pop — `FUNCNAME` stays the
  in-function value during local teardown, then becomes the caller's frame after 2947.

### 3. Reads — resolve for free via existing machinery

| Form | Path | Result |
|------|------|--------|
| `$FUNCNAME` / `${FUNCNAME}` | `lookup_var` → `vars.get("FUNCNAME").scalar_view()` returns element `[0]` for an indexed array (`shell_state.rs:32`) | current function (empty outside a function) |
| `${FUNCNAME[0]}` / `${FUNCNAME[1]}` | existing indexed-element path (`get_array`) | current / caller |
| `${FUNCNAME[@]}` / `[*]` | `expand_array_param` `collect_values` | current→outermost |
| `${#FUNCNAME[@]}` | existing length path | call depth |
| `${!FUNCNAME[@]}` | existing keys path | `0 … depth-1` |

No changes to `expand.rs` / `param_expansion.rs` / `lookup_var` are required.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/shell_state.rs` | `Shell::sync_funcname()` (+ unit tests). |
| `src/executor.rs` | Two `shell.sync_funcname();` calls in `call_function` (after the `function_arg0` push and pop) (+ unit tests). |
| `tests/scripts/funcname_diff_check.sh` | bash-diff harness. |
| `docs/bash-divergences.md` | DELETE M-107 (Tier-2 count 20→19). |

## Behaviour matrix (target = bash)

| Input | Result |
|---|---|
| top level: `echo "[${FUNCNAME:-unset}] ${#FUNCNAME[@]}"` | `[unset] 0` |
| `f(){ echo "$FUNCNAME"; }; f` | `f` |
| `inner(){ echo "${FUNCNAME[@]}"; }; outer(){ inner; }; outer` | `inner outer` |
| `inner(){ echo "${#FUNCNAME[@]} ${FUNCNAME[1]}"; }; outer(){ inner; }; outer` | `2 outer` |
| `g(){ echo "$FUNCNAME"; }; f(){ g; echo "$FUNCNAME"; }; f` | `g` then `f` |
| after `f` returns, top level `${FUNCNAME:-unset}` | `unset` |
| recursion `r(){ [ $1 -gt 0 ] && { echo ${#FUNCNAME[@]}; r $(($1-1)); }; }; r 2` | `1` then `2` (depth grows per recursive frame; deepest call's guard is false so it doesn't echo) |

## Edge cases

- **Outside any function:** `FUNCNAME` unset (`$FUNCNAME` empty, `${#FUNCNAME[@]}` = 0). ✓
- **`$(...)` substitution:** the COW `Shell` clone deep-copies `function_arg0` and the
  `FUNCNAME` var and re-syncs on its own calls, so a cmd-sub inside a function sees the
  correct stack. ✓
- **Pathological (the accepted "low"-severity tradeoff of the stored approach):** a user
  `FUNCNAME=…`, `local FUNCNAME`, `unset FUNCNAME`, or an env-inherited `FUNCNAME` is
  overwritten on the next function enter and removed when the stack empties. Idiomatic
  code never assigns `FUNCNAME`; bash itself treats it as dynamic.

## Testing

1. **Unit tests** (`src/shell_state.rs` for `sync_funcname`; `src/executor.rs` for the
   `call_function` wiring): `function_arg0` of `["outer","inner"]` → `FUNCNAME[0]=inner`,
   `[1]=outer`, len 2; empty stack → `get_array("FUNCNAME")` is `None` and
   `lookup_var("FUNCNAME")` is `None`; after a simulated call+return the array is restored
   (and after a nested return).
2. **`funcname_diff_check.sh`**: nested functions printing `$FUNCNAME`, `${FUNCNAME[@]}`,
   `${#FUNCNAME[@]}`, `${FUNCNAME[1]:-none}`, plus the top-level unset case — byte-identical
   bash↔huck.
3. **Full regression:** suite + all harnesses green; clippy clean.

## Notes
- **Payoff:** bash_completion's `${FUNCNAME[…]}` diagnostics now resolve (M-107's motivating
  case).
- **Git safety:** implementer subagents must NOT `git checkout <sha>`; the controller
  verifies the branch tip before merge. Commit trailer:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
