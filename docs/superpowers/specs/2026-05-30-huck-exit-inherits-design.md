# huck v57 — `exit` inherits `$?` (M-74)

## Goal

Make bare `exit` (no args) inherit the last command's exit status,
matching bash/POSIX.

After v57:

- `exit N` — exit with status `N mod 256` (unchanged from v56).
- `exit` — exit with the current `last_status` (was: always 0).
- `exit foo` — "numeric argument required" + status 2 (unchanged).

Surfaced by v56's `printf` implementer when writing
`printf_no_args_usage_error`: the test had to use `rc=$?` capture
instead of relying on `exit` to propagate printf's status-2.

New tracked divergence: **M-74: `exit` inherits `$?`**.

## Scope

Strictly the one-line semantic fix + tests + docs. No other
changes to `builtin_exit`.

## Architecture

`src/builtins.rs::builtin_exit` currently:

```rust
fn builtin_exit(args: &[String]) -> ExecOutcome {
    match args.first() {
        None => ExecOutcome::Exit(0),
        ...
    }
}
```

Change to take `&Shell`:

```rust
fn builtin_exit(args: &[String], shell: &Shell) -> ExecOutcome {
    match args.first() {
        None => ExecOutcome::Exit(shell.last_status()),
        Some(code_str) => match code_str.parse::<i32>() {
            Ok(code) => ExecOutcome::Exit(code.rem_euclid(256)),
            Err(_) => {
                eprintln!("huck: exit: {code_str}: numeric argument required");
                ExecOutcome::Continue(2)
            }
        },
    }
}
```

Update the dispatch arm in `run_builtin` to pass `shell`.

`last_status()` is `i32`; bash does NOT mod-256 the inherited
form (since `$?` is already in the standard 0..=255 range plus
signal +128 conventions). The kernel's `_exit(status)` truncates
to a byte anyway. Pass through unchanged.

## Behavior table

| Input | Before v57 | After v57 |
|---|---|---|
| `false; exit` (top-level script ending) | exits 0 | exits 1 |
| `true; exit` | exits 0 | exits 0 |
| `(exit 42); exit` | exits 0 | exits 42 (subshell set $?=42) |
| `printf 'oops'; exit 5` | exits 5 | exits 5 (unchanged) |
| `exit abc` | status 2 + stderr (unchanged) | unchanged |

## Test plan

### Unit tests in `src/builtins.rs::mod exit_tests` (2 new tests)

1. `exit_no_args_inherits_last_status` — `shell.set_last_status(42); builtin_exit(&[], &shell)` → `ExecOutcome::Exit(42)`.
2. `exit_no_args_inherits_zero_when_clean` — fresh `Shell::new(); builtin_exit(&[], &shell)` → `ExecOutcome::Exit(0)`.

If a `mod exit_tests` doesn't already exist, create it; otherwise append. (Most likely it doesn't — `exit` was a one-line function.)

### Integration tests in `tests/exit_inherits_integration.rs` (2 tests)

1. `bare_exit_after_false_returns_1` — script `false\nexit\n` → process exit code 1.
2. `bare_exit_after_true_returns_0` — script `true\nexit\n` → process exit code 0.

Use the same `run_capture(script) -> (stdout, stderr, exit_code)` helper as v55's `read_integration.rs` (with the `i32` exit code in the return tuple).

## Implementation tasks

1. **Fix + 2 unit tests + 2 integration tests** —
   `src/builtins.rs` (signature + body + dispatch) and new
   `tests/exit_inherits_integration.rs`.

2. **Docs** — M-74 entry in `docs/bash-divergences.md`;
   change-log entry; README v57 row.

## Acceptance criteria

- `cargo test --all-targets` green (PTY flake tolerated).
- `cargo clippy --all-targets -- -D warnings` clean.
- New unit + integration tests pass.
- `bash -c 'false; exit' ; echo $?` matches huck's behavior post-fix.
- `tests/printf_integration.rs::printf_no_args_usage_error` is NOT
  modified by this iteration — it still uses the `rc=$?` capture
  pattern (which works correctly either way). v57 just removes the
  underlying reason that test couldn't use the simpler `exit-with-
  inherited-status` pattern. A future iteration could simplify that
  test if desired; out of scope for v57.
