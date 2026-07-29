# v343 — order-preserving AssocMap Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `VarValue::Associative(Vec<(String,String)>)` with a hand-rolled order-preserving hash map (`AssocMap`) for O(1) element get/set/insert, fixing the O(N²) assoc-insert cost — with ZERO observable behavior change (full bash-suite runner stays PASS 31, no diffs move).

**Architecture:** New `assoc_map` module: `HashMap<String,usize>` index + insertion-ordered `Vec<(String,String)>`. The v342 L-44 view (`assoc_order.rs`) is unchanged — it consumes `AssocMap::pairs()` (the insertion-ordered slice), so all output is byte-identical.

**Tech Stack:** Rust (`huck-engine`), bash-vs-huck diff-check harnesses, the bash test-suite runner.

**Design reference:** `docs/superpowers/specs/2026-07-29-assoc-order-preserving-map-design.md`. Issue: [#325](https://github.com/jdstanhope/huck/issues/325).

## Global Constraints

- **Branch:** `v343-assoc-map` (off `main`). Do NOT push to `main` or merge; hand the PR to the user.
- **Commit trailer:** every commit ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Formatting:** `cargo fmt --all` before every commit (CI enforces `--check`).
- **This box OOMs on `cargo test --workspace`.** Test per-crate single-threaded ONLY: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. Build with `cargo build -p huck` / `--release`. Guard sweeps with `ulimit -v 1500000` + `timeout`.
- **Bash source** at `/tmp/bash-5.2.21`; `export BASH_SOURCE_DIR=/tmp/bash-5.2.21` for the runner.
- **THE INVARIANT (load-bearing):** `AssocMap.order`'s index MUST equal bash insertion order — insert appends a new key; update keeps position; remove shift-preserves survivors' relative order. If this holds, the L-44 view and ALL output are byte-identical.
- **NO behavior change.** If any test's EXPECTED output would change, that's a migration bug, not an intended change — stop and investigate. Tests may only change if they constructed the OLD `Vec` variant directly (→ `AssocMap`), never their assertions.
- **No new dependency** (hand-rolled, no `indexmap`).

---

### Task 1: The `AssocMap` module + unit/property tests

**Files:**
- Create: `crates/huck-engine/src/assoc_map.rs`
- Modify: `crates/huck-engine/src/lib.rs` (`pub(crate) mod assoc_map;`)

**Interfaces:**
- Produces: `pub(crate) struct AssocMap` with `new`, `get(&str)->Option<&str>`, `contains_key`, `len`, `is_empty`, `insert(String,String)`, `remove(&str)->bool`, `pairs()->&[(String,String)]`, `iter()`, `impl FromIterator<(String,String)>`, `#[derive(Debug,Clone,Default,PartialEq,Eq)]`.

- [ ] **Step 1: Write the module (verbatim from the spec) with unit + property tests**

Create `crates/huck-engine/src/assoc_map.rs` using the `AssocMap` code from the spec (the struct + `new`/`get`/`contains_key`/`len`/`is_empty`/`insert`/`remove`/`pairs`/`iter` + `FromIterator`). Add tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_update_len() {
        let mut m = AssocMap::new();
        m.insert("a".into(), "1".into());
        m.insert("b".into(), "2".into());
        assert_eq!(m.get("a"), Some("1"));
        assert_eq!(m.len(), 2);
        m.insert("a".into(), "9".into());               // update in place
        assert_eq!(m.get("a"), Some("9"));
        assert_eq!(m.len(), 2);
        assert_eq!(m.pairs(), &[("a".into(), "9".into()), ("b".into(), "2".into())]); // a keeps pos
    }

    #[test]
    fn remove_preserves_order() {
        let mut m: AssocMap = [("a","1"),("b","2"),("c","3"),("d","4")]
            .iter().map(|(k,v)| (k.to_string(), v.to_string())).collect();
        assert!(m.remove("b"));
        assert!(!m.remove("zzz"));
        assert_eq!(m.pairs().iter().map(|(k,_)| k.as_str()).collect::<Vec<_>>(), vec!["a","c","d"]);
        // index still correct after reindex
        assert_eq!(m.get("d"), Some("4"));
        m.insert("e".into(), "5".into());
        assert_eq!(m.pairs().last().unwrap().0, "e");
    }

    #[test]
    fn from_iter_dup_key_first_pos_last_value() {
        // bash: declare -A a=([k]=1 [x]=2 [k]=3) -> k keeps first pos, value 3.
        let m: AssocMap = [("k","1"),("x","2"),("k","3")]
            .iter().map(|(k,v)| (k.to_string(), v.to_string())).collect();
        assert_eq!(m.pairs().iter().map(|(k,_)| k.as_str()).collect::<Vec<_>>(), vec!["k","x"]);
        assert_eq!(m.get("k"), Some("3"));
    }

    #[test]
    fn property_matches_vec_reference() {
        // Random op sequences: AssocMap must match a Vec-based reference for
        // both key-set and ordered pairs. Deterministic PRNG (no external dep).
        fn vec_ref_insert(v: &mut Vec<(String,String)>, k: String, val: String) {
            if let Some(s) = v.iter_mut().find(|(kk,_)| *kk == k) { s.1 = val; } else { v.push((k, val)); }
        }
        fn vec_ref_remove(v: &mut Vec<(String,String)>, k: &str) { v.retain(|(kk,_)| kk != k); }
        let mut seed: u64 = 0x9e3779b97f4a7c15;
        let mut rng = || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; seed };
        for _ in 0..300 {
            let mut m = AssocMap::new();
            let mut r: Vec<(String,String)> = Vec::new();
            for _ in 0..200 {
                let key = format!("k{}", rng() % 30);
                if rng() % 4 == 0 {
                    m.remove(&key); vec_ref_remove(&mut r, &key);
                } else {
                    let val = (rng() % 1000).to_string();
                    m.insert(key.clone(), val.clone()); vec_ref_insert(&mut r, key, val);
                }
                assert_eq!(m.pairs(), r.as_slice(), "ordered pairs must match Vec reference");
            }
        }
    }
}
```

Add `pub(crate) mod assoc_map;` to `lib.rs` (alphabetically). The struct/functions may be `dead_code` until Task 2 wires them — `#[allow(dead_code)]` per item ONLY if CI denies warnings (it doesn't); note it.

