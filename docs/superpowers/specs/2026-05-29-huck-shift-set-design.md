# huck v50 — `shift` and `set --` (M-65)

## Goal

Add two POSIX special builtins that manipulate positional parameters:

- `shift [N]` — remove the first N positional parameters (N defaults
  to 1).
- `set [args]` — replace positional parameters; with no args, list
  all shell variables.

Both are POSIX special builtins. After v50, common script patterns
like `while [ $# -gt 0 ]; do ... shift; done` and `set -- $(...)` work.

New tracked divergence: **M-65: `shift` and `set --`**.

## Scope decisions (locked)

This is a tight, focused change. No scope questions surfaced worth
asking. The supported subset:

1. **`shift`**: full POSIX. N defaults to 1; negative or non-numeric
   N → error 1; N > count → error 1.
2. **`set` with no args**: list all shell variables sorted as
   `name='value'`. Bash format.
3. **`set --` and `set -- args`**: clear or replace positional.
4. **`set args` (no leading `--`)**: bash-faithful — also replaces
   positional, as long as the first arg doesn't start with `-` or
   `+`.
5. **`set -e`/`set -x`/`set -u`/etc.** (shell options): explicitly
   rejected with status 2 + a clear "not yet supported" message.
   These are a future iteration.

## Out of scope (deferred)

- All `set` option flags (`-e`, `-x`, `-u`, `-o pipefail`, `+e`,
  etc.). Explicitly rejected with status 2 so users get clear
  signal rather than silent success.
- `set -o` / `set +o` output listing.
- Coupling shift/set to a parent-shell flag — there is none.

## Architecture

Single-file change in `src/builtins.rs`. Two new builtins + their
helpers + dispatch arms in `run_builtin` + `BUILTIN_NAMES` /
`is_special_builtin` updates.

### `builtin_shift`

```rust
fn builtin_shift(args: &[String], shell: &mut Shell) -> ExecOutcome {
    let n: usize = match args.first() {
        None => 1,
        Some(s) => match s.parse::<usize>() {
            Ok(n) => n,
            Err(_) => {
                eprintln!("huck: shift: {s}: numeric argument required");
                return ExecOutcome::Continue(1);
            }
        },
    };
    if n > shell.positional_args.len() {
        eprintln!("huck: shift: shift count out of range");
        return ExecOutcome::Continue(1);
    }
    shell.positional_args.drain(0..n);
    ExecOutcome::Continue(0)
}
```

- No args → shift 1.
- `s.parse::<usize>()` rejects negatives, decimals, non-numeric, and
  empty.
- N == 0 is a valid no-op (drains 0 elements).
- Trailing args after N are silently ignored (bash-compat).

### `builtin_set`

```rust
fn builtin_set(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if args.is_empty() {
        let mut names: Vec<String> = shell.var_names().map(|s| s.to_string()).collect();
        names.sort();
        for name in &names {
            if let Some(v) = shell.lookup_var(name) {
                let _ = writeln!(out, "{}={}", name, set_escape_value(&v));
            }
        }
        return ExecOutcome::Continue(0);
    }

    let first = &args[0];
    if first == "--" {
        shell.positional_args = args[1..].to_vec();
        return ExecOutcome::Continue(0);
    }
    if (first.starts_with('-') || first.starts_with('+')) && first.len() > 1 {
        eprintln!("huck: set: {first}: options not yet supported in this version");
        return ExecOutcome::Continue(2);
    }
    // No leading -- or option flag — treat as positional replacement.
    shell.positional_args = args.to_vec();
    ExecOutcome::Continue(0)
}

fn set_escape_value(v: &str) -> String {
    format!("'{}'", v.replace('\'', r#"'\''"#))
}
```

- No args: list all shell variables sorted, format `name='value'`.
  `Shell.var_names()` returns an iterator over `&str`; collect into
  `Vec<String>` for sorting.
- `--` alone: clear positional (empty slice after `--`).
- `--` + args: replace.
- Leading `-` or `+` longer than 1 char: option flag → reject with
  status 2.
- Plain args (no leading `-`/`+`): replace positional (bash-faithful
  fallback).

### Dispatch + `BUILTIN_NAMES` + `is_special_builtin`

