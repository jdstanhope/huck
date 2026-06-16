# v170: naming consolidation (low-risk batch) — Design

**Status:** approved 2026-06-16
**Iteration:** v170
**Origin:** a cross-codebase naming-consistency review (2026-06-16) found that
the codebase is mostly well-named, with a few clusters where different verbs name
the same concept. This iteration does the **low-risk** subset: codify the
existing-good conventions in docs, and apply the renames whose call sites are all
internal and compiler-verifiable. The higher-risk variable-mutation verb
consolidation and the cosmetic nits are deferred.

## Goal

Consolidate a handful of inconsistent function names onto the project's de-facto
conventions, and document those conventions so they stop being implicit. Pure
refactor — no behavior change.

## Why this is safe (and how it's verified)

Renaming Rust identifiers is build-verified: the compiler flags every *missed*
code reference (a build error, not a silent bug). The residual risks the
compiler cannot see, and how each is handled here:

- **String-dispatch names** (huck dispatches builtins by string in
  `run_builtin` / `BUILTIN_NAMES`): none of the renamed symbols here are builtin
  functions or appear in string literals (verified by grep). Builtin dispatch
  strings are NOT touched.
- **Substring collisions** done consistently (the one real trap): `resolve_spec`
  is a substring of the unrelated `resolve_spec_or_error` (job-spec, builtins.rs).
  All renames are applied with word-boundary scoping (`\bNAME\b` / per-site
  edits), never a blanket text replace.
- **Stale non-code references** (docs/comments): updated as part of the change;
  doc-comments that already say "Scans" become consistent with the new `scan_*`
  names.

(rust-analyzer's semantic rename would be the ideal tool but its LSP server is
crashing in this environment; the compiler + word-boundary grep + the full
bash-diff suite are a sufficient substitute for Rust.)

## The changes

### 1. Convention doc (`docs/architecture.md`)

Add a short "Naming conventions" subsection codifying the conventions the
codebase already mostly follows, so future code is consistent:

- **Retrieval:** `get_*` = borrow a stored container (returns `&T`); `lookup_*` =
  compute one resolved value (returns owned `Option<String>`); `resolve_*` =
  follow indirection to a concrete path/target (namerefs, paths).
- **Lexing/scanning:** `scan_*` = advance a `CharCursor` and collect a span;
  `split_*` = partition an already-collected `&str`; `parse_*` = produce
  AST/structure from tokens; `tokenize` = source → tokens.
- **Execution:** `run_*` = execute an AST node/construct; `execute*` = the public
  crate entry points; `eval_*` = compute a value from an expression.

### 2. `read_*` → `scan_*` (lexer.rs — all 8 cursor-advancing collectors)

These all take `&mut CharCursor` and collect a span — `scan_*` is already the
plurality verb for that, and two of them have doc-comments that literally say
"Scans". Rename (collision-checked — no `scan_<name>` target exists):

| current | new |
|---|---|
| `read_dollar_expansion` | `scan_dollar_expansion` |
| `read_ansi_c_quoted` | `scan_ansi_c_quoted` |
| `read_var_name` | `scan_var_name` |
| `read_braced_param_expansion` | `scan_braced_param_expansion` |
| `read_subscript` | `scan_subscript` |
| `read_array_literal` | `scan_array_literal` |
| `read_array_element_word` | `scan_array_element_word` |
| `read_braced_name` | `scan_braced_name` |

All references are within `lexer.rs`. (`consume_…_verbatim` thin wrappers keep
their distinct, meaningful suffix and are NOT renamed.)

### 3. `resolve_spec` → `run_spec` (completion_spec.rs)

The compspec evaluator. `run_spec_with_empty_fallback` already exists, so
`run_spec` is the established root; this frees `resolve_*` to mean only "follow
indirection." **Word-boundary scoped** so the unrelated `resolve_spec_or_error`
(builtins.rs, job-spec resolution) is untouched.

### 4. `enumerate_action` → `complete_action` (completion_spec.rs)

Joins the `complete_*` candidate-producer family (`complete_command`,
`complete_file`, `complete_variable`), so one verb means "produce completion
candidates."

### 5. `eval_arith_word` disambiguation (param_expansion.rs)

`expand::eval_arith_word` (the `$(())` body evaluator) and
`param_expansion::eval_arith_word` (the `${var:off:len}` offset/length evaluator,
which prints `huck: arithmetic:` and sets `$?` on error) are **different
operations** that happen to share a name. Rename the `param_expansion.rs` one →
**`eval_substring_index`** to reflect its distinct role; leave
`expand::eval_arith_word` as-is. (This is a disambiguation, not a dedup — verified
they differ in expansion function, empty-handling, and error behavior.)

## Verification

- Per rename: `grep -nE '\bNAME\b' src/` to confirm the reference set and rule out
  string/substring collisions, then edit those sites.
- `cargo build` (verifies every code reference) and `cargo test --lib` /
  `cargo test` (unit + integration) pass with **0 failures**; `cargo clippy
  --lib --bins` clean.
- All **93** bash-diff harnesses pass (behavior unchanged — these renames touch
  no behavior).
- No new tests, no new harness, no `docs/bash-divergences.md` change (pure
  refactor, no divergence resolved or introduced).

## Scope boundary

In scope: the convention doc + the four rename groups above. **Deferred** (not in
this iteration): the variable-mutation verb consolidation
(`set_`/`assign_`/`store_`/`extend_`/`append_`/`install_` sprawl — many call
sites, its own careful pass), and the cosmetic nits from the review
(`operand`/`body` noun drift, `is_name_cont` vs `is_user_name_continue`, traps
`fire_`/`dispatch_`, `format_spec_for_print`, accumulator-local names). No
behavior changes anywhere.
