# huck v114 — preserve alternate/default word quoting under an unquoted outer expansion (M-110) Design

**Status:** approved design, ready for implementation plan.
**Implements:** correct field-splitting of the substituted *word* in
`${param+word}` / `${param-word}` (and the colon variants `:+`/`:-`) when the
**outer** `${…}` is **unquoted** — new **M-110** (Tier-1 bug; it produces a wrong
argument count). This is the converse-M-105 sub-divergence deferred in v110, now
the last blocker for `mise ` + `<TAB>`.
**Why now:** bash_completion's `__get_cword_at_cursor_by_ref` (line 339) calls
`_upvars -a${#words[@]} $2 ${words+"${words[@]}"} -v $3 "$cword" …`. With
`COMP_WORDS=(mise "")`, the `words` array has a trailing empty element. huck's
`${words+"${words[@]}"}` (unquoted outer) DROPS that empty element, so the
expanded arg count (1) no longer matches `-a${#words[@]}` (2); `_upvars` then sees
a value where it expected a flag → `bash_completion: : : invalid option` (×2).
**Branch (impl):** `v114-alternate-word-quoting`.

## Background — the bug (verified against bash)

With the **outer** `${…}` **unquoted**, the alternate/default `word`'s own
quoting must be honored:

| Fragment (`set -- …; echo $#`) | bash | huck (now) |
|---|---|---|
| `a=(x "" y); ${a[@]+"${a[@]}"}` | 3 (`<x><><y>`) | 2 (empty dropped) |
| `a=("a b" c); ${a[@]+"${a[@]}"}` | 2 (`<a b><c>`) | 3 (re-split) |
| `x=1; ${x+"a b"}` | 1 (`<a b>`) | 2 (re-split) |
| `words=(mise ""); ${words+"${words[@]}"}` | 2 | 1 |

The **fully-quoted outer** idiom already works and MUST stay working:

| Fragment | bash | huck (now) |
|---|---|---|
| `a=(x "" y); "${a[@]+"${a[@]}"}"` | 3 | 3 ✓ |
| `x=1; "${x+a b}"` | 1 | 1 ✓ |

**Root cause:** the v109/v110 `${param+word}` paths return the substituted word as
a flat `ExpansionResult::Value(String)` (scalar) / `WordList(Vec<String>)` (array)
— discarding each field's quoted-ness. When the outer `${…}` is unquoted, the
`expand()` consumer IFS-joins + re-splits the whole thing as if unquoted →
drops empty fields, re-splits spaced fields. bash preserves the word's own
quoting regardless of the outer.

## Architecture — gate on the outer `quoted` flag; add `ExpansionResult::Fields`

**Key invariant:** only the **unquoted-outer** path is wrong. The quoted-outer
path is byte-correct today. So the fix gates on the outer `quoted` flag and only
changes the unquoted-outer alternate/default word — guaranteeing **zero
regression** to quoted-outer.

**`expand(word, shell)` already produces exactly the right fields for the
unquoted-outer case**: it honors the word's INNER quoting (so `"${a[@]}"` →
`[x, "", y]` with the empty preserved; `a b` unquoted → `[a, b]`). So the fix is
to return those fields verbatim instead of a flattened string.

### Component 1 — new `ExpansionResult::Fields(Vec<Field>)` (`src/param_expansion.rs`)
Add to the `ExpansionResult` enum:
```rust
    /// Pre-split, quoting-final fields from expanding a substituted *word*
    /// (the alternate of `${p+word}` / the default of `${p-word}`) when the
    /// OUTER `${…}` is unquoted. Each `Field` is already a final word — the
    /// consumer emits them as-is (no further IFS-splitting / re-joining), so
    /// quoted-empty fields survive and quoted-spaced fields are not re-split.
    /// (M-110)
    Fields(Vec<crate::expand::Field>),
```
(`crate::expand::Field` is `pub`, with `pub chars: String` + `pub quoted: Vec<bool>`.)

### Component 2 — thread `quoted` into the scalar modifier path (`src/param_expansion.rs`)
- Add a `quoted: bool` parameter to `expand_modifier_with_value(name, modifier,
  source, quoted, shell)`.
