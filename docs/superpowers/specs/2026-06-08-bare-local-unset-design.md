# huck v115 — bare `local NAME` declares an *unset* local (M-111) Design

**Status:** approved design, ready for implementation plan.
**Implements:** `local NAME` with no value declares a function-local variable that
is **unset** (not set-to-empty), matching bash — new **M-111** (Tier-1 bug). The
last blocker for `mise ` + `<TAB>`.
**Why now:** bash_completion's `_get_comp_words_by_ref` does `local exclude flag i
OPTIND=1 … vcur vcword vprev vwords` (bare). bash leaves `vcword`/`vwords` UNSET,
so `[[ -v vcword ]]` / `[[ -v vwords ]]` are false and those branches are skipped.
huck creates them **set-but-empty**, so the tests pass → empty strings get
appended to `upvars` → `local '': not a valid identifier` and a malformed
`_upvars` arg list → `bash_completion: : : invalid option` on `mise<TAB>`.
**Branch (impl):** `v115-bare-local-unset`.

## Background — the bug (verified against bash)

| In a function | bash | huck (now) |
|---|---|---|
| `local x; [[ -v x ]] && echo SET \|\| echo UNSET` | `UNSET` | `SET` |
| `local x; echo "[${x-DEF}]"` | `[DEF]` | `[]` (huck's set-empty fails the non-colon "set?" test) |
| `local x; echo "[${x:-d}]"` | `[d]` | `[d]` (colon form treats set-empty as null — already matches) |
| `declare x; [[ -v x ]]` (top level) | `UNSET` | `UNSET` ✓ (already correct) |

The observable divergences are the **non-colon** `${x-DEF}` / `${x+alt}` forms and
`[[ -v x ]]` (and `${#x}`-style "is it set" logic), all of which treat huck's
spurious set-empty as "set". The colon forms (`:-`/`:+`) coincidentally match
because they test null-or-unset.

Bash semantics that MUST hold after the fix:
- `x=outer; f(){ local x; echo "[${x-DEF}]"; x=5; echo "in=$x"; }; f; echo "out=$x"`
  → `[DEF]` / `in=5` / `out=outer` (bare local shadows as UNSET; a later
  assignment is local; the outer value is restored on return).
- `local x=` → SET to empty (`[[ -v x ]]` true) — unchanged.
- `local x=val` → SET to `val` — unchanged.

**Root cause:** both `local` code paths set the variable to an empty string for a
bare name:
- `builtin_local_decl` (`src/builtins.rs:~1359-1362`, the live declaration path):
  `// Bare local NAME with no value: set empty scalar … shell.set(name, String::new());`
- `builtin_local` (`src/builtins.rs:~688`, the legacy string-args path):
  `shell.set(name, value.unwrap_or_default());` — `value` is `None` for a bare name.

## How huck's local scoping works (why the fix is sound)

A variable's "local-ness" in huck is recorded by `snapshot_for_local_scope` /
`shell.snapshot_var(name)`: it captures the pre-`local` state (set-to-X, or
unset) into the current `local_scopes` frame; on function return,
`call_function` restores each snapshotted name via `restore_var`. The variable
itself lives in `shell.vars`. So a bare `local x` should: (1) take the snapshot
(records the outer value, marks `x` local to this frame), then (2) **unset** `x`
in `shell.vars`. Result: `x` is genuinely unset during the function (so `[[ -v x ]]`
is false and `${x-DEF}` substitutes), a later `x=5` writes to `shell.vars` (local,
since the frame will restore the outer), and the outer binding is restored on
return. This is exactly bash, and needs **no new state** — only swapping the
set-empty for an unset.

## The fix

### Component 1 — `builtin_local_decl` bare-scalar arm (`src/builtins.rs:~1359`)
Replace:
```rust
                } else {
                    // Bare `local NAME` with no value: set empty scalar,
                    // matching the legacy builtin_local behavior.
                    shell.set(name, String::new());
                }
```
with:
```rust
                } else {
                    // Bare `local NAME` with no value: declare it function-local
                    // but UNSET (matches bash + `declare NAME`). The snapshot
                    // above records the outer value so it is restored on return;
                    // unsetting here makes `[[ -v NAME ]]` / `${NAME-d}` see it as
                    // unset until assigned. (M-111)
                    shell.unset(name);
                }
```
(`snapshot_for_local_scope(shell, name)` is already called just above; the
readonly check above still rejects shadowing a readonly. The `-a`/`-A` branches
are untouched.)

### Component 2 — `builtin_local` legacy path bare-name case (`src/builtins.rs:~688`)
Replace:
```rust
        shell.set(name, value.unwrap_or_default());
```
with:
```rust
        match value {
            // `local NAME=` / `local NAME=val`: set (possibly empty).
            Some(v) => shell.set(name, v),
            // Bare `local NAME`: declare local but UNSET (M-111). The snapshot
            // above records the outer value for restore-on-return.
            None => shell.unset(name),
        }
```
(The snapshot block just above is unchanged; this path is fixed for consistency
since both implementations exist — `local` normally routes through
`builtin_local_decl` via `run_declaration_builtin`, but keeping the legacy path
correct avoids a latent divergence.)

## Must-not-regress
- `local NAME=` → set-empty (`[[ -v NAME ]]` true); `local NAME=val` → set.
- `local NAME` followed by `NAME=val` in the function → local, outer restored on
  return; a nested function sees the assigned value (dynamic scope unchanged).
- `local -a NAME` / `local -A NAME` (bare array/assoc) → empty array/assoc
  (unchanged); `local -a NAME=(...)` unchanged.
- Readonly shadow rejection (`local ro` when `ro` is readonly) → error, no
  snapshot/unset (unchanged).
- `local` outside a function → "can only be used in a function" (unchanged).
- `declare`/`typeset` (top-level + `-g`) bare names — already correct, untouched.
- Multiple bare names (`local a b c`) → all three unset-local.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/builtins.rs` | `builtin_local_decl` bare-scalar arm → `unset`; `builtin_local` bare-name (`value=None`) → `unset` |
| `tests/bare_local_unset_integration.rs` | NEW — `[[ -v ]]` / `${x-d}` / later-assign-local / restore cases |
| `tests/scripts/bare_local_unset_diff_check.sh` | NEW — 39th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-111 `[fixed v115]`; changelog; README row; Tier counts |

## Testing

1. **Integration** (`tests/bare_local_unset_integration.rs`, binary-driven):
   - `f(){ local x; [[ -v x ]] && echo SET || echo UNSET; }; f` → `UNSET`.
   - `f(){ local x; echo "[${x-DEF}]"; }; f` → `[DEF]`.
   - bare-local-then-assign + restore: `x=outer; f(){ local x; x=5; echo "in=$x"; }; f; echo "out=$x"` → `in=5` / `out=outer`.
   - `local x=` still set: `f(){ local x=; [[ -v x ]] && echo SET || echo UNSET; }; f` → `SET`.
   - `local x=val`: `f(){ local x=v; echo "$x"; }; f` → `v`.
   - multiple bare: `f(){ local a b; [[ -v a ]] || [[ -v b ]] && echo someset || echo allunset; }; f` → match bash.
   - the bash_completion shape: a function with `local vcur vcword`; build `upvars` via `[[ -v vcword ]]` and assert no empty element / no `not a valid identifier`.
   Verify each against the system bash first.
2. **39th bash-diff harness** `tests/scripts/bare_local_unset_diff_check.sh` —
   byte-identical fragments for the above (using `${x-DEF}` / `[[ -v ]]` / `$#`
   readouts so output is deterministic).
3. **Regression**: full suite (2806+), all 39 harnesses, clippy clean. Watch the
   `functions`/`function_keyword`/`declare`/`local`/`arrays` suites.
4. **Payoff**: `mise ` + `<TAB>` (or sourcing bash_completion's
   `_get_comp_words_by_ref -n : cur prev` shape) no longer prints
   `bash_completion: : : invalid option` / `local: '': not a valid identifier`.
   Report before/after.

## Edge cases & notes
- **`local x` when `x` was already set in the SAME frame** (e.g. a prior `local
  x=1; local x`): bash re-declares it unset; with the snapshot's `already_saved`
  guard the frame keeps the first snapshot (outer value) and the unset applies —
  verify it matches bash (re-`local` of an already-local name unsets it).
- **`local -a NAME` `[[ -v NAME ]]`**: an empty indexed array is "unset" for `-v`
  in bash (no element 0). huck creates an empty array; if `[[ -v NAME ]]` then
  diverges, it is a separate pre-existing array-`-v` edge, out of scope here
  (the bug is the scalar bare-local set-empty).
- The fix is symmetric with huck's `declare NAME` (no value), which already
  leaves the variable unset.
