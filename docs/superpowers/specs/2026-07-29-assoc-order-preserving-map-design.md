# v343 — order-preserving associative map for O(1) element access

**Issue:** [#325 — associative arrays: O(n) element access](https://github.com/jdstanhope/huck/issues/325).

**Goal:** replace the `Vec<(String,String)>` backing associative arrays with a
hand-rolled **order-preserving hash map** (`AssocMap`), giving O(1) average
element get/set/insert (fixing the O(N²) insert-loop cost) while preserving
insertion order — so the v342 L-44 iteration-order view (`assoc_order.rs`) and
all observable behavior stay **byte-identical**. This is a behavior-preserving
performance refactor: **zero output change, zero category movement** (full runner
stays PASS 31).

## Background

`VarValue::Associative(Vec<(String,String)>)` makes every element access a linear
scan: `lookup_associative_element` / `store_assoc_element` do
`pairs.iter().find(|(k,_)| k == key)` (O(n)); a script assigning N keys is O(N²).
The v342 L-44 order is a display-time view over the insertion-ordered Vec and is
NOT the perf issue — element access is. This refactor makes access O(1) without
adopting bash's bucket-table as storage (rejected in #325: too invasive, couples
huck's core store to bash internals).

## The `AssocMap` type (hand-rolled, no new dependency)

New module `crates/huck-engine/src/assoc_map.rs`:

```rust
use std::collections::HashMap;

/// An order-preserving string→string map for associative arrays. `order`
/// holds `(key, value)` in INSERTION order (the basis the L-44 view sorts on);
/// `index` maps each live key to its position in `order` for O(1) access.
/// Invariant: `index[k] == i` iff `order[i].0 == k`, and `order` has no
/// duplicate keys. Insertion order of survivors is preserved across updates
/// and removes (matching bash), so `assoc_order.rs` output is unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssocMap {
    order: Vec<(String, String)>,
    index: HashMap<String, usize>,
}

impl AssocMap {
    pub fn new() -> Self { Self::default() }

    /// O(1) get.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.index.get(key).map(|&i| self.order[i].1.as_str())
    }
    pub fn contains_key(&self, key: &str) -> bool { self.index.contains_key(key) }
    pub fn len(&self) -> usize { self.order.len() }
    pub fn is_empty(&self) -> bool { self.order.is_empty() }

    /// O(1): update in place if present (position unchanged, like bash), else
    /// append (new key = highest insertion index).
    pub fn insert(&mut self, key: String, value: String) {
        if let Some(&i) = self.index.get(&key) {
            self.order[i].1 = value;
        } else {
            self.index.insert(key.clone(), self.order.len());
            self.order.push((key, value));
        }
    }

    /// O(n): shift-remove, reindexing the tail so survivors keep their relative
    /// insertion order (matching bash `unset a[k]`). Returns whether present.
    pub fn remove(&mut self, key: &str) -> bool {
        match self.index.remove(key) {
            Some(i) => {
                self.order.remove(i);
                for (j, (k, _)) in self.order.iter().enumerate().skip(i) {
                    self.index.insert(k.clone(), j);
                }
                true
            }
            None => false,
        }
    }

    /// The `(key, value)` pairs in INSERTION order (what the L-44 view consumes).
    pub fn pairs(&self) -> &[(String, String)] { &self.order }
    pub fn iter(&self) -> std::slice::Iter<'_, (String, String)> { self.order.iter() }
}

impl FromIterator<(String, String)> for AssocMap {
    /// Build from `(k,v)` pairs in order; a later duplicate key updates in place
    /// (keeping the FIRST position), matching bash compound-assignment semantics.
    fn from_iter<I: IntoIterator<Item = (String, String)>>(it: I) -> Self {
        let mut m = AssocMap::new();
        for (k, v) in it { m.insert(k, v); }
        m
    }
}
```

Notes:
- `remove`'s reindex loop is O(n); `unset` is rare and the hot path (insert/set)
  is now O(1). Tombstoned O(1) remove was considered and rejected (complicates
  every iteration; YAGNI).
- Duplicate-key handling in `from_iter`/`insert` must match bash: in
  `declare -A a=([k]=1 [k]=2)`, the key keeps its FIRST position with the LAST
  value. `insert`'s update-in-place already does this. (Verify against bash.)

## Migration

1. **Storage type:** `VarValue::Associative(Vec<(String,String)>)` →
   `VarValue::Associative(AssocMap)` (both the value enum and the mirror at
   `shell_state.rs:163`, plus `AssignSource::Associative` if it carries the Vec).
2. **Accessors** (`shell_state.rs`) — rewrite to use `AssocMap`:
   - `get_associative` — return `Option<&AssocMap>` (or `Option<&[(String,String)]>`
     via `.pairs()`; pick whichever minimizes caller churn — callers currently do
     `.cloned()`/`.iter()`, both satisfiable by exposing `.pairs()`).
   - `lookup_associative_element` → `map.get(key)` (O(1)).
   - `set_associative_element` / `store_assoc_element` → `map.insert(...)` (O(1)).
   - `append_associative_element` → get-then-insert (concatenate) via `AssocMap`.
   - `unset_associative_element` → `map.remove(key)` (O(n), order-preserving).
   - `replace_associative` → build an `AssocMap` from the new pairs.
3. **Construction sites:** the `declare -A a=(…)` assignment path
   (`AssignSource::Associative`) and any place that builds an associative value
   construct an `AssocMap` (via `FromIterator` or `insert` loop) instead of a Vec.
4. **Direct match sites** (~15, in `shell_state.rs`, `array_transforms.rs`,
   `builtins.rs`): `VarValue::Associative(pairs)` arms that iterate — use
   `pairs.pairs()` / `pairs.iter()`. The L-44 enumeration sites
   (`expand_assoc_param`, `render_declare_value_part`, `@K`/`@k`) feed
   `assoc_order::assoc_ordered_pairs(map.pairs())` — unchanged logic, new source.
5. `assoc_order.rs` itself is UNCHANGED — it operates on `&[(String,String)]`,
   which `AssocMap::pairs()` provides.

## The load-bearing invariant

The L-44 view sorts by `(fnv1(key) & 1023, order_index)`. It stays byte-identical
iff `AssocMap`'s `order` index equals bash insertion order: **insert appends** a
new key; **update keeps position**; **remove shift-preserves** survivors. The
`AssocMap` methods maintain exactly this. This is the one subtle correctness
property; the property test + the existing `assoc_order_diff_check.sh` guard it.

## Verification

- **Behavior-preserving = the whole suite unchanged.** `cargo test -p huck-engine`
  (lib) + the assoc/declare/array integration bins + `run_diff_checks.sh`
  (esp. `assoc_order_diff_check.sh`) all green with NO expected-value changes.
  The full bash-suite runner **stays PASS 31 / FAIL 51 with zero category
  movement** — the primary correctness signal (a behavior change would move a
  diff).
- **`AssocMap` unit tests:** get/insert(update-in-place)/remove/iter; order
  preserved across update+remove; `from_iter` duplicate-key = first-position /
  last-value; empty/len.
- **Property test:** random sequences of insert/update/remove cross-checked
  against a `Vec`-based reference oracle for both key-set and ordered pairs.
- **Micro-benchmark / timed script:** demonstrate O(N²)→O(N) — e.g. time
  `for ((i=0;i<30000;i++)); do a[k$i]=$i; done` before vs after (should drop from
  seconds to well under a second).
- Confirm the full runner count is unchanged (31) — no flips, no regressions.

## Scope / non-goals

- NOT adopting bash's bucket-table as storage (the coupling the #325 discussion
  rejected). L-44 stays a decoupled view.
- No `indexmap` dependency (hand-rolled).
- `unset` stays O(n) (shift-remove); tombstoned O(1) remove is out of scope.
- No behavior change whatsoever — if any test's expected output changes, that's a
  bug in the migration, not an intended change.

## Summary of touched files

- `crates/huck-engine/src/assoc_map.rs` (new) — `AssocMap` + tests.
- `crates/huck-engine/src/lib.rs` — `mod assoc_map;`.
- `crates/huck-engine/src/shell_state.rs` — `VarValue::Associative(AssocMap)` +
  the ~6 accessors + construction/match sites.
- `crates/huck-engine/src/{array_transforms.rs, builtins.rs}` — match-site
  iteration via `.pairs()`.
- `crates/huck-engine/src/{expand.rs, arith.rs, executor.rs}` — any assoc
  construction/match site (audit).
- Possibly `crates/huck-engine/tests/*` — only if a test constructed the old
  `Vec` variant directly (update to `AssocMap`); NO expected-output changes.
