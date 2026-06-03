# v79 — `break N` / `continue N` (Loop Levels) Design Spec

**Date**: 2026-06-03
**Iteration**: v79
**Divergences closed**: M-30 partial (`break N` and `continue N` level
arguments; plus the previously-uncatalogued silent-success of `break`/
`continue` outside any loop, now diagnosed and exit-1 per bash).

## Goal

Add bash's multi-level loop control:

- **`break N`** exits N enclosing loops (default N=1).
- **`continue N`** continues the Nth enclosing loop (default N=1).
- **N > loop depth** silently caps to loop depth (bash compat).
- **N ≤ 0 or non-numeric** errors with status 128 and a bash-style diagnostic.
- **`break` / `continue` outside any loop** errors with status 1 and a
  bash-style diagnostic ("only meaningful in a `for', `while', or
  `until' loop"). Previously silent exit 0.

## Non-goals

- **`return N`** is already correct — N is the exit status, not a loop
  level. The M-30 entry's title misleadingly grouped `return` with
  break/continue; the flipped entry corrects this.
- **`return abc` error path** (bash: status 128 "numeric argument
  required"; huck: silent fallback to `$?`). Possible future L-* entry
  but out of scope.
- **`return -N` signed-to-unsigned wrap** (bash: interprets as
  `256-N`). Not in scope.

## Architecture overview

Four touchpoints, each contained:

| File | What changes |
|---|---|
| `src/builtins.rs` | `ExecOutcome::LoopBreak` / `LoopContinue` gain a `u32` level payload. New `builtin_break` / `builtin_continue` helpers parse N from args, check `shell.loop_depth`, cap level, emit the variant or a status-1/-128 diagnostic. |
| `src/shell_state.rs` | New `Shell.loop_depth: u32` field; init 0 in `Shell::new()`. |
| `src/executor.rs` | `run_for` / `run_while` / `run_arith_for` each increment `shell.loop_depth` on entry and decrement on exit (single-return-path wrapper, no RAII). Match arms for `LoopBreak(n)` / `LoopContinue(n)` use the decrement-and-bubble pattern: `n==1` is consumed by this loop, `n>1` bubbles as `n-1` to the outer loop. `call_function` saves+restores `shell.loop_depth` (set to 0 inside the function). |
| (no new files) | Tests added inline in existing test modules + one new `tests/loop_levels_integration.rs` + one new `tests/scripts/loop_levels_diff_check.sh`. |

No changes to lexer, expand, command (parser), or shell.rs.

## AST change

`ExecOutcome` in `src/builtins.rs`:

```rust
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ExecOutcome {
    Continue(i32),
    Exit(i32),
    LoopBreak(u32),         // was: LoopBreak — payload is the level (1-based; 1 = innermost)
    LoopContinue(u32),      // was: LoopContinue
    FunctionReturn(i32),
}
```

**`u32`** (not `usize`) because the value is always small and bash uses
int. Payload semantics:

- Level is 1-based. `1` = "break this loop"; `2` = "break this loop AND
  the next one out"; etc.
- Level `0` is **never emitted** — the builtin rejects N≤0 with status 128.
- The builtin caps emitted level to `shell.loop_depth` so loops can
  safely subtract on bubble-up without an "off the top" case.

### Affected match sites

Every existing pattern-match on `LoopBreak`/`LoopContinue` must be
updated. The mechanical changes:

- **Loop runners** (`run_for`, `run_while`, `run_arith_for`): change
  bare `LoopBreak` to `LoopBreak(1)` and `LoopBreak(n)` patterns; add
  the decrement-and-bubble logic. Same for Continue.
- **Pipeline / subshell / sequence propagation sites** (~6 in
  `src/executor.rs`): change `LoopBreak | LoopContinue` to
  `LoopBreak(_) | LoopContinue(_)` — they just propagate.
- **Top-level coercion** (`src/executor.rs:78` and ~3 similar): change
  `LoopBreak | LoopContinue => 0` to `LoopBreak(_) | LoopContinue(_) =>
  0`. Defensive — should not fire in practice because the builtin's
  depth check prevents leaks.

Confirm all sites via `cargo build`'s exhaustiveness check.

## Shell.loop_depth + tracking

New field:

```rust
// src/shell_state.rs
pub struct Shell {
    // ... existing ...
    pub loop_depth: u32,
}
```

Initialized to 0 in `Shell::new()`.

### Loop runner wrapping

Each of `run_for`, `run_while`, `run_arith_for` increments/decrements
around its body via a single-return-path wrapper:

```rust
fn run_for(clause: &ForClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    shell.loop_depth = shell.loop_depth.saturating_add(1);
    let result = run_for_inner(clause, shell, sink);
    shell.loop_depth = shell.loop_depth.saturating_sub(1);
    result
}

fn run_for_inner(clause: &ForClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    /* existing body, with the new LoopBreak(n)/LoopContinue(n) match arms */
}
```

This avoids needing an RAII guard (which would conflict with the
`&mut Shell` borrow needed inside the body). `saturating_add`/`sub`
defends against pathological depth values.

Apply identically to `run_while` and `run_arith_for`.

### Function boundary

`call_function` in `src/executor.rs` saves and restores `loop_depth` so
that `break` inside a function called from a loop errors instead of
escaping the caller's loop (matches bash):

```rust
fn call_function(
    name: &str,
    body: Box<Command>,
    args: Vec<String>,
    shell: &mut Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
    let saved_loop_depth = std::mem::replace(&mut shell.loop_depth, 0);
    // ... existing positional_args save + scope setup ...
    let result = run_command(&body, shell, sink);
    // ... existing RETURN trap + scope teardown ...
    shell.loop_depth = saved_loop_depth;
    // ... existing positional_args restore ...
    result
}
```

### Subshells

Subshells fork before executing the body. The forked child inherits
`loop_depth` from the parent at fork time. A `break` inside a
subshell-inside-a-loop affects only the child (which is about to exit
anyway); the parent's `loop_depth` is unaffected. Matches bash. No new
code needed — the existing fork machinery handles it.

## Builtin rewrites

### Dispatch change

`run_builtin` in `src/builtins.rs:115ish` currently inlines the
`break`/`continue`/`return` cases. Change to function calls:

```rust
"break" => builtin_break(args, shell),
"continue" => builtin_continue(args, shell),
"return" => builtin_return(args, shell),  // existing inline body, extracted
```

(`builtin_return` is just a refactor — the existing inline body becomes
a named helper. No behavior change.)

### Shared parser

```rust
/// Parses the loop-level argument for `break` / `continue`.
/// `Ok(N)` is the validated positive level (defaults to 1 with no args).
/// `Err(status)` is the exit status to return on parse failure, with the
/// diagnostic already printed.
fn parse_loop_level(args: &[String], cmd: &str) -> Result<u32, i32> {
    let Some(arg) = args.first() else { return Ok(1) };
    match arg.parse::<i64>() {
        Ok(n) if n >= 1 => Ok(n.min(u32::MAX as i64) as u32),
        Ok(_) => {
            eprintln!("huck: {cmd}: {arg}: loop count out of range");
            Err(128)
        }
        Err(_) => {
            eprintln!("huck: {cmd}: {arg}: numeric argument required");
            Err(128)
        }
    }
}
```

Exit status 128 matches bash for both error paths.

Excess positional args after the level (e.g. `break 2 garbage`) are
ignored — bash also ignores them.

### builtin_break / builtin_continue

```rust
fn builtin_break(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if shell.loop_depth == 0 {
        eprintln!("huck: break: only meaningful in a `for', `while', or `until' loop");
        return ExecOutcome::Continue(1);
    }
    let level = match parse_loop_level(args, "break") {
        Ok(n) => n,
        Err(status) => return ExecOutcome::Continue(status),
    };
    let capped = level.min(shell.loop_depth);
    ExecOutcome::LoopBreak(capped)
}

fn builtin_continue(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if shell.loop_depth == 0 {
        eprintln!("huck: continue: only meaningful in a `for', `while', or `until' loop");
        return ExecOutcome::Continue(1);
    }
    let level = match parse_loop_level(args, "continue") {
        Ok(n) => n,
        Err(status) => return ExecOutcome::Continue(status),
    };
    let capped = level.min(shell.loop_depth);
    ExecOutcome::LoopContinue(capped)
}
```

## Loop runner decrement logic

Inside each `run_*_inner`, the body-result match arms become:

```rust
match execute_sequence_body(&clause.body, shell, sink) {
    ExecOutcome::Exit(code) => return ExecOutcome::Exit(code),

    ExecOutcome::LoopBreak(1) => {
        last = ExecOutcome::Continue(0);
        break;
    }
    ExecOutcome::LoopBreak(n) => {
        // Bubble to the outer loop with one fewer level.
        return ExecOutcome::LoopBreak(n - 1);
    }

    ExecOutcome::LoopContinue(1) => {
        last = ExecOutcome::Continue(0);
        // fall through to next iteration (and the step, for arith-for)
    }
    ExecOutcome::LoopContinue(n) => {
        return ExecOutcome::LoopContinue(n - 1);
    }

    ExecOutcome::FunctionReturn(code) => return ExecOutcome::FunctionReturn(code),
    ExecOutcome::Continue(c) => { last = ExecOutcome::Continue(c); }
}
```

The decrement-on-bubble pattern means each loop consumes one level.
Because the builtin already capped `level` to `shell.loop_depth`, the
bubble can never reach level 0 unexpectedly; the outermost relevant
loop sees `n == 1` and breaks.

**`run_arith_for` step interaction with `continue N`**:

- A `LoopContinue(1)` reaching this loop's body match arm falls
  through to step evaluation (then the next iteration). Same as bash:
  inner-loop `continue` runs the inner loop's step.
- A `LoopContinue(n)` with n>1 returns `LoopContinue(n-1)`
  immediately, skipping THIS loop's step.
- The outer arith-for loop, when it eventually receives the bubbled
  `LoopContinue(1)`, falls through to ITS step — so the outer loop's
  step runs.

Tracing `continue 2` from inside an inner arith-for inside an outer
arith-for: inner loop sees `LoopContinue(2)`, returns `LoopContinue(1)`
without running the inner step. Outer loop sees `LoopContinue(1)`,
falls through to outer step + next iteration. Inner step skipped;
outer step runs. Matches bash.

## Error semantics summary

| Input | huck behavior (post-v79) | bash status |
|---|---|---|
| `break` (in loop) | `LoopBreak(1)` → exit 0 | 0 |
| `break 1` (in loop) | `LoopBreak(1)` → exit 0 | 0 |
| `break 2` (in 2+ loops) | `LoopBreak(2)` → exits both | 0 |
| `break 999` (in 2 loops) | capped to 2, exits both | 0 |
| `break 0` | "loop count out of range" stderr, exit 128 | 128 |
| `break -1` | "loop count out of range" stderr, exit 128 | 128 |
| `break abc` | "numeric argument required" stderr, exit 128 | 128 |
| `break` (outside any loop) | "only meaningful in a `for'..." stderr, exit 1 | 1 |
| `break` inside function called from loop | same as outside (saved depth = 0) | 1 |

`continue` follows the same pattern with "continue" in the diagnostics.

## Testing

### Unit tests in `src/builtins.rs::tests` (~10)

- `break_no_args_emits_level_1` — `shell.loop_depth=1`, `builtin_break(&[], &mut sh)` → `LoopBreak(1)`.
- `break_with_arg_n_emits_level_n_when_in_loop`.
- `break_caps_to_loop_depth` — `loop_depth=2`, `break 999` → `LoopBreak(2)`.
- `break_outside_loop_errors_with_status_1`.
- `break_zero_errors_with_status_128`.
- `break_negative_errors_with_status_128`.
- `break_non_numeric_errors_with_status_128`.
- `continue_no_args_emits_level_1` (mirror).
- `continue_outside_loop_errors_with_status_1` (mirror).
- `continue_caps_to_loop_depth` (mirror).

### Unit tests in `src/executor.rs::tests` (~8)

Drive end-to-end via `crate::shell::process_line(input, &mut shell, false)`:

- `break_in_inner_loop_exits_inner_only` — verify outer loop iterates remaining values.
- `break_2_in_inner_loop_exits_both`.
- `break_999_caps_to_2_in_two_loops`.
- `continue_in_inner_loop_continues_inner`.
- `continue_2_in_inner_loop_continues_outer`.
- `break_inside_function_called_from_loop_errors`.
- `loop_depth_zero_after_loop_exits` — verify `shell.loop_depth == 0` after any loop returns.
- `loop_depth_restored_after_function_return` — verify depth restoration through `call_function`.

### Integration tests `tests/loop_levels_integration.rs` (~6)

Binary-driven via stdin-pipe:

- `break_2_in_nested_for` — `for i in 1 2; do for j in a b; do echo $i$j; break 2; done; done` → only `1a`.
- `continue_2_in_nested_for` — `... continue 2 ...` → `1a 2a` (skip rest of inner each time, advance outer).
- `break_overshoot_caps` — `for i in 1; do break 999; done; echo ok` → `ok`.
- `break_outside_loop_errors` — `break 2>&1; echo $?` → diagnostic + `1`.
- `break_inside_function_called_from_loop` — function calls `break`, asserts diagnostic + outer loop continues.
- `mixed_for_while_break_2` — for-loop containing a while-loop, `break 2` exits both.

### Bash-diff harness `tests/scripts/loop_levels_diff_check.sh` (~8)

Byte-identical to bash 5.2:

- `for i in 1 2; do for j in a b; do echo $i$j; break 2; done; done`
- `for i in 1 2; do for j in a b; do if [ "$j" = "b" ]; then continue 2; fi; echo $i$j; done; done`
- `for i in 1; do break 999; done; echo ok`
- `break 2>&1; echo $?`
- `break abc 2>&1; echo $?`
- `break 0 2>&1; echo $?`
- `break -1 2>&1; echo $?`
- `continue 2>&1; echo $?` (outside loop)

(Some of these include stderr content from bash that we'll need to
match byte-for-byte. If huck's diagnostic differs in punctuation, the
fragment may need a `grep`-style strip — flag during implementation.)

## Scope estimate

| Section | LOC |
|---|---|
| AST change + match-site updates | ~30 |
| `Shell.loop_depth` field + init | ~5 |
| Loop runner wrappers + decrement logic | ~30 |
| Function boundary save/restore | ~5 |
| Builtin rewrites (helper + 3 builtins) | ~60 |
| Tests (unit + integration) | ~250 |
| Bash-diff harness | ~80 |
| Docs | ~30 |
| **Total** | **~130 LOC code + ~360 LOC tests** |

**3 tasks**:

1. AST change + `Shell.loop_depth` + builtin rewrites + ~10 builtin
   unit tests.
2. Loop runner decrement logic + function-boundary save/restore +
   ~8 executor unit tests.
3. Integration tests + bash-diff harness + docs (flip M-30 to
   `[fixed v79]`, correct the entry text since `return N` was
   wrongly grouped, change-log entry, README v79 row, Summary table
   + Last updated stamp refresh).

## Deferrals

- `return abc` error path (status 128) — possible future L-* low-impact.
- `return -N` bash signed-to-unsigned wrap — possible future L-* low-impact.
- `break` / `continue` with extra trailing args being silently ignored
  (matches bash; no fix needed).

## Change-log entry (for `docs/bash-divergences.md`)

To add after merge (date placeholder updated by implementer):

> **2026-06-XX**: M-30 (`break N` / `continue N` loop levels) shipped
> as v79. Also closes a previously-uncatalogued gap: `break` /
> `continue` outside any loop now produces a bash-style diagnostic +
> exit 1 (was silent exit 0). `ExecOutcome::LoopBreak` and
> `LoopContinue` gain a `u32` level payload (1-based; capped to actual
> loop depth by the builtin). New `Shell.loop_depth: u32` field;
> incremented by `run_for` / `run_while` / `run_arith_for` (saturating
> ops); saved+restored across `call_function` so a `break` in a
> function called from a loop correctly errors as out-of-loop.
> Loop runners use the decrement-and-bubble pattern: `LoopBreak(1)`
> is consumed by this loop; `LoopBreak(n>1)` bubbles as `n-1`. Same
> for Continue. New `builtin_break` / `builtin_continue` / extracted
> `builtin_return` helpers in `src/builtins.rs`. Bash status 128 for
> `break 0` / `break -1` / `break abc` (loop count out of range /
> numeric argument required). Excess trailing args silently ignored
> (matches bash). `return N` was already correct in v0; the M-30
> entry's title misleadingly grouped it with break/continue — entry
> text now corrects this. ~10 unit + ~8 executor + ~6 integration +
> ~8 bash-diff fragments byte-identical to bash 5.2 (huck's 7th
> harness). Test count grows by ~32.

## Open questions

None. All architectural decisions resolved during brainstorm.
