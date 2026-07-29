# v342 — bash-faithful associative-array iteration order (L-44) + `declare -c`

**Issues:** [#32 — associative-array iteration order](https://github.com/jdstanhope/huck/issues/32) (L-44, the keystone)
and [#321 — `declare -c` (capitalize-first attribute)](https://github.com/jdstanhope/huck/issues/321). Target flip: **`casemod`**.

**Goal:** implement bash 5.2.21's exact associative-array iteration order so huck's
assoc output/expansion matches bash byte-for-byte, and add the `declare -c`
attribute — together flipping the `casemod` bash-suite category to 0-diff PASS
(runner PASS 30 → 31). L-44 also shrinks `assoc`/`appendop` and unblocks them for
follow-ups (it does NOT flip them alone — they have other roots).

## Background & feasibility (validated)

bash iterates associative arrays in hash-table order, not insertion order. huck
stores them insertion-ordered (`VarValue::Associative(Vec<(String,String)>)`), so
every enumeration diverges (L-44). I reverse-engineered and validated bash
5.2.21's exact order (200 + 200 randomized tests, up to 120 keys, incl.
updates/deletes/collisions — **0 mismatches**):

- **Hash:** 32-bit FNV-1 — `i = 2166136261; for each byte b: i = i.wrapping_mul(16777619); i ^= b`. (Verified byte-for-byte against bash's compiled `hash_string`.)
- **Bucket:** `fnv1(key) & 1023` — the table has **1024 buckets**. (Not 128 — `DEFAULT_HASH_BUCKETS` is 128 but assoc arrays behave as 1024; empirically confirmed exhaustively.)
- **Order:** buckets ascending (0→1023); within a bucket, **newest-inserted-first** (bash head-inserts into the chain; its `assoc_to_word_list_internal` flattener nets to bucket-ascending, chain head→tail = newest→oldest).
- **No growth to model:** bash grows only at `nentries ≥ nbuckets*2 = 2048`, so any array < ~2000 keys stays at 1024 — no rehash/chain-reversal. (Arrays ≥ 2048 keys would diverge; exotic, documented as a known limit.)
- Updating an existing key keeps its position; `unset` removes it from its chain. Both validated.

Because huck's storage already gives the keys **and** their insertion order (Vec
index; new keys append, updates keep position — confirmed at
`shell_state.rs:3017`), the bash order is a **pure iteration-order view** computed
at enumeration time — no storage change.

## Root 1 — associative iteration order (L-44, #32)

### The order function
Add (e.g. a new `crates/huck-engine/src/assoc_order.rs`, or in `shell_state.rs`):

```rust
/// bash 5.2.21 hash_string: 32-bit FNV-1 over the key's bytes.
pub(crate) fn assoc_hash(key: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in key.bytes() {
        h = h.wrapping_mul(16777619);
        h ^= b as u32;
    }
    h
}

/// Indices into `pairs` in bash's associative-array iteration order:
/// bucket = hash & 1023 ascending; within a bucket, newest-inserted first
/// (descending insertion index). `pairs` is in insertion order (Vec index).
pub(crate) fn assoc_bash_order(pairs: &[(String, String)]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..pairs.len()).collect();
    // Stable sort by (bucket asc, index desc). Sort key: (bucket, usize::MAX - i).
    idx.sort_by_key(|&i| (assoc_hash(&pairs[i].0) & 1023, usize::MAX - i));
    idx
}
```

(`ASSOC_NBUCKETS = 1024` as a named const. A follow-up could model growth for
≥2048-key arrays; out of scope.)

### Routing — every associative ENUMERATION site
huck's `Vec<(String,String)>` storage stays insertion-ordered; only sites that
LIST all keys/values switch to `assoc_bash_order`. Sites (from #32 + code audit):

- `${a[@]}` / `${a[*]}` (values) and `${!a[@]}` / `${!a[*]}` (keys) — `expand.rs::expand_assoc_param`.
- `${a[@]<op>}` per-element transforms and `${a[@]:off:len}` slicing — `expand.rs`.
- `${a[@]@A}` / `@K` / `@k` — `array_transforms.rs` (+ its `expand.rs` routing).
- `declare -p a`, bare `declare`, `declare -A` listing (the `declare -A x=([k]=v …)` render) — `shell_state.rs` / `builtins.rs`.
- `for k in "${!a[@]}"` — uses `${!a[@]}`, covered by the keys path.

Each of these currently iterates `pairs` in Vec order; change to iterate
`assoc_bash_order(pairs)` instead. Indexed arrays (ordered by numeric index) are
untouched. Lookups by key, membership, assignment, and `${#a[@]}` (count) are
order-independent and unchanged.

### Insertion-order preconditions (verify, don't assume)
- Compound `declare -A a=([k1]=v1 [k2]=v2 …)` must populate the Vec left-to-right (matches bash's left-to-right `hash_insert`), so within-bucket collision order matches. Verify huck does this.
- A re-assigned existing key keeps its Vec position (huck: yes, `shell_state.rs:3017`); a new key appends. `unset a[k]` removes the pair. These make Vec-index == bash insertion index.

## Root 2 — `declare -c` (capitalize-first attribute)

### Symptom (casemod)
`casemod.tests` uses `declare -c qux; qux=$TEXT`. bash's `-c` capitalizes the
first letter of the assigned value (`declare -c x="hello world"` → `Hello world`).
huck rejects `-c` (`declare: -c: invalid option`).

### Fix
Mirror the existing `-u`/`-l` case-fold attribute machinery:
- Extend `CaseFold` (`shell_state.rs:~3420`) with a `Capitalize` variant; add the
  arm to `apply_case_fold` (uppercase the first char, rest unchanged — reuse the
  same logic as `${var@u}`/UpperFirst).
- Accept `-c` (and `+c`) in the `declare`/`typeset`/`local`/`export` flag parser
  wherever `-u`/`-l` are handled, setting `CaseFold::Capitalize`.
- `-c` is applied on assignment exactly like `-u`/`-l`, and shown in `declare -p`
  as `declare -c` (mirroring `-u`/`-l` rendering).

`-u`/`-l`/`-c` are mutually the same slot (last one wins, as in bash). Verify
`declare -p` renders the `-c` flag (casemod doesn't `declare -p` the `-c` var, but
correctness matters).

## Verification

- New `tests/scripts/assoc_order_diff_check.sh`: assoc enumeration in bash order
  across `${a[@]}`, `${!a[@]}`, `${a[@]^^}`, `${a[@]@A}`, `declare -p`, `for`-in —
  with key sets exercising distinct buckets AND within-bucket collisions, plus
  updates and `unset`. Byte-identical vs bash.
- A `declare_c_diff_check.sh` (or extend `casemod`/`declare` harness) for `-c`.
- **Property test** (Rust unit): a randomized `assoc_bash_order` check mirroring
  the 200-case validation (fixed seed) so the model can't silently drift.
- Official runner: `casemod` → 0-diff PASS. Measure `assoc`/`appendop`/`quotearray`
  diff shrinkage (record; not expected to flip — other roots remain).
- **No-regression** vs an origin/main baseline: the assoc-order change is global —
  confirm no currently-PASS category regresses (any PASS category that enumerates
  an assoc array), and that indexed-array categories are byte-identical.
- Full `run_diff_checks.sh` sweep; `associative_arrays`/`declare`/`array*`/`param*`
  integration bins. Update any huck-internal test that asserts the OLD
  insertion-order assoc output (expected — grep for them).
- Full runner PASS 30 → 31 (casemod), no other regressions.

## Scope / non-goals

- L-44 does NOT flip `assoc` (BASH_ALIASES/BASH_CMDS, L-46 bare-attr, error
  wording, integer-assoc), `appendop` (integer-array arith, counts, numeric assoc
  key), or `quotearray` (arith-subscript-with-special-key parsing). Those stay
  FAIL with shrunk diffs; follow-up issues capture their residuals.
- Assoc growth/rehash for ≥2048-key arrays — deferred (documented limit).
- This reproduces bash 5.2.21's specific hash/table; acceptable (huck's compat
  target).

## Summary of touched files

- `crates/huck-engine/src/assoc_order.rs` (new) — `assoc_hash` + `assoc_bash_order` + const.
- `crates/huck-engine/src/expand.rs` — route assoc value/key/transform/slice enumeration.
- `crates/huck-engine/src/array_transforms.rs` — route `@A`/`@K`/`@k`.
- `crates/huck-engine/src/shell_state.rs` / `builtins.rs` — route `declare -p`/bare-declare assoc render; `CaseFold::Capitalize` + `-c` flag.
- `tests/scripts/assoc_order_diff_check.sh` (+ `-c`) — harnesses.
- `docs/bash-test-suite-baseline.md`, memory.
