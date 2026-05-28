# huck v42 — `kill -s SIGNAME` + `kill -n NUM` (M-40)

## Goal

Close bash divergence M-40: add `-s SIGNAME` and `-n NUM` long-form
signal selection to the `kill` builtin. The existing `-<sig>` short
form (e.g. `kill -TERM`, `kill -15`) and default-SIGTERM behavior
remain unchanged.

After v42 the `kill` surface is:

```
kill [-s sigspec | -n signum | -sigspec] pid | %job ...
kill -l [SIGNUM | SIGNAME ...]      (from v41)
```

## Scope decisions (locked)

1. **Include both `-s` and `-n`**: parallel long-form flags. Both
   produce the same dispatch as the existing `-<sig>` path via a
   shared helper.
2. **Refactor the existing per-target send loop**
   (`src/builtins.rs:822+`) into a shared
   `send_signal_to_targets(sig, targets, shell) -> ExecOutcome`
   helper. All four signal-sending paths (`-s`, `-n`, `-<sig>`,
   default-SIGTERM) call it.

## Out of scope (deferred)

- M-41 (full platform signal set: SEGV, ABRT, FPE, BUS, ILL, TRAP).
  Larger refactor; needs new entries in `KILLABLE`/`TRAPPABLE`.
- POSIX `-s 0` / `-n 0` (signal-0 existence-check). Signal 0 isn't in
  `killable_signals()`, so `-s 0` errors. Could be added later if a
  user reports needing it.
- Combined-flag forms like `-sn`. Bash doesn't really use these for
  `kill` either.

## Architecture

All code changes confined to `src/builtins.rs`. No new files, no
changes to `src/traps.rs`. No new types.

### New helper: `send_signal_to_targets`

Extracted from the existing `builtin_kill` body
(`src/builtins.rs:822-868` — the `let mut any_failed = false; for
target in targets { ... }` loop). Signature:

```rust
fn send_signal_to_targets(
    sig: i32,
    targets: &[String],
    shell: &mut Shell,
) -> ExecOutcome
```

Behavior: identical to the current inline loop:
- For each target:
  - If starts with `%`: resolve via `resolve_spec_or_error`, lookup
    job's pgid, `killpg(pgid, sig)`. On failure, print
    `huck: kill: ({target}) - {errno}` to stderr.
  - Else parse as positive `i32`, call `kill(pid, sig)`. Same error
    pattern.
  - Unparseable: `huck: kill: {target}: arguments must be process or
    job IDs`, mark failed.
- Returns `Continue(1)` if any failed, `Continue(0)` otherwise.

### Restructured `builtin_kill` dispatcher

```rust
fn builtin_kill(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    // -l path (from v41) unchanged
    if matches!(args.first().map(|s| s.as_str()), Some("-l")) {
        return handle_kill_l(&args[1..], out);
    }

    // -s NAME or -n NUM (v42)
    match args.first().map(|s| s.as_str()) {
        Some("-s") => return kill_with_s_flag(&args[1..], shell),
        Some("-n") => return kill_with_n_flag(&args[1..], shell),
        _ => {}
    }

    // -<sig> | default SIGTERM (existing v32-era logic)
    let (sig, targets) = if let Some(first) = args.first() {
        if let Some(rest) = first.strip_prefix('-') {
            let sig = match rest.parse::<i32>() {
                Ok(n) if (0..=64).contains(&n) => n,
                Ok(_) => {
                    eprintln!("huck: kill: {rest}: invalid signal number");
                    return ExecOutcome::Continue(1);
                }
                Err(_) => match signal_by_name(rest) {
                    Some(n) => n,
                    None => {
                        eprintln!("huck: kill: {rest}: invalid signal");
                        return ExecOutcome::Continue(1);
                    }
                },
            };
            if args.len() < 2 {
                eprintln!("huck: kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | %job ...");
                return ExecOutcome::Continue(2);
            }
            (sig, &args[1..])
        } else {
            (libc::SIGTERM, args)
        }
    } else {
        eprintln!("huck: kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | %job ...");
        return ExecOutcome::Continue(2);
    };

    send_signal_to_targets(sig, targets, shell)
}
```

The usage string is bumped from `kill [-sig] pid | %job ...` to
`kill [-s sigspec | -n signum | -sigspec] pid | %job ...` (matches
bash).

### New helper: `kill_with_s_flag`

```rust
fn kill_with_s_flag(args: &[String], shell: &mut Shell) -> ExecOutcome
```

`args` here is everything after the `-s` token.

```rust
let name = match args.first() {
    Some(n) => n,
    None => {
        eprintln!("huck: kill: -s: option requires an argument");
        return ExecOutcome::Continue(2);
    }
};
let sig = match signal_by_name(name) {
    Some(n) => n,
    None => {
        eprintln!("huck: kill: {name}: invalid signal specification");
        return ExecOutcome::Continue(1);
    }
};
let targets = &args[1..];
if targets.is_empty() {
    eprintln!("huck: kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | %job ...");
    return ExecOutcome::Continue(2);
}
send_signal_to_targets(sig, targets, shell)
```

### New helper: `kill_with_n_flag`

```rust
fn kill_with_n_flag(args: &[String], shell: &mut Shell) -> ExecOutcome
```

