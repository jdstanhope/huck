# v334 — Flip the `array2` bash-suite category Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix unquoted `${arr[@]}`/`${arr[*]}` word-splitting under an empty IFS so the `array2` bash-suite category reaches 0-diff (Summary PASS 22→23, FAIL 60→59).

**Architecture:** One root, three edits in `crates/huck-engine/src/expand.rs`: split the unquoted array-`WordList` field-splitting by IFS class (empty IFS keeps elements separate; non-empty IFS keeps the existing join-then-split), and route unquoted `${arr[*]}` through `WordList` (like `[@]`) for both indexed and associative arrays.

**Tech Stack:** Rust; huck-engine (`expand.rs`); bash-diff harness.

Spec: `docs/superpowers/specs/2026-07-24-array2-category-flip-design.md`
Issue: [#291](https://github.com/jdstanhope/huck/issues/291)

## Global Constraints

- bash 5.2.21 fidelity — byte-identical incl. stderr + exit. With `A=(bob 'tom dick harry' joe)`, `set ${A[@]}` / `set ${A[*]}`:
  - IFS='' → `$#`=3 (`bob` / `tom dick harry` / `joe`); IFS=' ' → 5; IFS='/' → 3; unset IFS → 5.
  - Quoted `"${A[@]}"` → 3 (separate); `"${A[*]}"` → 1 (joined with IFS[0]).
  - Empty-element `A=(a '' b)`: IFS='' → 2, IFS=' ' → 2, IFS='/' → 3.
- Behavior for NON-empty IFS must be IDENTICAL to before (join-with-IFS[0]-then-split); only empty-IFS behavior changes. The `array`/`more-exp`/`new-exp` category diff counts must stay UNCHANGED from main (793/111/796).
- Do NOT change: the `SK::All` (`[@]`) expansion result; the quoted `"${arr[*]}"` join; the `Fields` split arm.
- Commit trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`; `cargo fmt --all` before committing. Per repo memory: build with `cargo build -p huck`; per-crate tests single-threaded; NEVER `cargo test --workspace`; guard sweeps with `ulimit -v 1500000` + `timeout`; run `-p huck` integration bins single-threaded before push; NO GPL bash text; no `Closes #N` in commits (bare `#N`).

---

### Task 1: Empty-IFS unquoted array expansion + category flip

**Files:**
- Modify: `crates/huck-engine/src/expand.rs` (unquoted `WordList` split in `expand_part`; `SK::Star` in `expand_array_param` + `expand_assoc_param`)
- Create: `tests/scripts/array_at_star_diff_check.sh`

- [ ] **Step 1: Write the harness (red)**

Create `tests/scripts/array_at_star_diff_check.sh` (model on an existing `-c` bash-diff harness — a reusable `check "label" 'fragment'` comparing `bash --norc --noprofile -c` vs `"$HUCK_BIN" -c`, byte-identical stdout+stderr+exit, huck path normalized). Cases (verify each against `bash --norc --noprofile` FIRST):
```sh
# matrix: {${A[@]}, ${A[*]}, "${A[@]}", "${A[*]}"} x {IFS='', ' ', '/', unset}
#   A=(bob 'tom dick harry' joe); set <expr>; echo "$#|$1|$2|$3"
# empty-element counts: A=(a '' b); set ${A[@]}; echo $#   (IFS '', ' ', '/')
# surrounding text: IFS=''; A=(p q); set x${A[@]}y; echo "$#|$1|$2"   -> 2|xp|qy
# associative: IFS=''; declare -A m=([x]=bob [y]='t d' [z]=joe); set ${m[*]}; echo $#  -> 3
```
Build (`cargo build -p huck`) and run — the empty-IFS unquoted cases FAIL (huck joins into one word).

- [ ] **Step 2: Split the unquoted `WordList` field-splitting by IFS class**

In `expand.rs`, `expand_part`, the `ExpansionResult::WordList(words)` arm's `else` (unquoted) branch currently is:
```rust
} else {
    // Unquoted: join with first IFS char then
    // let word-splitting do the rest.
    let ifs = shell.ifs();
    let sep = ifs_join_sep(&ifs);
    let joined = words.join(&sep);
    emit_split_fields(&joined, &ifs, current, result, has_emitted);
}
```
Replace with:
```rust
} else {
    let ifs = shell.ifs();
    if ifs.is_empty() {
        // Empty IFS: no field-splitting, but each array element still stays a
        // SEPARATE word (bash keeps `IFS=''; A=(bob 'x y' joe); set ${A[@]}` as
        // 3 fields, not 1 — joining with the empty IFS[0] would collapse them).
        // An empty element yields no word (unquoted null removal), matching
        // bash's `IFS=''; A=(a '' b)` → 2 fields. Element boundaries are field
        // boundaries; surrounding text attaches to the first/last non-empty
        // element.
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
        // Non-empty IFS: join with IFS[0] then field-split. Whitespace IFS
        // collapses the resulting empty fields and non-whitespace IFS preserves
        // them — exactly bash's element-boundary behavior in these cases.
        let sep = ifs_join_sep(&ifs);
        let joined = words.join(&sep);
        emit_split_fields(&joined, &ifs, current, result, has_emitted);
    }
}
```

- [ ] **Step 3: Route unquoted `${arr[*]}` through `WordList` (indexed)**

In `expand_array_param`, the `(PM::None, SK::Star)` arm currently always joins to a `Value`. Gate on `quoted`:
```rust
(PM::None, SK::Star) => {
    if quoted {
        // Quoted `"${a[*]}"` is a single word: elements joined with the first IFS char.
        let ifs = shell.ifs();
        let sep = ifs_join_sep(&ifs);
        ExpansionResult::Value(collect_values(shell).join(&sep))
    } else {
        // Unquoted `${a[*]}` behaves like unquoted `${a[@]}`: each element is a
        // separate word (field boundary), IFS-split within. A joined Value would
        // collapse the elements when IFS is empty.
        ExpansionResult::WordList(collect_values(shell))
    }
}
```

- [ ] **Step 4: Same for associative `${m[*]}`**

In `expand_assoc_param`, the `(PM::None, SK::Star)` arm:
```rust
(PM::None, SK::Star) => {
    if quoted {
        // Quoted `"${m[*]}"` — single word, joined with IFS[0].
        let ifs = shell.ifs();
        let sep = ifs_join_sep(&ifs);
        ExpansionResult::Value(values.join(&sep))
    } else {
        // Unquoted `${m[*]}` behaves like `${m[@]}`: separate words (a joined
        // Value would collapse them under an empty IFS).
        ExpansionResult::WordList(values)
    }
}
```

- [ ] **Step 5: Confirm the harness passes** — the whole matrix + empty-element + surrounding-text + associative cases byte-identical to bash.

- [ ] **Step 6: `array2` flips + regression**
```bash
cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1   # green
# array/IFS/expansion integration bins:
for t in arrays_integration associative_arrays_integration ifs_integration \
         array_literal_expansion_integration param_expansion_integration indirect_expansion_integration; do
  cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 2>&1 | grep "test result" || echo "(no bin: $t)"
done
cargo build --release -p huck
HUCK_BASH_TEST_CATEGORY=array2 HUCK_TEST_TIMEOUT=60 BASH_SOURCE_DIR=/tmp/bash-5.2.21 \
  timeout 120 bash tests/bash-test-suite/runner.sh 2>&1 | grep -iE "array2 \|"   # PASS 0-diff
# no regression in the big array categories (must match main's 793/111/796):
for c in array more-exp new-exp; do
  HUCK_BASH_TEST_CATEGORY=$c HUCK_TEST_TIMEOUT=60 BASH_SOURCE_DIR=/tmp/bash-5.2.21 \
    timeout 150 bash tests/bash-test-suite/runner.sh > /tmp/reg_$c.md 2>&1
  sc=$(grep -oE "/tmp/huck-bash-tests[^ ]*" /tmp/reg_$c.md | head -1)
  echo "$c diff=$(wc -l < $sc/$c.diff) (main: array=793 more-exp=111 new-exp=796)"
done
ulimit -v 1500000; timeout 550 bash tests/scripts/run_diff_checks.sh   # 226/226
```

- [ ] **Step 7: Docs + memory**
  - `docs/bash-test-suite-baseline.md`: prepend "Updated by v334 (#291, 2026-07-24 UTC): `array2` flipped to PASS (0-diff). Summary PASS 22→23, FAIL 60→59."
  - `project_huck_iterations.md` + `MEMORY.md`: record v334 (array2 flip; the empty-IFS unquoted `[@]`/`[*]` root; the per-IFS-class split; unquoted `[*]`→WordList; verified no-regression via main-worktree baseline).

- [ ] **Step 8: fmt + commit**
```bash
cargo fmt --all
git add crates/huck-engine/src/expand.rs tests/scripts/array_at_star_diff_check.sh docs/bash-test-suite-baseline.md
git commit -m "$(cat <<'EOF'
v334: empty-IFS unquoted ${arr[@]}/${arr[*]} keeps elements separate; flips array2 (#291)

Under an empty IFS, unquoted array [@]/[*] joined elements into one word; bash
keeps each element a separate word (empty IFS = no splitting, elements still
separate, empty elements dropped). Split the unquoted WordList field-splitting by
IFS class and route unquoted ${arr[*]} through WordList (indexed + associative).
Non-empty-IFS behavior unchanged (array/more-exp/new-exp diffs match main).
Flips the array2 bash-suite category (22 -> 0 diff, Summary PASS 22->23).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```
(Memory files live outside the repo — update in the same session, not this commit.)

---

## Self-Review

- **Spec coverage:** the `WordList` split (Step 2), unquoted `[*]`→WordList indexed (Step 3) + associative (Step 4), harness (Step 1), flip + regression (Step 6). All in Task 1 (single root, single file).
- **Placeholders:** none — exact code for all three edits.
- **Type consistency:** `ExpansionResult::{WordList(Vec<String>), Value(String)}`; `emit_split_fields(value, ifs, current, result, has_emitted)`; `Field::push_str(&str, bool)`; `ifs_join_sep(&str) -> String`.
- **Scope:** one root; the no-split `${arr[@]}`-joins-with-space divergence is pre-existing and out of scope; the review must confirm non-empty-IFS behavior is unchanged (the regression risk) and the empty-element null-removal matches bash per IFS class.
