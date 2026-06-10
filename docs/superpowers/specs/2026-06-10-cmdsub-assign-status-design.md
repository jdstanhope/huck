# huck v126 — command-substitution exit status in a bare assignment (`$?` after `VAR=$(cmd)`) Design

**Status:** approved design, ready for implementation plan.
**Implements:** a bare (standalone) assignment command's exit status = the exit
status of the **last command substitution** executed in its right-hand sides
(left-to-right), or **0** if no command substitution ran.
**Why now:** this is the root cause of nvm's `→ N/A` (surfaced after v125 fixed
`→ ∞`). `nvm_resolve_local_alias` does `VERSION="$(nvm_resolve_alias "$1")";
EXIT_CODE=$?`; huck leaves `$?`=0 after the assignment, so `EXIT_CODE` is always
0 → it returns "success" with an empty `VERSION` → `nvm_ls "v24.16.0"`
short-circuits empty → `nvm_version` → `N/A`.
**Branch (impl):** `v126-cmdsub-assign-status`.

## Background — probed bash 5.x semantics

| fragment | bash `$?` | huck (pre-fix) |
|---|---|---|
| `x=$(false)` | `1` | `0` |
| `x=$(exit 7)` | `7` | `0` |
| `x=5` (no cmd-sub) | `0` | `0` |
| `x=$(false) y=$(exit 2)` (two assigns) | `2` (last) | `0` |
| `x="$(false)$(exit 5)"` (two subs, one RHS) | `5` (last executed) | `0` |
| `readonly x; x=$(false)` | `1` (readonly error) | (error) |
| `local v=$(exit 9)` *(inside a function)* | `0` (`local` is a command) | `0` |
| `declare d=$(exit 4)` | `0` (`declare` is a command) | `0` |
| `x=$(exit 3) true` (assignment **prefix** to a command) | `0` (the command's status) | `0` |

**Rule:** only a **bare/standalone assignment command** (one or more
`name=value`, with **no** command word and **not** a declaration builtin) takes
`$?` from the last command substitution in its RHS expansions (or 0 if none).
Declaration builtins (`local`/`declare`/`readonly`/`export`) and
assignment-prefixes-to-a-command keep the builtin's / command's status — these
go through the `Exec` path, not the bare-assignment path, so they are already
correct and must stay unchanged.

## Root cause (in code)

A bare assignment parses to `SimpleCommand::Assign(items)` and is dispatched by
`run_single` (`src/executor.rs:2665`):
```rust
SimpleCommand::Assign(items) => {
    let mut st = 0;
    for a in items {
        let name = a.target.name();
        if shell.is_readonly(name) { eprintln!("huck: {name}: readonly variable"); st = 1; break; }
        if apply_one_assignment(a, shell).is_err() { st = 1; break; }
    }
    ExecOutcome::Continue(st)   // <-- always 0 on success, clobbering the cmd-sub status
}
```
`apply_one_assignment` expands each RHS; a command substitution in the RHS runs
via `run_substitution` (`src/expand.rs:1174`), which **already** sets the
parent's `$?` correctly (`shell.set_last_status(status)`, expand.rs:1177). But
the `Assign` arm returns `Continue(0)` on success, and the caller overwrites
`$?` with that 0.

## Architecture — a dedicated cmd-sub status field

Add a `Shell` field that tracks the most recent command substitution's exit
status, set by `run_substitution`, and have the bare-assignment arm read it.
A dedicated field (rather than reusing `last_status`) is required because `$?`
inside an RHS (`x=$?`) must read the **previous** command's status via the
existing pre-assignment snapshot — resetting `last_status` to 0 before the loop
would corrupt that. The new field tracks command substitutions without touching
`$?`/snapshot semantics.

### Component 1 — `Shell.last_cmd_sub_status: Option<i32>`
Add to the `Shell` struct (`src/shell_state.rs`), default `None`. Internal
bookkeeping; not exported as a variable.

### Component 2 — `run_substitution` records it (`src/expand.rs:1174`)
```rust
pub fn run_substitution(seq: &Sequence, shell: &mut Shell) -> String {
    let mut cloned = shell.clone();
    let (output, status) = executor::execute_capturing(seq, &mut cloned);
    shell.set_last_status(status);
    shell.last_cmd_sub_status = Some(status);   // NEW: track for bare-assignment status
    strip_trailing_newlines(&output)
}
```
(Setting `Some(status)` each time makes "last cmd-sub wins" automatic across
multiple subs in one RHS and across multiple assignment items.)

### Component 3 — the bare-assignment arm reads it (`src/executor.rs:2665`)
```rust
SimpleCommand::Assign(items) => {
    shell.last_cmd_sub_status = None;     // scope: only THIS assignment's RHS subs count
    let mut st = 0;
    for a in items {
        let name = a.target.name();
        if shell.is_readonly(name) { eprintln!("huck: {name}: readonly variable"); st = 1; break; }
        if apply_one_assignment(a, shell).is_err() { st = 1; break; }
    }
    if st == 0 {
        st = shell.last_cmd_sub_status.unwrap_or(0);  // last cmd-sub status, or 0 if none
    }
    ExecOutcome::Continue(st)
}
```
- On a readonly/apply error, `st` stays `1` (the assignment error wins — bash
  returns 1 there regardless of any cmd-sub). Do NOT override with the cmd-sub
  status in that case.
- The reset to `None` at the top scopes the tracking to this command, so a
  command substitution from an earlier command can't leak in.

No other code reads `last_cmd_sub_status`; the `Exec` path (declaration
builtins, assignment-prefix-to-command) never consults it, so those statuses are
unchanged.

## Scope & correctness
- Every probed bash case above is satisfied (single, multi-item last-wins,
  multi-sub-one-RHS last-wins, plain→0, readonly→1, `local`/`declare`/prefix
  unchanged).
- `$?` inside an RHS (`false; x=$?; echo $x` → `x=1`) is unaffected — the
  snapshot mechanism reads `last_status`, which this change does not perturb.

## Must-not-regress
- `local`/`declare`/`typeset`/`readonly`/`export NAME=$(cmd)` statuses (the
  `Exec`/declaration-builtin path) — keep the builtin's status.
- Assignment-prefix-to-a-command `NAME=$(cmd) prog` — keep `prog`'s status.
- Plain `x=5` → 0.
- `x=$?` reads the previous command's status (snapshot).
- Existing `expand_command_sub_updates_parent_last_status` and the assignment
  snapshot tests.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/shell_state.rs` | Add `last_cmd_sub_status: Option<i32>` to `Shell` (default `None`). |
| `src/expand.rs` | `run_substitution` sets `shell.last_cmd_sub_status = Some(status)`. |
| `src/executor.rs` | `run_single`'s `SimpleCommand::Assign` arm reads it for the return status. Unit tests. |
| `tests/cmdsub_assign_status_integration.rs` (NEW) | probed cases vs bash (file-arg, L-27). |
| `tests/scripts/cmdsub_assign_status_diff_check.sh` (NEW) | 49th bash-diff harness. |
| `README.md` | harness count 48 → 49. |
| `docs/bash-divergences.md` | no entry to delete (bug was undocumented); add a `[deferred]` entry only if a residual is found. |

## Testing
1. **Unit** (`executor.rs` / `shell_state.rs`): drive `run_single` on a
   `SimpleCommand::Assign` whose RHS is `$(exit 7)` and assert the outcome is
   `Continue(7)`; a plain `x=5` → `Continue(0)`; two items last-wins. (If
   constructing `SimpleCommand::Assign` is verbose, cover via integration and
   keep a minimal unit check that `last_cmd_sub_status` is set by a substitution.)
2. **Integration** (`tests/cmdsub_assign_status_integration.rs`, binary vs bash,
   file-arg): the probed table — `x=$(false); echo $?` → 1; `x=$(exit 7)` → 7;
   `x=5` → 0; `x=$(false) y=$(exit 2)` → 2; `x="$(false)$(exit 5)"` → 5;
   `false; x=$?; echo $x` → 1 (snapshot); `local v=$(exit 9)` inside a function
   → 0; `x=$(exit 3) true; echo $?` → 0.
3. **49th bash-diff harness** `cmdsub_assign_status_diff_check.sh` — ~8 fragments,
   byte-identical (compare `echo $?` outputs).
4. **Regression**: full suite, all 49 harnesses, `cargo clippy --all-targets`.
5. **Payoff**: non-interactive `nvm alias` and PTY `nvm ls` — the alias
   destinations now show real versions (e.g. `default -> lts/* (-> v24.16.0)`),
   **no `→ N/A`**. Honest note: the job-notification noise (L-28) and ~30s
   runtime remain (separate; → v127).

## Edge cases & notes
- **`$(cmd)` as a standalone command** (not an assignment), e.g. `$(false)`:
  unaffected by this change (it's not a `SimpleCommand::Assign`). Out of scope.
- **Arithmetic / parameter-expansion in the RHS** (`x=$((1/0))`, `x=${y:?err}`):
  these set status via their own paths, not `run_substitution`; this change does
  not alter them. If a probe shows a divergence there, it is separate — log it,
  don't fix here.
- **Append assignment `x+=$(false)`**: same `SimpleCommand::Assign` path — covered.
