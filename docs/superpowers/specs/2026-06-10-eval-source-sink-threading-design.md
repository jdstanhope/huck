# huck v132 — sink/context threading for `eval` / `source` Design

**Status:** approved design, ready for implementation plan.
**Implements:** make `eval` and `source`/`.` run their commands with the ENCLOSING
execution's `StdoutSink` and job-control context instead of a fresh top-level
`StdoutSink::Terminal`. Fixes the `nvm ls-remote` interactive HANG, the command-
substitution output leak, and the ignored redirect on `eval`/`source`.
**Branch (impl):** `v132-eval-source-sink-threading`.

## Background — measured root cause (this session)

`eval` and `source`/`.` execute their commands through
`crate::shell::process_line` / `run_sourced_contents`, both of which call
`executor::execute(seq, shell, src)`. `execute` (src/executor.rs:47) hard-codes
`let mut sink = StdoutSink::Terminal;`, discarding whatever sink the surrounding
execution established. They are dispatched in `run_builtin` with only
`(args, shell)` — never the `StdoutSink`. Consequences:

| symptom | huck | bash |
|---|---|---|
| `x=$(eval 'seq 1 500000')` (interactive, PTY) | **HANG** | captures ~3.4MB |
| `x=$(eval 'echo hi')` | leaks `hi` to terminal, `x=''` | `x='hi'` |
| `x=$(source f)` (f prints) | leaks, `x=''` | captured |
| `eval 'echo R' >file` | redirect ignored (`file` empty) | `file` = `R` |
| `x=$(seq 1 500000)` (no eval) | ✅ works | ✅ |
| trap action inside `$()` | matches bash (terminal) | (same) |

**Why the HANG:** with a `Terminal` sink, an external command (curl) hits the
INTERACTIVE job-control path (`matches!(sink, Terminal) && !in_subshell &&
!in_completion` → setpgid + give_terminal_to). Inside a command substitution with
substantial I/O that terminal handoff deadlocks (the v124/v108 tty-deadlock
class). Non-interactively the same case only leaks (no tty → no job control).
`nvm ls-remote` is exactly `$(eval "curl … -o -")` downloading `index.tab`
(hundreds of KB) → interactive hang.

**Traps are already correct** — `trap 'echo x' DEBUG; y=$(true)` produces
byte-identical output in huck and bash (the trap action's output goes to the
terminal in BOTH, not into the `$()` capture). So traps are OUT of scope.

## Architecture — thread the sink, reusing v125's `with_redirect_scope`

`eval`/`source` are "run a body, honoring redirects + the enclosing sink" — the
SAME shape as a function call. The function-call dispatch
(src/executor.rs:3083-3091) already does exactly the right thing:
```rust
} else if !bypass_functions && let Some(body) = shell.functions.get(...).cloned() {
    if cmd.stdin.is_some() || cmd.stdout.is_some() || cmd.stderr.is_some() {
        with_redirect_scope(&cmd.stdin, &cmd.stdout, &cmd.stderr, shell, sink,
            move |shell, inner_sink| call_function(&name, body, args, shell, inner_sink))
    } else {
        call_function(&name, body, args, shell, sink)   // threads the CURRENT sink
    }
}
```
v132 gives `eval`/`source` an analogous arm.

### Component 1 — `execute_with_sink` (src/executor.rs)
Refactor `execute` to take the sink:
```rust
pub fn execute_with_sink(seq: &Sequence, shell: &mut Shell, source: &str,
                         sink: &mut StdoutSink) -> ExecOutcome {
    // … the CURRENT body of `execute` VERBATIM, but using the passed `sink`
    //    instead of `let mut sink = StdoutSink::Terminal;` …
}
pub fn execute(seq: &Sequence, shell: &mut Shell, source: &str) -> ExecOutcome {
    let mut sink = StdoutSink::Terminal;
    execute_with_sink(seq, shell, source, &mut sink)
}
```
Behavior-preserving: every existing `execute` caller (REPL `shell.rs`, traps,
`run_sourced_contents`, command subst's `execute_capturing` already uses
`execute_sequence_body` directly) is unchanged. The background fast-path and
`execute_sequence_body` call inside `execute` move verbatim into
`execute_with_sink`.

### Component 2 — `process_line_in_sink` (src/shell.rs)
```rust
pub fn process_line_in_sink(line: &str, shell: &mut Shell, expand_aliases: bool,
                            sink: &mut crate::executor::StdoutSink) -> ExecOutcome {
    // … the CURRENT body of `process_line` VERBATIM, but the final
    //    `executor::execute(&sequence, shell, line)` becomes
    //    `executor::execute_with_sink(&sequence, shell, line, sink)` …
}
pub fn process_line(line: &str, shell: &mut Shell, expand_aliases: bool) -> ExecOutcome {
    let mut sink = crate::executor::StdoutSink::Terminal;
    process_line_in_sink(line, shell, expand_aliases, &mut sink)
}
```
All current `process_line` callers (REPL, traps, `command -v` helper, etc.) keep
Terminal behavior.

