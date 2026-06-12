# huck v147 — `export -f` (export shell functions) Design

**Status:** approved design, ready for implementation plan.
**Implements:** M-121 — `export -f NAME` exports a shell function to child processes
(both directions: huck emits `BASH_FUNC_*` into child environments, and huck imports
`BASH_FUNC_*` from its own environment at startup). Builds on v146's `generate`
AST→source serializer.
**Branch (impl):** `v147-export-f`.

## Background

bash exports functions via the environment using a `BASH_FUNC_<name>%%` variable
whose value is the function body as `() { … }`:
```
$ foo(){ echo hi; }; export -f foo; env | grep BASH_FUNC
BASH_FUNC_foo%%=() {  echo hi
}
$ bash -c 'foo'        # a child shell imports & runs it
hi
```
- `export -f NAME` marks an EXISTING function exported; `export -f nope` →
  `bash: export: nope: not a function`, rc 1.
- `export -f` (no names) and `declare -fx` LIST exported functions: the body
  followed by `declare -fx NAME`.
- `export -p` does NOT include functions.
- `unset -f NAME` removes the function AND its export (the `BASH_FUNC_*` var
  disappears).

huck has v146's `generate` (renders a `Command` AST back to source) and two
external-spawn sites that pass `process.envs(shell.exported_env())`
(src/executor.rs:3436, :5047). `export -f NAME` is currently a v145 rc-0 no-op.

## Architecture

### 1. Exported-function tracking (`src/shell_state.rs`)
Functions are stored as a bare `Rc<HashMap<String, Box<Command>>>` (no attribute
slot). Add a parallel `exported_functions: HashSet<String>` to `Shell`. Helpers:
- `mark_function_exported(name)`, `unmark_function_exported(name)`,
  `is_function_exported(name) -> bool`, `exported_function_names() -> Vec<String>`
  (sorted).
- `define_function` keeps an existing export mark (re-defining an exported function
  stays exported). `remove_function`/`unset -f` also calls `unmark_function_exported`.

### 2. `export -f` + listing (`src/builtins.rs`)
The v145 `export -f` no-op (in `builtin_export_decl`, the `if func { … }` arm)
becomes:
- **With operands:** for each NAME — if `shell.functions` contains NAME →
  `mark_function_exported(NAME)`; else `eprintln!("huck: export: {NAME}: not a
  function")` + rc 1 (per-operand). Do NOT create a variable.
- **No operands:** LIST every exported function (sorted) as
  `generate::function_to_source(name, body)` followed by `declare -fx NAME`.
