# huck v117 — array-literal element field-expansion (M-112) Design

**Status:** approved design, ready for implementation plan.
**Implements:** full field-expansion of array-literal elements (`arr=(…)` /
`arr+=(…)`) — word-splitting, command-substitution splitting, pathname
globbing, and the quoted/unquoted `${arr[@]}` / `$@` multi-field rule — so one
syntactic element can expand to zero, one, or many array values, matching bash.
This is the corrected **M-112** (Tier-1 bug).
**Why now:** it's the actual remaining `mise<TAB>` blocker. bash_completion's
`_upvars` callers build `upargs=(-aN "$2" "${words[@]}" -v …)`; huck collapses
the nested `"${words[@]}"` to a SINGLE element, so `_upvars` receives the wrong
argument count (desynced from `-aN`), mis-parses, and prints
`bash_completion: : : invalid option` while failing to propagate `words`/`cword`
back to `_get_comp_words_by_ref`.
**Branch (impl):** `v117-array-literal-expansion`.

## Background — the bug (verified against bash this session)

huck's array-literal evaluator expands each syntactic element to exactly ONE
string (`expand_assignment`), so NO element ever splits, globs, or fans out:

| construct | bash | huck (now) |
|---|---|---|
| `s="a b c"; arr=($s)` | 3 | 1 |
| `arr=($(echo x y z))` | 3 | 1 |
| `w=(a b c); arr=("${w[@]}")` | 3 | 1 |
| `w=(a b c); arr=(${w[@]})` | 3 | 1 |
| `arr=(a $s [9]=z b)` (s="x y") | n=5, idx `0 1 2 9 10` | n=4, idx `0 1 9 10` |
| `arr=(a); s="b c"; arr+=($s)` | 3 | 2 |
| `e=; arr=(a $e b)` | 2 (unquoted-empty drops) | 3 (spurious empty) |
| `arr=([0]=$s)` (s="a b c") | 1 (subscript value not split) | 1 ✓ |
| `arr=(a "$e" b)` (e=) | 3 (quoted-empty kept) | 3 ✓ |
| `w=(a b c); arr=("${w[*]}")` | 1 (joined) | 1 ✓ |
| `arr=(/nonexistent_xyz*)` | 1 literal (no match) | 1 ✓ |

**Root cause:** `build_array_map` (`src/executor.rs:4144`) and the `a+=(…)`
append arm do `expand_assignment(&e.value, shell)` per element — the
single-string join path. Command arguments, by contrast, already go through the
field-producing path (`expand` → `glob_expand_fields_opts`) and are correct
(`f "${w[@]}"` → 3 args, `set -- $s` → 3). So only the array-literal path is
wrong.

**Note — corrected diagnosis.** v116's M-112 entry attributed this to a
"`_upvars` dynamic-scope unset-reveal idiom" not propagating. That was a wrong
theory built from a hand-written repro without checking the argument count.
Per-step instrumentation of the real `_upvars` this session showed it receives
6 args instead of 8 — the array-literal collapse in the CALLER, not anything
about `unset`/dynamic scope (which work correctly). v117 corrects the M-112
text alongside the fix.

## Architecture — route bare elements through the command-arg field path

Each array-literal element is either **bare** (`arr=(foo $bar "${baz[@]}")`) or
**subscripted** (`arr=([5]=x [k]=y)`). bash field-expands bare elements (split +
glob + `[@]`/`$@`) and treats a subscripted element's value as a single word
(no splitting). huck must mirror this:

- **Bare element** → expand via the existing `glob_expand_word(&e.value, shell)`
  (`src/executor.rs:2030`), the SAME function command arguments use. It returns
  `Result<Vec<String>, ()>` after `expand` (per-char quoted mask, IFS split of
  unquoted fields, `[@]`/`$@` multi-field, drop unquoted-empty, keep
  quoted-empty) + `glob_expand_fields_opts` (pathname globbing honoring
  `shopt` nullglob/failglob/nocaseglob). Each returned string becomes one array
  value at the next implicit index.
- **Subscripted element** → unchanged: `expand_assignment(&e.value, shell)` (one
  joined string) assigned at the evaluated subscript.

This deletes the bug class by reuse — no new splitting/globbing logic.

## Components & data flow

### Component 1 — `build_array_map` (replace path `arr=(…)`)
`src/executor.rs:4144`. Current loop assigns one value per element and advances
`implicit = idx + 1`. New loop:
```text
implicit = 0
for e in elements:
    if e.subscript is Some(sw):
        idx = eval_subscript(sw, shell, name)?      // unchanged
        map.insert(idx, expand_assignment(&e.value, shell))
        implicit = idx + 1
    else:   // bare
        for field in glob_expand_word(&e.value, shell)?:   // 0..n fields
            map.insert(implicit, field)
            implicit += 1
```
**The implicit index advances per produced FIELD, not per element** — this is
what makes `arr=(a $s [9]=z b)` (s="x y") produce indices `0 1 2 9 10`. A
`failglob` no-match inside a bare element returns `Err(())` (propagated like a
command argument), matching the existing `glob_expand_word` contract.

### Component 2 — append path `arr+=(…)`
`src/executor.rs` ~4155 (the `if a.append` arm of `(Bare(name), Some(elements))`).
Currently `elements.iter().map(|e| expand_assignment(&e.value, shell)).collect()`
then `shell.append_array(name, &values)`. Rework to share the per-element logic:
compute the starting implicit index as the current array's `max_index + 1` (0 if
unset/empty), then run the same subscripted/bare loop as Component 1, producing
a `BTreeMap<usize, String>` of NEW entries, and merge them onto the existing
array (without clearing it). Bare elements field-expand; subscripted `[i]=v`
elements assign at `i` (and reset implicit to `i+1`). Reuse a shared helper so
replace and append share one code path (see Component 4).