- [ ] **Step 2: Run the tests; verify PASS**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 assoc_map`
Expected: all pass. The property test (300×200 ops) locks the order invariant.

- [ ] **Step 3: `cargo fmt --all` and commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/assoc_map.rs crates/huck-engine/src/lib.rs
git commit -m "$(cat <<'EOF'
v343: AssocMap — order-preserving string map for assoc arrays (#325)

HashMap<key,idx> + insertion-ordered Vec: O(1) get/insert/update, O(n)
shift-remove (order-preserving). Property test cross-checks a Vec reference.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Migrate `VarValue::Associative` to `AssocMap` (the whole change, atomically)

A Rust type change breaks every site until all are fixed, so this is one task: change the type, update every site so the crate compiles AND every existing test passes UNCHANGED.

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (the `VarValue` enum + its mirror ~163; `get_associative` ~2992; `lookup_associative_element` ~3013; `unset_associative_element` ~3063; `store_assoc_element` ~2139; the `assign` handlers for `AssignDest::Element{Subscript::Key}` and `AssignDest::Whole`+`AssignSource::Associative`; every `VarValue::Associative(pairs)` match arm)
- Modify: `crates/huck-engine/src/array_transforms.rs` (`@A`/`@K`/`@k` arms → `.pairs()`)
- Modify: `crates/huck-engine/src/builtins.rs` (`render_declare_value_part` + type-check arms → `.pairs()`)
- Modify: `crates/huck-engine/src/expand.rs` (`expand_assoc_param` snapshot: `get_associative(name)` now returns `&AssocMap` → `.pairs().to_vec()`)
- Audit + modify as needed: `crates/huck-engine/src/{arith.rs, executor.rs}` (any assoc construction/match)

**Interfaces:**
- Consumes: `crate::assoc_map::AssocMap`.

- [ ] **Step 1: Change the storage type**

In `shell_state.rs`, change both `VarValue::Associative(Vec<(String, String)>)` occurrences (the enum ~41 and the mirror ~163) to `VarValue::Associative(crate::assoc_map::AssocMap)`. This will break compilation everywhere the variant is constructed or destructured — that is the work-list for the rest of this task.

- [ ] **Step 2: Update the accessors (make them O(1))**

- `get_associative` → return `Option<&crate::assoc_map::AssocMap>` (change the signature + the `Some(pairs)` arm). Callers that did `.cloned()` become `.map(|m| m.pairs().to_vec())`; callers that did `.iter()` become `m.iter()`; `.is_some()` unchanged.
- `lookup_associative_element` → `self.get_associative(name).and_then(|m| m.get(key).map(str::to_string))` (O(1)).
- `unset_associative_element` → replace `pairs.retain(...)` with `map.remove(key);` (order-preserving).
- `store_assoc_element` (~2139) → replace the `pairs.iter_mut().find(...) else pairs.push(...)` block with `map.insert(key, value);` (O(1)).
- The `assign` handler for `AssignDest::Whole` + `AssignSource::Associative(pairs)` → build the stored value with `VarValue::Associative(pairs.into_iter().collect())` (Vec → `AssocMap` via `FromIterator`). (Keep `AssignSource::Associative(Vec<(String,String)>)` as the transient input type — only STORAGE becomes `AssocMap`.)
- The `assign` handler for `AssignDest::Element{Subscript::Key}` append (`AssignKind::Append`) → get current via `map.get(key)`, concatenate, `map.insert(...)`.

- [ ] **Step 3: Update every remaining `VarValue::Associative(pairs)` match/iterate site**

Compile (`cargo build -p huck`) and fix each error:
- Read/iterate sites (`array_transforms.rs` `@A`/`@K`/`@k`; `builtins.rs` render + type checks; `shell_state.rs` `scalar_view` ~56, declare-p paths; `expand.rs` `expand_assoc_param`): use `pairs.pairs()` (the slice) or `pairs.iter()`. The L-44 sites feed `assoc_order::assoc_ordered_pairs(map.pairs())` — same logic, new source.
- Construction sites (anywhere a `VarValue::Associative(vec)` is built): `VarValue::Associative(vec.into_iter().collect())`.
- Type-check/`matches!` sites: `VarValue::Associative(_)` unchanged.
Repeat until `cargo build -p huck` is clean (0 errors, 0 warnings).

- [ ] **Step 4: Run the full per-crate test suite — must pass UNCHANGED**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`
Expected: ALL pass. **If a test fails on an EXPECTED-VALUE mismatch, the migration broke behavior — fix the code, do NOT change the assertion.** A test may only be edited if it CONSTRUCTED the old `Vec` variant directly (e.g. `seed_*` helpers or `VarValue::Associative(vec![...])` in a test) — convert the construction to `AssocMap` / `.into_iter().collect()`, leaving assertions intact. List any such construction-only edits in the report.

