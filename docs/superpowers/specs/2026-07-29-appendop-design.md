# v344 — flip the `appendop` bash-suite category

**Issue:** [#327 — appendop: assoc scalar-append, integer array element-0
append, runtime POSIXLY_CORRECT](https://github.com/jdstanhope/huck/issues/327).

**Goal:** flip the `appendop` bash-suite category to PASS (byte-identical
output) by fixing the three residual roots in the `+=` assignment path.
Target: full runner PASS 31 → 32.

## Background

After L-44 (#32 / v342) fixed associative iteration order, `appendop` shrank to
19 residual diff lines, all in the assignment path and decomposing into three
bounded roots. huck already handles the common `+=` cases correctly — a bare
scalar `+=` on an **indexed** array (`x=(1 2 3); x+=z` → `1z 2 3`), integer
scalar `+=` (`declare -i s=5; s+=3` → `8`), `set -o posix` persistence, and
startup `POSIXLY_CORRECT`. The three roots are the narrow gaps beside those.

## Root A — associative `arr+=scalar` appends to key `"0"`

`declare -A f=([a]=1); f+=zero` → bash adds `f[0]="zero"`; huck errors
`f: scalar append not valid on associative array` (appendop1.sub:17).

bash treats a bare scalar `+=` on an array (indexed **or** associative) as
`arr[0]+=scalar`. huck already does this for indexed arrays but the
associative-variant dispatch rejects it.

**Location:** `executor.rs` `apply_one_assignment`, the associative branch
(`if shell.get_associative(target_name).is_some()`), arm
`(AssignTarget::Bare(name), None)` — currently `sh_error_to!(… "scalar append
not valid on associative array")` for `a.append` and `"scalar assignment not
valid …"` otherwise.

**Fix:** expand the RHS (`param_expansion::expand_word_to_string(&a.value,
shell)`, matching the sibling associative element arm) and route to key `"0"`:
`append_associative_element(name, "0", &s)` when `a.append`, else
`set_associative_element(name, "0".to_string(), s)`. Both route through
`assign()`, so the Root B integer fix below applies automatically to integer
associative arrays.

**Correction (found during implementation):** the original spec claimed the
**non-append** case (`f=zero`) is a bash type-mismatch that should keep
erroring. That was wrong — verified against bash 5.2.21: `declare -A
f=([a]=1); f=zero` assigns `f[0]="zero"` and exits 0 (a bare scalar assignment
to an array name targets element `[0]` for associative arrays too, exactly like
indexed). So huck's pre-existing `scalar assignment not valid on associative
array` error was itself a divergence; Root A now fixes **both** the set and the
append forms.

## Root B — integer array element append honors `-i` (arithmetic add)

`declare -ai a=(2 2 3); a+=1` → bash `a[0]=3` (2 + 1 arithmetic); huck
`a[0]="21"` (appendop1.sub:24).

`arr+=scalar` on an indexed array routes through
`executor.rs`'s general append arm → `shell.append_indexed_element(name, 0,
&s)` → `assign()` with `AssignKind::Append`. The bug is in **`assign()`
itself**: the Element+Append arms (both `Subscript::Index` and
`Subscript::Key`) concatenate the existing value with the RHS **then**
`eval_integer_coerce` the whole string — so `coerce("2" + "1") = 21`, when bash
evaluates `a[0] + 1 = 3` arithmetically. This also mis-handles the explicit
`a[i]+=n` form on integer arrays, so fixing it centrally is both simpler and
more correct than a per-call-site patch.

**Location:** `shell_state.rs` `assign()`, the two Element+Scalar arms
(`AssignDest::Element { sub: Subscript::Index(idx) }` and
`{ sub: Subscript::Key(key) }`).

**Fix:** in the `op == AssignKind::Append` branch, when
`self.is_integer(&n)`, compute the sum arithmetically instead of concatenating:

```rust
let v = if op == AssignKind::Append {
    let existing = self.lookup_indexed_element(&n, idx).unwrap_or_default(); // or lookup_associative_element for the Key arm
    if self.is_integer(&n) {
        // Integer array: `arr[i]+=x` is arithmetic addition (bash), not concat.
        let base = if existing.is_empty() { "0".to_string() } else { existing };
        eval_integer_coerce(self, &format!("({})+({})", base, v))
    } else {
        existing + &v
    }
} else {
    v
};
```

The subsequent `if self.is_integer(&n) { eval_integer_coerce(&v) }` line then
re-coerces the already-numeric string (`coerce("3") = "3"`), a harmless no-op —
no double-add. Non-integer arrays are untouched (still concatenate). The
`base` empty-string guard makes a first append (`declare -Ai f; f+=1`) evaluate
`(0)+(1)` rather than the syntax-error-to-`0` that an empty operand would give.

## Root C — runtime `POSIXLY_CORRECT` toggles posix mode

appendop2.sub:14 assigns `POSIXLY_CORRECT=1` mid-script, then relies on posix
mode for special-builtin prefix-assignment persistence
(`x+=5 eval …; echo "$x"` → bash `25`, huck `2`). huck honors
`POSIXLY_CORRECT` only at **startup** (`startup_posix` in `shell.rs`); bash
re-checks it on every assign and unset.

Confirmed: `POSIXLY_CORRECT=1 huck -c '…'` (startup) is already correct;
`huck -c 'POSIXLY_CORRECT=1; …'` (runtime) is not.

**Fix — single chokepoint via the existing `reseed_special_on_assign` hook.**
Both scalar-store paths (`store_scalar` and `export_set`, the latter covers
the inline env-prefix `POSIXLY_CORRECT=1 cmd` form) already call
`reseed_special_on_assign(name, value)` before storing. Add a
`"POSIXLY_CORRECT"` case that sets `self.shell_options.posix = true` and
returns **`false`** (unlike RANDOM/SECONDS/BASH_ARGV0, which return `true` to
suppress storage) — POSIXLY_CORRECT is a real environment variable and must
still be stored. One case, both paths covered.

**Unset direction (bash correctness):** unsetting `POSIXLY_CORRECT` turns posix
mode back off in bash. Add a guarded clear (`if name == "POSIXLY_CORRECT" {
self.shell_options.posix = false; }`) to the unset paths (`unset_var` — the
`unset` builtin's path — and `unset` for internal callers). The name guard
makes it a no-op for every other variable.

**Interaction with `set -o posix`:** bash binds the two — `set -o posix`
enables posix mode and `unset POSIXLY_CORRECT` disables it — so clearing on
unset matches bash and does not regress the `set -o posix` path (that path sets
the flag directly and is unaffected by an assign/unset of a *different*
variable). The appendop test exercises only the *assign* direction; the unset
clear is included for correctness and is verified not to regress any category.

## Verification

- **Official `appendop` runner** produces zero diff (the flip signal).
- **Diff-check harness:** extend an existing assignment harness (or add an
  `appendop`-shaped `_diff_check.sh`) with one fragment per root:
  - assoc `arr+=scalar` → `[0]`; assoc integer `arr+=n` → arithmetic on `[0]`;
  - indexed integer `arr+=n` → arithmetic on `[0]`; indexed non-integer append
    (regression guard — must stay concat);
  - runtime `POSIXLY_CORRECT=1` then prefix-assignment persistence; and
    `unset POSIXLY_CORRECT` turning it back off.
- **Unit tests** for the integer-append arms in `assign()` (indexed +
  associative arithmetic-add; non-integer concat unchanged; first-append
  empty-base → `0`) and for `reseed_special_on_assign` returning `false` for
  POSIXLY_CORRECT while toggling the flag.
- **No-regression:** Root C toggles a global mode flag, so run the full
  bash-suite runner and confirm PASS moves **31 → 32** with no category
  regressing; run `tests/scripts/run_diff_checks.sh` green; `cargo test
  -p huck-engine` (lib) + the assoc/declare/array integration bins.

## Scope / non-goals

- Root A covers both `f=zero` (set) and `f+=zero` (append) → key `"0"`, per the
  correction above (bash accepts both).
- NOT a broader posix-mode audit — Root C wires only the assign/unset toggle
  of the `posix` flag; the flag's existing single consumer
  (special-builtin prefix-assignment persistence) is unchanged.
- The broad assoc/append cluster (#323 — `assoc`/`quotearray` residuals) stays
  open; this iteration flips only `appendop`.

## Summary of touched files

- `crates/huck-engine/src/executor.rs` — Root A: assoc bare-scalar arm routes
  `f=v`/`f+=v` to key `"0"` (`set_`/`append_associative_element`).
- `crates/huck-engine/src/shell_state.rs` — Root B: `assign()` Element+Append
  integer arms arithmetic-add. Root C: `reseed_special_on_assign`
  POSIXLY_CORRECT case (returns `false`); `unset`/`unset_var` guarded clear.
- `tests/scripts/*_diff_check.sh` — new/extended harness fragments.
