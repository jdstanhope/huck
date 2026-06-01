# huck v66 — `eval` (M-80)

## Goal

Add the POSIX `eval` special builtin. Joins its arguments with
spaces, re-parses the result, and executes it in the current
shell context. Reuses the existing `process_line` path
(already used by `source`/`.`, trap actions).

After v66:

- `eval echo hi` → `hi` on stdout.
- `eval` (no args) → exit 0.
- `eval "X=5"` → sets X=5 in the current shell.
- `eval "echo a; echo b"` → both echoed.
- `eval exit 7` → propagates `ExecOutcome::Exit(7)` (exits the
  shell).
- `eval false` → exit 1.
- Inline assignments preceding `eval` PERSIST (eval is a POSIX
  special builtin) — `FOO=bar eval echo \$FOO` leaves `FOO=bar`
  set in the parent.
- `eval` returns the exit status of the LAST command executed
  in the re-parsed string.

New tracked divergence: **M-80: `eval`**.

## Scope decisions

Trivial. No tier choice needed — the implementation is essentially
`process_line(args.join(" "), shell, true)`. Add to
`BUILTIN_NAMES` and `is_special_builtin`.

## Out of scope

Nothing. `eval` semantics are well-defined and small. The
expand-aliases flag (`true`) matches what the REPL passes — in
interactive bash, eval expands aliases; non-interactive bash
doesn't (controlled by `shopt expand_aliases`). huck's
`process_line` accepts the bool so we just pass `true`. Small
divergence — bash gates this on shopt; huck always enables it.
Acceptable for v66; document.

## Architecture

### `builtin_eval`

```rust
fn builtin_eval(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if args.is_empty() {
        return ExecOutcome::Continue(0);
    }
    let joined = args.join(" ");
    if joined.trim().is_empty() {
        return ExecOutcome::Continue(0);
    }
    crate::shell::process_line(&joined, shell, true)
}
```

### Dispatch + BUILTIN_NAMES + is_special_builtin

- Append `"eval"` to `BUILTIN_NAMES`.
- Add `"eval"` to `is_special_builtin` (the doc comment literally
  says "expand here as huck adds eval/exec/:/readonly" — :/readonly
  are already there; this finishes eval).
- Dispatch arm: `"eval" => builtin_eval(args, shell)`.

## Behavior table

| Input | Behavior |
|---|---|
| `eval` | exit 0, no output |
| `eval ""` (empty arg) | exit 0 (joined whitespace) |
| `eval echo hi` | stdout `hi`, exit 0 |
| `eval "echo a; echo b"` | stdout `a\nb`, exit 0 |
| `eval false` | exit 1 |
| `eval exit 7` | shell exits with code 7 |
| `eval "X=5"` | X=5 persists in shell |
| `FOO=bar eval echo \$FOO` | stdout `bar`; FOO=bar persists in shell (POSIX special) |
| `eval "syntax(error"` | parse error printed; exit 2 (whatever process_line returns) |
| `eval 'X=$Y' Y=10` | joins to `X=$Y Y=10`. (One full command line — same semantics as typing it at the prompt.) |

## Test plan

### Unit tests in `src/builtins.rs::mod eval_tests` (6 tests)

1. `eval_no_args_exits_zero` — `run_builtin("eval", &[], ...)` → `Continue(0)`.
2. `eval_empty_arg_exits_zero` — single empty-string arg → `Continue(0)`.
3. `eval_simple_command_runs` — `eval echo hi` → buf contains "hi\n", exit 0.
4. `eval_assignment_persists` — `eval X=5_FOR_EVAL` → `shell.lookup_var("X_FOR_EVAL")` returns "5_FOR_EVAL".
   (Actually `X=5_FOR_EVAL` would set X to "5_FOR_EVAL". Use unique var name `EVAL_X_T4` to avoid collision.)
5. `eval_false_returns_one` — `eval false` → `Continue(1)`.
6. `eval_exit_propagates` — `eval exit 7` → `Exit(7)`.

### Integration tests in `tests/eval_integration.rs` (4 tests)

1. `eval_simple_command` — `eval echo hi` → stdout has "hi".
2. `eval_multi_statement` — `eval "echo a; echo b"` → both lines.
3. `eval_assignment_persists` — `eval "X=hello"; echo "[$X]"` → "[hello]".
4. `eval_exit_propagates` — `eval exit 7\necho unreached\nexit` → process exits with code 7, "unreached" NOT printed.

### Smoke

`cargo test --all-targets` green (PTY flake tolerated).

## Implementation tasks

1. **builtin_eval + 6 unit + 4 integration tests** — modify
   `src/builtins.rs`; new test file `tests/eval_integration.rs`.

2. **Docs** — new M-80 entry; change-log; README v66 row.

## Acceptance criteria

- 6 unit tests pass.
- 4 integration tests pass.
- `cargo test --all-targets` green.
- `cargo clippy --all-targets -- -D warnings` clean.
- `eval` is in `is_special_builtin` (inline assignments
  preceding it persist).
- `eval` returns the last command's exit status; `exit N`
  inside eval propagates.
- M-80 doc entry added.
