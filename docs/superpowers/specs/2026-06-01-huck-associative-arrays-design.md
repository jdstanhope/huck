# huck v72 — Associative Arrays (M-83)

**Status:** design complete; ready for plan.
**Date:** 2026-06-01.
**Closes:** M-83 (new), M-79's `-A` row (still deferred after v64-65 and v71).
**Builds on:** v71 indexed arrays (M-82) — most parser/expansion/declare machinery is reused.

## Goal

Add bash-compatible associative arrays to huck: string-keyed insertion-ordered maps with `declare -A`, element access, all-elements expansion with correct quoting, append-by-key, slicing, `local -A` / `readonly -A`, `declare -p` formatting, and the standard type-mismatch error paths. Full parity with v71's indexed-array surface where the operation makes sense for string keys.

## Out of scope (deferred to later iterations)

- `mapfile`/`readarray` builtins.
- `read -A` (the associative-array sibling of `read -a`).
- `BASH_REMATCH` array population (still deferred from v71).
- Per-element substitution `${m[@]/pat/repl}` and per-element case modification `${m[@]^^}` (still deferred from v71; would land for indexed AND associative together).
- Integer attribute on associative arrays (`declare -Ai` rejected, same as v71's `declare -ai`).
- Exporting associative arrays (`export m=([k]=v)` rejected, same as v71).
- `nameref` (`declare -n`) targeting associative-array elements.

## Architecture

### Value model

`src/shell_state.rs` extends the v71 `VarValue` enum with a third variant:

```rust
pub enum VarValue {
    Scalar(String),
    Indexed(BTreeMap<usize, String>),       // v71
    Associative(Vec<(String, String)>),     // v72 — insertion-order preserved
}
```

`Vec<(String, String)>` is the deliberate choice over `HashMap`, `BTreeMap`, or `indexmap`:

- **Insertion order preserved**: matches bash 4.0+ (whose hash table maintains insertion order on first add).
- **No new dependency**.
- **Linear lookup/insert/delete** is acceptable: shell associative arrays are typically small (configuration maps, command tables); O(n) per op is negligible.
- **Deterministic for tests**: every run produces the same iteration order.

Operations on `Vec<(String, String)>`:

- **Insert/update** (`set(k, v)`): linear scan for existing key; if found, update value in place (position preserved); else push to back.
- **Lookup** (`get(k) -> Option<&str>`): linear scan.
- **Delete** (`remove(k)`): linear scan + `Vec::remove(idx)`.
- **Iteration**: in-order, yields `(K, V)` pairs in insertion order.

`Variable::scalar_view()` for `Associative` returns `""` (associative arrays have no element 0; bash's `$m` on an associative array is empty).

New `Variable::associative()` constructor mirrors the v71 `Variable::scalar()`.

### Shell methods (new)

In `src/shell_state.rs`:

- `Shell::get_associative(&self, name: &str) -> Option<&Vec<(String, String)>>` — `None` for unset / scalar / indexed.
- `Shell::lookup_associative_element(&self, name: &str, key: &str) -> Option<String>`.
- `Shell::set_associative_element(&mut self, name: &str, key: String, value: String) -> Result<(), AssignErr>` — readonly-checks; preserves insertion order on update.
- `Shell::append_associative_element(&mut self, name: &str, key: &str, value: &str) -> Result<(), AssignErr>` — `m[k]+=v` concat.
- `Shell::unset_associative_element(&mut self, name: &str, key: &str) -> Result<(), AssignErr>`.
- `Shell::replace_associative(&mut self, name: &str, elements: Vec<(String, String)>) -> Result<(), AssignErr>` — for compound `declare -A m=([k]=v)`.
- `Shell::declare_associative(&mut self, name: &str) -> Result<(), DeclareErr>` — creates empty assoc on unset, errors on existing indexed/scalar (matches bash).

### Parser

No lexer/parser changes — v71's machinery already handles `m[key]=v`, `m+=(...)`, `${m[key]}`, `${m[@]}`, etc. The new work is purely runtime dispatch in expansion + executor + builtins.

### Subscript evaluation

The same lexical form `m[expr]` evaluates differently per array type. New helper in `src/expand.rs`:

```rust
/// Expands a subscript Word to a string key. Variable expansion and
/// command substitution apply, but no arithmetic. Used for associative
/// array subscripts.
pub(crate) fn eval_subscript_key(subscript: &Word, shell: &mut Shell) -> String {
    expand_word_to_string(subscript, shell)
}
```

Dispatch at every subscripted operation:

```rust
match shell.vars.get(name).map(|v| &v.value) {
    Some(VarValue::Associative(_)) => /* use eval_subscript_key */,
    Some(VarValue::Indexed(_)) | Some(VarValue::Scalar(_)) | None => /* use eval_subscript (v71, arith) */,
}
```

For **unset variables**, dispatch defaults to numeric (arith) — matches the bash gotcha that `m[foo]=v` on unset `m` creates an indexed array with `m[0]=v`.

### Expansion semantics

In `src/expand.rs::expand_array_param` (added in v71), extend the dispatch to handle the associative variant. All 9 forms from v71 map to associative with the iteration-order substitution noted above:

| Form | Behavior on Associative |
|------|-------------------------|
| `${m[k]}` | string-key lookup |
| `"${m[@]}"` | WordList in insertion order |
| `${m[@]}` unquoted | same, then word-split per IFS |
| `"${m[*]}"` | scalar joined by first char of $IFS |
| `${m[*]}` unquoted | same, then word-split per IFS |
| `${#m[@]}` / `${#m[*]}` | Vec length |
| `${!m[@]}` / `${!m[*]}` | string keys in insertion order (WordList for `[@]` quoted; IFS-joined scalar otherwise) |
| `${#m[k]}` | char-length of element value |
| `${m[@]:o:l}` | `slice_word_list` over value-list in insertion order |
| Bare `${m}` | empty string (no element 0) |
| Modifier on `${m[k]}` (e.g., `:-default`) | same as scalar via `expand_modifier_with_value` |

`slice_word_list` from v71 works unchanged — it operates on `&[String]`, type-agnostic.

`apply_modifier_to_value` from v71 works unchanged — it operates on `Option<String>`.

Nounset wire-in: `${m[missing]}` with `set -u` fires `pending_fatal_pe_error` with `huck: m[key]: unbound variable` (same shape as v71's element message).

### Assignment execution

In `src/executor.rs::apply_one_assignment`, the dispatch becomes 3-way per variable type:

```
match (target, value-is-array-literal, existing-variable-type) {
    (Bare(name), Some(elements), Associative) => replace_associative(name, expand_elements(elements)),
    (Bare(name), Some(elements), Indexed | Scalar | unset) => /* v71 path: replace_array or compound-init-as-indexed */,
    (Bare(name), None, Associative) => /* error: "must use subscript when assigning associative array" */,
    (Bare(name), None, _) => /* v71 path: try_set or append-element-0 */,
    (Indexed{name, sub}, Some(_), _) => /* v71 path: reject */,
    (Indexed{name, sub}, None, Associative) => {
        let key = eval_subscript_key(sub, shell);
        if append { append_associative_element(name, &key, value) }
        else { set_associative_element(name, key, value) }
    },
    (Indexed{name, sub}, None, Indexed | Scalar | unset) => /* v71 path: numeric */,
}
```

`build_array_map` from v71 stays unchanged (it handles indexed compound literals). A new sibling `build_associative_map` walks `ArrayLiteralElement`s and:

- For each element, the subscript MUST be `Some(_)` (positional elements in associative-mode literal are an error).
- Expands subscript-as-string and value-as-string.
- Inserts into the result vector, preserving insertion order.

Pseudocode:

```rust
fn build_associative_map(
    elements: &[ArrayLiteralElement],
    shell: &mut Shell,
) -> Result<Vec<(String, String)>, ()> {
    let mut out: Vec<(String, String)> = Vec::new();
    for e in elements {
        let key = match &e.subscript {
            Some(sw) => eval_subscript_key(sw, shell),
            None => {
                eprintln!("huck: associative array must use [key]=value form");
                return Err(());
            }
        };
        let val = expand_word_to_string(&e.value, shell);
        // Update in place if key exists; else append.
        if let Some(slot) = out.iter_mut().find(|(k, _)| k == &key) {
            slot.1 = val;
        } else {
            out.push((key, val));
        }
    }
    Ok(out)
}
```

Snapshot/restore for inline-prefix `m=([k]=v) cmd` works via the existing `Variable::clone()` path (all three VarValue variants are Clone).

### Builtin updates

In `src/builtins.rs`:

- **`builtin_declare_decl`** (the v71 `_decl` variant): add `-A` flag handling parallel to `-a`. `flags.associative: bool`.
  - `declare -A NAME` (no value) → call `shell.declare_associative(name)`. Errors per type-mismatch rules.
  - `declare -A NAME=([k]=v ...)` → call `apply_one_assignment` with the compound literal; the dispatcher above routes to `replace_associative`.
  - `declare -Ai` → error: "huck: declare: integer associative arrays not yet supported".
  - `declare -aA` (both flags) → error: "huck: declare: cannot specify both -a and -A".
  - Bare `declare -A` (no names) → list associative arrays only.

- **`builtin_local_decl`**: add `-A` flag, mirroring `-a`. Snapshot/restore inherited from existing Variable::clone path.

- **`builtin_readonly_decl`**: compound associative RHS (`readonly NAME=([k]=v)`) — already routes through `apply_one_assignment`. Confirm the path works without changes.

- **`builtin_export_decl`**: rejects assoc compound RHS via the existing `assign_value_is_array` helper — already covers Associative (it just checks for trailing `WordPart::ArrayLiteral`, type-agnostic).

- **`format_declare_line`**: extend the array branch with an associative case:

  ```rust
  VarValue::Associative(pairs) => {
      let mut parts: Vec<String> = Vec::new();
      for (k, v) in pairs {
          let key_escaped = escape_double_quote_value(k);
          let val_escaped = escape_double_quote_value(v);
          parts.push(format!("[\"{key_escaped}\"]=\"{val_escaped}\""));
      }
      format!("=({})", parts.join(" "))
  }
  ```

  Attribute order: `A` (associative) replaces `a` (indexed). Format: `declare -A NAME=([key1]="v1" [key2]="v2")`. Bash escapes keys with `"..."` quoting; we match.

- **`builtin_unset`** (`parse_subscripted_arg` path): the unset-element form `unset m[key]` already extracts `(name, sub_text)`. The dispatch decides string vs numeric based on `m`'s current type. Update the existing wiring:

  ```rust
  match shell.vars.get(name).map(|v| &v.value) {
      Some(VarValue::Associative(_)) => {
          let key = eval_subscript_key(&sub_word, shell);
          shell.unset_associative_element(name, &key)
      }
      _ => /* v71 path: arith subscript + unset_array_element */,
  }
  ```

### Type-mismatch errors

`Shell::declare_associative` enforces the bash rules:

```rust
pub fn declare_associative(&mut self, name: &str) -> Result<(), DeclareErr> {
    match self.vars.get(name).map(|v| &v.value) {
        None => {
            self.vars.insert(name.to_string(), Variable {
                value: VarValue::Associative(Vec::new()),
                exported: false,
                readonly: false,
                integer: false,
            });
            Ok(())
        }
        Some(VarValue::Associative(_)) => Ok(()),  // already, no-op
        Some(VarValue::Indexed(_)) => {
            eprintln!("huck: declare: {name}: cannot convert indexed to associative array");
            Err(DeclareErr::TypeMismatch)
        }
        Some(VarValue::Scalar(_)) => {
            eprintln!("huck: declare: {name}: cannot convert scalar to associative");
            Err(DeclareErr::TypeMismatch)
        }
    }
}
```

For `m=(x y z)` on an associative `m`: bash treats this as an error ("must use subscript with associative array"). Implementation in `apply_one_assignment`'s `Bare + ArrayLiteral` branch: if the compound literal has any element without a subscript AND the existing var is associative, error out.

For `m+=(x y)` on associative: same check.

## Error handling

| Path | Error | Status |
|------|-------|--------|
| `declare -A` on indexed | `huck: declare: NAME: cannot convert indexed to associative array` | 1 |
| `declare -A` on scalar | `huck: declare: NAME: cannot convert scalar to associative` | 1 |
| `declare -Ai NAME` | `huck: declare: integer associative arrays not yet supported` | 1 |
| `declare -aA NAME` | `huck: declare: cannot specify both -a and -A` | 1 |
| `m=(x y)` where m is associative | `huck: NAME: must use [key]=value form` | 1 |
| `m+=(x y)` where m is associative | `huck: NAME: must use [key]=value form` | 1 |
| `export m=([k]=v)` | `huck: export: cannot export arrays` (existing) | 1 |
| Readonly violation on assoc element | `huck: <ctx>: NAME: readonly variable` (existing) | 1 |
| `set -u` + `${m[missing_key]}` | `huck: NAME[key]: unbound variable` | 1 |

## Testing

### Unit tests (in-source)

- `mod assoc_value_tests` (`src/shell_state.rs`): Vec<(K,V)> ops — update-in-place preserves position; lookup; delete; iteration order; `scalar_view` returns "".
- `mod assoc_subscript_tests` (`src/expand.rs`): `eval_subscript_key` expands `$var`, command sub, literal; confirms no arith conversion.
- `mod assoc_assign_tests` (`src/executor.rs`): `declare -A; m[foo]=bar`; compound literal `declare -A m=([k]=v)`; `m+=([k2]=v2)`; `m[k]+=v` concat; gotcha cases (no-declare → indexed); type-mismatch errors (`declare -A` on indexed); rejection of positional-list on assoc.
- `mod assoc_expansion_tests` (`src/expand.rs`): all 9 expansion forms; iteration order verified; nounset on missing key.
- `mod assoc_declare_tests` (`src/builtins.rs`): `declare -A`, `declare -p` formatting, `declare -A NAME=(...)`, `declare -Ai` rejection, `declare -aA` rejection, bare `declare -A` listing.

~25-30 unit tests total.

### Integration tests (`tests/associative_arrays_integration.rs`)

10 binary-driven scripts:
1. `declare -A m; m[foo]=bar; echo "${m[foo]}"` round-trip.
2. `declare -A m=([a]=1 [b]=2); for k in "${!m[@]}"; do echo $k; done` insertion order.
3. `${#m[@]}` after adds and deletes.
4. `m[k]+=` concat.
5. `m+=([k1]=v1 [k2]=v2)` adds keys.
6. `unset m[k]` removes one key; remaining stay in order.
7. `local -A m` scoped to function (outer preserved).
8. `readonly` blocks element write.
9. `declare -A` on existing indexed → error.
10. `set -u` + `${m[missing]}` → unbound diagnostic + fatal.

### Manual bash-diff check

Extend `tests/scripts/arrays_diff_check.sh` with associative fragments:

```bash
'declare -A m=([foo]=bar [baz]=qux); echo "${m[foo]}"; echo "${#m[@]}"'
'declare -A m; m[a]=1; m[b]=2; for k in "${!m[@]}"; do echo $k; done'
'declare -A m; m[k]=hi; m[k]+=_bye; echo "${m[k]}"'
'declare -A m=([x]=1 [y]=2); unset m[x]; echo "${!m[@]}"; echo "${#m[@]}"'
'declare -A m=([z]=1 [a]=2); m[k]=3; for k in "${!m[@]}"; do echo $k; done'
```

The insertion-order guarantee means bash and huck should produce byte-identical output. (Bash's hash table is order-preserving on first-add since bash 4.0.)

## Documentation

`docs/bash-divergences.md`:
- Update **M-82** Deferred list: change "associative arrays / `declare -A` (v72 candidate)" to "associative arrays — shipped v72, see M-83".
- Add **M-83: Associative arrays** Tier-2 entry marked `[fixed v72]`. Body describes the supported surface + the Deferred list.
- Update **M-79** (declare): `-A` row → fixed v72; `-Ai` rejected.
- Add change-log entry dated 2026-06-01.

`README.md`: add row `| v72 | associative arrays (M-83) |`.

## Risks

- **Subscript-dispatch ambiguity**: a variable might be looked up multiple times during a complex expansion. The dispatch must be consistent — once we've decided the variable is associative, all subscripts in that expansion use string semantics. Mitigation: dispatch is local to each `${m[…]}` or `m[…]=` operation; no global state.
- **The gotcha**: `m[foo]=v` without `declare -A m` silently writes to `m[0]` (indexed). Users coming from Python/Ruby will be surprised. This matches bash exactly; we MUST NOT add helpfulness here, since the bash-diff harness would diverge. Document the gotcha in M-83.
- **Insertion-order on update**: `m[a]=1; m[b]=2; m[a]=3` — after the third statement, does `a` keep its original position (head) or move to the tail? Bash: **keeps original position**. Our `Vec<(K,V)>` does the same via "find-then-update-in-place". Verified in `assoc_value_tests::update_existing_preserves_position`.
- **`declare -p` formatting**: associative keys may contain `"` and `\`. The existing `escape_double_quote_value` helper handles both. Verify it produces bash-compatible output for keys with special characters.
- **Memory**: `Vec<(String, String)>` clones happen on snapshot/restore. For a 1000-entry associative array in a tight loop, this is 1000 clones per inline-prefix command. Acceptable for v72; if it becomes a hot path later, `Rc<Vec<...>>` is a natural refactor (out of scope).
