# v223 — `declare -xF` export filter/format + FUNCNAME write protection

## Status

Design approved 2026-06-25. Two independent `func`-category correctness fixes
(L-61 blockers a + b). Clears two of func's three remaining blockers; func will
NOT flip (blocker c, FUNCNEST, remains).

## Background

L-61 lists three remaining `func` blockers. This iteration takes (a) and (b):

### (a) `declare -F` / `-xF` export filter + format

Two bugs in huck's function-listing path, both measured against bash 5.2.21:

1. **Format** — bash reflects the export attribute in the `-F` listing form:
   a non-exported function lists as `declare -f NAME`, an exported one as
   `declare -fx NAME`. This holds for plain `declare -F` (no `-x`). huck always
   prints `declare -f NAME`.
2. **Filter** — `declare -xF` should list ONLY exported functions; huck ignores
   `-x` and lists every function. This is the func-category hunk: with no
   exported functions, bash prints nothing but huck prints 7 spurious
   `declare -f a` … `declare -f f1` lines.

`declare -xf` (body form) already filters correctly — it routes through
`list_exported_functions`. Only the names-only `-F` path misses the filter and
the format. The explicit-name form `declare -F NAME` (bare name output) is
correct and unchanged.

### (b) FUNCNAME write protection

bash silently discards EVERY write to `FUNCNAME` (rc 0, no error):
`FUNCNAME=7`, `+=`, `FUNCNAME[0]=x`, `for FUNCNAME in …`, `read FUNCNAME`, and
writes from inside a function. Outside a function `$FUNCNAME` is empty; inside,
it reflects the real call stack. huck lets `FUNCNAME=7` persist because the
call-stack rebuild (`rebuild_call_stack_vars`) only runs on function entry/return
— at top level a user write is never overwritten. The func-category hunk:
`FUNCNAME=7; echo $FUNCNAME` prints `7` (huck) vs empty (bash).

## Goals

1. `declare -F` (listing) prints `declare -fx NAME` for exported functions,
   `declare -f NAME` otherwise.
2. `declare -xF` lists only exported functions (in the `declare -fx NAME` form);
   with none exported it prints nothing.
3. Writes to `FUNCNAME` are silently discarded (rc 0, no error) through all user
   write paths — assignment (`=`, `+=`, `[i]=`), `for`, and `read`. `$FUNCNAME`
   continues to reflect the call stack (empty at top level).
4. No regression: `declare -f`/body listings, `declare -F NAME` (explicit),
   `declare -xf`, and non-FUNCNAME variables are byte-identical to before; no
   currently-PASS bash-test category regresses.

## Non-goals / Out of scope

- **func blocker (c) FUNCNEST** — not in this iteration. func stays FAIL (one
  blocker away).
- **BASH_SOURCE / BASH_LINENO write protection** — bash protects these
  identically (same call-stack-array mechanism), but unlike FUNCNAME they are
  POPULATED at top level, so guarding their writes risks breaking legitimate
  initialization. Deferred as a documented follow-on; this fix is complete for
  FUNCNAME alone.
- No new `declare` flags or output modes beyond the `-F`/`-xF` format+filter.

## Design

### Component 1 — `builtins.rs` function listing

- `emit_function` (~1070): in the names-only listing branch
  (`names_only && !explicit`), choose the header by export attribute:
  `declare -fx {name}` if `shell.is_function_exported(name)`, else
  `declare -f {name}`. (The explicit-name branch — bare `{name}` — and the body
  branch are unchanged.)
- `declare_list_functions` (~1037): add a `want_export: bool` parameter. In the
  no-names listing loop, skip functions where `!shell.is_function_exported(n)`
  when `want_export` is set. (Explicit-name args are unaffected — match bash,
  which applies the export filter only to the bulk listing.)
- Dispatch (~1807): pass the already-parsed `want_export` into
  `declare_list_functions`.

`shell.is_function_exported(name) -> bool` already exists
(`shell_state.rs:2161`).

### Component 2 — `shell_state.rs` FUNCNAME guard

- Add `Shell::set` (~999) and `Shell::assign` (~1585) early guards mirroring the
  existing restricted-mode `check_special_assign` pattern: if the (resolved)
  target name is `FUNCNAME`, return success WITHOUT writing — `set` returns,
  `assign` returns `Ok(())`. In `assign`, place the guard AFTER target
  resolution (so `FUNCNAME[0]=x` and a nameref to FUNCNAME are caught via the
  resolved `name`).
- Use a single private predicate `fn is_write_protected_var(name: &str) -> bool`
  (returns `name == "FUNCNAME"`) so the sibling arrays can be added later in one
  place.
- Verify `read`'s variable setter (`builtin_read`) routes through `set`/`assign`;
  if it writes via a lower-level path, apply the same guard there so
  `read FUNCNAME` is also protected.
- The call-stack rebuild (`rebuild_call_stack_vars` → `set_indexed_var` /
  `vars.remove`) does NOT go through `set`/`assign`, so FUNCNAME population is
  unaffected.

## Testing / Verification

- **Unit tests** (`shell_state.rs`): `set("FUNCNAME", …)` and `assign` to
  FUNCNAME are no-ops (value stays unset / call-stack value); a non-protected var
  still writes; the rebuild still populates FUNCNAME inside a simulated call
  stack.
- **Unit tests** (`builtins.rs`): `emit_function` names-only emits `declare -fx`
  for an exported function and `declare -f` for a plain one; `declare_list_functions`
  with `want_export=true` lists only exported.
- **Diff harnesses** vs live bash 5.2.21: a new `declare_func_export_diff_check.sh`
  (plain `-F`, `-xF`, `-xf`, mixed exported/plain) and a
  `funcname_assign_diff_check.sh` (`FUNCNAME=7`, `for FUNCNAME`, `read FUNCNAME`,
  inside-function assign, exit code) — all byte-identical.
- `cargo test --workspace` green (~3697).
- Re-run `func` (diff shrinks: the `declare -f a…f1` block and the
  `outside: FUNCNAME = 7` hunk gone) and `cprint`/`herestr` (no regression).
  Record whether any category incidentally flips (not predicted).

## Risks

- **FUNCNAME guard over-broad.** A legitimate internal `shell.set("FUNCNAME", …)`
  would be silently dropped. Mitigation: the rebuild uses `set_indexed_var`, not
  `set`/`assign`; a grep confirms no maintenance path writes FUNCNAME via
  `set`/`assign`. The unit test "rebuild still populates FUNCNAME" guards this.
- **`-x` filter applied too widely.** Restrict the filter to the no-names listing
  branch; leave explicit `declare -F NAME` untouched (matches bash). Guarded by a
  `declare -F NAME` no-regress harness fragment.

## Divergence-doc bookkeeping (on merge)

- Update `docs/bash-divergences.md` L-61: remove blockers (a) and (b); re-letter
  FUNCNEST as the sole remaining blocker; note func is now one blocker from PASS.
- Add a low-severity `[deferred]` note that BASH_SOURCE/BASH_LINENO writes are not
  yet protected (top-level-population caveat).
- Update the iteration memory.