- Keep the existing 3-arg `expand_modifier(name, modifier, shell)` wrapper,
  delegating with `quoted = false`:
  `expand_modifier_with_value(name, modifier, ParamLookup::Scalar, false, shell)`.
  This keeps the ~30 existing unit-test call sites of `expand_modifier`
  compiling unchanged. (The ~12 unit tests that assert `Value(...)` for
  `UseAlternate`/`UseDefault` will now observe `Fields(...)` and must be updated
  to expect `Fields(vec![Field…])` — mechanical, see Testing.)
- Update the non-test callers of `expand_modifier_with_value` to pass a `quoted`
  argument: the element-lookup arms in `expand_array_param` / `expand_assoc_param`
  (`ParamLookup::Element(...)`) pass their own `quoted`; `expand_indirect` passes
  its `quoted`. The production scalar caller in `expand()` calls a new
  `expand_modifier_quoted(name, modifier, *quoted, shell)` (or `expand_modifier`
  extended) so the real outer `quoted` reaches the arm; `expand_assignment`
  likewise passes the WordPart's `quoted`.

### Component 3 — the `UseAlternate` / `UseDefault` arms emit `Fields` when unquoted
- **Scalar** (`expand_modifier_with_value`, `src/param_expansion.rs`):
  - `UseAlternate { word, colon }`: if the operand is null (per `colon`) →
    `Empty` (unchanged). Else → if `!quoted` `Fields(crate::expand::expand(word,
    shell))`, else `Value(expand_word_to_string(word, shell))` (current).
  - `UseDefault { word, colon }`: if null → if `!quoted`
    `Fields(crate::expand::expand(word, shell))`, else
    `Value(expand_word_to_string(word, shell))`. Else → `Value(raw)` (the
    variable's own value; unchanged — it splits correctly via the outer flag).
- **Indexed array** (`expand_array_param`, has `quoted`):
  - `(PM::UseAlternate, SK::All|SK::Star)`: set → if `!quoted`
    `Fields(expand(word, shell))`, else the current WordList/`[*]`-join path;
    unset → `Empty`.
  - `(PM::UseDefault, SK::All|SK::Star)`: set → the array itself (current:
    `[@]` WordList(values) / `[*]` joined Value — unchanged); unset → if
    `!quoted` `Fields(expand(word, shell))`, else current.
- **Associative** (`expand_assoc_param`): identical to the indexed arms.

`AssignDefault` (`:=`) and `ErrorIfUnset` (`:?`) are unchanged (out of scope).

### Component 4 — consumers of `Fields` (`src/expand.rs`)
- In `expand()` (`WordPart::ParamExpansion` match, alongside the `WordList`
  arm ~`:908`), add:
  ```rust
  crate::param_expansion::ExpansionResult::Fields(fields) => {
      // Already-final fields: concatenate the first onto the in-progress
      // field, push the rest as new fields. No IFS-split, no re-join —
      // preserves quoted-empty fields and quoted-spaced fields verbatim.
      for (i, f) in fields.into_iter().enumerate() {
          if i > 0 {
              result.push(std::mem::take(&mut current));
          }
          current.chars.push_str(&f.chars);
          current.quoted.extend(f.quoted);
          has_emitted = true;
      }
  }
  ```
  (If `fields` is empty — e.g. a set var with an empty alternate word — nothing
  is appended and `has_emitted` stays as-is, matching the `Empty` semantics.)
- In `expand_assignment()` (the no-split context, ~`:1023`), add a `Fields` arm
  that joins (assignment never word-splits), mirroring the `WordList` arm:
  ```rust
  crate::param_expansion::ExpansionResult::Fields(fields) => {
      let ifs = shell.ifs();
      let sep = ifs_join_sep(&ifs);
      let joined = fields.iter().map(|f| f.chars.as_str()).collect::<Vec<_>>().join(&sep);
      result.push_str(&joined);
  }
  ```

## Why this is correct + regression-safe
- **Unquoted outer**: `Fields(expand(word))` reproduces bash exactly — `expand`
  honors the word's inner quoting (quoted fields, incl. empties, are kept; `@`
  arrays stay separate; unquoted-inner text splits).
- **Quoted outer**: the `!quoted` gate keeps the *current* `Value`/`WordList`
  paths, which are already byte-correct (verified: `"${a[@]+"${a[@]}"}"` → 3,
  `"${x+a b}"` → 1). Untouched.
- The `Fields` variant is only ever produced under `!quoted`, so the
  consumers' Fields arms run only there.

## Must-not-regress
- Quoted-outer alternate/default (the v109 safe idiom `"${arr[@]+"${arr[@]}"}"`,
  `"${x+word}"`) — byte-unchanged.
