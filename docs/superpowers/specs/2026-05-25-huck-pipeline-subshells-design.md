# v25: Pipelines as Subshells — Design Spec

## Goal

Allow ANY `Command` (simple, compound, function call, function-def, brace
group) as a pipeline stage, with each stage running in its own forked
subshell so side effects are isolated per stage. Closes M-10 (functions
as pipeline stages) and the v17/v18-era "compound commands in pipelines"
limitation in one stroke, and fixes I-04 (builtins-in-pipelines affect
parent) as a deliberate semantics shift.

Pre-v25:
- `cmd | myfunc` — `myfunc` is dispatched as an external and fails with
  "command not found" (function table doesn't survive the std::process
  fork+exec).
- `if true; then cat; fi | grep foo` — parse error; `Pipeline.commands`
  only holds `SimpleCommand`.
- `cd /tmp | echo hi` — `cd` runs in the parent shell, mutates parent's
  cwd (I-04 divergence from bash).

After v25:
- `cmd | myfunc` — `myfunc` runs in a forked subshell with the parent's
  function table inherited via fork. Output flows through the pipe.
- `if true; then cat; fi | grep foo` — the `if` runs in a forked
  subshell; `cat` reads from the pipe; `grep` reads `cat`'s output.
- `cd /tmp | echo hi` — `cd` runs in a forked subshell; parent's cwd is
  unchanged. Matches bash.

## Scope

User-selected: **all compound commands + functions** as pipeline stages,
with **all stages** (including simple-command builtins) running in
forked subshells. Subshell-syntax `(list)` (M-11) is NOT in scope —
separate iteration.

| Pipeline stage shape | Pre-v25 | Post-v25 |
| --- | --- | --- |
| External command (`cat`, `grep`) | fork+exec via std::process — already in a subshell | unchanged (path A) |
| Builtin command (`cd`, `echo`, `test`) | runs in parent shell, side effects persist (I-04) | fork+in-process subshell, side effects local (path B) |
| Function call | dispatched as external, fails with "not found" | fork+in-process subshell, function body runs (path B) |
| `if`/`while`/`for`/`case` | parse error | fork+in-process subshell, compound body runs (path B) |
| `{ … }` brace group | parse error | fork+in-process subshell (path B) |
| Function definition (`f() {…}`) | parse error in pipelines | fork+in-process subshell; function is registered in the child only (lost on _exit), then exit 0 (path B) |

Single-command "pipelines" (one stage, no `|`) keep the existing path:
no fork, run in parent. Only multi-stage pipelines invoke the new
subshell machinery.

## Semantic model

**Side-effect isolation per stage**: every stage runs in a forked process
that exits when its body finishes. Variable assignments, `cd`, `export`,
`unset`, function definitions, inline assignments, and any other
mutations to shell state are local to the subshell. Per POSIX 2.12 and
bash exactly.

**Inheritance via fork**: the child inherits the parent's full shell
state at fork time — vars, exported env, functions, positional
parameters, last status, jobs table (though job-table operations in the
child are meaningless), history (read-only — the child shouldn't
persist). This is what fork-via-libc does for free.

**`exit N` in a pipeline stage**: exits the SUBSHELL with N (after
masking to 8 bits per B-05), not the parent. The parent sees N as the
stage's exit status via `waitpid`.

**`return N` in a function pipeline stage**: per POSIX, `return` is only
meaningful inside a function; the v22 work already maps
`ExecOutcome::FunctionReturn(N)` to the function's exit. In a subshell,
that `N` becomes the subshell's exit status.

