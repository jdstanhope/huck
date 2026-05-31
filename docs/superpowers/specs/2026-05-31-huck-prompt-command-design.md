# huck v61 — `PROMPT_COMMAND` (M-76 cont.)

## Goal

Bash extension: run the value of `PROMPT_COMMAND` as a shell
command before each PS1 prompt is displayed. Common uses: update
the terminal title, sync history to disk, set dynamic prompt
state. v60 deferred this; v61 ships it.

After v61:

- `PROMPT_COMMAND='echo "[$?] $(date +%H:%M)"'` prints that line
  before every PS1.
- `PROMPT_COMMAND='history -a'` would sync history (once we have
  `history -a`; functional in v61 even though the command itself
  is just a no-op-ish `history` call).
- `PROMPT_COMMAND='exit 7'` exits the shell with status 7 right
  away (matches bash).
- `PROMPT_COMMAND=''` (set but empty) is a no-op.
- Unset `PROMPT_COMMAND`: no-op (the v60 default).
- Non-interactive runs (piped stdin) don't fire it — bash also
  skips it there.
- PROMPT_COMMAND does NOT fire before PS2 (continuation lines).
- PROMPT_COMMAND can modify `$?`; whatever value it leaves
  becomes the `$?` the user's next typed command sees (matches
  bash).

This finishes the `PROMPT_COMMAND` row that M-76 listed as
deferred.

## Scope decisions

**String form only.** Bash 5+ also accepts an array form
(`PROMPT_COMMAND=("cmd1" "cmd2")`); huck has no arrays, so
deferred.

## Out of scope (deferred)

- Array-form `PROMPT_COMMAND`.
- Re-entrancy guard. Bash doesn't fire PROMPT_COMMAND while
  PROMPT_COMMAND is itself running. huck naturally has the same
  property because `process_line` never re-enters the REPL
  outer loop; no special guard needed.

## Architecture

One new helper in `src/shell.rs`:

```rust
/// Fires `$PROMPT_COMMAND` if set, non-empty, and the shell is
/// interactive. Returns Some(exit_code) when PROMPT_COMMAND
/// returns `ExecOutcome::Exit` (e.g. `PROMPT_COMMAND='exit 7'`),
/// in which case the outer REPL must clean up and exit. Returns
/// None otherwise; on Continue, updates `shell.last_status` so
/// PS1 expansion's `\?` reflects PROMPT_COMMAND's exit code
/// (matches bash).
pub fn fire_prompt_command(shell: &mut Shell) -> Option<i32>;
```

Wire-in at the top of the outer REPL loop (`run` function in
`src/shell.rs`), after the existing helper refresh and before
`read_logical_command`:

```rust
if let Some(exit_code) = fire_prompt_command(&mut shell) {
    crate::traps::fire_exit_trap(&mut shell);
    shell.hangup_jobs();
    shell.history.save();
    return exit_code;
}
```

The exit-path cleanup (`fire_exit_trap` + `hangup_jobs` +
`history.save`) mirrors the existing user-command Exit handling
at lines 74-79.

## Behavior table

| Scenario | Behavior |
|---|---|
| `PROMPT_COMMAND` unset | No-op |
| `PROMPT_COMMAND=''` | No-op |
| `PROMPT_COMMAND='echo hi'` | "hi" printed before each PS1 |
| `PROMPT_COMMAND='false'` | `$?` becomes 1 (visible in next `\?` of PS1 and in next user command) |
| `PROMPT_COMMAND='exit 7'` | Shell exits with status 7 immediately |
| `PROMPT_COMMAND` set, but stdin is piped (non-interactive) | No-op |
| Continuation lines (PS2 prompt) | PROMPT_COMMAND does NOT fire (only before PS1) |
| `PROMPT_COMMAND='not-a-command'` | Error to stderr, `$?` reflects the failure, shell continues |

## Test plan

### Unit tests in `src/shell.rs::prompt_command_tests` (5 tests)

1. `fires_when_set` — set `PROMPT_COMMAND='true'` + `is_interactive=true`; call `fire_prompt_command`; returns None; `last_status` is 0.
2. `last_status_reflects_pc` — set `PROMPT_COMMAND='false'` + `is_interactive=true`; call; returns None; `last_status` is 1.
3. `no_op_when_unset` — `is_interactive=true`, no PROMPT_COMMAND var; call; returns None; `last_status` unchanged.
4. `no_op_when_empty` — `PROMPT_COMMAND=''` + interactive; returns None; `last_status` unchanged.
5. `propagates_exit` — `PROMPT_COMMAND='exit 7'` + interactive; returns `Some(7)`.
6. `silent_when_non_interactive` — `PROMPT_COMMAND='false'` but `is_interactive=false`; returns None; `last_status` unchanged.

(6 tests really, not 5.)

### Integration tests

Skipped. PROMPT_COMMAND only fires in interactive mode; piped
stdin (the test harness path) doesn't trigger it. Covered by
unit tests.

### Smoke

`cargo test --all-targets` green (PTY flake tolerated).

## Implementation tasks

1. **Foundation + 6 unit tests** — add `fire_prompt_command` in
   `src/shell.rs`; wire into the outer loop; add a small test
   module.

2. **Docs** — update M-76 to note `PROMPT_COMMAND` shipped;
   change-log; README v61 row.

## Acceptance criteria

- 6 unit tests pass.
- `cargo test --all-targets` green.
- `cargo clippy --all-targets -- -D warnings` clean.
- PROMPT_COMMAND fires before PS1 in interactive mode.
- PROMPT_COMMAND does NOT fire in non-interactive mode.
- PROMPT_COMMAND does NOT fire before PS2 (continuation).
- PROMPT_COMMAND's `exit N` exits the shell with status N.
- M-76 entry updated to remove `PROMPT_COMMAND` from the
  deferred list.