In `src/builtins.rs`:

1. Add `"set"` and `"shift"` to `BUILTIN_NAMES`.
2. Add `"set" => builtin_set(args, out, shell)` and `"shift" =>
   builtin_shift(args, shell)` to `run_builtin` (positioned near
   `"trap"`).
3. Extend `is_special_builtin` to include `"set"` and `"shift"`.
   POSIX classifies both as special.

### `is_special_builtin` doc comment update

The current doc comment (around `src/builtins.rs:30-33`) explicitly
mentions `set`/`shift` as future additions. Trim it to remove
those names (`/`set`/`shift`/`trap`/...`/) since they're now
supported. Leave the remaining "future" names: `eval`, `exec`, `:`,
`readonly`, `.`.

## Test plan

### Unit tests in `src/builtins.rs#[cfg(test)] mod shift_tests`

7 tests:

1. `shift_no_args_removes_first` — positional `[a, b, c]`; `shift`
   → `[b, c]`, status 0.
2. `shift_n_removes_n` — `[a, b, c, d]`; `shift 2` → `[c, d]`.
3. `shift_default_when_no_args_equals_one` — explicit test that
   `shift` and `shift 1` behave identically.
4. `shift_too_large_errors_status_1` — `[a]`; `shift 5` → status 1,
   positional unchanged.
5. `shift_zero_is_noop` — `[a, b]`; `shift 0` → unchanged, status 0.
6. `shift_non_numeric_errors_status_1` — `shift abc` → status 1.
7. `shift_negative_errors_status_1` — `shift -1` → status 1
   (parse::<usize> fails on negative).

### Unit tests in `src/builtins.rs#[cfg(test)] mod set_tests`

6 tests:

8. `set_no_args_lists_sorted_vars` — pre-load 3 vars out of order;
   output is sorted; format `name='value'`.
9. `set_double_dash_alone_clears_positional` — positional `[a, b]`;
   `set --` → empty, status 0.
10. `set_double_dash_with_args_replaces` — positional `[]`;
    `set -- one two` → `[one, two]`.
11. `set_bare_args_replaces_positional` — positional `[]`;
    `set one two three` → `[one, two, three]`.
12. `set_dash_e_rejects_with_status_2` — `set -e` → status 2.
13. `set_plus_x_rejects_with_status_2` — `set +x` → status 2.

### Integration tests at `tests/shift_set_integration.rs`

2 binary-driven tests:

1. `shift_advances_positional_in_function` — script defines a
   function that does `shift; echo $1`; call with multiple args;
   stdout shows the shifted-to arg.
2. `set_then_for_loop_positional` — script:
   `set -- one two three\nfor arg in "$@"; do echo $arg; done\nexit\n`.
   Stdout has lines `one`, `two`, `three`.

### Smoke

`cargo test --all-targets` must pass. PTY flake tolerated.

## Implementation tasks

1. **Builtins + unit tests**:
   - Add `builtin_shift`, `builtin_set`, `set_escape_value` in
     `src/builtins.rs`.
   - Add dispatch arms in `run_builtin`.
   - Add `"set"`, `"shift"` to `BUILTIN_NAMES`.
   - Extend `is_special_builtin`.
   - Update the doc comment on `is_special_builtin` to drop
     `set`/`shift` from the future list.
   - Append `mod shift_tests` (7 tests) and `mod set_tests` (6
     tests) at end of file.

2. **Integration tests**: create `tests/shift_set_integration.rs`
   with the 2 scenarios.

3. **Docs**: add **M-65: `shift` and `set --`** entry; change-log
   entry; README v50 row.

Three tasks. TDD per task.

## Acceptance criteria

- All 13 unit tests pass.
- Both integration tests pass.
- `cargo test --all-targets` passes (modulo PTY flake).
- `cargo clippy --all-targets -- -D warnings` passes.
- `docs/bash-divergences.md` has the new M-65 entry as
  `[fixed v50]`.
- `shift` works at top level and inside functions.
- `set --` and `set -- args` and bare `set args` all replace
  positional.
- `set -e`/`set -x`/etc. error with status 2 + clear message.
- `is_builtin("shift")` and `is_builtin("set")` return true;
  `is_special_builtin` includes both.
