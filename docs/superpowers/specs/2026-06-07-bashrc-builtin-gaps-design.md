# huck v107 — three `~/.bashrc` gaps: `[[ -o ]]` / `declare -g` / `unset -f` Design

**Status:** approved design, ready for implementation plan.
**Implements:** three small, independent fixes surfaced by sourcing the user's
`~/.bashrc` after v106:
1. **`[[ -o optname ]]`** — the `[[ ]]` unary "is a `set -o` option enabled?" test
   (new **M-103** `[fixed v107]`). git-sh-prompt line 406 `[[ -o PROMPT_SUBST ]]`
   currently fails `unterminated '[[ ]]'`, which cascades: the whole `__git_ps1`
   function fails to define, so its body's `local` statements run at top level
   (the `local: can only be used in a function` flood).
2. **`declare -g`** — force global scope (updates **M-79**, flip its deferred
   `-g` to `[fixed v107]`). `declare -g X=1` currently errors "not yet implemented".
3. **`unset -f` / `unset -v`** — remove a function / variable (flip **M-49**
   `[deferred]` → `[fixed v107]`). `unset -f NAME` currently errors
   "`-f`: not a valid identifier".
**Branch (impl):** `v107-bashrc-gaps`.

## Gap 1 — `[[ -o optname ]]` (M-103)

bash: `[[ -o NAME ]]` is true iff the `set -o` option `NAME` is currently enabled
(`emacs`/`vi`/`pipefail`/`errexit`/`posix`/…). Unknown or off options → false (no
error). This is the `set -o` namespace, NOT `shopt`.

- **Parser (`src/command.rs`)**: add a `OptEnabled` variant to the `TestUnaryOp`
  enum (~`:387`); add `"-o" => Some(TestUnaryOp::OptEnabled)` to the `try_unary_op`
  literal table (~`:1987`). `parse_test_atom` (~`:2120`) then reads the operand
  (option name) automatically via `next_test_word` — no further parser change. The
  current "unterminated" failure is exactly because `try_unary_op` returns `None`
  for `-o`, so `-o` becomes a bare LHS word and the following name has no operator.
- **Evaluator (`src/executor.rs`)**: in `eval_test_expr`'s Unary arm (~`:1210`),
  special-case `OptEnabled` like `VarSet` is special-cased (it needs `shell`):
  `return Ok(option_get(shell, &operand).unwrap_or(false))`. `eval_unary` (the pure
  `(op,&str)->bool` at ~`:1246`) gets an `unreachable!()` arm for `OptEnabled`
  (mirroring `VarSet` at ~`:1260`).
- **Option query (`src/builtins.rs`)**: `option_get(shell, name) -> Option<bool>`
  (~`:3948`) is the helper — it returns the live value for implemented options
  (errexit/nounset/pipefail/verbose/xtrace) and the `SETO_TABLE` default for other
  recognized names, `None` for unknown. `option_get(shell,name).unwrap_or(false)`
  gives bash-faithful false for unknown/off. **Make `option_get` `pub(crate)`** so
  `src/executor.rs` can call it (its only shared seam).
- Composes with the existing `[[ ]]` parser (multi-line v87, `&&`/`||`, `!`, `( )`
  grouping) and `test`/`[` is untouched (`[ -o opt ]` is the POSIX binary `-o`
  combinator — leave it; only `[[ ]]` gets the unary `-o`).

## Gap 2 — `declare -g` (M-79 update)

bash: `-g` creates/modifies the variable at GLOBAL scope even inside a function;
at top level it's a no-op flag. huck has no per-scope var maps — all vars live in
`shell.vars`; "locality" is implemented by snapshotting a var's pre-state into the
current function frame (`shell.local_scopes.last()`) via `snapshot_for_local_scope`
(`src/builtins.rs:~815`), restored on function exit.

- **Accept `-g`** in BOTH flag-parsing loops: `builtin_declare` (~`:926`, the
  string-arg path) and `builtin_declare_decl` (~`:1520`, the DeclArg path) — both
  currently group `g` into the rejected set. Add a `global: bool` flag.
- **Effect**: for each name, when `global` is set, (a) SKIP the
  `snapshot_for_local_scope(shell, name)` call (~`:986`) so the write to
  `shell.vars` is NOT rolled back on function exit, and (b) remove any pre-existing
  snapshot entry for `name` from `shell.local_scopes.last_mut()` (so an outer
  `local`/`declare`'s snapshot doesn't restore-over the global write on exit). At
  top level (`local_scopes` empty) `-g` is automatically a no-op
  (`snapshot_for_local_scope` early-returns). `-g` composes with `-x`/`-i`/`-r`/etc.

