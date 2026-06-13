# huck v153 — `BASH_SOURCE` / `BASH_LINENO` (call-stack arrays) Design

**Status:** approved design, ready for implementation plan.
**Adds:** `BASH_SOURCE` and `BASH_LINENO` — bash's call-stack companion arrays to
`FUNCNAME`. Resolves L-40. Also fixes a latent v151 bug: `FUNCNAME` is missing the `main`
bottom frame in script mode.
**Branch (impl):** `v153-bash-source-lineno`.
**Scope:** FULL bash parity for the base/edge cases (`main`/`environment`/`source` frames,
script vs `-c` vs sourced vs interactive, top-level presence). Arrays must also work with
variable-name tab completion.

## Background — verified bash 5.x semantics

All three arrays are views of ONE underlying frame stack (`[0]` = current/top):
- `BASH_SOURCE[i]` = the source file where `FUNCNAME[i]`'s code was defined.
- `BASH_LINENO[i]` = the line number (in `BASH_SOURCE[i+1]`) at which `FUNCNAME[i]` was invoked.
- `FUNCNAME[i]` = the frame's function name (a real function, `"source"`, or `"main"`).

Verified observations:

| Context | FUNCNAME | BASH_SOURCE | BASH_LINENO |
|---|---|---|---|
| script, `g`←`f`←top (g@L7 in f, f@L9) | `g f main` | `S S S` | `7 9 0` |
| script top-level (no func) | unset (#0) | `[S]` (#1) | `[0]` (#1) |
| `-c` top-level | unset | unset | unset |
| `-c`, in `f` (f@L1) | `f` (no `main`) | `environment` | `1` |
| sourced file top-level (sourced@L2) | unset | `sub main` | `2 0` |
| inside a running `source` (in `srcfn`) | `source srcfn main` | … | … |
| fn in sourced lib, called from script | `libfn caller_fn main` | `lib app app` | `3 5 0` |

Key rules distilled: a base **`main`** frame exists only in **script** mode (not `-c`/
interactive); `-c`-defined functions get def-source **`"environment"`**; `source`/`.` pushes
a **`"source"`** frame while the sourced file runs; `BASH_SOURCE`/`BASH_LINENO` are present
whenever the stack is non-empty (so they appear at script top-level while `FUNCNAME` is unset);
`FUNCNAME` is reported **unset** when the top frame is the base `main` frame.

## Architecture

### 1. The unified frame stack (replaces v151's `function_arg0`)

`Shell.call_stack: Vec<Frame>` (bottom = oldest), replacing `function_arg0: Vec<String>`:
```rust
pub struct Frame {
    pub funcname: String,  // a function name, "source", or "main"
    pub source: String,    // file where this frame's code was defined / lives
    pub call_line: u32,    // line in the caller where this frame was invoked (0 for base)
    pub is_base: bool,     // true only for the synthetic top-level "main" frame
}
```
Three frame kinds are pushed (each push/pop is immediately followed by `sync_call_arrays`):
- **base `main`** — pushed once at script-file startup: `{ "main", script_path, 0, is_base:true }`.
  NOT pushed for `-c`/interactive (so `-c` has no `main` frame and `BASH_SOURCE` is unset at
  `-c` top level).
- **`source`** — pushed by `source`/`.` while a sourced file runs:
  `{ "source", sourced_path, current_lineno, false }`, popped after.
- **function** — pushed by `call_function`: `{ name, function_source[name].unwrap_or("environment"), current_lineno, false }`, popped after. `current_lineno` (v152) at the call IS the
  call-site line.

### 2. Per-function def-source

New `Shell.function_source: HashMap<String, String>` (parallel to the `functions` table, which
stores no metadata), cloned with the COW shell alongside `functions`. `define_function` records
`function_source[name] = self.call_stack.last().map(|f| f.source.clone()).unwrap_or_else(|| "environment".into())` — the source of the frame in which the definition executes (script path at
script top-level, the sourced file inside a `source`, `"environment"` under `-c`/interactive).
`remove_function` (unset -f) removes the entry.

### 3. Sync → stored arrays (the tab-completion mechanism)

Rename/extend v151's `sync_funcname` → `Shell::sync_call_arrays(&mut self)`, rebuilding the three
arrays in the **vars table** from `call_stack`:
- stack empty → remove `FUNCNAME`, `BASH_SOURCE`, `BASH_LINENO`.
- non-empty → set `BASH_SOURCE` and `BASH_LINENO` (indexed arrays, frames reversed). Set
  `FUNCNAME` (reversed funcnames) UNLESS the top frame `is_base` — then remove `FUNCNAME`
  (script top-level: `BASH_SOURCE`/`BASH_LINENO` present, `FUNCNAME` unset).

Because the arrays are stored (unexported, non-readonly `VarValue::Indexed`, like `PIPESTATUS`),
they appear in `shell.var_names()` → variable-name tab completion (`${BASH_<TAB>` /
`$FUNCNAME<TAB>`) and `declare -p` work with NO completion-specific code. Assignment to them is
overwritten on the next frame change (the documented dynamic-var tradeoff, same as v151).

### 4. `$0` preservation

`$0` currently returns `function_arg0.last()` (innermost function name) or `shell_argv0`. Preserve
exactly: derive `$0` from the innermost NON-base, NON-`"source"` (i.e. real-function) frame's
`funcname`, falling back to `shell_argv0`. (A helper `Shell::current_function_name()` keeps the
behavior identical; `expand.rs:220` and `lookup_var` `"0"` use it.)

## Files & responsibilities

| File | Change |
|------|--------|
| `src/shell_state.rs` | `Frame` struct; `call_stack: Vec<Frame>` (replaces `function_arg0`); `function_source` map; `sync_call_arrays` (extends `sync_funcname`); `current_function_name()` for `$0`; `lookup_var` `"0"` arm updated. |
| `src/executor.rs` | `call_function` pushes/pops a function frame (with `function_source` + `current_lineno`) + `sync_call_arrays`, replacing the `function_arg0` push/pop. |
| `src/builtins.rs` | `source`/`.` (`run_sourced_contents`) pushes/pops a `"source"` frame + sync; `define_function` records def-source; `remove_function` clears it. |
| `src/shell.rs` | script-file startup pushes the base `main` frame (not for `-c`/interactive). |
| `src/expand.rs` | `$0`-via-`function_arg0` site (`:220`) → `current_function_name()`. |
| `tests/bash_source_lineno.rs` | integration tests. |
| `tests/scripts/bash_source_lineno_diff_check.sh` | bash-diff harness (full matrix). |
| `docs/bash-divergences.md` | DELETE L-40; adjust the L-29/FUNCNAME notes if needed. |

## Behaviour matrix (target = bash; all byte-identical)

Covers the table above: script in-function (`g f main` / sources / call-lines), script top-level
(`BASH_SOURCE=[script]`, `FUNCNAME` unset, `${#BASH_SOURCE[@]}`=1), `-c` top-level (all unset),
`-c` in-function (`f` / `environment` / call-line, no `main`), sourced-file top-level
(`BASH_SOURCE=[sub main]`, `BASH_LINENO=[srcline 0]`), function defined in a sourced lib
(`libfn caller_fn main` / `lib app app` / call-lines), inside a running `source`
(`FUNCNAME=[source … main]`). Plus: `$FUNCNAME`/`$BASH_SOURCE`/`$BASH_LINENO` scalar = `[0]`;
`${#…[@]}` depths; `${!…[@]}` indices.

## Tab completion

- `${BASH_<TAB>` inside a function (or at script top-level, where `BASH_SOURCE`/`BASH_LINENO`
  are set) offers `BASH_SOURCE`, `BASH_LINENO`. `$FUNCNAME<TAB>` offers `FUNCNAME` when set.
  This works because the arrays are stored in the vars table (`var_names()`), needing no
  completion-specific change.
- Verified by a completion test driving `analyze` + `complete_variable` with a shell whose
  `call_stack` has a frame (so the arrays are populated).

## Edge cases / divergences

- A user function literally named `main` or `source`: the synthetic frames are distinguished by
  `is_base` / frame kind, not name-matching, so the `FUNCNAME`-unset rule keys off `is_base`. A
  user `main` called in a script yields two `main` entries in `FUNCNAME` (as bash does).
- Assignment to `FUNCNAME`/`BASH_SOURCE`/`BASH_LINENO`: overwritten on the next frame change.
- `$()` command-substitution bodies / here-docs: parsed via the line-0 path; frames inside a
  cmd-sub clone reflect the clone's own stack (cloned `call_stack`).
- Interactive mode: like `-c` (no base `main` frame).

## Testing

1. **Unit** (`shell_state.rs`): `sync_call_arrays` produces the right three arrays for a
   constructed `call_stack` (function/source/base frames); `FUNCNAME` unset when top `is_base`;
   `$0`/`current_function_name` unchanged.
2. **Integration** (`tests/bash_source_lineno.rs`): the behaviour-matrix cases via `huck -c`
   and `huck <file>`, asserting outputs verified against real bash.
3. **`bash_source_lineno_diff_check.sh`**: full matrix byte-identical to bash (script/sourced
   cases via file-args per the L-27 caveat).
4. **Regression:** the v151 `funcname_diff_check.sh` still passes AND a new script-mode FUNCNAME
   check (`main` frame now present); full suite + all harnesses + clippy green.

## Notes
- This reworks v151's `FUNCNAME` machinery (`function_arg0` → `call_stack`, `sync_funcname` →
  `sync_call_arrays`); the v151 tests/harness must stay green (with the script-mode `main`-frame
  addition).
- Builds on v152's `current_lineno` for `BASH_LINENO` call-site capture.
- **Git safety:** implementer subagents must NOT `git checkout <sha>`; controller verifies the
  branch tip before merge. Commit trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