### Component 3 — sink-threaded source reader (src/builtins.rs)
`run_sourced_contents` gets a sink-threaded variant:
```rust
pub(crate) fn run_sourced_contents_in_sink(contents, path, shell,
        sink: &mut StdoutSink) -> ExecOutcome {
    // … current body VERBATIM, but the per-unit
    //    `crate::executor::execute(&seq, shell, span)` becomes
    //    `crate::executor::execute_with_sink(&seq, shell, span, sink)` …
}
pub(crate) fn run_sourced_contents(contents, path, shell) -> ExecOutcome {
    let mut sink = crate::executor::StdoutSink::Terminal;
    run_sourced_contents_in_sink(contents, path, shell, &mut sink)
}
```

### Component 4 — `eval_in_sink` / `source_in_sink` (src/executor.rs)
```rust
pub(crate) fn eval_in_sink(args: &[String], shell: &mut Shell,
                           sink: &mut StdoutSink) -> ExecOutcome {
    if args.is_empty() { return ExecOutcome::Continue(0); }
    let joined = args.join(" ");
    if joined.trim().is_empty() { return ExecOutcome::Continue(0); }
    crate::shell::process_line_in_sink(&joined, shell, true, sink)
}
pub(crate) fn source_in_sink(args: &[String], shell: &mut Shell,
                             sink: &mut StdoutSink) -> ExecOutcome {
    // builtin_source's logic (arg check, source_depth cap, resolve_source_path,
    // read_to_string, positional-args save/restore) VERBATIM, but the
    // `run_sourced_contents(...)` call becomes
    // `run_sourced_contents_in_sink(..., sink)`.
}
```
`builtin_source`'s helpers (`resolve_source_path`, `source_depth`) stay in
builtins.rs; `source_in_sink` calls them via `crate::builtins::...` (make them
`pub(crate)` as needed) OR the source logic is factored so both share it. The
implementer picks the least-churn factoring (see plan); the REQUIREMENT is that
`source_in_sink` reproduces builtin_source's behavior with the sink threaded.

### Component 5 — dispatch arm in `run_exec_single` (src/executor.rs ~3092)
Insert BEFORE the generic `else if builtins::is_builtin(...)` branch, AFTER the
function-call branch:
```rust
} else if resolved.program == "eval" {
    let args = resolved.args;
    if cmd.stdin.is_some() || cmd.stdout.is_some() || cmd.stderr.is_some() {
        with_redirect_scope(&cmd.stdin, &cmd.stdout, &cmd.stderr, shell, sink,
            move |shell, inner_sink| eval_in_sink(&args, shell, inner_sink))
    } else {
        eval_in_sink(&args, shell, sink)
    }
} else if resolved.program == "source" || resolved.program == "." {
    let args = resolved.args;
    if cmd.stdin.is_some() || cmd.stdout.is_some() || cmd.stderr.is_some() {
        with_redirect_scope(&cmd.stdin, &cmd.stdout, &cmd.stderr, shell, sink,
            move |shell, inner_sink| source_in_sink(&args, shell, inner_sink))
    } else {
        source_in_sink(&args, shell, sink)
    }
}
```
Placement AFTER the function branch preserves "a user function named `eval`
shadows the builtin" (current behavior). `command eval …` reaches here too (the
`command` collapse sets `resolved.program = "eval"`, `bypass_functions = true`).

### Component 6 — `builtin_eval` / `builtin_source` delegate (src/builtins.rs)
Keep `run_builtin`'s `"eval"`/`"."`/`"source"` arms working (any non-executor
caller) by delegating to the new helpers with a Terminal sink:
```rust
fn builtin_eval(args: &[String], shell: &mut Shell) -> ExecOutcome {
    let mut sink = crate::executor::StdoutSink::Terminal;
    crate::executor::eval_in_sink(args, shell, &mut sink)
}
fn builtin_source(args: &[String], shell: &mut Shell) -> ExecOutcome {
    let mut sink = crate::executor::StdoutSink::Terminal;
    crate::executor::source_in_sink(args, shell, &mut sink)
}
```
(DRY — one implementation each. The executor path uses the real sink; run_builtin
keeps Terminal.)

