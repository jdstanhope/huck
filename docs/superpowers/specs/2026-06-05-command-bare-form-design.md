# huck v99 — `command CMD` bare form Design

**Status:** approved design, ready for implementation plan.
**Implements:** the bare form `command CMD [args]` — run CMD suppressing shell
**function** (and alias) lookup, finding only builtins and `$PATH` commands.
Today huck errors `command: bare form (without -v/-V) is not supported in this
version`; only `command -v`/`-V` introspection (M-70, v53) works.
**Primary driver:** `~/.nvm/nvm.sh` uses `command sort` / `command sed` /
`command mkdir` / `command printf` etc. **167 times** — the dominant remaining
nvm blocker.
**Closes:** **M-85** (`[deferred]` → `[fixed v99]`).
**Branch (impl):** `v99-command-bare-form`.

## Verified bash 5.2 semantics (the contract)

`command CMD [args]` runs CMD as a normal command but **suppresses shell-function
lookup**; only a builtin named CMD or a CMD found in `$PATH` runs.
- `ls() { echo FUNC; }; command ls` → runs the real `/bin/ls` (NOT `FUNC`).
- `command echo hi` → the `echo` **builtin** runs (`command` finds builtins).
- `command sort` → external `/usr/bin/sort`.
- `FOO=bar command env` → inline assignment applies (env shows FOO=bar).
- `command command ls` → still runs `ls` (double `command` collapses).
- `command nonexistent` → `command not found`, rc 127 (same as a normal miss).
- `-p`: uses a default PATH (bash's `getconf PATH`). v99 accepts `-p` and resolves
  via the **current `$PATH`** (default-PATH value is a low-impact sub-divergence).
- `-v` / `-V`: introspection — UNCHANGED (already implemented; keep).
- `command` with no operand → rc 0 (unchanged).

## Section 1 — Executor interception (`src/executor.rs`)

The simple-command dispatch lives in `run_exec_single`: `let resolved =
resolve(cmd, shell)?` then a chain — control-builtin → **function**
(`shell.functions.get`) → builtin (`is_builtin`) → PATH-exec. Intercept
`command` here, BEFORE the chain:

1. Change `let resolved` to `let mut resolved` (or rebind).
2. If `resolved.program == "command"`, scan the leading `resolved.args` as flags:
   - `-v` / `-V` anywhere in the leading flags → **do NOT intercept**; let dispatch
     proceed so the `command` builtin runs `builtin_command` (introspection,
     unchanged).
   - `-p` → accept (set `used_p = true`; affects nothing beyond using `$PATH` in
     v99) and continue scanning.
   - `--` → stop flag scanning.
   - `-x` (other unknown `-…`) → `eprintln!("huck: command: {x}: invalid option")`,
     return rc 2 (matches `builtin_command`).
   - first non-flag word = the inner CMD.
   - **Bare form**: if an inner CMD exists, rewrite `resolved.program = CMD`,
     `resolved.args = <words after CMD>`, set `bypass_functions = true`. If the
     new `resolved.program` is again `"command"`, repeat the scan (collapse
     `command command …`). If NO inner CMD remains (`command`, `command -p`,
     `command --`) → return rc 0.
3. Gate the function-lookup arm — `} else if !bypass_functions && let Some(body) =
   shell.functions.get(&resolved.program).cloned() {` — so a function named CMD is
   skipped. The control-builtin, builtin, and PATH-exec arms are **unchanged**, so
   `command` still finds builtins (`command echo`) and externals (`command sort`).

Everything downstream (inline-assignment snapshot/restore via `resolved`,
redirects, `open_stage_files`, status) flows through the rewritten `resolved`
unchanged — `command sort >f` and `FOO=x command cmd` work for free.

## Section 2 — `builtin_command` (`src/builtins.rs`)

Leave `builtin_command` as-is for the `-v`/`-V` introspection + no-args paths
(reached only when `command` is NOT intercepted — i.e. `-v`/`-V` present or no
operand). Its bare-form rejection (`"bare form … not supported"`, builtins.rs
~5593) becomes unreachable from `run_exec_single` (the executor intercepts the
bare-CMD case first) but is KEPT as defensive code / for any direct
`run_builtin("command", …)` call. No behavioral change to introspection.

## Edge cases & notes

- **`command` finds builtins**: bypassing functions does NOT bypass builtins —
  `command echo`/`command cd`/`command read` run the builtin (the builtin arm is
  ungated). Matches bash.
- **`command <declaration-builtin>`** (`command declare -x X`): rare. The
  `decl_args` pre-parsed by `resolve` were computed for the OUTER `command`, not
  the inner declaration builtin, so an array-style RHS may not re-parse. v99
  handles the common external/regular-builtin case; `command declare`/`local`
  with compound RHS is a documented best-effort edge (note as a low sub-divergence
  if it misbehaves). nvm uses only external commands after `command`.
- **`command` not found**: the unchanged PATH-exec arm yields the normal
  `command not found` + rc 127.
- **`-p` default PATH**: accepted; resolution uses current `$PATH`. The exact
  bash default-PATH value is a low-impact sub-divergence (logged).
- **Inline assignments / redirects**: unchanged — they bind to the rewritten
  `resolved`.
- **No regression for non-`command` commands**: the interception only fires when
  `resolved.program == "command"`; everything else is byte-unchanged.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/executor.rs` | `run_exec_single`: `command`-bare-form interception (flag scan + program/args rewrite + `bypass_functions`); gate the function arm on `!bypass_functions` |
| `src/builtins.rs` | none required (keep `builtin_command` for `-v`/`-V`/no-args; bare-form rejection becomes defensive) — possibly remove the now-unreachable-from-exec rejection's user-facing message if the team prefers, but keeping it is fine |
| `tests/command_bare_form_integration.rs` | NEW — function-bypass, builtin-still-runs, external, assignment, double-command, not-found, `-v`/`-V` unchanged |
| `tests/scripts/command_bare_form_diff_check.sh` | NEW — 24th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-85 `[fixed v99]`; `-p` default-PATH sub-divergence note; `command <declaration-builtin>` edge note; changelog; README row |

## Testing

1. **Integration** (`tests/command_bare_form_integration.rs`):
   - `ls() { echo FUNC; }; command echo bypass` → the `echo` builtin prints
     `bypass` (and a function-bypass case: define a function shadowing a builtin
     or external, `command <name>` runs the non-function).
   - Function bypass for an EXTERNAL: `true() { echo F; }; command true; echo $?`
     → runs `/bin/true` (rc 0, no `F`).
   - `command echo hi` → `hi` (builtin runs).
   - `FOO=barv command env | grep '^FOO=' ` style, or simpler: assignment visible
     to the external.
   - `command command echo nested` → `nested`.
   - `command no_such_cmd_xyz; echo $?` → 127.
   - `command -v echo` / `command -V echo` → introspection UNCHANGED.
   - `command` (no operand) → rc 0; `command -p echo hi` → `hi`.
2. **bash-diff harness** `tests/scripts/command_bare_form_diff_check.sh` (24th):
   deterministic fragments — `f() { echo FUNC; }; command f 2>/dev/null; echo $?`
   (wait — `f` isn't a builtin/PATH cmd, so `command f` → not found 127; use a
   name that IS a real command: `echo() { printf FUNC; }; command echo hi`),
   `command echo hi`, `command printf '%s\\n' x`, `command command echo y`,
   `command nosuch; echo $?`, `command -v echo` — byte-identical to bash.
3. **Regression**: existing `command -v`/`-V` tests pass; non-`command` dispatch
   unchanged; the full suite green.
4. **End-to-end**: `nvm.sh`'s `command sort`/`command sed`/`command mkdir` now
   execute; re-bisect nvm — the M-85 runtime errors are gone (a parse gap in
   `nvm_list_aliases` may remain, separately).