**`break`/`continue` in a pipeline-stage subshell**: stray when not
inside a loop AT the subshell level — neutralized to exit 0, matching
the REPL's I-03 behavior. If the stage IS a loop (`while …; do …; done
| cat`), `break` exits the loop normally and the subshell's exit status
is the loop's last-iteration status.

**Pipeline exit status**: unchanged — last stage's exit (POSIX). The
existing B-09 `wait_pgrp_pipeline` helper handles this correctly without
modification.

**Job control**: subshell stages join the same process group as
external stages (the pipeline's pgrp leader). Ctrl-Z stops the entire
pipeline; `fg` resumes. The existing B-09 stop-handling path applies.

## AST changes

`src/command.rs`:

```rust
pub struct Pipeline {
    // BREAKING CHANGE (v25): was Vec<SimpleCommand>; now Vec<Command> so
    // any Command type can be a pipeline stage. A stage that is itself a
    // Command::Pipeline is rejected at parse time (POSIX disallows
    // `a | (b | c)` syntax at the pipeline-stage level; pipelines compose
    // at the sequence level only).
    pub commands: Vec<Command>,
}
```

Every consumer that destructured `&pipeline.commands[i]` as
`SimpleCommand` needs to widen to `Command` and handle the new variants.
Most consumers are in `src/executor.rs` and tests.

## Parser changes

`src/command.rs::parse_pipeline_with_first` (and any helper that
collects pipeline stages):

- The "first stage" already comes from `parse_command` which produces a
  `Command`. Today it's wrapped into `Pipeline { commands: vec![simple] }`
  via some intermediate extraction. Post-v25, the wrap happens directly:
  `Pipeline { commands: vec![first_command] }`.
- After `|`, parse the next stage via `parse_command` (which already
  handles all Command types). Reject `Command::Pipeline(_)` as a stage
  with a clear error (`ParseError::NestedPipeline` or similar).
- Redirects on compound-command stages already parse correctly (the
  compound carries its own redirects via the simple-command in its body
  or via the BraceGroup/etc. wrapping). No new redirect logic.
- Function defs as stages parse fine (consistency with bash). Harmless.

## Fork infrastructure

`src/executor.rs` gains:

```rust
/// Forks a subshell and runs `cmd` in the child with the supplied
/// stdio fds dup2'd to 0/1/2 (and the originals closed). After the body
/// runs, the child `_exit`s with the resulting status. Returns the
/// child pid in the parent.
///
/// Async-signal-safety: between fork and the child's first non-trivial
/// work (dup2 + close + signal-reset), only async-signal-safe calls
/// happen. huck is single-threaded so allocations during the subsequent
/// `execute` call are fine.
fn fork_and_run_in_subshell(
    cmd: &Command,
    shell: &mut Shell,
    stdin_fd: RawFd,
    stdout_fd: RawFd,
    stderr_fd: RawFd,
    pgid_target: i32,        // 0 = become own pgrp leader; >0 = join this pgrp
    parent_fds_to_close: &[RawFd],  // pipe ends that exist in parent that this child must close
) -> Result<i32, io::Error>;
```

**Child-side work** (after `libc::fork() == 0`):

1. `reset_job_control_signals_in_child()` (existing helper).
2. `libc::setpgid(0, pgid_target)`.
3. `libc::dup2(stdin_fd, 0)`, `dup2(stdout_fd, 1)`, `dup2(stderr_fd, 2)`.
4. Close each of `stdin_fd`/`stdout_fd`/`stderr_fd` if it's not 0/1/2
   (avoid closing the dup'd fd; only close originals).
5. Close every fd in `parent_fds_to_close` (pipe ends the parent holds
   that this child must NOT keep open — otherwise readers downstream
   would never see EOF).
6. Run the body via the existing `execute_command(cmd, shell, sink)` or
   equivalent. The shell state was forked from the parent; vars,
   functions, positionals, last_status are all available.
7. Translate `ExecOutcome` to i32:
   - `Continue(c)` / `Exit(c)` → `c & 0xFF`
   - `LoopBreak` / `LoopContinue` → 0 (stray-in-subshell, neutralize)
   - `FunctionReturn(n)` → `n & 0xFF`
8. `libc::_exit(status)`. NOT `std::process::exit` — that runs Rust
   destructors which can do weird things with file handles inherited
   from the parent.

**Parent-side work** (after `libc::fork() > 0`):

1. `libc::setpgid(child_pid, pgid_target)` defensively.
2. Return the child pid to the caller.

The caller (the rewritten `run_multi_stage` loop) is responsible for:
- Closing the parent's end of any pipe the child took (so EOF propagates
  correctly when the child writes finish).
- Recording the pid for the eventual `wait_pgrp_pipeline` call.

## Per-stage execution

`run_multi_stage` is restructured around raw pipe fds and per-stage
dispatch.

**Stage classification:**

```rust
enum StageKind<'a> {
    /// A single SimpleCommand::Exec whose program resolves to an
    /// external binary (not a function, not a builtin).
    External(&'a SimpleCommand),
    /// Everything else — builtin, function, compound, function-def.
    /// Runs via fork_and_run_in_subshell.
    InProcess(&'a Command),
}

fn classify_stage(cmd: &Command, shell: &Shell) -> StageKind<'_> {
    match cmd {
        Command::Pipeline(p) if p.commands.len() == 1 => {
            if let SimpleCommand::Exec(exec) = &p.commands[0] {
                let prog = exec.program_text();   // best-effort static resolution
                if !shell.functions.contains_key(&prog) && !builtins::is_builtin(&prog) {
                    return StageKind::External(&p.commands[0]);
                }
            }
        }
        _ => {}
    }
    StageKind::InProcess(cmd)
}
```

Note `program_text` is best-effort: if the program word is dynamic
(`$cmd args`), classification falls through to InProcess (which still
works — the subshell will resolve the program at execution time).
Slightly less efficient (one extra fork) but correct.

**Pipe management** — switch the whole loop to raw fds:

- Before the loop: no pipes.
- For each stage i: create pipe `(read_i, write_i)` if `i < N - 1`.
  - Stage i's stdout is `write_i` (or the inherited stdout if `i == N - 1`
    and no `>file` redirect).
  - Stage i+1's stdin is `read_i` (or the inherited stdin if `i == 0`
    and no `<file`/`<<heredoc` redirect).
- After spawning stage i, parent closes `write_(i-1)` (the previous
  pipe's write end is now in stage i; parent doesn't need it).
- After spawning stage N-1 (the last), parent closes `read_(N-2)`.
- For each child fork, the `parent_fds_to_close` argument lists EVERY
  pipe fd currently held by the parent that the child shouldn't keep
  open. Critical for EOF propagation.

**Stage spawn dispatch:**

```rust
let pid = match classify_stage(stage_cmd, shell) {
    StageKind::External(simple) => {
        spawn_external_with_fds(simple, shell, stdin_fd, stdout_fd, stderr_fd, pgid_target, &parent_fds)?
    }
    StageKind::InProcess(cmd) => {
        fork_and_run_in_subshell(cmd, shell, stdin_fd, stdout_fd, stderr_fd, pgid_target, &parent_fds)?
    }
};
```

`spawn_external_with_fds` is the existing `std::process::Command::spawn`
path, adapted to take raw stdio fds (via `Stdio::from_raw_fd`). Existing
external-command behavior preserved.

**Inline assignments around each stage** (v23 per-stage scoping):
unchanged in structure. Apply in parent before fork (so child inherits
via fork); restore in parent immediately after spawn (so next stage
doesn't see them). For path B, the child's copy of `apply`-modified
state runs the body; the child's restore would be a no-op since
`_exit` discards everything anyway.

## Pipe-fd plumbing rewrite

Today `run_multi_stage` uses `Carry::ChildStdout(ChildStdout)` and
`Carry::Buffer(Vec<u8>)` for inter-stage data, mixed with `Stdio::piped()`
on `std::process::Command`. Post-v25, this becomes:

```rust
let mut prev_pipe_read: Option<RawFd> = None;
let mut parent_holds: Vec<RawFd> = Vec::new();  // fds parent must close eventually

