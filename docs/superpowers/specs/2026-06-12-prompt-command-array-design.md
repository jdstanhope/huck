# huck v148 — PROMPT_COMMAND array execution Design

**Status:** approved design, ready for implementation plan. (Short iteration.)
**Fixes:** an array `PROMPT_COMMAND` runs only element `[0]`; bash 5.1+ runs every
element. This is why oh-my-posh's prompt doesn't render under huck after
`source ~/.bashrc` (mise + oh-my-posh both register hooks as array elements; huck
runs only mise's, never `_omp_hook`, so `PS1='$(_omp_get_primary)'` is never set).
**Branch (impl):** `v148-prompt-command-array`.

## Root cause (diagnosed)

`fire_prompt_command` (src/shell.rs:561) reads `PROMPT_COMMAND` via
`shell.lookup_var` — the SCALAR view. For an indexed array, `scalar_view` returns
element `[0]` (shell_state.rs:32). So with
`PROMPT_COMMAND=(_mise_hook_prompt_command _omp_hook)`, huck executes only
`_mise_hook_prompt_command`. Verified live: huck stores PROMPT_COMMAND as a proper
array (`declare -p` shows `[0]`/`[1]`) but runs only `[0]`.

bash 5.1+ semantics (verified): an indexed-array `PROMPT_COMMAND` runs EACH element
as a separate command, in ascending index order; EMPTY elements are skipped; sparse
arrays are honored (iterate present indices in order).

## Fix

In `fire_prompt_command`, branch on whether `PROMPT_COMMAND` is an indexed array:
- **Array** (`shell.get_array("PROMPT_COMMAND") -> Some(&BTreeMap<usize, String>)`):
  iterate the map's VALUES in ascending key order (BTreeMap iterates sorted), and for
  each NON-EMPTY element run `process_line(elem, shell, true)` in order.
  - An element returning `ExecOutcome::Exit(code)` STOPS the loop and returns
    `Some(code)` (the REPL handles shell-exit — matches the current scalar path).
  - Otherwise each element's `Continue(status)` updates `shell.set_last_status(status)`
    (so `$?`/PS1's `\?` reflect the LAST element, like bash). Skipped (empty) elements
    don't touch status.
  - An empty array (or all-empty elements) → run nothing, return `None`.
- **Scalar / associative / unset** → the EXISTING scalar path
  (`lookup_var` non-empty → `process_line`), UNCHANGED.

Keep the existing guards: non-interactive shells return `None` immediately; the
function's signature and return contract (`Option<i32>`) are unchanged.

## Behaviour matrix (target = bash)

| `PROMPT_COMMAND` | runs |
|---|---|
| `="cmd"` (scalar) | `cmd` (unchanged) |
| `=(a b)` | `a` then `b` |
| `=(a "" b)` | `a` then `b` (empty skipped) |
| sparse `[2]=b [0]=a` | `a` then `b` (index order) |
| `=(_mise_hook_prompt_command _omp_hook)` | both → oh-my-posh prompt renders |
| `=("exit 7")` / `[0]="exit 7"` | shell exits rc 7 (Exit propagates) |
| unset / empty | nothing |

## Files & responsibilities

| File | Change |
|------|--------|
| `src/shell.rs` | `fire_prompt_command`: array branch (run each non-empty element in index order; Exit propagates; last element sets `$?`); scalar path unchanged. |
| `src/shell.rs` mod tests | array-PROMPT_COMMAND unit tests (both elements run via side effects; empty-element skip; Exit propagation; scalar still works). |
| `tests/pty_interactive.rs` (if cheap) | optional: an interactive PTY test that a two-element array PROMPT_COMMAND runs both hooks. |
| `docs/bash-divergences.md` | If an open `[deferred]` entry covers this, DELETE it; else no entry needed (it was an undocumented bug). |

## Testing

1. **Unit tests** (`src/shell.rs` mod tests, mirroring the existing scalar
   `fire_prompt_command` tests at ~728): set `shell.is_interactive = true`; set
   `PROMPT_COMMAND` to an array via `replace_array` of two commands that each have an
   observable side effect (e.g. `MARKER_A=1` / `MARKER_B=1`, or append to a var); call
   `fire_prompt_command`; assert BOTH side effects happened (and in order). Plus:
   empty-element-skipped; an `exit 7` element → `Some(7)`; a scalar still runs (existing
   tests stay green).
2. **Payoff (manual/PTY):** after sourcing the real bashrc, the oh-my-posh glyph prompt
   renders (both mise + omp hooks fire). A PTY test asserting two array hooks both run is
   the automated proxy if cheap; otherwise the unit tests are the gate.
3. **Full regression:** suite + all harnesses green; the existing scalar PROMPT_COMMAND
   tests unaffected. clippy clean.

## Notes
- `get_array` returns `Option<&BTreeMap<usize, String>>` (shell_state.rs:1040) — borrow
  ends before the `&mut shell` `process_line` calls; clone the element strings (or
  collect a `Vec<String>` of non-empty values) up front to satisfy the borrow checker.
- **Git safety:** implementer subagents must NOT `git checkout <sha>`; the controller
  verifies the branch tip before merging. Commit trailer:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
