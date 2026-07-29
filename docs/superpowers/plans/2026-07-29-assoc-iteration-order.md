# v342 — assoc iteration order (L-44) + `declare -c` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make huck's associative-array enumeration match bash 5.2.21's hash-table iteration order (L-44) and add the `declare -c` attribute, flipping the `casemod` bash-suite category to 0-diff PASS (runner PASS 30 → 31).

**Architecture:** A pure iteration-order *view* over huck's existing insertion-ordered storage (`VarValue::Associative(Vec<(String,String)>)`). A small `assoc_order` module computes bash's order; every assoc *enumeration* site reorders its pairs through it before iterating. Storage, lookup, assignment, and counts are untouched. Plus a `CaseFold::Capitalize` attribute for `declare -c`.

**Tech Stack:** Rust (`huck-engine`), bash-vs-huck diff-check harnesses, the bash test-suite runner.

**Design reference:** `docs/superpowers/specs/2026-07-29-assoc-iteration-order-design.md`. Issues: [#32](https://github.com/jdstanhope/huck/issues/32) (L-44), [#321](https://github.com/jdstanhope/huck/issues/321) (`declare -c`).

## Global Constraints

- **Branch:** all work on `v342-assoc-order` (off `main`). Do NOT push to `main` or merge; hand the PR to the user.
- **Commit trailer:** every commit ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Formatting:** `cargo fmt --all` before every commit (CI enforces `--check`).
- **This box OOMs on `cargo test --workspace`.** Test per-crate single-threaded ONLY: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. Build with `cargo build -p huck` / `cargo build --release --locked --bin huck`. Guard sweeps with `ulimit -v 1500000` + `timeout`.
- **Bash source** at `/tmp/bash-5.2.21`; `export BASH_SOURCE_DIR=/tmp/bash-5.2.21` for the runner.
- **The validated bash model (do NOT deviate):** bucket = `fnv1(key) & 1023` (1024 buckets); FNV-1 32-bit `h=2166136261; for byte: h=h.wrapping_mul(16777619); h^=byte`; iterate buckets ascending; within a bucket newest-inserted-first (descending Vec index). Verified against bash with 400 randomized tests, 0 mismatches. NO growth modeling (arrays <2048 keys stay at 1024).
- **Harness pattern:** `check "<label>" '<fragment>'` runs the fragment through `bash -c` and `huck -c`, asserts byte-identical incl. rc.

---

### Task 1: The `assoc_order` module + property test

**Files:**
- Create: `crates/huck-engine/src/assoc_order.rs`
- Modify: `crates/huck-engine/src/lib.rs` (add `mod assoc_order;`)

**Interfaces:**
- Produces: `pub(crate) fn assoc_hash(key: &str) -> u32`; `pub(crate) fn assoc_bash_order(pairs: &[(String, String)]) -> Vec<usize>`; `pub(crate) const ASSOC_NBUCKETS: u32 = 1024`. Also a convenience `pub(crate) fn assoc_ordered_pairs(pairs: &[(String,String)]) -> Vec<(String,String)>` returning the pairs cloned in bash order.

- [ ] **Step 1: Write the module with a discriminating unit test FIRST**

Create `crates/huck-engine/src/assoc_order.rs`:

```rust
//! bash 5.2.21 associative-array iteration order (L-44, #32). huck stores
//! assoc arrays insertion-ordered; bash iterates them in hash-table order.
//! This reproduces bash's order as a view: bucket = FNV-1(key) & 1023 (1024
//! buckets), buckets ascending, within a bucket newest-inserted first.
//! Validated against bash 5.2.21 with 400 randomized cases (incl. collisions,
//! updates, deletes), 0 mismatches. No growth modeling — arrays < 2048 keys
//! never rehash (bash grows at nentries >= nbuckets*2).

pub(crate) const ASSOC_NBUCKETS: u32 = 1024;

/// bash's `hash_string`: 32-bit FNV-1 over the key's bytes.
pub(crate) fn assoc_hash(key: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in key.bytes() {
        h = h.wrapping_mul(16777619);
        h ^= b as u32;
    }
    h
}

/// Indices into `pairs` (insertion order) in bash iteration order:
/// bucket asc, then newest-inserted-first (descending index) within a bucket.
pub(crate) fn assoc_bash_order(pairs: &[(String, String)]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..pairs.len()).collect();
    idx.sort_by_key(|&i| (assoc_hash(&pairs[i].0) & (ASSOC_NBUCKETS - 1), usize::MAX - i));
    idx
}

/// The pairs cloned into bash iteration order.
pub(crate) fn assoc_ordered_pairs(pairs: &[(String, String)]) -> Vec<(String, String)> {
    assoc_bash_order(pairs).into_iter().map(|i| pairs[i].clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p(ks: &[&str]) -> Vec<(String, String)> {
        ks.iter().enumerate().map(|(i, k)| (k.to_string(), i.to_string())).collect()
    }
    fn order(ks: &[&str]) -> Vec<String> {
        assoc_ordered_pairs(&p(ks)).into_iter().map(|(k, _)| k).collect()
    }

    #[test]
    fn hash_matches_bash() {
        // hash_string values verified against bash's compiled C.
        assert_eq!(assoc_hash("qux"), 980750837);
        assert_eq!(assoc_hash("foo"), 1083137555);
        assert_eq!(assoc_hash("k18"), 4202936175);
    }

    #[test]
    fn order_matches_bash_examples() {
        // Verified against bash 5.2.21 `declare -A a=([k]=v ...); echo "${!a[@]}"`.
        assert_eq!(order(&["one", "two", "three"]), vec!["two", "three", "one"]);
        assert_eq!(order(&["foo", "bar", "baz", "qux"]), vec!["qux", "foo", "bar", "baz"]);
        assert_eq!(order(&["apple", "banana", "cherry", "date", "fig"]),
                   vec!["cherry", "apple", "fig", "date", "banana"]);
        assert_eq!(order(&["x", "y", "z", "a", "b", "c"]), vec!["z", "y", "x", "c", "b", "a"]);
    }

    #[test]
    fn within_bucket_newest_first() {
        // Find two keys that genuinely collide (same bucket) and assert the
        // newer insertion (higher index) comes first. (Real-bash collision
        // behavior is additionally covered by the diff-check harness's
        // dup0/dup1 case in Task 2.) Compute a colliding pair deterministically:
        let mut a = None;
        'outer: for i in 0..2000u32 {
            for j in (i + 1)..2000u32 {
                let (ki, kj) = (format!("c{i}"), format!("c{j}"));
                if assoc_hash(&ki) & 1023 == assoc_hash(&kj) & 1023 {
                    a = Some((ki, kj));
                    break 'outer;
                }
            }
        }
        let (ki, kj) = a.expect("a colliding key pair must exist among c0..c1999");
        // Insert ki (index 0) then kj (index 1); same bucket → kj (newest) first.
        let pairs = vec![(ki.clone(), "0".to_string()), (kj.clone(), "1".to_string())];
        assert_eq!(assoc_bash_order(&pairs), vec![1, 0], "newest-in-bucket first");
    }
}
```

Add `mod assoc_order;` to `crates/huck-engine/src/lib.rs` (near the other `mod` declarations).

- [ ] **Step 2: Run the unit tests; verify they PASS**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 assoc_order`
Expected: PASS (the hash + example-order assertions are bash-ground-truth; if any fails, the model was transcribed wrong — fix the transcription, NOT the assertions).

- [ ] **Step 3: `cargo fmt --all` and commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/assoc_order.rs crates/huck-engine/src/lib.rs
git commit -m "$(cat <<'EOF'
v342: assoc_order module — bash 5.2.21 hash iteration order (#32)

FNV-1 hash, 1024 buckets, bucket-asc + within-bucket newest-first. Pure
order view; validated against bash (hash + example orders as unit tests).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Route `${m[...]}` expansion through bash order

**Files:**
- Modify: `crates/huck-engine/src/expand.rs` (`expand_assoc_param`, ~383; the snapshot at ~396)
- Test: `tests/scripts/assoc_order_diff_check.sh` (new)

**Interfaces:**
- Consumes: `crate::assoc_order::assoc_ordered_pairs`.

- [ ] **Step 1: Add the failing harness**

Create `tests/scripts/assoc_order_diff_check.sh` (mirror an existing `*_diff_check.sh` header + `check()` helper). Add cases:

```bash
S='declare -A a=([one]=1 [two]=2 [three]=3 [foo]=f [bar]=b [qux]=q)'
check "assoc values @"   "$S; printf '<%s>' \"\${a[@]}\"; echo"
check "assoc keys !@"     "$S; printf '<%s>' \"\${!a[@]}\"; echo"
check "assoc values *"    "$S; printf '<%s>' \"\${a[*]}\"; echo"
check "assoc for-in"      "$S"$'\n'"for k in \"\${!a[@]}\"; do printf '[%s=%s]' \"\$k\" \"\${a[\$k]}\"; done; echo"
check "assoc transform ^^" "$S; printf '<%s>' \"\${a[@]^^}\"; echo"
check "assoc transform #"  "$S; printf '<%s>' \"\${a[@]#?}\"; echo"
check "assoc slice"        "$S; printf '<%s>' \"\${a[@]:1:3}\"; echo"
# collision + update + unset
C='declare -A a; a[dup0]=1; a[dup1]=2; a[x]=3; a[dup0]=9; unset "a[x]"'
check "assoc upd/unset !@" "$C; printf '<%s>' \"\${!a[@]}\"; echo"
```

- [ ] **Step 2: Build + run; confirm assoc cases FAIL**

Run: `cargo build -p huck && bash tests/scripts/assoc_order_diff_check.sh`
Expected: the assoc cases FAIL — huck emits insertion order, bash hash order.

- [ ] **Step 3: Reorder the snapshot in `expand_assoc_param`**

In `expand.rs::expand_assoc_param` (~383), the function snapshots `let pairs: Vec<(String,String)> = shell.get_associative(name).cloned().unwrap_or_default();` (~396), then derives `values`/`keys` and all transforms/slices from `pairs`. Immediately after the snapshot, reorder it:

```rust
let pairs: Vec<(String, String)> = shell.get_associative(name).cloned().unwrap_or_default();
// L-44 (#32): bash iterates assoc arrays in hash order, not insertion order.
// Reorder the snapshot once; `values`/`keys`/transforms/slicing below all
// derive from `pairs`, so this covers every ${m[@]}/${!m[@]}/${m[@]<op>} form.
let pairs = crate::assoc_order::assoc_ordered_pairs(&pairs);
```

Confirm (read the function) that `values`, `keys`, per-element transforms, and slicing all derive from this `pairs` snapshot (not a re-fetch). If any path re-fetches `get_associative`, reorder there too.

- [ ] **Step 4: Build + run; confirm assoc expansion cases PASS**

Run: `cargo build -p huck && bash tests/scripts/assoc_order_diff_check.sh`
Expected: the `${a[@]}`/`${!a[@]}`/`${a[*]}`/for-in/transform/slice/collision cases PASS. (declare-p/@A/@K cases, if any added, still fail — Task 3.)

- [ ] **Step 5: Per-crate lib tests**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`
Expected: PASS. If a lib test asserts the OLD insertion-order assoc *expansion* output, update it to bash order (note which in the report).

- [ ] **Step 6: `cargo fmt --all` and commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/expand.rs tests/scripts/assoc_order_diff_check.sh
git commit -m "$(cat <<'EOF'
v342: assoc ${m[@]}/${!m[@]} expansion in bash hash order (#32)

expand_assoc_param reorders its pairs snapshot through assoc_ordered_pairs,
covering values/keys/per-element transforms/slicing in one chokepoint.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Route `declare -p` / bare-declare render + `@A`/`@K`/`@k` through bash order

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (assoc render at ~1129)
- Modify: `crates/huck-engine/src/array_transforms.rs` (`@K` ~181, `@k` ~248, `@A` ~87/113)
- Modify: `crates/huck-engine/src/shell_state.rs` (any assoc render/enumeration used by declare output, e.g. ~56, ~2139 — verify which produce the `[k]=v` listing)
- Test: `tests/scripts/assoc_order_diff_check.sh` (extend)

**Interfaces:**
- Consumes: `crate::assoc_order::assoc_ordered_pairs`.

- [ ] **Step 1: Add failing render/transform harness cases**

Extend `tests/scripts/assoc_order_diff_check.sh` before the total line:

```bash
check "assoc declare -p"  "$S; declare -p a"
check "assoc bare declare" "$S; declare -A | grep '^declare -A a='"
check "assoc @A"           "$S; printf '%s\n' \"\${a[@]@A}\""
check "assoc @K"           "$S; printf '%s\n' \"\${a[@]@K}\""
check "assoc @k"           "$S; printf '<%s>' \"\${a[@]@k}\"; echo"
```

- [ ] **Step 2: Build + run; confirm these FAIL**

Run: `cargo build -p huck && bash tests/scripts/assoc_order_diff_check.sh`
Expected: the declare-p/bare-declare/@A/@K/@k cases FAIL (insertion order).

- [ ] **Step 3: Reorder at each render/transform site**

At each site that iterates `VarValue::Associative(pairs)` to PRODUCE OUTPUT, reorder before iterating (do NOT reorder storage — reorder a local view):
- `builtins.rs:~1129` — the `declare -p`/bare-`declare` assoc listing: iterate `crate::assoc_order::assoc_ordered_pairs(pairs)` instead of `pairs`.
- `array_transforms.rs:~181` (`@K`) and `~248` (`@k`): iterate the reordered pairs.
- `array_transforms.rs @A` (~87/113): if it builds a `declare -A x=(…)` string, reorder its pairs too.
- `shell_state.rs`: check `~56` and `~2139` — if either renders the `[k]=v` listing for declare output, reorder; if it's a lookup/count/assignment, leave it. (State in the report which shell_state sites were reorder-vs-left.)

Do NOT touch: `get_associative` (lookup), assignment paths (`replace_associative`/`set_associative_element`/`append_associative_element`), `${#a[@]}` count, membership. Storage stays insertion-ordered.

- [ ] **Step 4: Build + run; confirm ALL assoc-order cases PASS**

Run: `cargo build -p huck && bash tests/scripts/assoc_order_diff_check.sh`
Expected: `Fail: 0`.

- [ ] **Step 5: Per-crate lib tests + grep for stale insertion-order assertions**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`
Then `grep -rn` the test modules for assoc `declare -p`/`@K`/`@k` expected strings asserting the OLD order; update them to bash order. Expected: PASS after updates (list them in the report).

- [ ] **Step 6: `cargo fmt --all` and commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/builtins.rs crates/huck-engine/src/array_transforms.rs crates/huck-engine/src/shell_state.rs tests/scripts/assoc_order_diff_check.sh
git commit -m "$(cat <<'EOF'
v342: assoc declare -p / @A / @K / @k render in bash hash order (#32)

Route the declare-p/bare-declare listing and the @A/@K/@k transforms through
assoc_ordered_pairs. Storage/lookup/assignment/count untouched.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: `declare -c` (capitalize-first attribute)

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (`CaseFold` enum + `apply_case_fold` ~3420; the `-c` render in `declare -p`)
- Modify: `crates/huck-engine/src/builtins.rs` (declare/typeset/local/export flag parser: accept `-c`/`+c`)
- Test: `tests/scripts/assoc_order_diff_check.sh` or a small `declare_c_diff_check.sh`

**Interfaces:**
- Consumes: existing `${var@u}`/UpperFirst capitalize logic if factored out; else replicate (uppercase first char, rest unchanged).

- [ ] **Step 1: Add failing harness cases**

```bash
check "declare -c basic"   'declare -c x="hello world"; echo "$x"'
check "declare -c reassign" 'declare -c x; x="foo BAR"; echo "$x"; x="BAZ qux"; echo "$x"'
check "declare -c decl-p"  'declare -c x="hello"; declare -p x'
check "declare -lc last-wins" 'declare -l -c x="HELLO"; echo "$x"'
```

Run `cargo build -p huck && bash tests/scripts/assoc_order_diff_check.sh` → these FAIL (`declare: -c: invalid option`). (Verify each expected value against bash first: `declare -c x="hello world"` → `Hello world`; `declare -l -c` → last-wins.)

- [ ] **Step 2: Add `CaseFold::Capitalize`**

In `shell_state.rs`, extend `CaseFold` with `Capitalize`, and add the arm to `apply_case_fold`:

```rust
Some(CaseFold::Capitalize) => {
    // Uppercase the first char, leave the rest unchanged (bash `-c` / ${v@u}).
    let mut cs = value.chars();
    match cs.next() {
        Some(c0) => c0.to_uppercase().collect::<String>() + cs.as_str(),
        None => value,
    }
}
```

(If huck already has a `capitalize_first`/UpperFirst helper for `${v@u}`, call it instead for consistency.)

- [ ] **Step 3: Accept `-c`/`+c` in the flag parser + `declare -p` render**

In `builtins.rs`, wherever the declare/typeset/local/export flag loop handles `-u`/`-l` (setting `CaseFold::Upper`/`Lower`), add `-c` → `CaseFold::Capitalize` (and `+c` to clear, matching `+u`/`+l`). `-u`/`-l`/`-c` share the one case-fold slot (last-wins). In the `declare -p` attribute-letter rendering, emit `c` for `CaseFold::Capitalize` (mirroring `u`/`l`).

- [ ] **Step 4: Build + run; confirm the `-c` cases PASS**

Run: `cargo build -p huck && bash tests/scripts/assoc_order_diff_check.sh`
Expected: `Fail: 0`.

- [ ] **Step 5: Per-crate lib tests**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`
Expected: PASS.

- [ ] **Step 6: `cargo fmt --all` and commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/shell_state.rs crates/huck-engine/src/builtins.rs tests/scripts/assoc_order_diff_check.sh
git commit -m "$(cat <<'EOF'
v342: declare -c capitalize-first attribute (#321)

CaseFold::Capitalize mirrors -u/-l: uppercases the first char of the value on
assignment; accepted by declare/typeset/local/export, rendered in declare -p.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Verify the casemod flip, prove no-regression, update docs + memory

**Files:**
- Modify: `docs/bash-test-suite-baseline.md`, memory files.

- [ ] **Step 1: Build release**

Run: `cargo build --release --locked --bin huck` → clean.

- [ ] **Step 2: `casemod` runner — 0-diff PASS**

```bash
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
HUCK_BASH_TEST_CATEGORY=casemod bash tests/bash-test-suite/runner.sh 2>/dev/null | grep -E '^\| casemod '
```
Expected: `| casemod | PASS |`. If FAIL, inspect the fresh `/tmp/huck-bash-tests-*/casemod.diff` and map residuals (should be L-44 order or `-c`).

- [ ] **Step 3: Measure the assoc cluster + no-regression baseline**

Build `origin/main` in a worktree; for `casemod assoc appendop quotearray` AND the currently-PASS categories that use assoc/indexed arrays (`array2 nquote1 nquote2 nquote3 arrays`-related), compare status + diff-line-count BASE vs v342. Expected: `casemod` FAIL→PASS; `assoc`/`appendop`/`quotearray` diffs SHRINK (record) but stay FAIL; **every PASS category stays PASS** (no assoc-order regression); indexed-array output byte-identical. (Command pattern per v341's Task-4 Step-3.)

- [ ] **Step 4: Full diff-check sweep**

```bash
cargo build -p huck
( ulimit -v 1500000; timeout 600 bash tests/scripts/run_diff_checks.sh )
```
Expected: green, incl. `assoc_order`, `associative_arrays`, `declare_*`, `array*`, `param*`.

- [ ] **Step 5: Integration bins**

```bash
for t in associative_arrays_integration declare_integration arrays_integration param_transform_integration; do
  ( ulimit -v 1500000; cargo test -p huck --test $t --jobs 1 -- --test-threads 1 ) 2>&1 | grep -E 'test result|error\[' || echo "MISSING/FAILED: $t"
done
```
Expected: each `test result: ok` (skip a nonexistent bin, note it).

- [ ] **Step 6: Full runner — confirm only casemod flipped**

```bash
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
bash tests/bash-test-suite/runner.sh 2>/dev/null | grep -E 'PASS:|FAIL:'
```
Expected: `PASS: 31`, `FAIL: 51`; PASS list gained exactly `casemod`.

- [ ] **Step 7: Update `docs/bash-test-suite-baseline.md`**

Dated `**Updated by v342 (#32/#321, 2026-07-29 UTC):**` note: `casemod` flipped via L-44 assoc hash-order (FNV-1, 1024 buckets, bucket-asc + newest-first) + `declare -c`; Summary PASS 30→31, FAIL 52→51; L-44 also shrank `assoc`/`appendop`; no regressions. Update the `## Summary` count block (PASS 30→31, FAIL 52→51) + PASS list. `casemod` row → PASS.

- [ ] **Step 8: Update memory files** (`project_huck_iterations.md` + `MEMORY.md` hook): FLIPS `casemod` 30→31 via L-44 (validated bash assoc order: FNV-1 + 1024 buckets + bucket-asc/newest-first, as an iteration-order VIEW over insertion-ordered storage, routed at ~5 enumeration chokepoints) + `declare -c`. Durable: the feasibility spike found the 1024-bucket size (not the 128 `DEFAULT_HASH_BUCKETS`) — that was the unlock; validated with 400 randomized bash cross-checks BEFORE the spec. L-44 flips no category ALONE (each has other roots) — it's a keystone; only casemod (=L-44+declare-c) flips here. Keep MEMORY.md under 17.1KB.

- [ ] **Step 9: Commit docs**

```bash
git add docs/bash-test-suite-baseline.md
git commit -m "$(cat <<'EOF'
v342: baseline — casemod flipped to PASS (30->31) (#32/#321)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```
(Memory files are outside the repo — Write tool, not git.)

- [ ] **Step 10: File follow-up issues** for the residual roots surfaced: `assoc` (BASH_ALIASES/BASH_CMDS, L-46 bare-attr, error wording, integer-assoc), `appendop` (integer-array `+=` arith, `${#}` counts, numeric assoc key), `quotearray` (arith-subscript with special-char key). One issue each (or reference existing ones), so the cluster's remaining work is tracked.

---

## Final review & PR (after all tasks)

- [ ] Review the whole branch diff for stray edits, and especially that NO assignment/lookup/count path was reordered (only enumeration/output).
- [ ] `cargo fmt --all --check` clean; `cargo build --workspace --locked` (build only) succeeds.
- [ ] Push `v342-assoc-order`, open a PR targeting `main` with body `Closes #32` + `Closes #321`, summarizing the validated model, the view approach, the casemod flip, and the no-regression evidence. Hand to the user; wait for CI green (do NOT self-merge).
