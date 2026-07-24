# v334 — Flip the `array2` bash-suite category to PASS

Issue: [#291](https://github.com/jdstanhope/huck/issues/291) — unquoted
`${arr[@]}`/`${arr[*]}` with empty IFS joins elements into one word.

## Problem

The `array2` bash-suite category (`array-at-star`) is a near-miss (diff 22 lines),
all from **one root**: unquoted `${arr[@]}` / `${arr[*]}` under an **empty IFS**.
Fixing it takes the category to **0-diff → PASS** (Summary PASS 22→23, FAIL 60→59).

bash: an unquoted `${arr[@]}`/`${arr[*]}` expands each element to a separate word,
then word-splits each. With an empty IFS there is no splitting, but the elements
stay separate. huck joined the elements with IFS[0] (empty → concatenation) → a
single word.

```
IFS=''; A=(bob 'tom dick harry' joe); set ${A[@]}; echo $#
  bash: 3        huck (before): 1   (<bobtom dick harryjoe>)
```

Verified bash rules (all now matched):
- **Non-empty IFS**: join-with-IFS[0]-then-split is correct (whitespace IFS
  collapses empty fields, non-whitespace preserves them). Unchanged.
- **Empty IFS**: elements stay separate; an empty element yields no word (null
  removal) — `IFS=''; A=(a '' b)` → 2; `IFS='/'; A=(a '' b)` → 3.
- Quoted `"${A[@]}"` (separate) and `"${A[*]}"` (joined with IFS[0]) — correct.
- Unquoted `${A[*]}` behaves identically to unquoted `${A[@]}`.
- Applies to indexed AND associative arrays.

## Design

Three edits in `crates/huck-engine/src/expand.rs`, all prototype-verified
byte-identical to bash 5.2.21 and jointly flipping the category.

### 1. Unquoted array-`WordList` field-splitting (`expand_part`)

The unquoted branch that consumed an `ExpansionResult::WordList` joined with
IFS[0] then split — collapsing the elements under empty IFS. Split by IFS class:

```rust
let ifs = shell.ifs();
if ifs.is_empty() {
    // Empty IFS: no splitting, but each array element stays a SEPARATE word;
    // an empty element yields no word (unquoted null removal). Element
    // boundaries are field boundaries; surrounding text attaches to the
    // first/last non-empty element.
    let mut emitted_any = false;
    for w in words.iter().filter(|w| !w.is_empty()) {
        if emitted_any {
            result.push(std::mem::take(current));
        }
        current.push_str(w, false);
        *has_emitted = true;
        emitted_any = true;
    }
} else {
    // Non-empty IFS: join with IFS[0] then field-split (unchanged) — whitespace
    // IFS collapses the empty fields, non-whitespace preserves them, exactly
    // bash's element-boundary behavior.
    let sep = ifs_join_sep(&ifs);
    let joined = words.join(&sep);
    emit_split_fields(&joined, &ifs, current, result, has_emitted);
}
```

### 2 + 3. Unquoted `${arr[*]}` → `WordList` (indexed + associative)

`(PM::None, SK::Star)` returned a pre-joined `Value` for both quoted and
unquoted. Gate on `quoted`: quoted `"${arr[*]}"` keeps the joined `Value`;
unquoted `${arr[*]}` returns `WordList` (like `[@]`) so it flows through the
per-element split above. Applied in both `expand_array_param` (indexed) and
`expand_assoc_param` (associative):

```rust
(PM::None, SK::Star) => {
    if quoted {
        let sep = ifs_join_sep(&shell.ifs());
        ExpansionResult::Value(<values>.join(&sep))
    } else {
        ExpansionResult::WordList(<values>)
    }
}
```

Non-empty-IFS behavior is preserved exactly (WordList unquoted → join-then-split
== the old Value → split), so no other category changes — verified: `array`,
`more-exp`, `new-exp` diff counts are byte-identical to main (793/111/796).

## Testing

Gate = bash 5.2.21 fidelity + `array2` at 0 diff.

1. **Bash-diff harness** `tests/scripts/array_at_star_diff_check.sh` (model on an
   existing `-c` harness), byte-identical incl. stderr + exit. A matrix over
   `expr ∈ {${A[@]}, ${A[*]}, "${A[@]}", "${A[*]}"}` × `IFS ∈ {'', ' ', '/', unset}`
   with `A=(bob 'tom dick harry' joe)`, asserting `$#`+`$1..$3` after `set $expr`;
   empty-element `A=(a '' b)` counts for the three IFS classes; surrounding text
   `x${A[@]}y`; and an associative `${m[*]}`/`${m[@]}` count. (Elements with
   spaces make the empty-IFS separation visible.)
2. **`array2` category** flips: `HUCK_BASH_TEST_CATEGORY=array2` → PASS, 0 diff.
3. **Regression**: huck-engine lib green; the array / IFS / expansion `-p huck`
   integration bins green (`arrays`, `associative_arrays`, `ifs`,
   `array_literal_expansion`, `param_expansion`, `indirect_expansion`, …); full
   `run_diff_checks.sh` sweep green; previously-flipped categories (nquote/dynvar/
   parser/rhs-exp) stay PASS; `array`/`more-exp`/`new-exp` diff counts UNCHANGED
   from main (no regression — the fix is behavior-preserving for non-empty IFS).

Per repo constraints: build with `cargo build -p huck`; per-crate tests
single-threaded; NEVER `cargo test --workspace`; guard sweeps with
`ulimit -v 1500000` + `timeout`; run the `-p huck` integration bins
single-threaded before push; NO GPL bash text.

## Scope

**In scope.** The empty-IFS unquoted array-expansion fix (the `WordList` split +
unquoted `${arr[*]}` → `WordList` for indexed + associative); the harness; the
category flip; regressions.

**Out of scope (pre-existing, unrelated).** Unquoted `${arr[@]}` in a NO-SPLIT
context (assignment `x=${A[@]}`, `case`, `[[ ]]`) should join with a SPACE not
IFS[0] (the `[@]` `SK::All` path is unchanged by this fix, so the divergence is
pre-existing; not in `array2`) — file a follow-up. Associative iteration order
(#32).

## Documentation

- Removes a divergence (no new intentional one). #291 auto-closes via the PR
  (`Closes #291`). `docs/bash-divergences.md` unchanged.
- Update `docs/bash-test-suite-baseline.md` ("Updated by v334": `array2` PASS,
  Summary PASS 22→23, FAIL 60→59); record in `project_huck_iterations.md` +
  `MEMORY.md`.
