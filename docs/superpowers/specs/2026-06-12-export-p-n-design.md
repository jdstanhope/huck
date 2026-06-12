# huck v145 — `export -p` / `export -n` Design

**Status:** approved design, ready for implementation plan.
**Implements:** M-48 — `export -p` (list exports in bash `declare -x` format) and
`export -n` (remove the export attribute). Also fixes the divergent bare-`export`
listing format.
**Branch (impl):** `v145-export-p-n`.

## Background — current behavior & the gap

`export` is a declaration command, so `export …` is dispatched (with `decl_args`)
to `builtin_export_decl` (src/builtins.rs:1175); the sibling `builtin_export`
(src/builtins.rs:482) handles `export` reached WITHOUT the declaration machinery
(`builtin export`, `command export`). Today BOTH:
- print bare `export` (no args) as `export NAME=value` — bash prints
  `declare -x NAME="value"`.
- parse `-a -p -n -f` and **ignore** them (builtins.rs:1208). So `export -p`
  lists nothing, and `export -n FOO` wrongly **exports** FOO.

bash reference (verified):
- bare `export` / `export -p` (no operands) → `declare -x NAME="value"` per
  exported var, sorted. Exported+readonly → `declare -rx NAME="value"`.
- `export -p NAME` (operands present) → `-p` is IGNORED; NAME is an export TARGET
  (`export -p X` exports X). No per-name listing.
- `export -n NAME` → removes the export attribute, KEEPS the value
  (`declare -- NAME="…"`). `export -n NAME=val` → assign THEN unexport. Works on a
  readonly var (`export R=1; readonly R; export -n R` → `declare -r R="1"`). Unset
  name → no-op. `export -pn A` → `-n` wins, A unexported.
- invalid flag (`export -z`) → `export: -z: invalid option` + usage, rc 2.

## Architecture — shared core, both entry points

`shell.unexport(name)` (src/shell_state.rs:772) already exists. `iter_vars()`
(:780) yields `(&String, &Variable)`. `format_declare_line(name, var)`
(src/builtins.rs:813) already renders bash's `declare -x NAME="value"` form (attr
order `airx`, value via `escape_double_quote_value`). Reuse all three.

Add a small shared module used by BOTH `builtin_export` and `builtin_export_decl`:

1. **`list_exported(out, shell)`** — collect `iter_vars().filter(|(_, v)| v.exported)`,
   sort by name, emit `format_declare_line(name, var)` per line. rc 0.

