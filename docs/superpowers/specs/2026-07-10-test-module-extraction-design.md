# v278 — Extract inline `#[cfg(test)]` modules into per-module test files

**Issue:** [#104](https://github.com/jdstanhope/huck/issues/104)
**Date:** 2026-07-10
**Type:** Pure refactor (no behavior change), file decomposition — step 1

## Problem

The six largest source files carry large inline `#[cfg(test)]` modules welded
directly onto the production logic. This inflates the *code* files and forces
unrelated test bulk into context on every read or edit of the logic (and forces
the logic into context on every test edit):

| File | total | code | inline test lines |
|------|------:|-----:|------:|
| `crates/huck-engine/src/builtins.rs` | 16,107 | ~9,300 | ~6,785 |
| `crates/huck-engine/src/executor.rs` | 11,964 | ~8,900 | ~3,065 |
| `crates/huck-syntax/src/parser.rs` | 9,478 | ~5,256 | ~4,221 |
| `crates/huck-engine/src/expand.rs` | 4,657 | ~2,440 | ~2,213 |
| `crates/huck-engine/src/shell_state.rs` | 4,863 | ~3,740 | ~1,123 |
| `crates/huck-engine/src/arith.rs` | 2,469 | ~1,295 | ~1,174 |

In `builtins.rs` the tests are wedged *in the middle* of the file (lines
~8640–15420), leaving `umask`/`ulimit`/`times`/`enable` code stranded after
them.

This is the first, lowest-risk step of a larger file-decomposition effort. The
subsequent code-domain split of `builtins.rs`/`executor.rs`/`lexer.rs`/etc. is
deferred to follow-on issues; this iteration deliberately touches **only** the
placement of test modules.

## Goal

De-bloat the six code files by relocating their inline `#[cfg(test)] mod …`
blocks into file-backed child modules, with **zero behavior change and zero
change to what is tested**.

Non-goals (explicitly deferred):

- Splitting the *code* by domain (recommendation #2/#3 — separate issues).
- Touching files that are not oversized (`lexer.rs`, `command.rs`,
  `generate.rs`, and all sub-1k files) even though some carry inline tests.
- Renaming any test module, function, or symbol.

## Design

### Mechanism

Rust resolves a child module declared in `foo.rs` by looking in a sibling
`foo/` directory. For each source file, each inline test module becomes a
one-line `#[cfg(test)] mod <name>;` declaration whose body moves verbatim into
a file under the new sibling directory.

**Load-bearing invariant:** every relocated test module stays a **direct child
of its original parent module** — no intermediate module layer is introduced.
Consequently the `use super::*;` at the top of each block resolves to exactly
the same module as before, and the move is a pure cut-and-paste with **no
import edits**.

### Per-file layout

The uniform rule across all six files: **each *top-level* `#[cfg(test)] mod
<name>` block moves to its own direct-child file `<parent>/<name>.rs`, keeping
its existing name.** The block is replaced in the code file by a one-line
`#[cfg(test)] mod <name>;`. Nested submodules ride along inside their
top-level module's file. Because every relocated module stays a direct child
of its original parent, `use super::*;` resolves unchanged — no import edits.

Two files have a single top-level test module:

```
parser.rs          ← `mod tests { … }` replaced by `#[cfg(test)] mod tests;`
parser/tests.rs    ← the verbatim body
```

- `parser.rs` → `parser/tests.rs`
- `arith.rs`  → `arith/tests.rs`

Four files have several top-level test modules and therefore get one file per
module:

- `builtins.rs` — **36 modules** (`tests`, `fg_bg_tests`, `kill_tests`,
  `cd_pwd_tests`, `disown_tests`, `history_tests`, `special_builtin_tests`,
  `alias_tests`, `shift_tests`, `set_tests`, `source_tests`, `local_tests`,
  `colon_tests`, `true_false_tests`, `command_tests`, `readonly_tests`,
  `read_tests`, `printf_tests`, `exit_tests`, `type_tests`, `hash_tests`,
  `dirstack_tests`, `declare_tests`, `integer_attr_tests`, `eval_tests`,
  `help_tests`, `set_options_tests`, `array_declare_tests`,
  `assoc_declare_tests`, `loop_levels_tests`, `pipefail_option_tests`,
  `getopts_step_tests`, `umask_tests`, `ulimit_tests`, `enable_tests`,
  `normalize_logical_tests`) → `builtins/<name>.rs`.
- `executor.rs` — **9 modules** (`tests`, `array_assign_tests`,
  `assoc_assign_tests`, `coproc_name_tests`, `arith_for_tests`,
  `loop_levels_executor_tests`, `select_menu_tests`,
  `g3_dbracket_extglob_noshopt_tests`, `errexit_andor_tests`) →
  `executor/<name>.rs`.
- `expand.rs` — **5 modules** (`tests`, `array_expansion_tests`,
  `positional_slicing_tests`, `assoc_expansion_tests`, `ifs_splitter_tests`) →
  `expand/<name>.rs`.
- `shell_state.rs` — **5 modules** (`tests`, `array_value_tests`,
  `assoc_value_tests`, `ifs_helper_tests`, `shopt_tests`) →
  `shell_state/<name>.rs`.

That is **57 new test files** total. Example for `builtins`:

```
builtins.rs                 ← each test block replaced by `#[cfg(test)] mod <name>;`
builtins/
  tests.rs
  fg_bg_tests.rs
  kill_tests.rs
  …  (36 files)
```

Because each `builtins/<name>.rs` is a direct child of `builtins`,
`use super::*;` continues to resolve to the `builtins` module — no rewrites.

The tidier-looking grouped alternative (`builtins/tests/<topic>.rs` under a
`tests/` subdirectory) is **rejected**: nesting under a `tests` module would
repoint `super` at that new layer, forcing a
`use super::*;` → `use crate::builtins::*;` rewrite in every moved file. That
trades the zero-edit guarantee for cosmetics and is out of scope.

### What stays in the code file

Only `#[cfg(test)] mod …` blocks move. `#[cfg(test)]`-gated **root helper
items** that live in the code region and are consumed by the tests stay where
they are:

- `builtins.rs` — `run_declaration_builtin_strs` (fn)
- `expand.rs` — `glob_expand_fields` (fn) and the test-only `impl Field`
- `shell_state.rs` — the test-only `impl Shell`

After the test modules relocate, they still reach these via `use super::*;`
(super = the parent module) with no edits. Moving them would force re-export
churn for no size benefit, and they are `#[cfg(test)]` so they never touch
production builds. They are out of scope.

## Verification

A pure move is verified by proving the test set is unchanged. On this dev box
`--workspace` OOM-kills the session, so tests run per-crate:

- `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` (~441 tests)
- `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` (~1,773 tests)

**Acceptance oracle:** the per-crate test count (and pass/fail breakdown) must
be byte-identical before and after each file's extraction. Any delta means a
module was dropped or a `cfg`/import broke.

Additionally:

- `cargo fmt --all --check` clean (CI enforces it).
- `cargo build -p huck` green.

## Docs

`docs/architecture.md`'s builtins/executor/parser rows are functional
descriptions naming symbols — none of which move — so no API edits are needed.
Add one line noting that unit tests now live in per-module `<file>/tests.rs`
files (and `builtins/*_tests.rs`).

## Implementation shape

One task per source file, each an isolated verbatim move plus a per-crate
test-count check, ordered smallest-first with `builtins.rs` last as the
largest. Six independent, individually-verifiable steps. Detailed in the
companion plan.