```rust
let num_arg = match args.first() {
    Some(s) => s,
    None => {
        eprintln!("huck: kill: -n: option requires an argument");
        return ExecOutcome::Continue(2);
    }
};
let n = match num_arg.parse::<i32>() {
    Ok(n) if (1..=64).contains(&n) => n,
    _ => {
        eprintln!("huck: kill: {num_arg}: invalid signal specification");
        return ExecOutcome::Continue(1);
    }
};
// Validate that n is one of the signals huck knows how to send.
if !crate::traps::killable_signals().iter().any(|(_, num)| *num == n) {
    eprintln!("huck: kill: {num_arg}: invalid signal specification");
    return ExecOutcome::Continue(1);
}
let targets = &args[1..];
if targets.is_empty() {
    eprintln!("huck: kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | %job ...");
    return ExecOutcome::Continue(2);
}
send_signal_to_targets(n, targets, shell)
```

`-n` requires the number to be in `killable_signals()` so it stays
consistent with `kill -l`'s accepted set. The `-<sig>` short form is
more permissive (`0..=64`) because that path predates the v41
table-driven validation; we don't tighten it in v42 to keep the
change scoped.

### Error message table

| Condition | Message | Status |
|---|---|---|
| `kill -s` (no name) | `huck: kill: -s: option requires an argument` | 2 |
| `kill -n` (no number) | `huck: kill: -n: option requires an argument` | 2 |
| `kill -s BOGUS pid` | `huck: kill: BOGUS: invalid signal specification` | 1 |
| `kill -s TERM` (no targets) | `huck: kill: usage: ...` | 2 |
| `kill -n 99 pid` | `huck: kill: 99: invalid signal specification` | 1 |
| `kill -n XYZ pid` | `huck: kill: XYZ: invalid signal specification` | 1 |
| `kill -n 15` (no targets) | `huck: kill: usage: ...` | 2 |

### Unchanged paths

- `kill -l ...` (v41) — completely untouched.
- `kill -<sig> pid` — body unchanged except the final dispatch now
  calls `send_signal_to_targets` instead of inlining the loop.
- `kill pid` (no flag → SIGTERM) — same.

## Test plan

### Unit tests in `src/builtins.rs#[cfg(test)] mod kill_tests`

10 new tests. The "does it actually send a signal?" tests use
`unsafe { libc::getpid() }` as the target and pick `SIGWINCH` as a
harmless self-signal:

1. `kill_s_with_name_resolves_and_dispatches` —
   `kill -s WINCH <getpid>`; expect status 0. The fact that the
   signal arrives is not asserted (it'd race with the test runner);
   only that the dispatch path returns success.
2. `kill_s_with_sig_prefix_resolves` — `kill -s SIGWINCH <getpid>` →
   status 0.
3. `kill_s_lowercase_name_resolves` — `kill -s winch <getpid>` →
   status 0.
4. `kill_s_missing_arg_returns_usage_status_2` — `kill -s` (no further
   args) → status 2.
5. `kill_s_invalid_name_returns_status_1` — `kill -s BOGUS 99999` →
   status 1. (PID 99999 might or might not exist; the test asserts
   the EARLY error from name resolution, not the send.)
6. `kill_s_no_targets_returns_usage_status_2` — `kill -s TERM` (no
   target) → status 2.
7. `kill_n_with_number_resolves_and_dispatches` —
   `kill -n <SIGWINCH-value> <getpid>` → status 0.
8. `kill_n_missing_arg_returns_usage_status_2` — `kill -n` → status 2.
9. `kill_n_invalid_number_returns_status_1` — `kill -n 99 12345` →
   status 1.
10. `kill_dash_sig_short_form_still_works_after_refactor` — regression
    test: `kill -WINCH <getpid>` still returns status 0 via the
    refactored `send_signal_to_targets`.

That's 10 tests; the implementer may merge tests 1-3 if redundant, but
the spec calls for at least these distinct verifications. Keep all 10
for clarity.

### Integration tests at `tests/kill_s_integration.rs`

3 binary-driven tests:

1. `kill_s_invalid_name_errors_status_1` — script
   `kill -s BOGUS 99999\necho $?\nexit\n` → stdout contains `1`.
2. `kill_s_missing_arg_errors_status_2` — script
   `kill -s\necho $?\nexit\n` → stdout contains `2`.
3. `kill_n_invalid_number_errors_status_1` — script
   `kill -n 99 99999\necho $?\nexit\n` → stdout contains `1`.

These avoid sending real signals from integration tests (which would
need a sentinel PID and have flakiness risks). The signal-delivery
path is exercised by the unit tests via self-signal.

### Smoke

`cargo test --all-targets` must pass. PTY flake tolerated.

## Implementation tasks

1. **Refactor + new helpers + unit tests**:
   - Extract `send_signal_to_targets` from the existing
     `builtin_kill` body.
   - Add `kill_with_s_flag` and `kill_with_n_flag`.
   - Update the dispatcher to delegate to them.
   - Bump the usage string.
   - Add the 10 unit tests.
2. **Integration tests**: create `tests/kill_s_integration.rs` with
   the 3 scenarios.
3. **Docs**: flip M-40 to `[fixed v42]` in
   `docs/bash-divergences.md`; add change-log entry; add v42 row to
   README.

Three tasks. TDD within each, one commit per task.

## Acceptance criteria

- All 10 unit tests pass.
- All 3 integration tests pass.
- `cargo test --all-targets` passes (modulo known PTY flake).
- `cargo clippy --all-targets -- -D warnings` passes.
- `docs/bash-divergences.md` shows M-40 as `[fixed v42]`.
- `kill -s TERM <pid>` correctly sends SIGTERM (verified via
  unit test using SIGWINCH + getpid()).
- `kill -n <N> <pid>` works for any N in `killable_signals()`.
- Existing `kill -TERM <pid>`, `kill -15 <pid>`, `kill <pid>` paths
  unchanged (regression test passes).
