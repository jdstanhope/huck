# v160: `declare -n` / `local -n` namerefs

**Status:** design approved 2026-06-15
**Scope:** comprehensive-core nameref support. The final remaining declaration
attribute (after v158 `-l`/`-u`). Builds directly on the v159 assignment-path
unification (the write seam) and hooks the read path.

## Motivation

A nameref (`declare -n ref=target`) makes `ref` an indirect reference to
another variable: reads of `$ref` read `$target`, writes to `ref` write
`target`. The dominant real-world use is pass-by-reference in functions
(`local -n out=$1`). huck currently rejects `-n` in all four declaration
builtins (`declare: -n: not yet implemented`). v160 implements it. v159
deliberately carved the write-side seam (`resolve_assign_target`) for exactly
this; v160 fills it in and adds the parallel read-side resolution.

## Bash behavior (verified empirically against bash 5.x)

- **Basic scalar**: `declare -n r=x; x=hello; echo "$r"` → `hello`; `r=bye` →
  sets `x` to `bye`.
- **`declare -p`**: `declare -n r=target` → `declare -n r="target"` (the
  variable's value IS the target-name string). Element target:
  `declare -n e=arr[0]` → `declare -n e="arr[0]"`.
- **`${!r}` on a nameref yields the TARGET NAME** (not value-as-name
  indirection): `x=val; declare -n r=x; echo "$r ${!r}"` → `val x`.
- **`unset -n r`** unsets the nameref itself (target survives); **`unset r`**
  resolves and unsets the TARGET (the nameref variable remains, now pointing at
  an unset name).
- **Direct self-reference** `declare -n r=r` → hard error rc 1
  (`nameref variable self references not allowed`).
- **Cycle** formed across declarations/assignments (`a→b→a`, or `declare -n r;
  r=r`) → `circular name reference` WARNING to stderr, empty resolution, rc 0.
- **Chains**: `declare -n a=b; declare -n b=c; c=5; echo "$a"` → `5`; `a=9` sets
  `c`. Resolution follows the full chain on both read and write.
- **Array-element target**: `arr=(p q r); declare -n e=arr[1]; echo "$e"` → `q`;
  `e=Q` sets `arr[1]`.
- **Whole-array nameref**: `arr=(a b c); declare -n r=arr` → `${r[1]}`=`b`,
  `${r[@]}`=`a b c`, `${#r[@]}`=`3`, `${!r[@]}`=`0 1 2`, bare `$r`=`a` (= arr[0]).
- **`local -n` pass-by-ref**: `f(){ local -n out=$1; out=filled; }; v=empty; f v;
  echo "$v"` → `filled`.
- **`read` into a nameref**: `declare -n r=x; printf 'hi\n' | { read r; }` sets
  `x` to `hi`.
- **Bind vs deref**: `declare -n r` (no target) then `r=x` BINDS the target
  (r now points at `x`; `declare -p r` → `declare -n r="x"`). Once bound,
  assignment dereferences.
- **Reassign**: `declare -n r=x; declare -n r=y` re-targets r to `y`.
- **Invalid target name**: `declare -n r="a b"` → rc 1
  (`invalid variable name for name reference`).
- **`[[ -v r ]]`** tests the TARGET's set-ness (false when target unset, true
  once set).
- **`for r in …` through a nameref**: bash itself does NOT route the loop
  assignment through the nameref (the target stays unchanged and `$r` reads
  empty inside the loop) — an inconsistent bash corner. OUT OF SCOPE (see below).

## Architecture

### Storage model
Add `nameref: bool` to the `Variable` struct (alongside `exported`/`readonly`/
`integer`/`case_fold`). A nameref variable's `value` is `Scalar(target)` where
`target` is the target NAME — a plain identifier (`x`) or a subscripted element
(`arr[0]`). This is bash's exact model and makes `declare -p` trivial. No new
`VarValue` variant.

