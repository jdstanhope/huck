# v158: `local` / `declare` case-fold attributes (`-l` / `-u`)

**Status:** design approved 2026-06-15
**Scope:** the `-l` (lowercase-on-assignment) and `-u` (uppercase-on-assignment)
attributes for the `declare`, `local`, `typeset`, and `readonly`/`export`
declaration paths. The third missing attribute, `-n` (nameref), is explicitly
**out of scope** and deferred to v159 — it stays erroring with the existing
"not yet implemented" message.

## Motivation

`declare`/`local` accept attribute flags that modify how a variable behaves.
huck implements `-a` (indexed), `-A` (associative), `-i` (integer), `-r`
(readonly), `-x` (export), `-g`/`-p`/`-f`/`-F`. The three remaining bash
attributes — `-l`, `-u`, `-n` — currently print
`huck: {declare,local}: -{l,u,n}: not yet implemented in this version`.

This iteration finishes the two small ones. `-l`/`-u` are case-coercion
attributes that mirror the existing `-i` integer-coercion machinery: a flag
stored on the variable that transforms the value on every assignment.

## Bash behavior (verified against bash 5.x)

All rules below were confirmed empirically with `bash --norc --noprofile`.

### Basic coercion
- `declare -l x; x=ABCdef; echo "$x"` → `abcdef` (value lowercased on assign).
- `declare -u x; x=ABCdef; echo "$x"` → `ABCDEF` (value uppercased on assign).
- Inline value in the declaration is folded: `declare -u x=hello` → `x` is `HELLO`.

### Not retroactive
Setting the attribute never folds the *current* value; folding applies only to
*subsequent* assignments:
- `x=ABC; declare -l x; echo "$x"` → `ABC` (unchanged).
- `declare -u x=AbC; echo "$x"` → `ABC`; then `declare -l x; echo "$x"` → `ABC`
  (still — the existing value is not re-folded); then `x=QrS; echo "$x"` → `qrs`
  (the new `-l` attribute folds the new assignment).

### Mutual exclusivity + same-command cancel
`-l` and `-u` are mutually exclusive. The applied attribute is computed
**per declaration command**:
- Both flags in ONE command cancel to *neither*, and clear any prior case
  attribute: `declare -lu x` → `declare -p x` shows `declare -- x`;
  `declare -lu x; x=AbC; echo "$x"` → `AbC` (no fold).
- Across SEPARATE commands the later wins and clears the earlier:
  - `declare -l x; declare -u x` → `declare -u x` (upper wins); `x=AbC` → `ABC`.
  - `declare -u x; declare -l x` → `-l` wins; `x=AbC` → `abc`.

### Removal
- `+l` removes a lowercase attribute, `+u` removes an uppercase attribute;
  future assignments are no longer folded:
  `declare -l x; x=ABC; declare +l x; x=DEF; echo "$x"` → `DEF`.
- `+l`/`+u` only clears the matching attribute; since a same-command `-lu`
  already left the variable with no case attribute, a following `+u`/`+l`
  is a no-op there.

### Arrays
The fold applies to **values**, element by element, on both whole-array and
single-element assignment:
- `declare -l arr; arr=(ABC DeF GHI); echo "${arr[@]}"` → `abc def ghi`.
- `arr[1]=XYZ; echo "${arr[1]}"` → `xyz`.
- Associative: `declare -lA m; m[Key]=VALUE; echo "${m[Key]}"` → `value`.
  The **key is NOT folded** (still `m[Key]`); only the value is.

### `+=` append
The fold applies to the result of an append:
`declare -l x; x=ABC; x+=DEF; echo "$x"` → `abcdef`.

### Interaction with `-i`
Integer coercion runs first, then the case-fold applies to the resulting
string: `declare -iu x; x=3+4; echo "$x"` → `7` (arith → `7`; uppercasing
digits is a no-op). For a value that produces letters via arithmetic this
ordering still holds (arith result stringified, then folded).

### Scoping (`local`)
`local -l`/`local -u` inside a function is function-scoped and restored on
return, exactly like `local -i`/`local -r`:
`f(){ local -l v=HELLO; echo "$v"; }; f; echo "${v:-unset}"` → `hello` then
`unset`.

### `declare -p`
The attribute appears in the serialized flags: `declare -l x; x=ab;
declare -p x` → `declare -l x="ab"`.

## Architecture

### Data model
Add a field to `Variable` (`src/shell_state.rs`):

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CaseFold { Lower, Upper }