for i in 0..n {
    let is_last = i == n - 1;

    // Set up this stage's stdin.
    let stdin_fd = if let Some(redir) = stage_explicit_stdin(stage_cmd) {
        open_redirect_to_raw_fd(redir, shell)?
    } else if let Some(pr) = prev_pipe_read.take() {
        pr
    } else {
        libc::STDIN_FILENO
    };

    // Set up this stage's stdout.
    let stdout_fd = if let Some(redir) = stage_explicit_stdout(stage_cmd) {
        open_redirect_to_raw_fd(redir, shell)?
    } else if !is_last {
        let (r, w) = pipe_pair()?;
        prev_pipe_read = Some(r);
        parent_holds.push(r);     // parent will close after spawning next stage
        parent_holds.push(w);     // parent will close after spawning this stage
        w
    } else {
        libc::STDOUT_FILENO
    };

    let stderr_fd = stage_explicit_stderr(stage_cmd)
        .map_or(libc::STDERR_FILENO, |r| open_redirect_to_raw_fd(r, shell));

    // Spawn (parent_fds_to_close = parent_holds minus this stage's stdin/stdout).
    let pid = spawn_stage(stage_cmd, stdin_fd, stdout_fd, stderr_fd, ...)?;

    // Close in parent: stdout_fd (the pipe write end this stage took)
    // and stdin_fd if it was a pipe read end from the previous iteration.
    close_in_parent(&[stdout_fd, /* prev pipe read if was-pipe */]);
}
```

The exact bookkeeping is a single-author concern; the implementer
should track it carefully. Use `tempfile`-like RAII where possible
(custom `OwnedFd` wrappers) so a panic mid-loop doesn't leak fds.

## Edge cases

- **Single-stage "pipeline"** (`cmd` alone): unchanged. No fork beyond
  what `run_exec_single` already does for externals. Builtins still run
  in-process in the parent (NO subshell). The "subshell every stage"
  rule applies only to multi-stage pipelines.

- **All-builtin pipeline** (`cd /tmp | pwd`): both stages fork. `cd /tmp`
  runs in subshell, parent cwd unchanged. `pwd` reports… the parent's
  cwd (since the `pwd` stage also runs in its own subshell, which
  inherits parent's cwd at fork time, BEFORE the `cd` subshell ran).
  Matches bash exactly.

- **Function returning before pipe drains** (`shortfn | cat`): the
  function exits early, its subshell exits, the write end of the pipe
  closes, `cat` sees EOF and exits. Normal POSIX behavior.

- **`exit N` in a non-last stage** (`exit 5 | cat`): the first stage's
  subshell exits 5; pipeline status (from the last stage) is `cat`'s
  exit (typically 0 if it read EOF cleanly). The parent shell does NOT
  exit — `exit` in a subshell exits only the subshell.

- **Stray `break`/`continue`/`return` in a subshell stage**: neutralized
  to exit 0 (matching the REPL's I-03). Documented.

- **Function-def as a pipeline stage** (`f() {…} | cat`): the function
  registers in the subshell's function table, subshell exits 0, parent
  never sees the function. Cat reads nothing. Harmless.

- **Inline assignments + function pipeline stage** (`FOO=hi myfunc |
  cat`): v23's per-stage scoping applies the assignment in the parent
  before fork; the child inherits FOO=hi via fork; the function runs in
  the subshell with FOO visible. Parent's FOO is restored after spawn
  (so the next stage doesn't see it). Even if myfunc modifies FOO,
  that's lost on _exit.

- **Heredoc + compound pipeline stage** (`if true; then cat; fi <<EOF
  | grep foo<NL>body<NL>EOF`): heredoc attaches to the if-stage via the
  v24 path. The subshell child gets the heredoc body as stdin via the
  v24 `pending_input` plumbing, BEFORE fork (parent writes the bytes
  into the pipe end the child will read from). Compose cleanly.

- **Job control on a multi-subshell pipeline**: Ctrl-Z sends SIGTSTP to
  the pgrp; every subshell child gets it and stops. Existing B-09
  pgrp-wait handles this — no changes.

- **`set -e` in a subshell stage**: huck doesn't have `set -e` yet
  (deferred). When it lands, errexit propagates the same way as exit
  status — failure causes subshell `_exit`, parent sees the status.

- **History recursion**: subshell children don't write to history
  (single in-memory `History` was forked, but Drop-via-_exit skips
  destructors so no save happens). Parent's history is untouched.

## Out of scope

- **Subshell syntax `(list)`** — M-11, separate iteration. Same fork
  mechanism, but needs explicit `(` `)` lexer/parser work.
- **`set -e`/`set -u`/`set -o pipefail`** — POSIX shell options; M-08
  and M-50, separate iteration. After v25, `set -o pipefail` becomes
  feasible (each stage's exit is already individually tracked in
  `wait_pgrp_pipeline`).
- **`$PIPESTATUS` array** — bash-only; same dependency on per-stage
  status tracking. Out of scope.
- **Heredoc into the first stage of a pipeline** — already works post-v24,
  but the heredoc body is written to the pipe via `pending_input`; verify
  the new fd plumbing doesn't break it.

## Tests

### Parser (`src/command.rs::tests`)

| Test | Covers |
| --- | --- |
| `parse_pipeline_with_if_stage` | `echo hi \| if true; then cat; fi` → `Pipeline { commands: [Pipeline(echo hi), If(...)] }` |
| `parse_pipeline_with_function_call_stage` | `echo hi \| myfunc` (myfunc undefined here — that's runtime; parser accepts) |
| `parse_pipeline_with_brace_group_stage` | `echo hi \| { cat; }` |
| `parse_pipeline_with_while_stage` | `seq 1 3 \| while read x; do echo got $x; done` (won't run without `read` builtin, but parser accepts) |
| `parse_pipeline_with_function_def_stage` | `echo hi \| f() { :; }` (rare but harmless; parses) |
| `parse_pipeline_rejects_nested_pipeline_stage` | `echo \| (echo \| cat)` — parser error (subshell syntax out of scope, and nested-pipeline form rejected explicitly) |

### Fork infrastructure (`src/executor.rs::tests` or new unit tests)

| Test | Covers |
| --- | --- |
| `fork_and_run_in_subshell_simple_builtin` | Fork, run `echo hi` builtin, parent reads "hi\n" from a pipe |
| `fork_and_run_in_subshell_function_call` | Define myfunc returning 7, fork, run it, parent sees exit 7 via waitpid |
| `fork_and_run_in_subshell_inherits_vars` | Set FOO in parent, fork, child runs `echo $FOO`, parent reads "value" |
| `fork_and_run_in_subshell_side_effects_dont_leak` | Subshell sets BAR, parent's BAR remains unset after subshell exits |
| `fork_and_run_in_subshell_exit_n_masks_to_8_bits` | Subshell runs `exit 300`, parent sees exit 44 (B-05 reuse) |

### Pipeline integration (`tests/pipeline_subshell_integration.rs`, new)

| Test | Script | Expected |
| --- | --- | --- |
| `pipeline_function_call_as_stage` | `myfunc() { while read x; do echo got:$x; done; }; printf 'a\nb\nc\n' \| myfunc` | (read isn't built — use a no-read function: `myfunc() { cat \| sed s/^/got:/; }; echo hi \| myfunc` — pick what works) |
| `pipeline_if_clause_as_stage` | `echo hi \| if true; then cat; fi` | `hi` |
| `pipeline_while_loop_as_stage` | `seq 1 3 \| while read x; do …` — defer if no `read` |
| `pipeline_brace_group_as_stage` | `echo hi \| { cat; }` | `hi` |
| `pipeline_function_def_as_stage_is_noop` | `echo hi \| f() { :; }` → exit 0, no output (cat'd nothing) |
| `pipeline_builtin_side_effect_does_not_leak` | `pwd; cd /tmp \| true; pwd` | (first pwd) same as (second pwd) — cd was scoped |
| `pipeline_var_assignment_does_not_leak` | `FOO=outer\nFOO=inner true \| cat\necho $FOO` | `outer` |
| `pipeline_exit_in_first_stage_does_not_exit_shell` | `exit 5 \| cat\necho still-here` | `still-here` |
| `pipeline_compound_with_redirect` | `if true; then cat; fi <<EOF \| grep foo<NL>foo<NL>bar<NL>EOF` | `foo` |
| `pipeline_function_inherits_inline_assignment` | `myfunc() { echo got:$FOO; }; FOO=hi myfunc \| cat` | `got:hi` |
| `pipeline_three_stages_compound_middle` | `echo hi \| { sed s/h/H/; } \| cat` | `Hi` |
| `pipeline_pgrp_stop_resume` (PTY) | start `cat \| if true; then sleep 1; fi`, Ctrl-Z → stopped notification, `fg` resumes | (PTY test) |

### History / docs

| Test | Covers |
| --- | --- |
| `bash-divergences.md` updates | M-10 → fixed, I-04 → fixed (no longer divergent), change-log entry |
| `README.md` v25 status row | Pipelines as subshells |

## Change log

- **2026-05-25**: Spec drafted; user-chosen scope = all compound commands
  + functions + simple-command subshell semantics (fixes I-04); libc::fork
  direct, hybrid path (std::process for externals, libc::fork for everything
  else).