2. **Flag parsing** — scan leading flags (`DeclArg::Plain` / `String` starting with
   `-`, len > 1) into a small struct `{ list: bool, unexport: bool, func: bool }`:
   - `-p` → `list = true`; `-n` → `unexport = true`; `-a` → accepted no-op (huck
     keeps this for `mise activate bash`'s `export -a chpwd_functions`); `-f` →
     `func = true` (DEFERRED — see below); combined (`-pn`) → each char; `--` →
     stop. An unrecognized flag char → `eprintln!("huck: export: {arg}: invalid
     option")` + a usage line + `return Continue(2)`.

3. **Dispatch after flags:**
   - **No operands:** if `unexport` → no-op rc 0 (nothing to unexport); else
     `list_exported` (covers bare `export`, `export -p`, `export -a`).
   - **Operands present:**
     - `func` (`-f`) → DEFERRED: huck does not export functions; do NOT treat the
       names as variables (avoid creating empty exported vars). No-op the operands,
       rc 0. (Documented divergence.)
     - `unexport` (`-n`) → per operand: `NAME=val` → `export_set`-then-`unexport`
       (a readonly target still errors on the assignment); bare `NAME` →
       `unexport(name)` (no-op if unset/already-unexported). Invalid identifier →
       error + rc 1 (per-operand, like the existing path).
     - else → existing export behavior (export / export_set, readonly check).

Both `builtin_export` (`&[String]`) and `builtin_export_decl` (`&[DeclArg]`) parse
their own flags/operands (the arg types differ) but call the SAME `list_exported`
and the same `-n` unexport handling, so `export -p` and `builtin export -p`
behave identically. (DRY: factor the shared listing + unexport-one-operand logic;
the thin per-entry flag scan stays local since the arg representations differ.)

## Behaviour matrix (target = bash)

| input | result |
|---|---|
| `export FOO=bar; export` | `declare -x FOO="bar"` (+ other exports, sorted) |
| `export FOO=bar; export -p` | same as bare `export` |
| `export R=1; readonly R; export -p` | `declare -rx R="1"` |
| `export -p FOO` (operand) | exports FOO (no listing) — matches bash |
| `X=hi; export X; export -n X; declare -p X` | `declare -- X="hi"` |
| `export Q=1; export -n Q=2; declare -p Q` | `declare -- Q="2"` |
| `export R=1; readonly R; export -n R; declare -p R` | `declare -r R="1"` |
| `export -n NOPE; echo $?` | `0` (no-op) |
| `export -pn A` (A exported) | A unexported |
| `export -z` | `huck: export: -z: invalid option` + usage, rc 2 |
| `export -a chpwd_functions` | no-op accept (mise), rc 0 |

## Scope & deferred

- **`-f` (function export) DEFERRED.** huck stores functions as parsed AST and has
  NO AST→source serializer (even `declare -f NAME` is a stub printing
  `declare -f NAME`, not the body). `export -f` requires serializing each function
  body to `BASH_FUNC_name%%=() { … }` env encoding + injecting it when spawning
  children + importing `BASH_FUNC_*` at startup. v145 makes `export -f NAME` a
  rc-0 no-op (NOT exporting an empty variable — the current latent bug). Logged as
  a new deferred divergence; the agreed path is a future iteration that **generates
  the function body from the AST in a NORMALIZED format** (a real un-parser, which
  also yields a proper `declare -f`), then `export -f` rides on it. (We will NOT
  capture raw source.)
- Arrays: `export -p` lists exported arrays via `format_declare_line` (which
  already renders indexed/assoc) — comes for free.

## Documented divergences
- **DELETE M-48** (`export -p`/`-n`) from Tier-2 (resolved).
- **ADD a new Tier-2 `[deferred]` entry** `M-48f` (or next free M-number): `export -f`
  (export shell functions) is unsupported — blocked on a function-AST→normalized-source
  serializer (the same prerequisite as a real `declare -f` body); `export -f NAME` is
  a rc-0 no-op in huck. The agreed follow-on is a future iteration that generates the
  function body from the AST in a normalized format (a real un-parser, also fixing
  `declare -f`), after which `export -f` rides on it.
- NET Tier-2 count: 19 → 19 (M-48 removed, `export -f` added). No Tier-4 change.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/builtins.rs` | `builtin_export_decl` (1175) + `builtin_export` (482): real `-p`/`-n` handling via a shared `list_exported` + shared `-n` unexport-one logic; bare/`-p` listing switches to `format_declare_line`; invalid flag → rc 2 + usage; `-f` → rc-0 no-op for operands. |
| `tests/scripts/export_p_n_diff_check.sh` (NEW, 65th) | Bash-diff over the behaviour matrix. |
| `docs/bash-divergences.md` | DELETE M-48; ADD the `export -f` deferred Tier-2 entry (net Tier-2 count unchanged at 19). |

## Testing

1. **Unit tests** (`src/builtins.rs` mod tests): bare `export` / `export -p` list in
   `declare -x` form (sorted, incl. a `declare -rx` for exported+readonly);
   `export -n NAME` clears the export attribute (assert `!is_exported` + value kept);
   `export -n NAME=val` assigns then unexports; `export -n` of an unset name → rc 0
   no-op; invalid flag → rc 2; `export -p FOO` exports FOO (operand, no listing);
   `export -f foo` does NOT create/export a variable `foo` (rc 0 no-op).
2. **Bash-diff harness** `tests/scripts/export_p_n_diff_check.sh` (65th) — the full
   matrix, byte-identical to bash. Use a FIXED set of vars (`env -i` style isolation
   or grep the specific names) so the ambient environment doesn't make the listing
   non-deterministic — each check should `export`/`unset` its own vars and assert via
   `declare -p NAME` / a `grep`-filtered `export -p`.
3. **Both entry points:** a test that `builtin export -p` (the `builtin_export`
   String path, via v142) lists identically to `export -p` (the decl path).
4. **Full regression:** entire suite + ALL harnesses green; ESPECIALLY existing
   `export`/`declare`/`mise`-related tests (the `export -a chpwd_functions` no-op
   must still work). `clippy` clean.

## Edge cases & notes
- Listing must be deterministic: sort exported names; the harness greps specific
  names to avoid ambient-env noise.
- `export -n` on a readonly: the bare-name unexport succeeds (attribute change,
  value protected); only a `NAME=val` assignment to a readonly errors.
- `export -p`/bare `export` value quoting comes from `format_declare_line` →
  `escape_double_quote_value` (already matches bash's `"a b\"c"` form).
- **Git safety:** implementer subagents must NOT `git checkout <sha>`; the
  controller verifies the branch tip before merging. Commit trailer:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