### Component 3 — associative `m=(…)` (`build_associative_map`)
`src/executor.rs:4118`. Associative literals use subscripted `[k]=v` elements;
bare elements are already a type error (handled upstream). Values are single
words (not split), matching bash. **No change** — verified by probe that the
mise path does not rely on associative bare-element splitting.

### Component 4 — shared element-expansion helper
Extract a private helper, e.g.
`fn expand_array_elements(elements, name, shell, start_index) -> Result<BTreeMap<usize,String>, ()>`,
implementing the subscripted/bare loop once. `build_array_map` calls it with
`start_index = 0`; the append arm calls it with `start_index = max+1` and merges.
Keeps the field/index logic in one place (DRY) and isolates it for unit tests.

## Scope & correctness

- Only **bare** elements gain field-expansion. Subscripted `[i]=v` and `[k]=v`
  values stay single (bash). Verified.
- The fix reuses `glob_expand_word`; quoting/empty/`[@]`/glob semantics are
  inherited from the command-arg path (already correct post M-105/M-110), so
  array literals and command args become consistent.
- `nullglob`/`failglob`/`nocaseglob`/`nocasematch` flow through unchanged
  (`glob_expand_word` reads `shell.glob_opts()`).
- `pending_fatal_pe_error` (a fatal `${v:?msg}` inside an element) surfaces via
  the same path command args use; the assignment aborts consistently.

## Must-not-regress

- Subscripted literals `arr=([0]=x [2]=y)`, `arr=([0]=$s)` (no value split).
- Associative `declare -A m; m=([k]="v w")` (value not split).
- Quoted `arr=("$s")` → 1 element; `arr=("${w[*]}")` → 1 joined element.
- Plain literals `arr=(a b c)`, append `arr+=(d)`.
- Readonly enforcement, integer attributes, and the existing
  `replace_array`/`append_array` mutators' error paths.
- Command-argument expansion (the donor path) — untouched.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/executor.rs` | `build_array_map` + append arm rewritten via a shared `expand_array_elements` helper routing bare elements through `glob_expand_word`; unit tests |
| `tests/array_literal_expansion_integration.rs` | NEW — split / cmdsub / glob / `[@]` / empties / mixed-index / append, binary-driven vs bash |
| `tests/scripts/array_literal_expansion_diff_check.sh` | NEW — 41st bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-112 `[fixed v117]` + corrected root-cause text; new deferred entry for the `eval x=(…)` command-argument array-literal panic; changelog; README row; Tier counts |

## Testing

1. **Unit** (`src/executor.rs`): `expand_array_elements` / `build_array_map`:
   - scalar split (`s="a b c"` → 3 values at 0,1,2), cmdsub split, `[@]` fan-out,
     quoted `[*]` join (1), unquoted-empty drop, quoted-empty keep, mixed
     bare+subscript index continuation (`a $s [9]=z b` → 0,1,2,9,10), append
     starting index.
2. **Integration** (`tests/array_literal_expansion_integration.rs`, binary vs
   bash) — the divergence table above, each asserted byte-identical (run as
   file-arg scripts per L-27, since some fragments contain history-sensitive
   characters).
3. **41st bash-diff harness** `tests/scripts/array_literal_expansion_diff_check.sh`
   — ~12 byte-identical fragments covering the table; matching-glob fragment in
   a fixtured temp dir.
4. **Regression**: full suite (2831+), all 41 harnesses, clippy
   `--all-targets` clean. Watch `arrays`/`assoc`/`param`/`completion`/`declare`
   suites — a regression there means the field path altered a working case;
   investigate vs bash first.
5. **Payoff**: drive the real `_upvars` / `__reassemble_comp_words_by_ref` /
   `__get_cword_at_cursor_by_ref` / `_get_comp_words_by_ref` chain with
   `COMP_WORDS=(mise "")`, `COMP_CWORD=1`. Expect `cword=1 nwords=2 prev=mise`
   with NO `: : invalid option` (matching bash). Report before/after. This
   should make `mise<TAB>` functional end-to-end; if a further gap surfaces,
   report it honestly at the merge gate (do not over-claim — the smoke is the
   gate, per the v109/v115/v116 lesson).

## Edge cases & notes

- **Index continuation across a fanned-out bare element**: the implicit counter
  increments per produced field, so a split element shifts subsequent implicit
  indices (bash-verified).
- **Append + subscript** (`a+=([5]=x)`): the subscripted element assigns at its
  index and resets the running implicit to `index+1`; bare appended elements
  continue from `max+1`. Confirm exact behavior with a bash probe during
  implementation; if an obscure corner differs, log it as a low `L-` divergence
  rather than special-casing.
- **The `eval x=(…)` command-argument panic** (unescaped parens; array literal
  reaching `expand()` as a command argument) is OUT OF SCOPE — a different code
  locus and off the mise path (real `_upvars` escapes its parens). Logged as a
  new deferred divergence in `docs/bash-divergences.md`, fixed in a later
  iteration.
- The donor path `glob_expand_word` already drops unquoted-empty and keeps
  quoted-empty (M-105/M-110); array literals inherit this, fixing the spurious
  `arr=(a $e b)` → 3 to bash's 2.
