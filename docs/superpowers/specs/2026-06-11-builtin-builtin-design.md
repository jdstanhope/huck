# huck v142 — the `builtin` builtin Design

**Status:** approved design, ready for implementation plan.
**Implements:** the `builtin NAME [args…]` builtin — run the shell builtin `NAME`
directly, bypassing functions and aliases, erroring if `NAME` is not a builtin.
**Branch (impl):** `v142-builtin-builtin`.

## Background — the bug it fixes

`mise activate bash` wraps `cd` with a function that calls `builtin cd` to invoke
the real `cd` from inside the `cd()` function without recursing:
```bash
function __zsh_like_cd() { … builtin "$@"; }   # mise line 137
function cd() { __zsh_like_cd cd "$@"; }         # mise line 154
```
huck has no `builtin` builtin (`type builtin` → not found), so after sourcing
`~/.bashrc` (which runs `eval "$(mise activate bash)"`), `cd` dies with
`huck: command not found: builtin`. `builtin` is uncatalogued (no M-entry).

bash semantics (verified):
- `builtin echo hi` → `hi`, rc 0.
- `builtin nosuchthing` → `bash: …: builtin: nosuchthing: not a shell builtin`, rc 1.
- `builtin` (no args) → rc 0.
- `cd(){ builtin cd "$@"; }; cd /tmp` → `/tmp` (no recursion — `builtin` bypasses
  the `cd` function).
- `f(){ builtin local x=5; echo "$x"; }; f` → `5` (declaration builtins work via
  `builtin`).

## Architecture — mirror the `command` machinery

`builtin` is `command`'s sibling (run a name bypassing function/alias lookup), but
restricted to builtins. It integrates into the same executor dispatch
(`run_exec_single`, `src/executor.rs`) right beside the existing `command`
handling (the pre-resolve declaration interception at ~2992 and the
`while resolved.program == "command"` strip loop at ~3016).