pub struct Variable {
    pub value: VarValue,
    pub exported: bool,
    pub readonly: bool,
    pub integer: bool,
    pub case_fold: Option<CaseFold>,   // NEW — mutually exclusive by construction
}
```

`None` = no case attribute. Mutual exclusivity is guaranteed by the type: a
variable can be `Lower`, `Upper`, or neither, never both. Update every
`Variable` constructor / struct-literal (notably `Variable::scalar`) to
initialize `case_fold: None`.

### Attribute mutators
Mirror the existing `mark_integer`/`unmark_integer`/`mark_readonly` helpers on
`Shell`:

```rust
pub fn set_case_fold(&mut self, name: &str, fold: Option<CaseFold>); // create-if-absent, like mark_integer
```

`set_case_fold(name, Some(Lower|Upper))` sets the attribute (creating an unset
scalar if the variable does not yet exist, matching `mark_integer`'s
create-on-bare-declare behavior). `set_case_fold(name, None)` clears it.

### Flag parsing (net effect per command)
In each declaration flag scanner — the `_decl` paths
(`builtin_local_decl`, `builtin_declare_decl`) and the legacy
`builtin_declare`/`builtin_local` paths — replace the three
`-{l,u}: not yet implemented` error sites with case-attribute handling.

Compute the **net effect for the command** before applying to names:
- Track `saw_l` / `saw_u` (minus form) and `plus_l` / `plus_u` (plus form).
- Net minus attribute:
  - `saw_l && saw_u` → `Clear` (set `case_fold = None` on each name).
  - `saw_l` only → `Some(Lower)`.
  - `saw_u` only → `Some(Upper)`.
  - neither → no minus change.
- Plus form: `plus_l` clears a `Lower`; `plus_u` clears an `Upper` (applied
  after the minus net effect — but in practice `+x` and `-x` of the same letter
  are not combined in one command; handle each independently and let the
  observable result match bash).

Apply the resulting `Option<CaseFold>` (or "leave unchanged") to each named
variable via `set_case_fold`, ordered consistently with how `-i`/`-r` are
applied today (the guard that integer/readonly use, e.g. the readonly-before-
mutation checks, stays as-is; case-fold has no failure mode).

### Coercion application
A single helper applies the fold to a value string:

```rust
fn apply_case_fold(fold: Option<CaseFold>, s: &str) -> String
// None -> s unchanged; Lower -> case_modify(.., Lower, all=true);
// Upper -> case_modify(.., Upper, all=true)
```

It reuses the existing `case_modify` helper that backs `${v^^}`/`${v,,}` (so it
inherits the documented L-04 Unicode behavior). Apply it at every value-write
site, **after** integer coercion:

- **`set_var`** (`src/shell_state.rs`): the scalar assignment path. The existing
  `do_integer_coerce` block computes the final string; fold the result of that
  (or the raw value when not integer) before storing. Look up the target's
  `case_fold` attribute — note this must read the attribute that will belong to
  the variable being assigned (the just-declared attribute), so the helper reads
  the current `Variable`'s `case_fold`, defaulting to `None` for a brand-new
  variable.
- **Array element setters** (`set_array_element`, `set_associative_element`):
  fold the element VALUE (not the subscript/key) using the array variable's
  `case_fold`.
- **`+=` append paths** (scalar and array-element append): fold the *post-append*
  result, matching bash (`x+=DEF` on a `-l` var → whole result lowercased).

Because folding reads the variable's own `case_fold`, the "not retroactive"
rule falls out naturally: declaring the attribute does not call any write path,
so the stored value is untouched until the next assignment.

### `declare -p` serialization
In the attribute-flag serialization (the `generate::*` / declare-print path that
already emits `-i`/`-r`/`-x`/`-a`/`-A`), emit `-l` when `case_fold == Some(Lower)`
and `-u` when `Some(Upper)`. Ordering of flags should match bash's
(`declare -l x="ab"`); place `-l`/`-u` consistently with the existing flag order
and verify byte-equality in the harness.

## Components touched

- `src/shell_state.rs` — `CaseFold` enum, `Variable.case_fold` field + all
  constructors, `set_case_fold`, the fold application in `set_var` /
  `set_array_element` / `set_associative_element` / append paths,
  `apply_case_fold` helper.
- `src/builtins.rs` — flag parsing in `builtin_local_decl`,
  `builtin_declare_decl`, `builtin_declare`, `builtin_local` (replace the
  not-yet-implemented sites); `declare -p` flag emission.
- `tests/scripts/local_case_attrs_diff_check.sh` — new bash-diff harness.
- Rust unit tests in `shell_state.rs`/`builtins.rs` for the net-effect flag
  logic and `apply_case_fold`.

## Out of scope (deferred)

- **`-n` nameref** — remains erroring; v159.
- **Locale-correct case mapping** — inherits the existing L-04 Rust
  `to_uppercase`/`to_lowercase` Unicode behavior used by `${v^^}`; not changed
  here (documented, shared with the existing modifier).

## Testing strategy

`tests/scripts/local_case_attrs_diff_check.sh` (gold standard, byte-identical
bash↔huck) covering, at minimum:

1. `declare -l x; x=ABCdef` → `abcdef`; `declare -u` → `ABCDEF`.
2. inline `declare -u x=hello` → `HELLO`.
3. not-retroactive: `x=ABC; declare -l x; echo "$x"` → `ABC`.
4. same-command cancel: `declare -lu x; x=AbC` → `AbC`; `declare -p` → `declare -- x`.
5. last-wins: `declare -l x; declare -u x; x=AbC` → `ABC`; reverse → `abc`.
6. removal: `declare -l x; x=ABC; declare +l x; x=DEF` → `DEF`.
7. arrays: `declare -l arr; arr=(ABC DeF GHI)` → `abc def ghi`; `arr[1]=XYZ` → `xyz`.
8. assoc value-not-key: `declare -lA m; m[Key]=VALUE` → value `value`, key `Key`.
9. `-i` interaction: `declare -iu x; x=3+4` → `7`.
10. `+=`: `declare -l x; x=ABC; x+=DEF` → `abcdef`.
11. `local -l`/`local -u` scoping inside a function.
12. `declare -p` flag emission for `-l` and `-u`.
13. `typeset -l`/`-u` (alias of declare) parity, and `local`/`declare` symmetry.

Plus unit tests: the flag net-effect computation (both-cancel, last-wins,
plus-removal) and `apply_case_fold`. Full `cargo test` + all
`tests/scripts/*_diff_check.sh` must stay green.