- Plain `"${a[@]}"` / `${a[@]}` / `${a[*]}` / `${#a[@]}` / `${!a[@]}` (no modifier).
- UseDefault **set** branch (`${a[@]-w}` when set → the array elements; `${x-w}`
  when set → the var value) — splits per the outer flag as today.
- `${x:-w}`/`${x:+w}` scalar in both quoted and unquoted contexts; nested
  `${a[@]+"${a[@]}"}` quoted and unquoted; assignment-context `x=${y+...}`.
- `:=`/`:?` modifiers; pattern/case/substring modifiers (use `expand_word_to_string`,
  untouched).

## Files & responsibilities

| File | Change |
|------|--------|
| `src/param_expansion.rs` | `ExpansionResult::Fields`; `quoted` param on `expand_modifier_with_value` (3-arg wrapper delegates `false`); `UseAlternate`/`UseDefault` arms emit `Fields(expand(word))` when `!quoted`; update the ~12 unit tests asserting `Value(...)` for those modifiers |
| `src/expand.rs` | `expand_array_param`/`expand_assoc_param` UseAlternate/UseDefault arms gate on `quoted` → `Fields`; pass `quoted` to `expand_modifier_with_value` element/indirect callers; production scalar caller passes real `quoted`; `Fields` arms in `expand()` + `expand_assignment()` |
| `tests/alternate_word_quoting_integration.rs` | NEW — the bisection set + the `_upvars`/`mise` shape |
| `tests/scripts/alternate_word_quoting_diff_check.sh` | NEW — 38th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-110 `[fixed v114]`; close the deferred converse-M-105 note; changelog; README row; Tier counts |

## Testing

1. **Unit** (`src/param_expansion.rs`): update the ~12 `UseAlternate`/`UseDefault`
   tests to expect `Fields(...)` (they call the 3-arg `expand_modifier`, i.e.
   `quoted=false`). Add a `Field`-comparison helper if needed (`Field` derives
   `PartialEq`). Add a case asserting the quoted path still returns `Value(...)`.
2. **Integration** (`tests/alternate_word_quoting_integration.rs`, binary-driven,
   `echo $#` + `printf '<%s>'` readouts):
   - `a=(x "" y); set -- ${a[@]+"${a[@]}"}; echo $#` → `3`; `printf '<%s>'` → `<x><><y>`.
   - `a=("a b" c); set -- ${a[@]+"${a[@]}"}; echo $#` → `2`.
   - `x=1; set -- ${x+"a b"}; echo $#` → `1`.
   - fully-quoted regression: `a=(x "" y); set -- "${a[@]+"${a[@]}"}"; echo $#` → `3`;
     `x=1; set -- "${x+a b}"; echo $#` → `1`.
   - UseDefault unset: `unset u; set -- ${u-"a b"}; echo $#` → `2`;
     `set -- ${u-"$u"}`-style as bash; assoc `declare -A m=([k]="a b"); set -- ${m[@]+"${m[@]}"}; echo $#`.
   - the `_upvars`/mise shape: `words=(mise ""); set -- -a${#words[@]} words ${words+"${words[@]}"} -v cword 1; echo $#` → `7`.
   Verify each against the system bash first.
3. **38th bash-diff harness** `tests/scripts/alternate_word_quoting_diff_check.sh`
   — byte-identical fragments for the bisection set + quoted-outer regressions.
4. **Regression**: full suite (2795+), all 38 harnesses, clippy clean. Watch the
   `arrays`/`associative_arrays`/`bashrc_zero_errors`/`mise_zero_errors`/`param`
   suites especially.
5. **Payoff**: `mise ` + `<TAB>` (or the `__get_cword_at_cursor_by_ref` /
   `_upvars` shape) no longer prints `bash_completion: : : invalid option`. Report
   before/after.

## Edge cases & notes
- **`AssignDefault` (`:=`) splitting** stays as-is (out of scope); a future M-note
  if it surfaces.
- **Assignment-context double-space**: `x=${y+a  b}` joins via IFS-sep (single
  space) — a pre-existing minor edge of the WordList assignment join, inherited
  by the Fields arm; not worth special-casing.
- **`Field` is `pub` with public `chars`/`quoted`** (`src/expand.rs:71-75`),
  constructible/comparable from `param_expansion.rs` and tests.
- This closes the **converse-M-105** sub-divergence explicitly deferred in v110.
