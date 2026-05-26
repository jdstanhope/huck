# v26: Special Parameters `$0`, `$$`, `$!` — Design Spec

## Goal

Implement three POSIX special parameters that huck currently returns empty for:
- `$0` — shell name (or function name when inside a function call)
- `$$` — shell process ID (cached so subshells return the parent's value)
- `$!` — PID of the most-recently-backgrounded pipeline's last stage

Closes audit-doc entries M-01, M-02, M-03 — all three flagged high-severity
in the bash-divergences reference because they appear in virtually every
real bash script (`$0` for usage messages and `[ "$0" = "$BASH_SOURCE" ]`
guards; `$$` for unique temp-file naming; `$!` for the "wait for the last
backgrounded job" idiom).

Pre-v26: `lookup_var("0")`, `lookup_var("$")`, `lookup_var("!")` all return
`None`, so `$0`, `$$`, `$!` expand to empty.

After v26 they all expand correctly.

## Scope

All three parameters in one iteration (user-confirmed). They share lexer +
AST plumbing (a `WordPart::Var { name }` with one of three magic names) and
have non-overlapping but small shell-state changes.

**Excluded from scope:**
- `$#`, `$@`, `$*` — already implemented.
- `$_` (bash-specific "last argument of previous command").
- `$-` (current option flags) — depends on `set -o` (M-08, deferred).
- `$BASHPID` (per-subshell PID) — bash extension, not POSIX.
- `$PPID` (parent of shell) — useful but separate; can land as a quick win later.

## Semantics

### `$0` — shell or function name

| Context | Value |
| --- | --- |
| Top-level REPL (no function active) | `argv[0]` of the huck binary (e.g. `huck`, `/usr/local/bin/huck`, or `target/debug/huck` during dev) |
| Inside a function call | the function name as declared (e.g. `myfunc`) |
| Nested function calls | the innermost function's name; restored on each return |

Persistence: like `positional_args`, `$0` is saved/restored at function-call
boundaries via a stack. The existing v22 `call_function` save/restore pattern
extends to a new `function_arg0: Vec<String>` field.

### `$$` — shell PID

Cached at shell startup via `libc::getpid()`. Stored as
`Shell::shell_pid: i32`. Because the value is captured at startup and is part
of normal shell state, fork (via v25's `fork_and_run_in_subshell`) inherits
the cached value automatically — subshells return the parent shell's PID,
matching POSIX 2.5.2 and bash exactly.

Never changes during the shell's lifetime.

### `$!` — last backgrounded PID

Stored as `Shell::last_bg_pid: Option<i32>`. Updated:
- After each backgrounded pipeline (`cmd &`) finishes spawning, set to the
  LAST stage's pid. Per POSIX: the value is the PID of the last command in
  the asynchronous pipeline.
- Read returns the stored value as a decimal string, or empty string if no
  background has happened yet (per bash — `$!` is unset until first `&`).

The pre-v25 "synthetic done" path for all-pure-builtin backgrounded
pipelines is gone (v25 now forks every stage, so every backgrounded
pipeline has real pids). No special handling for builtin-only backgrounded
pipelines needed.

In subshells: `$!` is per-shell-state, so it reflects backgrounds in THAT
subshell. A subshell that hasn't backgrounded anything returns empty even
if the parent had. This matches bash.

## Lexer changes

`src/lexer.rs::read_dollar_expansion`:

```rust
match chars.peek().copied() {
    // existing arms: '(', '{', '?', '@', '*', digits...
    Some('$') => {
        chars.next();
        parts.push(WordPart::Var { name: "$".to_string(), quoted });
    }
    Some('!') => {
        chars.next();
        parts.push(WordPart::Var { name: "!".to_string(), quoted });
    }
    // existing fall-through for non-special chars...
}
```

The `$0` case is already lexed correctly by the existing digit-handling arm
that produces `Var { name: "0" }`. No lexer change needed for `$0`.

**Braced forms**: `${0}` already works via the existing param-expansion
braced path (it produces `Var { name: "0" }`). `${$}` and `${!}` are NOT
valid bash syntax and aren't part of v26 scope — the existing
`LexError::InvalidVarName` rejection catches them.

**No new AST variants** — `WordPart::Var { name }` already exists; just
three new magic names (`"0"`, `"$"`, `"!"`) join the routing in
`Shell::lookup_var`.

## Shell state changes

`src/shell_state.rs`:

```rust
pub struct Shell {
    // existing fields...
    pub shell_pid: i32,
    pub last_bg_pid: Option<i32>,
    pub shell_argv0: String,
    pub function_arg0: Vec<String>,
}

impl Shell {
    pub fn new() -> Self {
        let shell_pid = unsafe { libc::getpid() };
        let shell_argv0 = std::env::args().next().unwrap_or_else(|| "huck".to_string());
        Self {
            // existing inits...
            shell_pid,
            last_bg_pid: None,
            shell_argv0,
            function_arg0: Vec::new(),
        }
    }
}
```

`shell_pid` is computed once at startup; never re-read. Forked subshells
(via v25's `fork_and_run_in_subshell`) inherit the value via memory clone —
they see the original parent shell's PID, which is the bash-correct
behavior.

`shell_argv0` is captured from `std::env::args().next()` at startup.
Fallback to `"huck"` if argv is empty (shouldn't happen in practice).

`function_arg0` is a stack so nested function calls work. Push on
function-call entry; pop on return. Outer (empty stack) → `$0` returns
`shell_argv0`.

## `lookup_var` changes

`Shell::lookup_var(name)` gets three early-return cases:

```rust
pub fn lookup_var(&self, name: &str) -> Option<String> {
    // New: special parameters.
    match name {
        "0" => return Some(
            self.function_arg0.last().cloned().unwrap_or_else(|| self.shell_argv0.clone())
        ),
        "$" => return Some(self.shell_pid.to_string()),
        "!" => return Some(
            self.last_bg_pid.map(|p| p.to_string()).unwrap_or_default()
        ),
        _ => {}
    }
    // ... existing positional + var lookup ...
}
```

`$!` returns `Some("")` (empty string) when unset rather than `None`,
matching bash's "expands to empty until first `&`" semantic. Returning
`None` would treat it like an undefined variable, which would then go
through different expansion paths (e.g., `${!:-default}` would use the
default — wrong; bash's `${!:-default}` returns the default ONLY if `$!`
truly is unset, which it is before any background). The semantic question
is: should `$!` round-trip as "unset" or as "set-to-empty"? Bash says set
but empty after first non-existent reference, but unset before. For
huck v26: keep it simple — return empty string post-startup (so the
substitution path is consistent), document the slight bash-divergence in
the doc. Cost of "true unset until first bg" is a tri-state vs option:
worth doing if cheap.

**Implementer decision point**: return `Some("")` (simple) vs round-trip
unset-ness via a tri-state. Default to `Some("")` for v26 simplicity;
revisit if a user hits the divergence.

## `$!` update site

After v25, all backgrounded pipelines flow through `run_background_sequence`.
The spawn loop now collects every stage's pid in `stage_pids: Vec<i32>`.
After the loop, set `shell.last_bg_pid = stage_pids.last().copied()` —
per POSIX, the LAST stage's pid.

For an empty pipeline (defensive, shouldn't happen): leave `last_bg_pid`
unchanged.

## `$0` update sites

`function_arg0` is pushed/popped wherever `positional_args` is pushed/popped.
Today that's in `call_function` (executor.rs, v22-era code):

```rust
fn call_function(body: &Command, args: Vec<String>, shell: &mut Shell, ...) -> ExecOutcome {
    let saved_positional = std::mem::replace(&mut shell.positional_args, args);
    // NEW: also save+set $0.
    shell.function_arg0.push(function_name.to_string());  // function_name from caller
    
    let outcome = run_command(body, shell, sink);
    
    shell.function_arg0.pop();
    shell.positional_args = saved_positional;
    
    // existing FunctionReturn handling...
}
```

The function name is available at the call site (it's the resolved program
name when dispatch found the function). Pass it as a new parameter to
`call_function`.

## Edge cases

- **`$0` in a function called from another function**: the stack pops to
  reveal the outer function's name. `f() { g; echo $0; }; g() { echo $0; };
  f` → output is "g\nf".
- **`$$` in a here-doc body expansion**: the body is expanded in the parent
  before the child reads it (v24 deferred-expansion design); $$ resolves to
  the parent's cached PID. Correct.
- **`$$` in a command substitution**: substitutions run in-process (no
  fork in huck — documented v5 limitation); $$ returns the parent shell's
  PID. Matches bash by coincidence (bash forks but returns parent's PID
  anyway).
- **`$!` after a foreground command**: NOT updated; only `&` sets it.
- **`$!` after a backgrounded compound command**: the compound runs in a
  forked subshell (v25); its pid IS a real pid and gets recorded.
- **`$!` and `wait $!`**: after `cmd &`, `wait $!` should wait for that
  specific command. huck's `wait` accepts pids; ensure `$!` expands first
  (it will, via normal Word expansion before `wait` sees its args).
- **`$0` in a subshell**: subshells inherit `function_arg0` via fork; if
  the subshell calls a different function, its own push/pop applies.
  Correct.

## Out of scope

- `$_` (bash last-argument): bash-specific, not POSIX.
- `$-` (option flags): depends on `set -o` infra.
- `$BASHPID` (per-subshell PID): bash extension. Could land as a separate
  small follow-up after this iteration.
- `$PPID` (shell's parent PID): POSIX but separate; small follow-up.
- Modifying `$0`/`$$`/`$!` via assignment: not allowed in bash either;
  any `0=foo` or `$=foo` is a syntax error or no-op.

## Tests

### Lexer (`src/lexer.rs::tests`)

| Test | Covers |
| --- | --- |
| `lexer_dollar_dollar_emits_var_name_dollar` | `$$` → `Var { name: "$" }` |
| `lexer_dollar_bang_emits_var_name_bang` | `$!` → `Var { name: "!" }` |
| `lexer_dollar_zero_emits_var_name_zero` | `$0` → `Var { name: "0" }` (already works; regression test) |
| `lexer_dollar_dollar_inside_double_quotes` | `"$$"` → quoted Var { name: "$" } |
| `lexer_braced_zero_works` | `${0}` → `Var { name: "0" }` via param-expansion path |
| `lexer_braced_dollar_rejected` | `${$}` → `LexError::InvalidVarName` (or similar) |
| `lexer_braced_bang_rejected` | `${!}` → error |

### Shell state (`src/shell_state.rs::tests`)

| Test | Covers |
| --- | --- |
| `shell_new_caches_pid_and_argv0` | `Shell::new()` populates `shell_pid` and `shell_argv0` |
| `lookup_var_dollar_returns_cached_pid` | sets known pid; `lookup_var("$")` returns the same string |
| `lookup_var_bang_unset_returns_empty` | fresh shell; `lookup_var("!")` returns `Some("")` |
| `lookup_var_bang_after_set_returns_pid_string` | `shell.last_bg_pid = Some(12345); lookup_var("!")` → `Some("12345")` |
| `lookup_var_zero_top_level_returns_argv0` | empty function_arg0; returns shell_argv0 |
| `lookup_var_zero_in_function_returns_function_name` | push "myfunc"; lookup_var("0") returns "myfunc" |
| `lookup_var_zero_nested_returns_innermost` | push "outer", push "inner"; lookup_var("0") returns "inner"; pop; returns "outer" |

### Executor wiring (`src/executor.rs::tests` or integration)

| Test | Covers |
| --- | --- |
| `call_function_pushes_arg0` | unit: invoke call_function with name "myfunc"; assert function_arg0 ends empty (popped) after return; lookup during body returned "myfunc" |
| `run_background_sequence_sets_last_bg_pid` | unit: run a backgrounded `true &`; assert `shell.last_bg_pid` was set |

### Integration (new `tests/special_params_integration.rs`)

| Test | Script | Expected |
| --- | --- | --- |
| `dollar_zero_top_level_contains_huck` | `echo $0\nexit\n` | output contains "huck" |
| `dollar_zero_in_function_is_function_name` | `f() { echo $0; }; f\nexit\n` | line "f" |
| `dollar_zero_nested_functions` | `f() { g; echo $0; }; g() { echo $0; }; f\nexit\n` | "g" then "f" |
| `dollar_zero_returns_to_shell_after_function` | `f() { echo $0; }; f; echo $0\nexit\n` | "f" then output containing "huck" |
| `dollar_dollar_top_level_is_integer` | `echo $$\nexit\n` | numeric (parse as i32) |
| `dollar_dollar_same_in_subshell` | `echo $$; echo $$ \| cat\nexit\n` | two identical numeric lines |
| `dollar_bang_unset_initially_is_empty` | `echo "[$!]"\nexit\n` | "[]" |
| `dollar_bang_set_after_backgrounded_cmd` | `sleep 0.1 &\necho [$!]\nwait\nexit\n` | line "[N]" where N is numeric |
| `dollar_bang_is_last_stage_of_pipeline` | `echo hi \| cat &\necho [$!]\nwait\nexit\n` | numeric PID; ideally matches what `wait %1` would target |
| `dollar_bang_preserves_across_subsequent_foreground` | `sleep 0.1 &\nBG_PID=$!\ntrue\necho [$BG_PID] [$!]\nwait` | both bracketed values identical (foreground doesn't change $!) |

### Doc updates

- `docs/bash-divergences.md`: M-01, M-02, M-03 all → `[fixed (2026-05-26)]`. Change-log entry.
- `README.md`: v26 status row.

## Change log

- **2026-05-26**: Spec drafted; user-chosen scope = all three special
  parameters (`$0` + `$$` + `$!`) in one iteration with bash semantics
  (function-name `$0` in functions; cached `$$` for subshell consistency).