- [ ] **Step 5: Harness + integration bins unchanged**

Run:
```bash
cargo build -p huck
bash tests/scripts/assoc_order_diff_check.sh
for t in associative_arrays_integration declare_integration arrays_integration; do
  ( ulimit -v 1500000; cargo test -p huck --test $t --jobs 1 -- --test-threads 1 ) 2>&1 | grep -E 'test result|error\[' | sed "s/^/[$t] /"
done
```
Expected: `assoc_order_diff_check.sh` `Fail: 0`; each integration bin `test result: ok`. No expected-value edits.

- [ ] **Step 6: `cargo fmt --all` and commit**

```bash
cargo fmt --all
git add -A
git commit -m "$(cat <<'EOF'
v343: back associative arrays with AssocMap — O(1) element access (#325)

VarValue::Associative now holds an AssocMap (O(1) get/set/insert, O(n)
order-preserving remove) instead of a Vec. Behavior-preserving: pairs() feeds
the L-44 order view unchanged; all output byte-identical, full test suite green.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Verify behavior-preservation + benchmark, update docs + memory

**Files:**
- Modify: `docs/bash-test-suite-baseline.md` (a note; NO count change — stays 31/51), memory files.

- [ ] **Step 1: Build release + full runner — PASS 31, ZERO movement**

```bash
cargo build --release --locked --bin huck
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
bash tests/bash-test-suite/runner.sh 2>/dev/null | grep -E 'PASS:|FAIL:'
```
Expected: `PASS: 31`, `FAIL: 51` — identical to v342. Any change = a behavior regression; investigate before proceeding.

- [ ] **Step 2: Confirm no diff moved (behavior-preservation spot-check)**

Build `origin/main` in a worktree at `/tmp/huck-v343-base` (`git worktree add`, `cargo build --release --locked --bin huck` inside it — KEEP it for the Step 4 benchmark). For the assoc categories (`assoc appendop casemod quotearray`) AND a few PASS categories (`array2 nquote nquote1`), confirm status AND diff-line-count are IDENTICAL BASE vs v343 (the whole point: no observable change).

- [ ] **Step 3: Full diff-check sweep**

Run: `( ulimit -v 1500000; timeout 600 bash tests/scripts/run_diff_checks.sh )`
Expected: green, same count as v342 (no harness expected-value changes).

- [ ] **Step 4: Benchmark — demonstrate O(N²)→O(N)**

Time a large assoc-insert workload on the v343 release binary vs the origin/main baseline binary:
```bash
SCRIPT='a=(); declare -A a; for ((i=0;i<30000;i++)); do a[k$i]=$i; done; echo ${#a[@]}'
echo "=== v343 ==="; time (echo "$SCRIPT" | ./target/release/huck)
echo "=== main baseline ==="; time (echo "$SCRIPT" | /tmp/huck-v343-base/target/release/huck)
```
Expected: v343 completes in well under a second; the baseline is dramatically slower (quadratic). Record both times. Then remove the worktree: `git worktree remove --force /tmp/huck-v343-base`.

- [ ] **Step 5: Update `docs/bash-test-suite-baseline.md`**

Add a dated `**Updated by v343 (#325, 2026-07-29 UTC):**` note: associative arrays now backed by an order-preserving `AssocMap` (O(1) element access; O(N²)→O(N) insert). **Behavior-preserving — no category movement (PASS stays 31/51)**; the L-44 view is unchanged. NO count-block edit (it stays 31/51).

- [ ] **Step 6: Update memory files** (`project_huck_iterations.md` + `MEMORY.md` hook): v343 — NON-FLIP perf refactor: `VarValue::Associative(Vec)` → order-preserving `AssocMap` (HashMap<key,idx>+order Vec; O(1) get/set/insert, O(n) shift-remove). Fixes O(N²) assoc-insert. Behavior-preserving (L-44 view unchanged, PASS stays 31). Durable: the INVARIANT is order-Vec-index==bash-insertion-order (insert-append/update-keep/remove-shift-preserve) — property-tested vs a Vec reference. Bench: 30k inserts quadratic→linear. Rejected bash-bucket-as-storage (couples core store to bash internals). Keep MEMORY.md under 17.1KB.

- [ ] **Step 7: Commit docs**

```bash
git add docs/bash-test-suite-baseline.md
git commit -m "$(cat <<'EOF'
v343: baseline note — assoc arrays backed by AssocMap, behavior-preserving (#325)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```
(Memory files are outside the repo — Write tool, not git.)

---

## Final review & PR (after all tasks)

- [ ] Review the whole branch diff — especially that the INVARIANT holds (insert-append/update-keep/remove-shift-preserve) and NO test assertion (expected value) changed — only construction-site edits, if any.
- [ ] `cargo fmt --all --check` clean; `cargo build --workspace --locked` (build only) succeeds.
- [ ] Push `v343-assoc-map`, open a PR targeting `main` with body `Closes #325`, summarizing the O(1) win, the behavior-preservation evidence (full suite unchanged, runner still 31, no diff moved), and the benchmark. Hand to the user; wait for CI green (do NOT self-merge).
