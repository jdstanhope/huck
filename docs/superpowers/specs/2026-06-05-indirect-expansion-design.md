# huck v95 — `${!var}` indirect expansion + `[[ ]]` empty-integer Design

**Status:** approved design, ready for implementation plan.
**Implements (primary):** bash's `${!var}` **indirect** parameter expansion —
resolve `var`'s value to a name, then expand that name. Covers bare `${!ref}`,
indirection through positionals (`${!OPTIND}`, `${!2}`), and composition with a
trailing modifier (`${!ref-default}`, `${!1#\~}`). This is the **dominant blocker
for `/usr/share/bash-completion/bash_completion`** — every one of its errors
traces to `${!...}` indirection (lines 238/281/369/389/395) and the parse
cascades those failures produce.
**Implements (bundled, small):** `[[ ]]` integer comparisons coerce an empty
operand to `0` silently (bash behavior), instead of printing `[[: bad integer:`.
Clears oh-my-posh's `[[: bad integer:` line.
**Defers:** prefix-name forms `${!prefix@}` / `${!prefix*}` (list variable names
by prefix) — **not used** by bash_completion/the bashrc (verified count 0).
Array-keys `${!arr[@]}` / `${!arr[*]}` already work (v71) and are unchanged.
**Closes:** the bare-`${!NAME}` half of the v71 indirect gap (the array-keys half
shipped in v71). Logged today as a new `[fixed v95]` note.
**Branch (impl):** `v95-indirect-expansion`.

## Verified bash 5.2 semantics (the contract)

`${!ref<op>word}`: resolve `ref` (a scalar, or `name[sub]`) to its value → an
*effective name* N → evaluate `${N<op>word}` (the trailing modifier, if any,
applies to N). N may be a variable name, a positional digit, or a special param.

- `ref=PATH; echo ${!ref}` → the value of `$PATH`.
- `ref=x; x=hi; echo ${!ref}` → `hi`.
- `set -- a b c; OPTIND=2; echo ${!OPTIND}` → `b` (indirection through a
  positional: `${2}`).
- `set -- a b; echo ${!2}` → indirection through `$2`'s value as a name.
- `ref=missing; echo "${!ref-fallback}"` → `fallback` (modifier applies to the
  effective name N=`missing`, which is unset).
- `ref=x; x=val; echo "${!ref-fallback}"` → `val`.
- `unset ref; echo "${!ref}"` → empty (and aborts under `set -u`).
- `ref=x; unset x; echo "${!ref}"` → empty.
- `a=(p q r); echo "${!a[@]}"` → `0 1 2` (array keys — UNCHANGED, already works).
- `[[ "" -ge 0 ]] && echo Y` → `Y` (empty operand treated as 0, no error).

## Section 1 — Lexer + AST (`src/lexer.rs`)

The `${!` branch in `read_braced_param_expansion` (`src/lexer.rs:1850-1876`)
currently: consume `!`, read the name, scan the subscript; if the subscript is
`[@]`/`[*]` → emit `ParamModifier::IndirectKeys`; **otherwise reject with
`InvalidBraceModifier("!")`** (line 1873). Replace that rejection so the bare /
modifier-composed indirect form is parsed:

- Keep the `[@]`/`[*]` → `IndirectKeys` case exactly as-is (array keys).
- For the `_ =>` case (no `[@]`/`[*]` subscript): instead of erroring, parse the
  remainder as a normal braced expansion **with indirection marked** — read the
  optional `[i]` subscript already scanned, then dispatch the trailing modifier
  via `dispatch_braced_modifier`, passing a new `indirect: true`.

**Carrier — `indirect: bool` field on `WordPart::ParamExpansion`.**
`dispatch_braced_modifier` (`src/lexer.rs:2190`) builds and pushes the full
`WordPart::ParamExpansion`, so the natural carrier is a new `indirect: bool`
field on that struct, threaded as a new parameter of `dispatch_braced_modifier`.
All existing callers pass `indirect: false`; the `${!` branch passes `true`.
(This is an implementation refinement of the brainstorm's "modifier-wrapper"
idea: because `dispatch_braced_modifier` emits the whole WordPart rather than
returning a modifier, a field composes with every modifier for free and is less
invasive than boxing.) Match sites that destructure `ParamExpansion` and don't
care about indirection add `..`.

Result shapes:
- `${!ref}` → `ParamExpansion { name: "ref", modifier: None, subscript: None, indirect: true }`.
- `${!ref-w}` → `… modifier: UseDefault{…}, indirect: true`.
- `${!a[i]}` → `… name:"a", subscript: Some(Index i), indirect: true` (indirection
  through an array element — supported; see §2 effective-name handling).
- `${!a[@]}` → unchanged `IndirectKeys` (NOT indirect-scalar).

## Section 2 — Eval (`src/param_expansion.rs`)

When `indirect` is set, evaluation runs in two steps:

1. **Resolve the through-value → effective name N.** Compute the scalar value of
   `(name, subscript)` using the existing scalar lookup (the same path
   `ParamModifier::None` uses, honoring an `[i]` subscript). Trim per bash (the
   value is taken verbatim as the name; bash does not word-split it).
   - If the through-value is empty / the source var is unset → the indirect
     result is empty; if `set -u` is active and the source is unset, raise the
     unbound-variable fatal error exactly as a normal unset reference would.