Update every `Variable { … }` struct literal + `Variable::scalar` to add
`nameref: false` (compiler-driven, as in v158 Task 1). Add a `nameref_target_of`
reader and a `set_nameref`/attribute mutator mirroring `case_fold_of`/
`set_case_fold`.

### The resolution helper
A single method resolves a name through the nameref chain:

```
enum ResolvedName {
    /// Plain variable target (possibly the original name if not a nameref).
    Name(String),
    /// Element target: `arr[subscript-text]` — subscript evaluated at use site.
    Element { name: String, subscript: String },
    /// Unbound nameref (attribute set, empty target) — no destination yet.
    Unbound(String),     // carries the nameref's own name (for bind-on-assign)
    /// A cycle was detected — warning already emitted; resolves to nothing.
    Cycle,
}

fn resolve_nameref(&self, name: &str) -> ResolvedName
```

Algorithm: start at `name`; while the current variable has the `nameref`
attribute and a non-empty value, parse its value (a name or `name[sub]`), track
visited names in a set, and follow to the next name. Stop when the current name
is not a nameref (→ `Name`/`Element` depending on whether the last link carried a
subscript), or empty (→ `Unbound`), or revisited (→ emit `circular name
reference` warning, return `Cycle`). A non-nameref `name` returns
`Name(name)` unchanged (the helper is a safe no-op for ordinary variables, so
call sites can resolve unconditionally).

`resolve_nameref` is `&self` (pure lookup) and is called wherever a name maps to
storage.

### Read-side hooks
- `lookup_var(name)`: resolve first; read the effective target. Resolving to an
  array → return the scalar view (`arr[0]`). `Cycle`/`Unbound` → `None`.
- Array-shape readers — `lookup_array_element`, the `${r[@]}`/`${r[*]}`/
  `${#r[@]}`/`${!r[@]}` paths in param-expansion: resolve `r`→`arr` then operate
  on the target array. An element-target nameref (`e=arr[0]`) read as a scalar
  resolves to that element.
- `${!r}` special case: in the `${!…}` handler (M-91), if the inner name is a
  nameref, return its TARGET NAME (the raw stored value) instead of value-as-name
  indirection. Ordinary `${!var}` is unchanged.
- `[[ -v r ]]` / `is_set` / `${r+…}` / `${r:-…}`: resolve, then test/default on
  the target. Most route through `lookup_var`/`is_set` already; add the resolve
  there.

