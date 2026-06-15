# v159: Unify the variable-assignment path

**Status:** design approved 2026-06-15
**Type:** structural refactor (behavior-preserving)

## Motivation

Setting a variable in huck has no single chokepoint. An "assignment" fans out
across six storage mutators on `Shell` — `try_set` (scalar + scalar `+=`),
`set_array_element`, `append_array_element`, `set_associative_element`,
`append_associative_element`, `replace_array`, `replace_associative`,
`extend_indexed` — plus a raw `set()` with ~124 call sites. The cross-cutting
concerns (readonly check, integer coercion, case-fold) are **applied at the
leaves**, duplicated and scattered: `try_set` does readonly + integer + case-fold,
`set_array_element` does readonly + case-fold, `replace_array` does case-fold,
raw `set()` does none.

This is why v158 (`declare -l`/`-u`) had to touch six mutators, and why two
folding gaps slipped through to review (`replace_associative` and `getopts`'s raw
`set()`). The next cross-cutting attribute — **v160 nameref (`declare -n`)**,
which must intercept every write — would face the same scatter. This iteration
extracts the missing abstraction: a single assignment funnel where every
cross-cutting concern is applied exactly once, so v160 becomes a one-place hook
instead of an N-leaf scatter.

## Goals

1. Introduce one chokepoint, `Shell::assign(dest, op, source)`, that applies all
   cross-cutting assignment logic in a fixed order, then dispatches to
   shape-specific storage primitives.
2. Reduce the existing leaf mutators to thin wrappers over `assign()` (Approach A),
   so the ~115 existing mutator call sites keep their names and signatures — the
   diff is "move logic up + add wrappers," not "rewrite every caller."
3. Carve an explicit, identity (no-op) target-name-resolution seam inside the
   funnel where v160 nameref will hook.
4. Ensure no value-producing user assignment path bypasses the funnel via raw
   `set()` (the `getopts`-class bug becomes structurally impossible).

## Non-goals (explicit)

- **Behavior changes of any kind.** This is a PURE refactor: the funnel must
  produce byte-identical results to today's code for every input. The existing
  full `cargo test` suite and all 85 `tests/scripts/*_diff_check.sh` harnesses
  staying green is the correctness proof — any output diff is a bug, not an
  intended change.
- **Not fixing L-43** (readonly assignment doesn't abort a non-interactive
  shell) — even though the funnel is the natural enforcement point. Separate
  future iteration.
- **Not fixing L-46** (bare attribute-only declaration prints `x=""`). Separate.
- **Not implementing nameref.** Only the identity seam is carved; the deref
  logic is v160.
- **Not unifying the leaf storage data structures.** `Scalar`/`Indexed`/
  `Associative` are genuinely different (`String`/`BTreeMap`/`Vec`); the funnel
  centralizes the cross-cutting LOGIC, not the storage representation.
- **Not "fixing" the existing attribute INCONSISTENCY.** Today integer
  coercion applies only on the scalar path (existing integer-flagged `Scalar`
  target), while case-fold applies on all value-bearing stores. The funnel must
  preserve this EXACTLY — it must NOT begin integer-coercing array elements
  (that would be a behavior change, however "more consistent" it looks).

## Architecture

### The funnel types

```rust
/// Where a value lands.
pub enum AssignDest {
    /// Whole variable: `name=…`, `name=(…)`, `read -a name`, `mapfile name`.
    Whole(String),
    /// A single element with an ALREADY-RESOLVED subscript.
    Element { name: String, sub: Subscript },
}

/// A subscript resolved by the caller (which holds expansion context). The
/// caller chooses Index vs Key based on the target variable's current shape,
/// exactly as `apply_one_assignment` does today (indexed/new → Index via
/// arith eval; associative → Key via string eval).
pub enum Subscript {
    Index(usize),
    Key(String),
}

/// Replace vs append (`=` vs `+=`).
pub enum AssignKind { Set, Append }

/// The value(s) to store, already fully expanded by the caller.
pub enum AssignSource {
    Scalar(String),
    Indexed(BTreeMap<usize, String>),       // array literal / read -a / mapfile
    Associative(Vec<(String, String)>),     // assoc literal
}
```

`Word`/expansion never enters the funnel. Expansion and subscript resolution
stay with the caller (they need shell/executor context); the funnel takes only
resolved primitives. This keeps the funnel a pure `Shell`-level operation.

### The funnel method

```rust
pub fn assign(
    &mut self,
    dest: AssignDest,
    op: AssignKind,
    source: AssignSource,
) -> Result<(), AssignErr>
```

Fixed order of operations (this ordering IS the value of the funnel):

