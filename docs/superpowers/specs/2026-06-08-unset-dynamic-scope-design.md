# huck v118 — `unset -v` dynamic-scope reveal/pop (M-115) Design

**Status:** approved design, ready for implementation plan.
**Implements:** bash's dynamic-scope `unset -v` semantics — `unset` of a variable
that is `local` to an *enclosing* function pops that one scope level and reveals
the next-enclosing binding, so the bash "upvar" idiom (`unset -v NAME; eval
NAME=value` in a callee writing into a caller's variable) works across an
intervening `local` declaration. This is **M-115** (Tier-1 bug).
**Why now:** it is the last known `mise<TAB>` blocker. bash_completion's
`_upvars` is called from `__get_cword_at_cursor_by_ref` (which declares `local
words cword`), itself called from `_get_comp_words_by_ref` (which also declares
`local words cword`). `_upvars` does `unset -v "$2" && eval $2=…` to write the
result back up the stack. huck loses the value, so `__get_cword_at_cursor_by_ref`
propagates an empty `words`/`cword` → `_init_completion` returns rc 1 → no
completion.
**Branch (impl):** `v118-unset-dynamic-scope`.

## Background — the bug (verified against bash this session)

huck's scope model is a flat `vars: HashMap<String, Variable>` plus a stack of
restore-snapshots `local_scopes: Vec<HashMap<String, Option<Variable>>>` (one
frame per active function call; `local NAME` records the pre-`local` value into
the top frame; `call_function` restores every snapshot on return). `Shell::unset`
only does `vars.remove(name)` — it never consults `local_scopes`. So when a
callee unsets a variable that an *enclosing* function declared `local`, that
enclosing frame's snapshot still fires on return and **clobbers** any value a
subsequent assignment wrote.

Probed bash behaviors (all run as file-arg scripts; `outer` has `x=orig`):

| # | fragment (abbrev) | bash | huck (now) |
|---|---|---|---|
| A | `inner(){ unset -v "$1"; eval $1=VAL; }; mid(){ local x=midval; inner x; }; outer(){ local x=orig; mid x; echo $x; }` | `VAL` | `orig` |
| B | `inner(){ unset -v "$1"; }; mid(){ local x=midval; inner x; echo ${x-U}; }; outer{…}` | mid `orig`, out `orig` | mid `U`, out `orig` |
| C | `inner(){ local x=innerlocal; unset -v "$1"; eval $1=VAL; }; outer(){ local x=orig; inner x; echo $x; }` | `orig` | `orig` ✓ |
| D | 3 intervening locals (`a`,`b` each `local x`), `leaf` unsets+evals | `orig` | `orig` ✓ |
| E | global `x`, `inner` unsets+evals (no locals) | `VAL` | `VAL` ✓ |
| F | `inner(){ local x=innerv; unset -v x; echo ${x-U}; }` | `U` (no reveal) | `U` ✓ |
| G | `unset` reaches past an intervening NON-local frame (`pass`) to the nearest enclosing local | mid `VAL`, out `VAL` | out `orig` |
| H | `unset` caller-local, then caller plain-reassigns after callee returns | mid/out `reassigned` | out `orig` |

**Derived bash rule.** `unset -v NAME` acts on the **nearest dynamically-visible
binding**:
- If NAME is local to the **current** function (top frame): the value is unset
  but the name stays local to the current function — a read shows UNSET (F), a
  later assignment re-localizes, and on return the enclosing binding is restored
  (C). This is huck's current `vars.remove` behavior — already correct.
- If NAME is **not** local to the current function but **is** local to some
  enclosing function: `unset` removes that enclosing function's local (popping
  exactly one level — D), revealing the next-enclosing binding (B). The pop is
  permanent for that frame: it will not restore on return, so a subsequent
  assignment promotes upward (A, G) and a caller's later plain assignment also
  targets the revealed binding (H).
- If NAME is not local anywhere: plain global unset (E).

## Architecture — a scope-aware `unset_var` for the `unset` builtin only

Add `Shell::unset_var(name)` implementing the rule, and route ONLY the `unset`
builtin's variable path through it. `Shell::unset` (the plain `vars.remove`)
stays unchanged for its many internal callers (v115 bare-`local`, `OPTARG`
reset, `PWD`/`OLDPWD`, `COMPREPLY`, test helpers) — none of which should do a
dynamic-scope reveal. This is surgical and reuses the existing snapshot model
rather than rewriting variable storage.

### Component 1 — `Shell::unset_var` (`src/shell_state.rs`, beside `unset`)
```text
fn unset_var(name):
    # nearest frame, innermost-first, holding a snapshot for `name`
    find topmost index i in local_scopes (from last to first) with frame[i].contains_key(name)
    match:
      i == last index (top / current function) OR no such i:
          vars.remove(name)                 # current-fn-local or global: plain unset, keep snapshot
      i is an ENCLOSING frame (i < last):
          snap = local_scopes[i].remove(name)   # Option<Option<Variable>>; pop that frame's local
          match snap:
              Some(Some(var)) => vars.insert(name, var)   # reveal the shadowed binding
              Some(None)      => vars.remove(name)         # shadowed binding was unset → stays unset
              None            => unreachable               # we only pop a frame we found has the key
```
Snapshots are `Option<Variable>`, so reveal restores scalars, indexed arrays,
and associative arrays uniformly. Removing the enclosing frame's entry is what
prevents the clobber on that frame's return.