### Write-side hook (the v159 seam)
`resolve_assign_target(dest)` (currently identity) becomes a `resolve_nameref`
call:
- `Whole(r)` where `r`→`x` → `Whole(x)`; where `r`→`arr[i]` →
  `Element{arr, Index/Key}` (evaluate the subscript text in the target's shape).
- **Bind-on-assign**: if `r` is an `Unbound` nameref, the assignment must set
  `r`'s OWN value (the target name) instead of dereferencing. The seam returns a
  signal (e.g. `Whole(r)` with a "bind" marker, or the funnel checks
  `Unbound` before applying attributes) so `assign()` stores the raw target name
  into `r`. This is the one place the seam is more than a name rewrite.
- `Cycle` → the warning is emitted and the write is dropped (rc 0, matching the
  cycle read behavior).

Because every assignment routes through the funnel (v159), this single seam
change makes `r=v`, `r+=v`, `r[i]=v`, `read r`, `printf -v r`, `getopts … r`, and
`local -n out=$1; out=…` all deref correctly.

### `unset` / `unset -n`
- `unset r`: resolve `r`→target, unset the target (the nameref variable stays).
- `unset -n r`: add the `-n` flag; unset the nameref variable `r` itself, no
  resolution.
- `unset arr[i]` where the name is a nameref to an array: resolve then unset the
  element.

### `read`
`read`/`read -a` already assign through the funnel-backed setters, so they deref
for free once the seam resolves. Confirm `read -a r` (r→arr) targets the array.

### Declaration builtins
The four declaration paths (`builtin_declare`, `builtin_declare_decl`,
`builtin_local`, `builtin_local_decl`) currently error on `b'n'`. Replace with:
- Set the `nameref` attribute on each named variable.
- If a value is given (`declare -n r=target`): validate the target name (a valid
  identifier or `name[sub]`); reject `declare -n r=r` (direct self-ref, rc 1);
  reject an invalid target name (rc 1). Store the target name as the variable's
  value WITHOUT dereferencing (declaring a nameref binds it; it does not assign
  through).
- `declare +n r`: remove the nameref attribute.
- `local -n` is function-scoped and restored on return (the attribute is a
  `Variable` field, so the existing local-scope snapshot/unwind handles it).

### `declare -p`
Emit `n` in the attribute flags and the raw target value:
`declare -n r="target"`. Flag ordering: place `n` consistently with bash
(verify against bash; bash shows `declare -n` typically alone, but combined with
other attrs follow bash's order). The value is the raw stored target name, never
dereferenced.

## Out of scope (deferred, logged as low divergences)

- **`for r in …` through a nameref** — bash's own behavior is inconsistent (the
  loop var doesn't route through). Match bash's common behavior elsewhere; log
  this corner rather than replicate a bash bug.
- **`[[ -R name ]]`** nameref test operator — completeness; rarely used. Log as
  a follow-on. (`[[ -v ]]` IS in scope.)
- **Arbitrary-depth chain pathologies** beyond what the visited-set handles
  uniformly — the cycle detector covers correctness; no special deep-chain work.

## Error handling

- Declare-time self-ref and invalid-target-name → rc 1, `huck:`-prefixed message
  (stderr-prefix divergence class; rc + stdout match bash).
- Resolve-time cycle → `huck: warning: NAME: circular name reference` to stderr,
  empty resolution, rc 0.
- All stderr wording uses huck's prefix; harnesses compare rc + stdout only.

## Testing strategy

`tests/scripts/nameref_diff_check.sh` (gold-standard byte-identical bash↔huck,
stdout + rc) covering at minimum:
1. scalar read + write through a nameref.
2. `declare -p` of a nameref (scalar target + element target).
3. `${!r}` yields target name; `$r` yields target value.
4. `unset r` (target) vs `unset -n r` (the ref).
5. chain `a→b→c` read + write.
6. array-element target read + write.
7. whole-array nameref: `${r[i]}`, `${r[@]}`, `${#r[@]}`, `${!r[@]}`, bare `$r`.
8. `local -n out=$1` pass-by-ref (incl. passing an array by name).
9. `read r` and `read -a r` into a nameref.
10. bind-vs-deref: `declare -n r; r=x` binds; second assign derefs.
11. reassign target; `declare +n` removal.
12. `[[ -v r ]]` on unset vs set target.
13. circular warning → rc 0, empty (stderr not compared).
14. direct `declare -n r=r` and invalid target → rc 1 (stderr not compared).

Plus Rust unit tests on `resolve_nameref`: plain (no-op), single hop, multi-hop
chain, cycle, element target, unbound.

Full `cargo test` + all existing harnesses stay byte-identical green (nameref is
additive — a non-nameref variable resolves to itself, so existing behavior is
unchanged).

## Components touched

- `src/shell_state.rs` — `Variable.nameref` field + literals; `ResolvedName` +
  `resolve_nameref`; `nameref_target_of`/`set_nameref`; `lookup_var` resolve
  hook; array-reader resolve hooks; `resolve_assign_target` seam (incl.
  bind-on-assign); `is_set` resolve.
- `src/param_expansion.rs` — `${!r}` target-name fork; array-shape (`${r[@]}`
  etc.) resolve hooks; modifier paths (mostly automatic via `lookup_var`).
- `src/builtins.rs` — `-n`/`+n` parsing in the four declaration builtins;
  `declare -p` `n`-flag emission; `unset -n` flag + resolve.
- `src/executor.rs` — confirm `read`/`for`/`[[ -v ]]` route correctly (mostly
  via the funnel + `lookup_var`).
- `tests/scripts/nameref_diff_check.sh` + Rust unit tests.