- `declare -fx` (declare's `-f` + `-x`) produces the SAME exported-function listing.
  (Wire the declare builtin's `-f`+`-x` combination to the shared lister.)

### 3. Child-env injection — the EXPORT direction (`src/executor.rs` + `generate`)
Add `generate::exported_function_value(body: &Command) -> String` →
`format!("() {}", command_to_source(body, 0))` (renders `() {\n    <body>\n}` — a
valid brace group a child bash/huck parses after prepending the name).

Add `shell.exported_function_env() -> Vec<(String, String)>` → for each exported
function still defined, `("BASH_FUNC_{name}%%", exported_function_value(body))`.

At BOTH spawn sites (executor.rs:3436, :5047), after `process.envs(shell.exported_env())`,
also `process.envs(shell.exported_function_env())`. These are SEPARATE from regular
exported variables (a `BASH_FUNC_*` var is never an ordinary huck variable).

### 4. Startup import — the IMPORT direction + SECURITY (`src/shell_state.rs`)
In `Shell::new`'s env-load loop (shell_state.rs:386): a key matching
`BASH_FUNC_<name>%%` is NOT inserted into `vars` (it's a function encoding, not a
variable). Collect these; after the `Shell` is built, import each via a SECURE
parse, then return the shell:

```
fn parse_imported_function(name: &str, value: &str) -> Option<Box<Command>>
```
- Reconstruct the source `"{name} {value}"` (= `name () { body }`) and
  `lexer::tokenize` + `command::parse` it.
- Accept ONLY a `Sequence` whose `first` is `Command::FunctionDef { name: n, body }`
  with `n == name`, `rest` EMPTY, and `background == false`. Anything else (parse
  error, trailing commands, a non-FunctionDef, a name mismatch) → `None` (skip).
- Return the `body`. NEVER execute the value.

For each imported `(name, body)`: `define_function(name, body)` +
`mark_function_exported(name)`.

**Shellshock (CVE-2014-6271) hardening:** the original bash bug *executed* the
function-definition env value, so a trailing `() { :;}; <malicious>` ran. huck only
PARSES the value and requires EXACTLY one `FunctionDef` with nothing after the `}`,
so a `BASH_FUNC_x%%=() { :; }; rm -rf ~` is rejected (not defined, not run). A
malformed value is silently skipped — huck must not crash or execute on hostile env.

## Behaviour matrix (target = bash)

| input | result |
|---|---|
| `f(){ echo hi; }; export -f f; env \| grep BASH_FUNC_f` | `BASH_FUNC_f%%=() { … }` |
| `f(){ echo imported; }; export -f f; bash -c f` | `imported` (child bash) |
| `f(){ echo imported; }; export -f f; huck -c f` | `imported` (child huck — import path) |
| `export -f nope` | `huck: export: nope: not a function`, rc 1 |
| `f(){ :; }; export -f f; unset -f f; env \| grep -c BASH_FUNC_f` | `0` |
| `f(){ echo x; }; export -f f; export -f` | `f ()\n{…}\ndeclare -fx f` |
| `f(){ echo x; }; export -f f; declare -fx` | same |
| `f(){ echo x; }; export -f f; export -p \| grep f` | (no function — vars only) |
| `BASH_FUNC_x%%='() { :; }; touch /tmp/pwn' huck -c ':'` | NOT defined, /tmp/pwn NOT created |

## Scope & non-goals
- **Interop both directions** (huck↔bash) via the bash-compatible `BASH_FUNC_<name>%%`
  / `() { … }` encoding.
- The function BODY value is huck's NORMALIZED `generate` output (re-parseable; not
  byte-identical to bash's — a child shell parses it, doesn't compare it). Inherits
  v146's normalized-form contract.
- **Out of scope:** function names with non-identifier characters (M-09a already
  restricts huck functions to POSIX identifiers — `BASH_FUNC_<name>%%` import/export
  only handles identifier names; a `BASH_FUNC_a.b%%` from bash is skipped on import,
  documented). `export -fn NAME` (un-export via `-n -f`) — bash supports it; include
  if cheap (the `-n`+`-f` combination → `unmark_function_exported`), else defer.

## Documented divergences
- **DELETE M-121** (resolved) from Tier-2. Tier-2 count 20 → 19.
- If a corner is deferred (e.g. non-identifier names, `export -fn`), add a single
  low-impact `[deferred]` note rather than leaving M-121.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/shell_state.rs` | `exported_functions: HashSet<String>` + mark/unmark/is/names helpers; `exported_function_env()`; `Shell::new` skips `BASH_FUNC_*%%` from vars and imports them via the secure `parse_imported_function`; `remove_function` un-marks. |
| `src/generate.rs` | `pub fn exported_function_value(body: &Command) -> String`. |
| `src/builtins.rs` | `export -f` marking + not-a-function error + the exported-function lister; `declare -fx` wired to the lister; `unset -f` un-export. |
| `src/executor.rs` | inject `shell.exported_function_env()` at the two spawn sites. |
| `tests/export_f_integration.rs` (NEW) | huck→bash + bash→huck + huck→huck interop, the not-a-function error, unset -f un-export, the SECURITY trailing-token rejection. |
| `tests/scripts/export_f_diff_check.sh` (NEW, 66th) | Bash-diff over the listing + `BASH_FUNC` env shape (the building blocks that ARE byte-comparable). |
| `docs/bash-divergences.md` | DELETE M-121 (Tier-2 20→19). |

## Testing

1. **Unit tests** (`src/shell_state.rs`/`src/builtins.rs` mod tests): `export -f f`
   marks it; `is_function_exported`; `unset -f` un-marks; `exported_function_env`
   yields `BASH_FUNC_f%%` → `() {…}`; `parse_imported_function` accepts a clean
   `() { echo hi; }` and REJECTS `() { :; }; evil` (trailing) + a bare command + a
   parse error.
2. **Integration** (`tests/export_f_integration.rs`, via the huck binary):
   - **huck→bash:** `huck -c 'f(){ echo OK; }; export -f f; bash -c f'` → `OK`.
   - **huck→huck:** `huck -c 'f(){ echo OK; }; export -f f; <huck-bin> -c f'` → `OK`
     (use `env!("CARGO_BIN_EXE_huck")` as the child).
   - **bash→huck:** spawn `<huck-bin> -c g` with the bash-shaped encoding
     `BASH_FUNC_g%%=() { echo FROMBASH; }` set on the CHILD's environment → prints
     `FROMBASH` (huck imported a function from the env). (Construct the child
     `std::process::Command` directly with `.env("BASH_FUNC_g%%", "() { echo FROMBASH; }")`.)
   - **not a function:** `huck -c 'export -f nope; echo rc=$?'` → stderr message + `rc=1`.
   - **unset -f un-export:** `huck -c 'f(){ :;}; export -f f; unset -f f; env | grep -c BASH_FUNC_f'` → `0`.
   - **SECURITY:** spawn `<huck-bin> -c ':'` with env
     `BASH_FUNC_x%%='() { :; }; touch /tmp/huck_pwn_<pid>'` → assert the file is NOT
     created AND `x` is NOT a defined function (`type x` → not found). Clean up.
3. **Bash-diff harness** `export_f_diff_check.sh` (66th): `export -f` listing +
   `declare -fx` + the `BASH_FUNC_NAME%%` env-var NAME shape (compare NAMEs, since the
   VALUE body is normalized/non-byte-identical — assert the env KEY `BASH_FUNC_f%%`
   exists in both, and the listing's `declare -fx f` trailer line matches).
4. **Full regression:** entire suite + ALL harnesses green; ESPECIALLY existing
   `export`/`declare`/`unset`/spawn tests, and confirm `Shell::new`'s BASH_FUNC
   special-casing doesn't disturb ordinary env-var loading (a normal `FOO=bar` still
   loads as a variable). `clippy` clean.

## Edge cases & notes
- A `BASH_FUNC_<name>%%` key where `<name>` is not a valid huck function identifier
  → skipped on import (documented; M-09a-adjacent).
- The injected `BASH_FUNC_*` env entries are computed at spawn time from the CURRENT
  exported set (a function exported then re-defined exports the new body).
- An exported function that is later `unset -f` is dropped from `exported_function_env`.
- `Shell::new` runs the import for EVERY huck process (incl. `-c`/script) — the env is
  the inheritance channel. Ensure it's robust to absent/garbage `BASH_FUNC_*`.
- **Git safety:** implementer subagents must NOT `git checkout <sha>`; the controller
  verifies the branch tip before merging. Commit trailer:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