## Gap 3 — `unset -f` / `unset -v` (M-49)

bash: `unset -f NAME` removes a function; `unset -v NAME` removes a variable; bare
`unset NAME` removes a variable. Flags precede names and apply to all following
names.

- **`builtin_unset`** (`src/builtins.rs:~525`) has no flag parsing — it loops every
  arg as a name, so `-f` hits the identifier check (~`:566`) and errors.
- Add a **leading-flag scan**: a leading `-f` selects the function namespace, `-v`
  the variable namespace (default). For `-f`: `shell.functions.remove(name)` (the
  map is public, `src/shell_state.rs:~240`; identifier validity still applies, no
  readonly/subscript logic). For `-v`/no-flag: the existing variable path
  (`shell.unset` / array-element / readonly guard) unchanged.
- Keep bare `unset NAME` as variable-only (huck's current behavior; bash's
  fallback-to-function-when-no-variable is a separate nicety — out of scope, note
  if it matters).
- `unset` is a POSIX special builtin; `-f`/`-v` are standard. `--` may end flags
  (accept it).

## Files & responsibilities

| File | Change |
|------|--------|
| `src/command.rs` | `TestUnaryOp::OptEnabled` + `"-o"` in `try_unary_op` (Gap 1) |
| `src/executor.rs` | `eval_test_expr` Unary `OptEnabled` arm → `option_get(...).unwrap_or(false)`; `eval_unary` unreachable arm (Gap 1) |
| `src/builtins.rs` | `option_get` → `pub(crate)` (Gap 1); accept `-g` in both declare flag loops + suppress/clear the local snapshot (Gap 2); `-f`/`-v` flag scan in `builtin_unset` (Gap 3) |
| `tests/bashrc_builtin_gaps_integration.rs` | NEW — all three |
| `tests/scripts/bashrc_builtin_gaps_diff_check.sh` | NEW — 32nd bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-103 `[fixed v107]`; M-79 `-g` + M-49 flipped; changelog; README row |

## Testing

1. **Gap 1**: `[[ -o emacs ]] && echo on || echo off` → `off` (emacs off by
   default, both shells); `set -o pipefail; [[ -o pipefail ]] && echo on` → `on`;
   `[[ -o errexit ]]` reflects `set -e`; an unknown option `[[ -o bogus ]]` → false
   (no error); the git-sh-prompt shape `[ -z "${ZSH_VERSION-}" ] || [[ -o PROMPT_SUBST ]] || echo fallback`;
   composition `[[ -o pipefail && -n x ]]`; multi-line `[[ -o\n pipefail ]]` (v87).
   Negation `[[ ! -o pipefail ]]`.
2. **Gap 2**: inside a function, `f() { declare -g G=1; }; f; echo "$G"` → `1`
   (G survives function exit); without `-g`, `f() { declare L=1; }; f; echo "[$L]"`
   → `[]` (unchanged); `-g` composes: `f() { declare -gx E=1; }; f; echo "$E"`;
   top-level `declare -g X=2; echo "$X"` → `2`.
3. **Gap 3**: `f() { :; }; unset -f f; type f 2>&1` → not found; `v=1; unset -v v; echo "[${v-}]"` → `[]`;
   `unset -f NONEXISTENT` → rc 0 (bash: unset of a missing function/var is success);
   bare `unset x` still removes the variable; `-f` does NOT remove a same-named
   variable and vice-versa.
4. **bash-diff harness** `tests/scripts/bashrc_builtin_gaps_diff_check.sh` (32nd):
   deterministic fragments for all three, byte-identical to bash 5.2.
5. **Regression**: full suite (2708+), all 31 harnesses; verify the cascade
   payoff — sourcing `/usr/lib/git-core/git-sh-prompt` no longer errors at line 346
   / floods `local: can only be used in a function` (report the next gap).

## Edge cases & notes

- **`[ -o opt ]`** (single-bracket) keeps the POSIX binary `-o` (logical OR) — only
  `[[ ]]` gains the unary `-o`. Do not touch `test`/`[`.
- **`declare -g` exact bash semantics**: at function scope `-g` writes global; it
  does not also create a local — matches the snapshot-suppression approach.
- **`unset` missing name**: bash returns success (rc 0) for `unset` of a
  non-existent var/function; preserve that.