## Correctness / must-not-regress
- **Behavior-preserving wrappers.** `execute`, `process_line`, `run_sourced_
  contents` keep identical signatures + Terminal-sink behavior; only eval/source
  get the threaded sink. The full existing suite (incl. nvm.sh sourcing, traps,
  `command`, redirects on functions) must stay green.
- **Job control fixed as a consequence.** Threading a `Capture` sink makes
  `matches!(sink, Terminal)` false → no interactive job control inside `$()` → no
  tty deadlock. A TOP-LEVEL `eval 'external'` keeps a `Terminal` sink → job
  control still engages (bash-correct).
- **Redirect wins over capture** (v125 invariant): `with_redirect_scope` forces a
  Terminal inner sink when a stdout redirect is present, so `x=$(eval 'echo y'
  >f)` writes `y` to `f` and captures nothing — matching bash.
- **L-25 residual unchanged**: `$(eval 'cmd' 2>&1)` capturing the body's stderr
  via the in-memory buffer is still bounded by huck's non-forking `$()` (a
  pre-existing limitation, not introduced or fixed here).

## Files & responsibilities

| File | Change |
|------|--------|
| `src/executor.rs` | `execute_with_sink` (refactor `execute`); `eval_in_sink`/`source_in_sink`; the eval/source dispatch arm in `run_exec_single`. |
| `src/shell.rs` | `process_line_in_sink` (refactor `process_line`). |
| `src/builtins.rs` | `run_sourced_contents_in_sink` (refactor `run_sourced_contents`); `builtin_eval`/`builtin_source` delegate to the new helpers; make `resolve_source_path`/source-core reachable for `source_in_sink`. |
| `tests/eval_source_sink_integration.rs` (NEW) | capture + redirect exact-byte tests. |
| `tests/eval_source_hang_pty.rs` (NEW) | PTY regression: `x=$(eval 'seq 1 500000')` and `x=$(eval 'cat')` complete, not hang. |
| `tests/scripts/eval_source_sink_diff_check.sh` (NEW) | bash-diff harness. |
| `docs/bash-divergences.md` | No open M-/L- entry exists for this; record in iteration history. Add a deferred entry ONLY if a residual is found. |

## Testing

1. **Integration `#[test]`s** (`tests/eval_source_sink_integration.rs`), exact bytes:
   - `x=$(eval 'echo hi'); echo "[$x]"` → `[hi]`
   - `x=$(eval 'echo a; echo b'); echo "[$x]"` → `[a\nb]` (multi-command)
   - `printf 'echo S\n' > f; x=$(source f); echo "[$x]"` → `[S]`
   - `eval 'echo R' > f` → file `f` contains `R\n` (redirect honored)
   - `eval 'echo E' 2> f` (stderr redirect on eval) → matches bash
   - top-level `eval 'echo top'` → prints `top` (Terminal path unaffected)
   - `x=$(eval 'seq 1 100' | wc -l)` → `[100]` (pipe inside eval-in-capture)
   - regression: `command eval 'echo c'` → `c`; a function named `eval` still
     shadows (define `eval(){ echo fn; }; eval x` → `fn`).
2. **PTY hang regression** (`tests/eval_source_hang_pty.rs`, expectrl/OsSession,
   mirror `subshell_tty_pty.rs`): interactively run `x=$(eval 'seq 1 500000'); echo
   "L=${#x} DONE"` → DONE arrives (no hang), and `L` is the captured length (>0).
   `x=$(eval 'printf "%0.sX" $(seq 1 200000)'); echo DONE2` → DONE2 arrives. Skip
   gracefully without a PTY.
3. **Bash-diff harness** `tests/scripts/eval_source_sink_diff_check.sh`: the
   capture + redirect + top-level cases above, byte-identical bash↔huck.
4. **Full regression:** entire unit + integration suite and ALL existing harnesses
   green (especially anything exercising `source`, `eval`, traps, `command`,
   redirects-on-functions, and nvm.sh sourcing); clippy clean.
5. **nvm payoff (best-effort, needs network):** `nvm ls-remote` via `~/.nvm/nvm.sh`
   in a PTY no longer hangs and lists versions. Report; if no network, state so and
   rely on the synthetic PTY regression.

## Edge cases & notes
- `eval`/`source` with NO redirect inside `$()` → `inner_sink == sink ==
  Capture(buf)` → captured. At top level → `Terminal` → printed + job-control for
  externals (correct).
- `with_redirect_scope` already flushes stdout before swapping fds and restores on
  every exit path (v125) — reused as-is, no new fd bookkeeping.
- `source_depth`/recursion cap and positional-args save/restore must be preserved
  exactly in `source_in_sink` (copy from `builtin_source`).
- No change to `in_subshell`/`in_completion` semantics; the fix works purely
  through the sink (a `Capture` sink already disables interactive job control).