1. **Resolve the target** through the nameref seam:
   `let dest = self.resolve_assign_target(dest);` — see "Nameref seam" below.
   Today this returns `dest` unchanged.
2. **Readonly check** on the resolved target name, once. For whole-array sources
   this happens before any element is stored, preserving today's
   "no partial write on a readonly array" guarantee (the executor's current
   pre-checks).
3. **Apply attributes to the value(s)**, replicating today's exact conditions:
   - **Integer coercion** — ONLY when `source` is `Scalar` AND the target is an
     existing integer-flagged `Scalar` (the current `try_set` condition). Not on
     array elements (preserve today's behavior).
   - **Case-fold** — fold a `Scalar` source; fold each VALUE of an `Indexed`/
     `Associative` source (never the key/subscript). Applied after integer
     coercion. This matches v158.
4. **Dispatch to a private storage primitive** that performs the actual
   data-structure mutation (the current leaf bodies, with their attribute logic
   removed since step 3 now owns it): scalar set/append (incl. the
   "`a=v` on an indexed array overwrites element 0" rule and scalar→indexed
   promotion), indexed element set/append, indexed whole-replace / extend,
   associative element set/append, associative whole-replace.

`reseed_special_on_assign` (the RANDOM/SECONDS interception) moves into the
shared scalar-storage primitive so BOTH `assign()` and raw `set()` honor it
(they both do today). It is NOT an attribute — it precedes storage.

### Leaf mutators become thin wrappers

Each existing public mutator keeps its signature and becomes a one-liner over
`assign()` (so the ~115 call sites are untouched):

| Wrapper | Delegates to |
| --- | --- |
| `try_set(name, v)` | `assign(Whole(name), Set, Scalar(v))` |
| `set_array_element(name, i, v)` | `assign(Element{name, Index(i)}, Set, Scalar(v))` |
| `append_array_element(name, i, v)` | `assign(Element{name, Index(i)}, Append, Scalar(v))` |
| `set_associative_element(name, k, v)` | `assign(Element{name, Key(k)}, Set, Scalar(v))` |
| `append_associative_element(name, k, v)` | `assign(Element{name, Key(k)}, Append, Scalar(v))` |
| `replace_array(name, map)` | `assign(Whole(name), Set, Indexed(map))` |
| `extend_indexed(name, map)` | `assign(Whole(name), Append, Indexed(map))` |
| `replace_associative(name, pairs)` | `assign(Whole(name), Set, Associative(pairs))` |

`try_set` keeps returning `Result<(), ()>` (readonly→`Err`); the array mutators
keep `Result<(), AssignErr>`. The wrappers adapt the funnel's `AssignErr` to each
existing return type so callers are unaffected.

### Raw `set()` stays — as the explicit no-attributes path

`set(name, value)` (the unchecked scalar writer) remains for the internal/shell-
maintained writes that intentionally bypass attributes and the readonly check
(env import, static builtin-variable installation, special-var maintenance,
`getopts`'s `OPTIND` numeric write). It delegates to the SAME private scalar-
storage primitive `assign()` uses (so `reseed_special_on_assign` + the indexed-
element-0 overwrite rule stay shared), but skips steps 1-3. Its role is
documented as "raw store, no attributes, no readonly check — for shell-internal
use only."

### Executor adapter

`apply_one_assignment` (executor.rs) keeps its job as the `Word` → `(dest, op,
source)` ADAPTER: it expands the RHS, resolves the subscript to `Index`/`Key`
based on the target's current shape (the existing
`get_associative(name).is_some()` dispatch), builds the `AssignSource`, and calls
the funnel (directly or via the wrappers). The type-mismatch diagnostics it
prints today (`scalar assignment not valid on associative array`, etc.) stay in
the adapter where the user-facing context lives.

### Value-producing builtins

`read`, `read -a`, `printf -v`, `getopts` (VAR/OPTARG), `mapfile`/`readarray`,
and the `${x:=default}` assignment-expansion already call `try_set` /
`replace_array` / `set_array_element` (verified in the v158 audit), so after the
wrappers delegate to `assign()`, they route through the funnel transitively.
This iteration's migration work is therefore: (a) confirm via audit that NO
value-producing USER path writes an attribute-eligible variable via raw `set()`
(only `OPTIND` and shell-internal special vars may), and (b) where any does,
switch it to the funnel. `getopts OPTIND` stays raw (numeric shell-maintained
var; matches the RANDOM/SECONDS class).

## Nameref seam

```rust
/// Resolves an assignment target through any nameref indirection. Identity
/// today (huck has no namerefs yet — v160). When `declare -n r=target` lands,
/// this is the ONE place that rewrites `r` → `target` (and, for
/// `declare -n r=arr[i]`, an `AssignDest::Whole(r)` into an
/// `AssignDest::Element{arr, Index(i)}`), with circular-reference detection.
fn resolve_assign_target(&self, dest: AssignDest) -> AssignDest {
    dest
}
```

The seam is a private method returning a possibly-rewritten `AssignDest`. Today
it is the identity function with a doc comment marking it as the nameref hook.
This is the explicit-seam decision: it shapes WHERE nameref attaches without
implementing any behavior, making v160 a change to one function body.

## Behavior-preservation invariants (the refactor's correctness contract)

The funnel MUST reproduce each of these exactly (they are the subtle current
behaviors a naive extraction could drop):

- `a=v` on an existing **indexed** array overwrites element 0, preserving the
  rest (not a whole replace).
- `a=v` on a **scalar** with a non-zero subscript promotes scalar→indexed
  (existing value becomes element 0).
- Scalar `+=` concatenates then folds the WHOLE result; array-element `+=`
  concatenates onto the existing element; `a+=(…)` appends after `max_index+1`
  (1 for a scalar-promote, 0 when unset).
- Integer coercion applies ONLY to a scalar RHS on an existing integer-flagged
  scalar target — NOT to array elements (preserve the current asymmetry).
- Case-fold applies to scalar values and to each value of array/assoc sources,
  never to keys/subscripts; after integer coercion.
- Associative-target dispatch: a scalar/positional-list assignment to an
  associative variable is a type error with the existing message; a whole-assoc
  `m=([k]=v)` routes to `replace_associative`; `replace_associative` /
  `replace_array` PRESERVE the variable's `exported` / `integer` / `case_fold`
  attributes (the v158 fix) and re-derive shape.
- Readonly: a write to a readonly variable prints `huck: NAME: readonly variable`,
  returns the appropriate error, and performs NO partial write on array sources.
- `reseed_special_on_assign`: assigning `RANDOM`/`SECONDS` reseeds/resets and
  stores nothing, via BOTH `assign()` and raw `set()`.

## Testing strategy

Because this is a pure refactor, the safety net is the existing suite — it must
stay **byte-identical green** with zero new expected diffs:

1. Full `cargo test` (all unit + integration suites) green, unchanged counts
   except for any tests that referenced now-private internals (adapt, don't
   weaken).
2. All 85 `tests/scripts/*_diff_check.sh` harnesses byte-identical — these
   exercise scalar/array/assoc/`+=`/integer/case-fold/`declare -p`/`read`/
   `printf -v`/`getopts`/`mapfile` end to end and are the primary proof.
3. Add focused unit tests asserting the funnel applies attributes UNIFORMLY:
   the same `assign()` call with an integer or case-fold attribute on the target
   produces the coerced/folded result regardless of whether it arrived via the
   scalar, element, or whole-array path (locks in the centralization).
4. Add a guard test / audit documenting that the value-producing builtins route
   through the funnel: e.g. set a case-fold attribute on a variable, drive each
   builtin (`read`, `printf -v`, `getopts`, `mapfile`) and assert the stored
   value is folded (proves no raw-`set()` bypass remains).
5. A static check (grep-based test or a documented audit step) that enumerates
   raw `set()` call sites and confirms each is a shell-internal/special-var write,
   not a user-assignment path.

## Components touched

- `src/shell_state.rs` — new `AssignDest`/`Subscript`/`AssignKind`/`AssignSource`
  types, `Shell::assign`, `resolve_assign_target` seam, private storage
  primitives (extracted from the current leaf bodies), the leaf mutators rewritten
  as wrappers, `set()` re-pointed to the shared scalar primitive.
- `src/executor.rs` — `apply_one_assignment` simplified to an adapter that builds
  `AssignSource`/`Subscript` and calls the funnel (logic largely unchanged; the
  storage calls now flow through wrappers/funnel).
- `src/builtins.rs` — audit + (if needed) re-point any value-producing raw
  `set()` to the funnel; no functional change expected.
- Tests: funnel-uniformity unit tests + a builtin-routing guard test; existing
  harnesses unchanged.

## Risks

- **Silent behavior drift** during extraction (the invariants above). Mitigated
  by the byte-identical harness suite and by doing the extraction in small,
  individually-verified steps (scalar path first, then indexed, then associative,
  running the full suite between each).
- **Borrow-checker friction** moving the read-attribute-then-mutate logic into
  one method (already a known pattern from v158: compute the transformed value
  into a local before taking `&mut`).
- **Hidden raw-`set()` user paths** not caught by the audit. Mitigated by the
  builtin-routing guard test (step 4) which drives each builtin and checks
  folding actually occurs.