### 1. Pre-resolve declaration interception (mirror `command`'s)
Before `resolve(cmd, shell)`, if `word_static_text(&cmd.program) == Some("builtin")`
AND the first operand's static text is a declaration command
(`is_declaration_command` — `declare`/`typeset`/`local`/`readonly`/`export`),
rewrite to an inner `ExecCommand { program: cmd.args[0].clone(), args:
cmd.args[1..].to_vec(), …redirects/inline_assignments… }` and `return
run_exec_single(&inner, shell, sink)`. This reuses the declaration machinery so
`builtin local x=1` / `builtin declare a=(1 2)` build correct `decl_args` and
dispatch `run_declaration_builtin` (declaration builtins can't be
function-shadowed, so the bypass is moot — same rationale as `command`'s block).

### 2. Post-resolve strip loop (a `builtin`-only flag)
After the existing `while resolved.program == "command"` loop, add a
`while resolved.program == "builtin"` loop:
```rust
    let mut require_builtin = false;
    while resolved.program == "builtin" {
        match resolved.args.first() {
            None => return ExecOutcome::Continue(0), // `builtin` alone
            Some(_) => {
                let new_program = resolved.args[0].clone();
                let new_args = resolved.args[1..].to_vec();
                resolved.program = new_program;
                resolved.args = new_args;
                resolved.decl_args = None;
                bypass_functions = true;
                require_builtin = true;
                // loop: collapse `builtin builtin …`
            }
        }
    }
```
(`builtin` takes NO options in bash — `builtin [shell-builtin [args]]` — so the
first operand is always the target name; a leading `-x` operand is just treated
as a builtin name and falls through to the not-a-builtin error.)

### 3. The builtin-only guard
After the strip loop, before dispatch:
```rust
    if require_builtin
        && !builtins::is_builtin(&resolved.program)
        && resolved.decl_args.is_none()
    {
        eprintln!("huck: builtin: {}: not a shell builtin", resolved.program);
        return ExecOutcome::Continue(1);
    }
```
(`decl_args.is_none()` is belt-and-suspenders: a declaration form is intercepted
pre-resolve and never reaches here with `require_builtin`; for non-declaration
`builtin cd`, `decl_args` is `None` and `is_builtin("cd")` is true, so this passes.)
Then the EXISTING dispatch (declaration → `run_declaration_builtin`; function →
skipped because `bypass_functions`; `eval`/`source` specials; `is_builtin` →
`run_builtin`) runs the target builtin.

### 4. Registration
- Add `"builtin"` to `BUILTIN_NAMES` (`src/builtins.rs:~24`) so `type builtin`,
  `command -v builtin`, `complete -b`, and `is_builtin("builtin")` recognize it.
- Add a defensive `"builtin" => ExecOutcome::Continue(0),` arm to the `run_builtin`
  match (the strip loop normally consumes `builtin` before dispatch; this guards a
  bare `builtin` that somehow reaches `run_builtin`, e.g. via a future path).

## Behaviour matrix (target = bash)

| input | result |
|---|---|
| `builtin cd /tmp; pwd` | `/tmp` (runs cd builtin) |
| `builtin echo hi` | `hi`, rc 0 |
| `builtin nosuchthing` | `huck: builtin: nosuchthing: not a shell builtin`, rc 1 |
| `builtin` (alone) | rc 0 |
| `cd(){ builtin cd "$@"; }; cd /x` | runs cd, no recursion |
| `f(){ builtin local x=5; echo $x; }; f` | `5` (declaration builtin) |
| `eval "$(mise activate bash)"; cd /tmp; pwd` | `/tmp` (the payoff) |

## Scope & must-not-regress
- The existing `command` handling is UNTOUCHED (the `builtin` loop is added after
  it, gated on `program == "builtin"`).
- `bypass_functions` is the existing flag (set by `command`'s bare form); `builtin`
  reuses it. `require_builtin` is the only new flag.
- Pathological combos (`command builtin cd`, `builtin command cd`) are not a goal;
  the strip loops run command-then-builtin once each, so `builtin command cd`
  leaves `program == command` (a builtin, so no error) and dispatches the
  `command` builtin — acceptable edge, not exercised by mise.

## Documented divergences
- **A user function named `builtin` cannot shadow the builtin** — `[intentional]`,
  low. Same class as L-19(c) (`command`): the interception runs before function
  lookup. Add a brief note (extend L-19, or a one-line `[intentional]` entry).
  POSIX discourages naming a function `builtin`; the unconditional interception is
  what makes `builtin` reliably bypass functions in the first place.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/executor.rs` | Pre-resolve `builtin`-declaration interception (mirror `command`'s ~2992); the `while resolved.program == "builtin"` strip loop + `require_builtin` flag after the `command` loop (~3059); the not-a-shell-builtin guard before dispatch. |
| `src/builtins.rs` | `"builtin"` in `BUILTIN_NAMES`; defensive `"builtin" => Continue(0)` in `run_builtin`; (optional) a `help` table entry. |
| `tests/builtin_builtin_integration.rs` (NEW) | The behaviour matrix via the huck binary (`-c`). |
| `tests/scripts/builtin_builtin_diff_check.sh` (NEW, 62nd) | Bash-diff over the same cases. |
| `docs/bash-divergences.md` | Extend L-19 (or add a one-line entry) for the can't-shadow-`builtin` `[intentional]` divergence (no count change if folded into L-19). |

## Testing

1. **Integration `#[test]`s** (`tests/builtin_builtin_integration.rs`), via the
   huck binary, asserting exact stdout/rc (compare to bash):
   - `builtin cd /tmp; pwd` → `/tmp` (rc 0).
   - `builtin echo hi` → `hi`.
   - `builtin nosuchthing 2>&1; echo rc=$?` → `huck: builtin: nosuchthing: not a shell builtin` + `rc=1`.
   - `builtin; echo rc=$?` → `rc=0`.
   - **the cd-wrapper repro:** `cd(){ builtin cd "$@"; }; cd /tmp; pwd` → `/tmp`
     (rc 0) — the core fix; without `builtin` this errored.
   - **bypass a `cd()` function:** `cd(){ echo SHADOW; }; builtin cd /tmp; pwd` →
     `/tmp` (the function is bypassed, not run).
   - **declaration builtin:** `f(){ builtin local x=5; echo "$x"; }; f` → `5`.
   - `type builtin` → recognizes it as a shell builtin; `command -v builtin` → `builtin`.
2. **Bash-diff harness** `tests/scripts/builtin_builtin_diff_check.sh` (62nd) — the
   same fragments via `-c`, byte-identical stdout + rc to bash. (Use `cd /tmp`/a
   stable dir so `pwd` is deterministic across both shells.)
3. **Full regression:** entire suite + ALL harnesses green — ESPECIALLY the
   existing `command`-builtin tests (the `command` path must be unchanged) and the
   function-dispatch / declaration-builtin tests. `clippy` clean.
4. **Payoff (manual/integration):** `eval "$(mise activate bash)"; cd /tmp; pwd` →
   `/tmp` (mise's `cd` wrapper now works end-to-end). Add as an integration test if
   `mise` is reliably available; otherwise verify manually and note it.

## Edge cases & notes
- `builtin` with a declaration target reaches dispatch ONLY via the pre-resolve
  recurse (so `require_builtin` + `decl_args` never conflict).
- `builtin builtin cd` collapses through the strip loop (program ends `cd`).
- A redirect on `builtin cd >file` is handled by the existing redirect machinery
  (the strip loop only rewrites program/args, leaving `cmd.stdin/stdout/stderr`).
- **Git safety:** implementer subagents must NOT `git checkout <sha>`; the
  controller verifies the branch tip before merging. Commit trailer:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