### Component 2 — wire the `unset` builtin
`builtin_unset` (`src/builtins.rs:526`): the variable path (currently
`shell.unset(arg)` at ~`:616`, reached for `-v` and the default no-flag form)
calls `shell.unset_var(arg)` instead. The `-f` (function) path and the
subscripted-element path (`unset arr[i]`, `unset_array_element` /
`unset_associative_element`) are unchanged.

## Scope & correctness
- Only whole-variable `unset` (the builtin's `-v`/default path) changes. `unset
  -f`, `unset arr[i]`, and all internal `Shell::unset` callers are untouched.
- The current-function-local case (top frame has the snapshot) keeps huck's
  existing `vars.remove` behavior (C/F verified) — KEEP the snapshot so the
  local attribute persists and the enclosing binding restores on return.
- Pops exactly one dynamic-scope level (the nearest enclosing local), matching
  bash (D); intervening non-local frames are skipped (G).
- Reveal value is the snapshot the popped frame recorded — i.e. the binding it
  was shadowing — which equals bash's revealed enclosing value (B/H).

## Must-not-regress
- `unset` of a current-function local (reads UNSET after; outer restored on
  return) — cases C/F.
- `unset` of a global; `unset` of a never-set name (no-op).
- `unset -f func`; `unset arr[i]` / `unset assoc[key]`.
- v115 bare `local NAME` (records snapshot then `shell.unset` — stays on the
  plain path; and even via `unset_var` the top-frame-has-snapshot branch is
  identical `vars.remove`).
- `readonly` unset rejection: the readonly guard lives in `builtin_unset`
  (`if shell.is_readonly(arg) { eprintln!…; continue }`) BEFORE the
  `shell.unset(arg)` call, so swapping that call to `shell.unset_var(arg)`
  preserves the existing behavior unchanged (verified: huck already matches
  bash — rc 1, variable kept; only the `huck: unset:` vs `<script>: line N:
  unset:` message prefix differs, a pre-existing orthogonal divergence).
  `unset_var` itself needs no readonly check.
- Function-exit restore (`call_function`) — unchanged; it simply finds fewer
  entries in a frame whose local was popped by `unset_var`.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/shell_state.rs` | add `unset_var` beside `unset`/`restore_var`; unit tests |
| `src/builtins.rs` | `builtin_unset` variable path → `shell.unset_var` |
| `tests/unset_dynamic_scope_integration.rs` | NEW — cases A–H binary-driven vs bash |
| `tests/scripts/unset_dynamic_scope_diff_check.sh` | NEW — 42nd bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-115 `[fixed v118]`; changelog; README row; Tier counts |

## Testing
1. **Unit** (`src/shell_state.rs`): drive `unset_var` against constructed
   `local_scopes` configurations — current-frame-local (plain remove, snapshot
   kept), enclosing-frame-local (pop + reveal Some/None), skip-intervening
   non-local frame, no-frame global. Assert `vars` and the popped frame state.
2. **Integration** (`tests/unset_dynamic_scope_integration.rs`, binary vs bash,
   file-arg per L-27): cases A–H from the table, each byte-identical to bash.
3. **42nd bash-diff harness** `tests/scripts/unset_dynamic_scope_diff_check.sh`
   — A–H plus the readonly-unset and `unset -f`/`unset arr[i]` regression
   guards, byte-identical.
4. **Regression**: full suite (2847+), all 42 harnesses, clippy
   `--all-targets`. Watch `local`/`declare`/`unset`/`getopts` (OPTARG reset) /
   `cd` (PWD/OLDPWD) / completion suites — a regression means an internal caller
   was wrongly routed through `unset_var` or the reveal disturbed a kept
   snapshot.
5. **Payoff**: drive the real `_upvars` / `__reassemble_comp_words_by_ref` /
   `__get_cword_at_cursor_by_ref` / `_get_comp_words_by_ref` / `_init_completion`
   chain with `COMP_WORDS=(mise "")`, `COMP_CWORD=1`. Expect huck to print
   `SMOKE cur=[] prev=[mise] cword=1 nwords=2 w0=[mise]` (matching bash) with
   `_init_completion` rc 0 — i.e. **`mise<TAB>` functional end-to-end**. Report
   before/after. **Honest gate (the v109/v115/v116/v117 lesson):** the smoke is
   the gate; if a further gap surfaces, report it plainly at the merge gate and
   scope the next iteration rather than over-claiming.

## Edge cases & notes
- **Readonly**: handled by the existing guard in `builtin_unset` (before the
  unset call) — unchanged by this iteration (see Must-not-regress).
- **`unset` of a name local in the current frame that ALSO shadows an enclosing
  local**: only the current (top) frame matters for the first-branch decision;
  the enclosing frame's snapshot is left intact (restored normally on its own
  return). Matches bash (C).
- **Multiple names** (`unset a b c`): the builtin loops; each name routes through
  `unset_var` independently.
- This does not implement bash's full dynamic-scope variable model; it
  implements the `unset`-reveal/pop behavior the upvar idiom needs. Other
  dynamic-scope corners (if any surface later) are separate.