2. **Re-expand `${N<modifier>}`.** Interpret N as a parameter reference and
   evaluate it with the carried `modifier`:
   - N is a **plain variable name** (`[A-Za-z_][A-Za-z0-9_]*`) → look up that
     variable, apply the modifier.
   - N is a **positional digit** (`1`, `2`, …) or special (`@`, `*`, `#`) → resolve
     as that positional/special param, apply the modifier.
   - N of the form `name[sub]` → resolve that array element (bash re-parses the
     indirection result as a full reference). Supported if reachable cheaply via
     the existing array-lookup; if the implementer finds it requires re-lexing,
     it may be limited to the plain-name/positional subset (which covers 100% of
     bash_completion) with the `name[sub]`-valued case documented as a follow-on.

Implement as a helper `expand_indirect(name, subscript, modifier, quoted, shell)
-> ExpansionResult` invoked from the modifier-dispatch when `indirect` is true,
reusing the existing scalar/positional/array lookup primitives so the trailing
modifier (`UseDefault`, `RemovePrefix`, etc.) is applied to N through the normal
path (no duplicated modifier logic).

## Section 3 — Bundled: `[[ ]]` empty operand → 0 (`src/executor.rs`)

At the integer-comparison arm (`src/executor.rs:943-955`), the operands `lhs` /
`rhs` are already-expanded strings. Today `lhs.parse::<i64>()` errors
(`bad integer: …`) on an empty/non-numeric operand. Change the parse so an
**empty (or all-whitespace) operand coerces to `0`** before the `i64` parse,
matching bash's treatment of an empty operand in `[[ ]]` arithmetic comparison.
A non-empty **non-numeric** operand keeps the current `bad integer` error
(full arithmetic evaluation of `[[ ]]` integer operands — bare identifiers,
`2+3`, etc. — is a separate, larger behavior and is **not** in scope here; note
it as a deferral).

## Files & responsibilities

| File | Change |
|------|--------|
| `src/lexer.rs` | `WordPart::ParamExpansion` gains `indirect: bool`; `dispatch_braced_modifier` takes/sets it; the `${!` branch parses the indirect scalar form instead of erroring; existing constructors pass `indirect: false` |
| `src/param_expansion.rs` | indirect eval (`expand_indirect`): through-value → effective name → re-expand with the carried modifier; `set -u` honored |
| `src/expand.rs` | `WordPart::ParamExpansion` match sites add `..`/`indirect` as the compiler requires (no behavior change for non-indirect) |
| `src/executor.rs` | `[[ ]]` integer arm: empty operand → 0 |
| `tests/indirect_expansion_integration.rs` | NEW — `${!ref}`, `${!OPTIND}`, `${!ref-def}`, `set -u`, `[[ "" -ge 0 ]]` |
| `tests/scripts/indirect_expansion_diff_check.sh` | NEW — 20th bash-diff harness (bash_completion idioms) |
| `docs/bash-divergences.md`, `README.md` | note bare-`${!NAME}` indirect `[fixed v95]`; `[[ ]]` empty-operand note; deferral for `${!pre@}` prefix + full `[[ ]]` arith operands; changelog; README row |

## Testing

1. **Unit** (lexer): `${!ref}` → `indirect:true, modifier:None`; `${!ref-w}` →
   `indirect:true, modifier:UseDefault`; `${!a[@]}` → still `IndirectKeys`
   (`indirect:false`); `${!2}` parses.
2. **Unit/integration** (param_expansion): `ref=PATH; ${!ref}` = `$PATH`;
   indirection through a positional; `${!ref-def}` unset-vs-set; `set -u` +
   `${!unset}` aborts.
3. **`[[ ]]`**: `[[ "" -ge 0 ]]`→Y; `[[ "" -eq 0 ]]`→Y; `[[ 3 -gt "" ]]`→Y;
   `[[ abc -ge 0 ]]` still errors (non-numeric, out of scope).
4. **bash-diff harness** `tests/scripts/indirect_expansion_diff_check.sh` (20th):
   real idioms — `ref=x; x=hi; echo ${!ref}`; `set -- a b c; OPTIND=2; echo
   ${!OPTIND}`; `r=m; echo "${!r-def}"`; `[[ "" -ge 0 ]]; echo $?` — byte-identical
   to bash 5.2.
5. **Regression**: `${!arr[@]}` / `${!arr[*]}` keys unchanged; all existing
   param-expansion + `[[ ]]` tests pass.
6. **End-to-end**: sourcing `bash_completion` no longer emits the `${!…}` /
   `unexpected token after command` / `printf: ': not a valid identifier` cascade
   (manual confirmation note in the changelog).

## Edge cases & notes

- **Indirection result that itself names an unset var** → empty (modifier, if
  any, decides the fallback). **Source var unset** under `set -u` → fatal unbound
  error like any unset reference.
- **`${!ref}` where ref's value is not a valid name** (e.g. contains spaces) →
  bash yields an error / empty; huck should not panic — treat as empty/no-match.
  Document whatever it does; bash_completion never does this.
- **`${!arr[@]}` keys vs `${!name}` indirect** are disambiguated exactly as bash
  does: a `[@]`/`[*]` subscript after `${!name` → keys; anything else → indirect.
- **No regression for non-indirect expansions**: the new field defaults to
  `false` at every existing construction site; the eval path is only taken when
  `indirect` is true.
- **`[[ ]]` non-numeric operand** (full arithmetic evaluation of operands) remains
  a deferral — only the empty→0 case is fixed here.
