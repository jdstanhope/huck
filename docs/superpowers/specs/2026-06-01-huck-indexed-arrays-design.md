# huck v71 — Indexed Arrays (M-82)

**Status:** design complete; ready for plan.
**Date:** 2026-06-01.
**Closes:** M-82 (new), v33 deferral of `$@`/`$*` slicing, v64's `declare -a` rejection.

## Goal

Add bash-compatible indexed arrays to huck: literal compound assignment, sparse subscripted access, all-elements expansion with correct quoting, append, slicing, `declare -a`, `local`-scoped arrays, and readonly enforcement. Closes the largest single bash gap and unlocks several follow-on iterations (associative arrays, `mapfile`/`readarray`, `read -a`, `DIRSTACK`, per-element modifiers).

## Out of scope (deferred to later iterations)

- Associative arrays / `declare -A` (v72).
- `mapfile` / `readarray` builtins.
- `read -a` (v55 deferred).
- `BASH_REMATCH` array population from `[[ … =~ … ]]`.
- Per-element substitution `${a[@]/pat/repl}` and per-element case modification `${a[@]^^}`.
- Integer attribute on arrays (`declare -ai NAME`) — error-out for now.
- `nameref` (`declare -n`) targeting array elements.
- Exporting arrays (bash itself doesn't truly export arrays; huck silently keeps them shell-local — matches bash).

## Architecture

### Value model

`src/shell_state.rs` introduces a `VarValue` enum and reshapes `Variable`:

```rust
pub enum VarValue {
    Scalar(String),
    Indexed(BTreeMap<usize, String>),
}

pub struct Variable {
    pub value: VarValue,
    pub exported: bool,
    pub readonly: bool,
    pub integer: bool,
}
```

`BTreeMap<usize, String>` provides:
- **Sparse storage**: `a[5]=x` with no `a[0..4]` is one entry; `${#a[@]}` reports 1; `${!a[@]}` prints `5`.
- **Sorted iteration**: `${a[@]}` walks in ascending key order; `${!a[@]}` likewise.
- **O(log n) per-element ops**.

A helper `Variable::scalar_view(&self) -> &str` returns:
- `&s` for `Scalar(s)`
- the element at key `0` (or `""`) for `Indexed`

This single method backs the bash rule that `$a` and `"$a"` and `${a}` on an indexed array all mean `${a[0]}`. Existing call sites that read scalar values (`prompt`, `arith`, `read`, expansion's bare-var paths, etc.) call `scalar_view()` and continue to work unchanged for both shapes.

### Shell methods (new / extended)

In `src/shell_state.rs`:

- `Shell::get_array(&self, name: &str) -> Option<&BTreeMap<usize, String>>` — `None` for unset or scalar.
- `Shell::get_array_mut(&mut self, name: &str) -> Option<&mut BTreeMap<usize, String>>`.
- `Shell::set_array_element(&mut self, name: &str, idx: usize, value: String) -> Result<(), AssignErr>` — handles readonly check + scalar-to-array promotion (existing scalar becomes index 0, new value goes at `idx`). Returns the v54 error type already used by `try_set`.
- `Shell::append_array_element(&mut self, name: &str, idx: usize, value: &str) -> Result<(), AssignErr>` — `a[i]+=v`.
- `Shell::replace_array(&mut self, name: &str, elements: BTreeMap<usize, String>) -> Result<(), AssignErr>` — for compound assignment; respects readonly.
- `Shell::append_array(&mut self, name: &str, elements: &[String]) -> Result<(), AssignErr>` — `a+=(x y z)`. New keys start at `max(existing_keys) + 1`.
- `Shell::unset_array_element(&mut self, name: &str, idx: usize)` — removes one key; whole-variable removal stays in `Shell::unset`.

`Shell::lookup_var` continues returning `Option<&str>` — backed by `scalar_view()`. A separate `Shell::lookup_array_element(name, idx)` returns `Option<&str>` for `${a[i]}`.

`Shell::try_set` (v54) keeps its existing `(name, value)` shape — it's the scalar write path. When called on a variable that is currently `Indexed`, it sets element 0 (matches bash: `a=v` on an array overwrites element 0, leaving others).

`Shell::snapshot_for_local_scope` (v52/v64) clones the whole `Variable`, so the change to `VarValue` flows through automatically — `local a=(...)` and `local -a a` snapshots work without new infrastructure.

### Parser & lexer

Three new syntactic recognitions, all in existing modules:

**(a) Compound RHS in assignments** — `src/lexer.rs` + assignment parsing:

In assignment context (a bareword followed by `=`), if the next non-whitespace character is `(`, scan a parenthesized word list:
- Honor nested `(...)` for command substitution within elements.
- Honor quoting (`"..."`, `'...'`, `$'...'`) inside elements.
- Tokenize each element as a normal `Word`.
- Recognize the bash sparse form `[idx]=value` as a special per-element shape: emit `(Some(subscript_word), value_word)`; otherwise `(None, value_word)`.
- Emit a new `Word::ArrayLiteral(Vec<(Option<Word>, Word)>)` variant on the existing `Word` enum.

Locations: `name=(...)` at top level, `name=(...)` in inline-prefix `name=(...) cmd`, `for x in $(name=(...))` (subshell context flows through), `declare NAME=(...)`, `local NAME=(...)`, `readonly NAME=(...)`.

**(b) Subscripted lvalue `name[expr]=value`** — assignment parsing:

After a bareword name, if the next character is `[`, scan a balanced subscript (respecting nested `[...]` for arithmetic, quoting). Emit `AssignTarget::Index { name: String, subscript: Word }` instead of the existing bare `name`. The subscript word is arith-evaluated at execute time.

Recognized in the same contexts as compound assignment, plus on the LHS of `+=` for element-append.

**(c) Subscripted reference inside `${...}`** — `src/lexer.rs` parameter-expansion path:

After scanning the name part of a parameter expansion, if `[` follows, scan a balanced subscript. Three subscript kinds:
- `@` → `SubscriptKind::All`
- `*` → `SubscriptKind::Star`
- anything else → `SubscriptKind::Index(Word)` (arith-evaluated)

Add a `subscript: Option<SubscriptKind>` field to the existing `ParameterExpansion` AST node. The subscript composes with all existing modifiers: `${a[@]:-default}`, `${a[i]:-default}`, `${a[@]:off:len}`, `${#a[i]}`, `${#a[@]}`, `${!a[@]}`, etc.

**Shared helper**: a single `read_subscript(&mut Lexer) -> Result<Word, LexError>` scans the bracketed expression and is called from both assignment and parameter-expansion contexts so the two surfaces stay in sync.

**`unset` parse**: `unset` currently parses words. To accept `unset a[i]`, the `unset` builtin parses each arg: if it matches `^[a-zA-Z_][a-zA-Z_0-9]*\[.+\]$`, treat as `(name, subscript)`; otherwise whole-variable unset. The unset builtin already runs late, so no lexer change is needed — the subscript portion arrives as a single word from the lexer (which doesn't split on `[` outside the contexts above).

### Expansion semantics

In `src/expand.rs`, parameter-expansion gets an array branch. Helper enum:

```rust
enum ArrayExpansion {
    Scalar(String),          // single string result
    WordList(Vec<String>),   // multiple words (for "${a[@]}" / unquoted ${a[@]})
}
```

Per construct:

- **`${a[i]}`** — arith-eval `i`; negative subscripts are treated relative to `max(existing_keys) + 1` (so `${a[-1]}` is the element at the maximum subscript), matching bash 4.3+. Returns `Scalar(value)` or `Scalar("")` if no element at that key.
- **`"${a[@]}"`** (quoted, `@`) — `WordList(values_in_key_order)`. No IFS splitting; preserves empty elements as empty words.
- **`${a[@]}`** unquoted — `WordList(values_in_key_order)`, then EACH element is subject to IFS splitting per the existing positional-args logic.
- **`"${a[*]}"`** quoted `*` — `Scalar(values joined by first_char($IFS))`. Default IFS = space.
- **`${a[*]}`** unquoted `*` — `Scalar(values joined by " ")` then word-split on IFS (same as `$*`).
- **`${#a[@]}` / `${#a[*]}`** — `Scalar(count)`, where count = `BTreeMap::len()`.
- **`${#a[i]}`** — `Scalar(byte-length of element value)`.
- **`${!a[@]}` / `${!a[*]}`** — sequence of subscript values, same quoting rules as `${a[@]}`/`${a[*]}`.
- **`${a[@]:off:len}` slicing** — sort keys, slice the value-list dense-index-wise (NOT by subscript value): `off=2` means "start at the 3rd present element." Negative `off` counts from the end of the present-element list. Bash requires a space before a negative offset (`${a[@]: -1}`) to disambiguate from the `:-default` modifier; huck matches that. Same helper services `${@:o:l}` and `${*:o:l}` — closes v33 deferral.
- **Bare `${a}` / `$a`** — `Scalar(scalar_view())` (element 0 or empty).

All existing modifier paths (`:-`, `:=`, `:?`, `:+`, `#`, `##`, `%`, `%%`, `/`, `//`, `:o:l`) compose: when the base value is a `Scalar` (including element lookups, `${a}`, `${#a[@]}`), the modifier applies normally; when it's a `WordList`, modifiers in v71 apply only to the slicing `:o:l` form (the rest — pattern substitution, case mod — defer per the scope decision).

### Assignment execution

In `src/executor.rs`:

**Compound assign** (`AssignTarget::Bare { name }` with RHS = `Word::ArrayLiteral`):
1. Readonly check on `name`.
2. Build a fresh `BTreeMap<usize, String>`.
3. Walk elements left-to-right:
   - If element has explicit subscript: arith-eval subscript, then expand the value word and place at that index.
   - Otherwise: place at `next_implicit_index`, which starts at 0 and advances past any explicit-subscript placements (matches bash).
4. Replace the variable's `value` with `Indexed(map)`. If it didn't exist, create with default attributes.
5. If a prior scalar attribute (readonly was already handled; integer is rejected with "integer arrays not supported"; exported is preserved).

**Element assign** (`AssignTarget::Index { name, subscript }`):
1. Arith-eval subscript → `idx: usize` (negative → error "huck: NAME: bad array subscript").
2. Expand RHS word → scalar string `value`.
3. `shell.set_array_element(name, idx, value)?`.

**Append-array** (`name+=(...)`):
1. Readonly check.
2. Expand the literal's elements as for compound assign.
3. If `name` is unset or scalar: promote (scalar becomes index 0) then append.
4. New implicit keys start at `max(existing_keys) + 1`.

**Append-element** (`name[i]+=v`):
1. Arith-eval `i`.
2. Lookup existing element (or "").
3. Concatenate `existing + new_value`.
4. `shell.set_array_element(name, idx, concatenated)?`.

**Inline prefix** (`a=(...) cmd`): v23's snapshot+restore cycle clones the whole `Variable`, so the `VarValue` change flows through. The compound-RHS branch is the only added code path.

### Builtin updates

- **`builtin_unset`**: parse arg; if `name[idx]` form, arith-eval `idx`, call `shell.unset_array_element`; else fall through to existing whole-variable unset. Readonly check (existing) covers both.
- **`builtin_declare`** (v64-65): the `-a` flag is currently rejected. Now:
  - `declare -a NAME` → create empty array (preserves existing if already array; promotes scalar to array with element 0 if scalar).
  - `declare -a NAME=(...)` → compound assign.
  - `declare -a NAME[i]=v` → element assign.
  - `declare -a` (bare) → list arrays only, in `declare -a NAME=(value...)` format.
  - `declare -p NAME` for an array prints `declare -a NAME=([0]="v0" [1]="v1" ...)` with the existing `\\`-escape rules from `format_declare_line`.
  - `declare -ai NAME` → "huck: declare: integer arrays not yet supported" + exit 1.
- **`builtin_local`** (v52): inherits compound-RHS support via the assignment parser; the local-scope snapshot machinery is `Variable`-shaped so it works unchanged. `local -a NAME` accepted analogous to `declare -a`.
- **`builtin_readonly`** (v54): inherits compound-RHS support. `readonly NAME=(x y z)` works; once readonly, every assignment path (compound, element, append, unset, `unset NAME[i]`) emits the v54 diagnostic.

### Existing-builtin no-ops

- **`builtin_export`**: rejects array compound-RHS (`export NAME=(...)`) with "huck: export: cannot export arrays" — matches bash's de-facto behavior (bash silently exports the name but not the array contents).
- **`builtin_read`** (v55): no change for v71; `read -a` deferred.

### Positional-parameter slicing (v33 deferral closure)

`${@:off:len}` and `${*:off:len}`: model `positional_args: Vec<String>` as a synthetic dense-indexed sequence; route through the same `slice_word_list` helper used for `${a[@]:o:l}`. Existing v33 substring code for `${var:o:l}` continues to handle scalar vars; the new helper handles word-list cases. Update M-16 status accordingly.

## Error handling

| Path | Error | Status |
|------|-------|--------|
| Bad subscript (negative pre-wrap, non-numeric after arith) | `huck: NAME: bad array subscript` | 1 |
| Readonly violation, any path | `huck: <ctx>: NAME: readonly variable` | 1 |
| `declare -ai NAME` | `huck: declare: integer arrays not yet supported` | 1 |
| `export NAME=(...)` | `huck: export: cannot export arrays` | 1 |
| Compound RHS parse error | `huck: syntax error near unexpected token …` | 2 |
| Subscript missing `]` | `huck: syntax error: missing ']'` | 2 |

`nounset` (v69) on an unset element: emits `huck: NAME[idx]: unbound variable` and sets pending_fatal_pe_error. Modifier paths (`${a[i]:-default}`) remain exempt, same as scalars.

## Testing

### Unit tests (in-source)

- `mod array_value_tests` (`src/shell_state.rs`): VarValue Scalar↔Indexed promotion, `scalar_view` on both shapes, empty-Indexed `scalar_view` is `""`.
- `mod array_subscript_tests` (`src/expand.rs`): subscript arith eval — literal, var-lookup, expression, out-of-range returns empty, negative wraps via length.
- `mod array_assign_tests` (`src/executor.rs`): compound literal (dense), compound literal with sparse `[5]=x`, element assign promotes scalar, append-array on empty / dense / sparse / mixed-with-explicit-subscripts, append-element on existing / missing, readonly blocks every path with the v54 diagnostic.
- `mod array_unset_tests` (`src/builtins.rs`): `unset a` removes whole; `unset a[i]` removes one key; `unset a[i]` on missing key is silent no-op; readonly errors.
- `mod array_declare_tests` (`src/builtins.rs`): `declare -a NAME`, `declare -a NAME=(...)`, bare `declare -a` lists, `declare -p NAME` formats, `declare -ai` errors.
- `mod array_expansion_tests` (`src/expand.rs`): all 9 forms from the table — element read, `${a[@]}` quoted/unquoted, `${a[*]}` quoted/unquoted, `${#a[@]}`, `${!a[@]}`, `${#a[i]}`, `${a[@]:o:l}`, bare `${a}`≡`${a[0]}`.
- `mod positional_slicing_tests` (`src/expand.rs`): `${@:o:l}`, `${*:o:l}`, negative offset, out-of-range len.

Approx 35-40 unit tests total.

### Integration tests (`tests/arrays_integration.rs`)

Approx 12 binary-driven scripts:
1. Literal compound assign + `${a[@]}` in a `for` loop.
2. Sparse subscript `a[5]=x` + `${#a[@]}` = 1 + `${!a[@]}` = `5`.
3. Element write/read round-trip.
4. Append-array `a+=(...)` extends correctly.
5. Append-element `a[i]+=v` concatenates.
6. Scalar-to-array promotion via element assign.
7. `"${a[@]}"` preserves empty elements as empty words (vs `${a[*]}` IFS-join).
8. `unset a[i]` then `${!a[@]}` shows the missing key gone.
9. `local a=(...)` is scoped to function (caller sees original).
10. `readonly a=(...)` blocks element write with v54 diagnostic.
11. `declare -a NAME=(...)` + `declare -p NAME` round-trip.
12. `set -u` × `${a[unset_index]}` errors with "unbound variable".

### Manual bash-diff check

Before merge, run `tests/scripts/arrays_diff_check.sh` (new): pipes the same fragments through `bash -c` and `huck -c`, diffs stdout/stderr/exit. Catches subtle quoting/IFS bugs unit tests can miss. Not gated by CI (no bash dependency in test harness), but listed in the PR description's manual-test plan.

## Documentation

`docs/bash-divergences.md`:
- Add **M-82: Arrays** as a new Tier-2 entry, marked `[fixed v71 partial]`. Body lists the supported surface verbatim from the architecture section + the deferral set.
- Update M-16 (substring expansion): the positional-slicing half is now closed; status becomes `[fixed v33 (scalars) + v71 (positionals)]`.
- Update M-72 (read, v55): cross-reference M-82; `read -a` still deferred but the array surface now exists.
- Update M-78 (dirstack, v63): cross-reference M-82; `DIRSTACK` exposure now feasible as a follow-on.
- Update M-79 (declare, v64-65): `-a` now [fixed v71]; `-A` still deferred; `-ai` rejected per scope.
- Update M-76 (PROMPT_COMMAND array, v61): array form unlocked; not implemented in v71 but no longer blocked.

Add a change-log entry dated 2026-06-01 (or implementation-completion date).

`README.md`: add row `| v71 | indexed arrays (M-82) |`.

## Risks

- **Variable rewrite blast radius**: every `Variable.value: String` reference needs updating. Mitigation: introduce `scalar_view()` first as a no-op (returns the existing `String`), then change the type. Many call sites need only the helper. Plan task 1 isolates this refactor.
- **Lexer subscript-scanning ambiguity**: `[...]` is also bash glob character-class syntax. Disambiguate by context — subscripts only inside `${...}` (already specially lexed) and immediately after a bareword in assignment LHS (a specific parser state). Outside those, `[...]` remains a glob class.
- **Compound-RHS parsing inside `for`/`case`/`while` conditions**: those don't assign, so the `name=(...)` recognition only fires in actual assignment contexts. No interaction.
- **Test isolation**: array tests must not leak state between cases — each unit test creates a fresh `Shell`; integration tests use the existing piped-stdin harness which forks a fresh process.
