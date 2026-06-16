# v171: array→indexed noun consolidation — Design

**Status:** approved 2026-06-16
**Iteration:** v171
**Origin:** the 2026-06-16 naming review flagged the variable-mutation "verb
sprawl" as the biggest remaining inconsistency. On inspection (reading the
bodies), the verbs are **principled** — `set`/`replace`/`extend`/`append`/`unset`
are thin wrappers over the one `assign()` funnel, each a distinct `Kind×Dest`,
atop the private `store_*` backend. The genuine inconsistency is the **noun**:
indexed-array methods use both `array` and `indexed`.

## Goal

Make the indexed-array method names consistent by standardizing on `indexed`
(the adjective already used internally), dropping the ambiguous `array`. Pure
rename — no behavior change, no verb changes, no layering changes.

## Problem

In bash both kinds are "arrays"; the distinguishing adjective is *indexed* vs
*associative*. huck's internal vocabulary already uses `indexed`
(`VarValue::Indexed`, `AssignSource::Indexed`, the private `store_indexed_*`,
`extend_indexed`, `set_indexed_var`). But six **public** methods name the
indexed case `array`, so the indexed/associative method pairs are asymmetric:

| indexed (says `array`) | associative (consistent) |
|---|---|
| `replace_array` | `replace_associative` |
| `set_array_element` | `set_associative_element` |
| `append_array_element` | `append_associative_element` |
| `unset_array_element` | `unset_associative_element` |
| `get_array` | `get_associative` |
| `lookup_array_element` | `lookup_associative_element` |

"array" meaning *only the indexed kind* is ambiguous and out of step with the
rest of the codebase.

## Design

Rename the six `*_array*` methods to `*_indexed*`:

| current | new |
|---|---|
| `replace_array` | `replace_indexed` |
| `set_array_element` | `set_indexed_element` |
| `append_array_element` | `append_indexed_element` |
| `unset_array_element` | `unset_indexed_element` |
| `get_array` | `get_indexed` |
| `lookup_array_element` | `lookup_indexed_element` |

After this every indexed/associative pair is symmetric (`replace_indexed` ↔
`replace_associative`, `get_indexed` ↔ `get_associative`, etc.) and the public
API matches the internal `Indexed` vocabulary.

**Untouched (verified principled):** every verb; the `assign()` funnel and its
private `store_*` backend; `extend_indexed` and `set_indexed_var` (already
`indexed`); `set`/`unset`/`try_set`/`try_unset` (canonical scalar ops, huge
traffic, bash-natural); the `*_associative_*` methods (already consistent).

## Safety (renames are compiler-verified; two specific guards)

1. **Substring collision:** `set_array_element` is a substring of
   `unset_array_element`. A `replace_all("set_array_element", …)` would also
   rewrite the `unset_array_element` occurrence. **Mitigation:** rename
   `unset_array_element` → `unset_indexed_element` **first** (full-name
   `replace_all`), then `set_array_element` → `set_indexed_element` (the
   `unset_…` form no longer contains the old `set_array_element` substring). All
   other five names are unique full identifiers.
2. **No target collision:** none of `replace_indexed` / `set_indexed_element` /
   `append_indexed_element` / `unset_indexed_element` / `get_indexed` /
   `lookup_indexed_element` already exists (verified).
3. **Never touch** `set`/`unset`/`assign`/`get_associative`/dispatch strings —
   these are different identifiers and are not substrings affected by the
   six full-name replacements.

Each name is replaced per-file (the methods are referenced across
`shell_state.rs`, `executor.rs`, `builtins.rs`, `expand.rs`,
`param_expansion.rs`, `arith.rs`, `completion_spec.rs`, `shell.rs`). The Rust
compiler then verifies every reference; behavior is verified unchanged by the
full suite + all 93 bash-diff harnesses.

## Verification

- `cargo build` (every code reference) after the renames; `cargo clippy --lib
  --bins` clean; `cargo test` 0 failures; all 93 harnesses pass.
- No new tests, no new harness, no `docs/bash-divergences.md` change (pure
  refactor).
- Update the `docs/architecture.md` "Naming conventions" section (added v170) to
  state that indexed/associative arrays use `indexed`/`associative` as the
  type adjective (not `array`).

## Scope boundary

Only the six `array→indexed` renames + the one-line doc note. **Deferred** (as
before): nothing else in the mutation area needs changing — the verbs and
layering are principled. The cosmetic nits from the naming review
(`operand`/`body`, traps `fire_`/`dispatch_`, `is_name_cont`,
`format_spec_for_print`) remain out of scope. No behavior change anywhere.
