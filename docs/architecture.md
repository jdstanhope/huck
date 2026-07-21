# huck Architecture Overview

This is a one-page map of how huck is structured, intended as the
starting point for anyone (human or LLM) extending the shell. It
covers the load-bearing types, the execution pipeline, the
iteration workflow, and where to add common new features. For the
list of pending bash-compat work see the GitHub issues labelled
`divergence`; `docs/bash-divergences.md` holds the intentional ones.
For the iteration history see `git log` + the long-running memory notes
(`project_huck_iterations.md`); there is no per-version table.

## Module map

The repo is a **4-member Cargo workspace** (v202 → v203), a layered stack with a
compiler-enforced acyclic dependency direction `syntax ← engine ← cli ← bin`:

- **`huck-syntax`** (`crates/huck-syntax/`) — the Shell-free **frontend**: `lexer`
  (the incremental atom-emitting `Lexer`), `parser` (the parser — pulls from the
  lexer, owns delimiter-matching), `command` (AST types), `brace_expand`,
  `generate` (AST→source), plus
  `errors.rs` (the crate-private message renderers behind the `Display` impls)
  and `util.rs`
  (`escape_double_quote_value`). No dependencies. As of v211 it ships polished
  public-API ergonomics (`Display` + `std::error::Error` on the error types,
  `#[non_exhaustive]` on the AST enums, `try_split_assignment_ref` peek variant,
  curated root re-exports + module-level doc with a runnable Quick example) and
  is publication-ready as a standalone crate.
- **`huck-engine`** (`crates/huck-engine/`) — the **terminal-free execution
  core**: expansion, execution, builtins, shell state, traps, jobs, completion
  candidate-generation, the readline keymap *data*, and the low-level headless
  runners (`huck_engine::shell::run_program` / `process_line`). Depends on `huck-syntax`;
  **rustyline-free** (a stray `use rustyline` here won't compile) — it is the
  embeddable, no-terminal interpreter. The embedding entry point is
  `huck_engine::Engine` (`new` / `builder` → `run` / `capture` / `run_file` +
  `var` / `set_var` / `set_args`), and the `huck` binary's headless `-c` / script
  path runs through it (`run` = `bash -c` semantics, `run_file` / `run_script` =
  script semantics). The advanced embedding path is `huck_engine::ExecBuilder`
  returned from `Engine::prepare(src)` — it supports stdin feed (`.stdin(bytes)`)
  and stderr-as-merged into stdout (`.merge_stderr()`), then runs either as
  `.run() -> i32` (fd 1/2 inherit) or `.capture() -> Output { stdout, stderr,
  exit_code }` (both buffers populated). Internally,
  `huck_engine::StderrSink::{Terminal, Merged, Capture}` is the symmetric
  counterpart of `StdoutSink`, threaded through the executor and the
  builtin-dispatch path; engine-level stdin redirection lives in
  `crates/huck-engine/src/stdin_pipe.rs` (`child_fd::make_pipe(true)` — a
  >=3-relocated, CLOEXEC pipe — + dup2(r, 0) save/restore guard). Sandbox knobs
  (v206) layer on top: `.cwd(path)` chdirs for the call
  (RAII via `cwd_scope.rs`, snapshotting OS cwd + shell `PWD`/`OLDPWD`);
  `.restricted()` selects `Policy::Sandbox` (v319; see below); `.timeout(dur)` spawns a timer thread
  (`timeout.rs`) that, on deadline, sets `Shell.timeout_flag` (polled by
  `executor::check_interrupt`) and SIGTERMs every pid in
  `Shell.live_external_children`, with the call returning exit 124.
  `ExecOutcome::Interrupted` carries an `InterruptReason::{Sigint,Timeout}`
  discriminator so the top-level reducer can map to 130 (SIGINT) or 124
  (timeout).
  Streaming callbacks (v207) layer on top: `.on_stdout_line(|line| …)` and
  `.on_stderr_line(…)` fire per complete line, on the embedder's thread (no
  `Send` bound), in real time even for external processes. Internally, builtin
  writes go through a thread-local `Callbacks` pointer that line-buffers via
  `line_buf.rs`; external-process waits use a new poll-based loop
  (`stream_loop.rs` + `wait_loop.rs` — `signalfd`/`poll` on Linux, `kqueue` on
  macOS) that replaces v205/v206's blocking `waitpid` + drainer-thread.
  Callbacks tee with `.run()` and `.capture()` — output still reaches the
  embedder's terminal / `Output.stdout` buffer in addition to firing events.
  Inner-capture scopes (command substitution `$(...)` and backticks)
  suspend callback dispatch via `callbacks_thread_local::suspend()` so the
  substitution's captured bytes don't leak to the embedder's view.
  Completion (v208) is exposed as `Engine::complete(line, cursor) -> Completion
  { start, candidates }`. `Candidate` gains a `kind: CandidateKind` tag
  (`Command` / `Variable` / `File` / `Directory` / `Custom`) so IDE / TUI
  embedders can render icons or sort by kind. Thin wrapper over the existing
  `completion::dispatch::resolve` internal API; `&mut self` because
  `complete -F func` callbacks may mutate shell state. The CLI's `HuckHelper`
  rustyline adapter drops `kind` (rustyline has no kind concept) — REPL
  behavior unchanged.
  Completion context (#248) is derived from `huck_syntax::parse_recover`'s
  `CursorContext` (see `recover.rs` below) rather than a hand-rolled character
  scanner: `dispatch::resolve` calls `parse_recover(&line[..pos])` and maps the
  returned `CursorContext { enclosing, position, word, word_start }` to a
  completion source via `cursor_to_completion` (`completion.rs`). Because the
  context now comes from the real (error-recovering) parser instead of a
  scanner re-deriving structure by hand, completion is correct inside nested
  constructs by construction — this fixed divergences in command-substitution
  bodies, arithmetic contexts, compound-command condition/list positions, and
  redirect-target words, where the old scanner produced no completion at all.
  Restricted-mode (v319) is a **policy abstraction**, `crates/huck-engine/src/policy.rs`:
  `Shell.policy: Policy` is `Unrestricted` (default) / `Rbash` / `Sandbox`, and
  every enforcement site is one line, `shell.policy.check(op)?`, where `op` is
  a variant of the `Op` enum (`Cd`, `Exec`, `CommandName`, `SourcePath`,
  `RedirectFile`). `Policy::Unrestricted` returns `Ok(())` from the first match
  arm, so the permitted path costs one branch. `Rbash` mirrors bash's `rbash`
  exactly (verified against 5.2.21); `Sandbox` is huck's embedding policy
  (`ExecBuilder::restricted()`) and differs in exactly one place — it denies a
  file-target redirect only when the target escapes the working directory
  (absolute path or a `..` component), so a sandboxed script can still write
  its own relative files, where `Rbash` denies every file-target redirect.
  Variable restriction is **not** a per-site check: entering a restricted
  policy marks `SHELL`/`PATH`/`HISTFILE`/`ENV`/`BASH_ENV` readonly via
  `Shell::mark_readonly`, so every write path (plain assignment, `+=`,
  `export`, `read`, `declare`, `unset`) reports through ordinary readonly
  machinery with that path's own wording — the old `restricted.rs` covered
  only plain assignment. Entry points: `argv[0] == "rbash"`, `-r` at
  invocation, and `set -r` at runtime; `set +r` is refused via `set`'s
  existing invalid-option path while restricted (one-way, matching bash) but
  succeeds at rc 0 in a normal shell. `shopt restricted_shell` reports
  **provenance** (was restriction entered at startup — `-r`/`rbash`/
  `ExecBuilder::restricted()` — vs. via a later `set -r`), tracked separately
  as `Shell.restricted_at_startup`, not current policy state. `restricted.rs`
  is deleted. See `docs/superpowers/specs/2026-07-20-restricted-policy-design.md`.
- **`huck-cli`** (`crates/huck-cli/`) — the interactive **REPL** (`run` + the
  rustyline `Editor` loop) and the line-editor *adapters*: the `HuckHelper`
  completer (`Candidate`→`rustyline::Pair`) and the readline apply
  (`parse_keyseq`/`function_to_cmd`). Depends on `huck-engine` + `rustyline`.
- **`huck`** (root) — a thin **binary**: `main.rs` → `huck_cli::run(args)`.

`huck-engine/src/lib.rs` re-exports the frontend (`pub use huck_syntax::{lexer,
command, …}`) so `crate::lexer::`/`crate::command::` paths inside the engine
resolve unchanged. **Run the suite per-crate, single-threaded** —
`cargo test -p huck-syntax --lib` / `cargo test -p huck-engine --lib`
(a bare `cargo test` from the root only runs the `huck` *bin* package's
integration tests, not the member crates' unit tests). Avoid
`cargo test --workspace` on memory-constrained machines: the parallel fan-out
can OOM; add `--jobs 1 -- --test-threads 1` under an `ulimit -v` guard there.
Build the `huck` binary with `cargo build -p huck`.

Two-tier layout within the engine: lexer/parser/AST (the `huck-syntax` crate) at
the bottom, expansion + execution above, builtins at the top.

| Module | Responsibility |
|---|---|
| `lexer.rs` | The incremental `Lexer`: pulls one token/word-part atom at a time. Owns the `Word` / `WordPart` / `SubscriptKind` types. Driven by a parser-controlled **mode stack** (`Mode::Command`/`ParamExpansion`/`CommandSub`/…); it emits *small atoms* and **never forward-scans for a matching delimiter** — the parser owns all delimiter-matching and recursion (front-end is strictly one-way, `parser → lexer`, since v268; the old fat-lexer "oracle" was deleted in v266). |
| `parser.rs` | The parser. Pulls live from a `&mut Lexer` and assembles both **words** (subscripts, param-expansions, array literals) and **structure** into the `command.rs` AST. Entry points `parse_sequence` / `parse_one_unit`. Owns delimiter-matching + recursion (nested `$( )`, `${ }`, `( )`, subscripts). |
| `recover.rs` | Error-recovery parse alongside strict `parse`/`parse_sequence`: `huck_syntax::parse_recover` takes a line truncated at the cursor (e.g. a completion prefix), synthesizes the minimal closers for any open construct instead of erroring on the unterminated tail, and returns a `RecoveredParse { tree, cursor: CursorContext }` — a walkable (possibly partial) tree plus the cursor's enclosing frames/word/position. For completion (iteration 2) and future editor tooling; strict `parse` is unchanged. |
| `command.rs` | AST types: `Sequence`, `Pipeline`, `Command`, `SimpleCommand`, `ExecCommand`, `RedirectSlot`/`Redirection`, `AssignTarget`, `Assignment`, `DeclArg`, `IfClause` / `WhileClause` / `ForClause` / `CaseClause`, `TestExpr` (for `[[ ]]`) + assignment-word helpers (`try_split_assignment`, `is_assignment_word`). |
| `shell_state.rs` | `Shell` struct (all session state) + `Variable` + `VarValue` (`Scalar`/`Indexed`/`Associative`) + `ShellOptions` + `AssignErr` / `DeclareErr`. Snapshot/restore primitives (`snapshot_var`/`restore_var`, `snapshot_for_local_scope`). |
| `expand.rs` | Word → Field expansion pipeline. Owns `Field`, `expand`, `emit_split_fields` (IFS-driven), `eval_subscript` (arith), `eval_subscript_key` (string), `slice_word_list`, `expand_array_param`, `expand_assoc_param`. |
| `param_expansion.rs` | Modifier-aware parameter expansion (`${var:-w}`, `${#var}`, `${var/pat/repl}`, `${var^^}`, etc.). `ExpansionResult` enum (`Value` / `Empty` / `Fatal` / `WordList`) + `ParamLookup` enum (`Scalar` / `Element(Option<&str>)`). |
| `executor.rs` | Walks `Command` / `Sequence` / `Pipeline` trees. `run_*` functions per AST shape. `apply_one_assignment` + `apply_inline_assignments` + `restore_inline_assignments`. Pipeline fork/exec; subshell forking for compound stages. ERR-trap + errexit wire-in via `maybe_errexit`. Child stdio for forked stages/subshells is carried by `ChildStdio`/`ChildFd` (see `child_fd.rs`). |
| `child_fd.rs` | `ChildFd { Inherit, Owned(OwnedFd) }` + `ChildStdio` — the owned "fd environment a child starts with", consumed by the two spawners. RAII (close-on-drop) replaces the old raw-fd-number sentinels (fd-plumbing Phase 1). |
| `builtins.rs` | All builtin functions + dispatch. `BUILTIN_NAMES` array, `is_special_builtin`, `is_declaration_command`, `run_builtin`, `run_declaration_builtin` (DeclArg-aware path for `declare`/`local`/`readonly`/`export`). `format_declare_line`, `parse_subscripted_arg`. |
| `arith.rs` | Arithmetic expression parser + evaluator (`$((expr))`, integer-attr coerce, subscript evaluation). |
| `test_builtin.rs` | `test` / `[` builtin. POSIX 0-4-arg short-form + recursive-descent grammar parser for combinators (`-a`/`-o`/`( )`). |
| `traps.rs` | Signal trap handling. EXIT/DEBUG/ERR/RETURN + real signals. `fire_*_trap` helpers. |
| `jobs.rs` / `job_spec.rs` | Background-job tracking, `%N` / `%cmd` job specifier resolution, `jobs`/`wait`/`fg`/`bg`/`kill`/`disown`. |
| `history.rs` | Command history + `!` expansion. `History` struct stores entries; `scan` handles `!!` / `!$` / `^a^b^`. |
| `prompt.rs` | PS1/PS2 escape expansion (`\u` user, `\h` host, `\w` cwd, `\\$`, `\!`, etc.). |
| `alias_expand.rs` | Alias substitution (interactive-mode only by default; `HUCK_EXPAND_ALIASES` env override). Per-input cycle protection. |
| `brace_expand.rs` | `{a,b,c}` and `{1..N}` brace expansion (runs before word-splitting). |
| `completion.rs` | Tab completion (commands, files, variables, arith-context). |
| `continuation.rs` | Multi-line input handling (backslash-newline, unclosed quotes, partial control structures). |
| `shell.rs` | Top-level CLI + REPL entry point. `process_line` (the canonical "execute string in current shell" path). |
| `lib.rs` | `huck` library crate root: declares the runtime modules (`pub mod`) AND re-exports the `huck-syntax` frontend at the crate root (`pub use huck_syntax::{lexer, command, brace_expand, generate, …}`) so `crate::lexer::`/`crate::command::` paths stay valid. Also holds the `#[cfg(test)] test_support` (`CWD_LOCK`) module. |
| `main.rs` | Thin binary shim: argv parsing + `huck::shell::run` invocation. All logic lives in the `huck` library crate. |

> Unit tests live in per-module sibling files: `<file>/tests.rs` (and, for
> files with several test modules such as `builtins.rs`/`executor.rs`, one
> `<file>/<name>_tests.rs` per module). Production symbols named in this map
> are unaffected.

## Execution pipeline

A line typed at the prompt traverses these stages:

```
input string
    │
    ▼  shell::process_line / shell::run
alias expansion           (alias_expand.rs)        — interactive only
    │
    ▼
brace expansion           (brace_expand.rs)        — {a,b}/{1..N}
    │
    ▼
parse → Sequence          (parser.rs::parse_sequence pulls from a &mut lexer.rs::Lexer)
    │                                                 — the parser drives the lexer's mode stack, assembling
    │                                                   words (quoting, $-expansions, subscripts, here-doc bodies)
    ▼                                                   AND structure into the command.rs AST
    │
    ▼
executor walk             (executor.rs::run_sequence/run_pipeline/run_command/run_exec_single)
    │   ├── inline-assignment snapshot       (apply_inline_assignments)
    │   ├── word expansion                   (expand.rs::expand → Field list)
    │   │     │
    │   │     ├── parameter-expansion        (param_expansion.rs::expand_modifier_with_value)
    │   │     ├── command-substitution       (recursive process_line for $(...))
    │   │     ├── arithmetic expansion       (arith.rs)
    │   │     ├── tilde expansion
    │   │     └── pathname expansion (glob)
    │   │
    │   ├── IFS word-splitting               (expand.rs::emit_split_fields)
    │   ├── declaration-command pre-parse    (DeclArg via try_split_assignment, then run_declaration_builtin)
    │   ├── builtin dispatch                 (builtins.rs::run_builtin)  OR
    │   └── fork+exec for external           (executor.rs)
    │
    ▼
ExecOutcome  (Continue(i32) / Exit(i32) / FunctionReturn(i32) / LoopBreak(u32, i32) / LoopContinue(u32))
```

Three points are load-bearing across multiple modules:

- **`process_line(line, shell, expand_aliases)`** in `shell.rs` is the
  canonical "execute string in current shell" path. Used by traps,
  `source`/`.`, `eval`, `PROMPT_COMMAND`, the rc file, and recursively
  by command substitution. Any new feature that wants to execute a
  string of shell code goes through it.

- **Inline assignment snapshot/restore** (`apply_inline_assignments` /
  `restore_inline_assignments` in `executor.rs`) implements POSIX's
  "`FOO=v cmd` mutates FOO only for cmd's duration" rule. The
  snapshot type is `Vec<(String, Option<Variable>)>` — full Variable
  clone via `snapshot_var`/`restore_var`, so it correctly round-trips
  arrays as well as scalars.

- **`local`-scope unwinding** (`snapshot_for_local_scope` in
  `shell_state.rs` + push/pop in `executor.rs::call_function`) gives
  each function call its own `local_scopes` frame. `local NAME` /
  `declare NAME` mutations inside a function snapshot the pre-state
  per-frame (idempotent — same name re-declared in the same call only
  snapshots once); function return restores each entry.

## Key types

These appear in many call-site signatures; learn them once.

- **`Word(pub Vec<WordPart>)`** (`lexer.rs`) — an unexpanded shell
  word. Each `WordPart` is one of: `Literal { text, quoted }`,
  `Var { name, quoted }`, `ParamExpansion { name, modifier, quoted,
  subscript }`, `CommandSub { sequence, quoted }`, `Tilde(spec)`,
  `ArrayLiteral(Vec<ArrayLiteralElement>)`, `AssignPrefix { target,
  append }` (parser-internal carrier for `+=` / subscripted LHS).
- **`SubscriptKind`** (`lexer.rs`) — `All` (`[@]`), `Star` (`[*]`),
  or `Index(Word)`. The subscript Word is evaluated either via arith
  (Indexed/Scalar) or as a string key (Associative).
- **`ParamModifier`** (`lexer.rs`) — None, Length, UseDefault,
  AssignDefault, ErrorIfUnset, AlternativeValue, StripPrefix,
  StripSuffix, Substring, Substitute, CaseConvert, IndirectKeys.
- **`AssignTarget`** (`command.rs`) — `Bare(String)` or
  `Indexed { name, subscript: Word }`.
- **`Assignment { target, value: Word, append: bool }`** —
  one assignment row. `SimpleCommand::Assign` and
  `ExecCommand.inline_assignments` both carry `Vec<Assignment>`.
- **`DeclArg`** (`command.rs`) — `Plain(String)` or
  `Assign(Assignment)`. Carries pre-parsed args for declaration
  commands so `ArrayLiteral` never reaches `expand()`.
- **`VarValue`** (`shell_state.rs`) — `Scalar(String)` /
  `Indexed(BTreeMap<usize, String>)` / `Associative(Vec<(String,
  String)>)`. `scalar_view()` gives backward-compat scalar access
  (`$a` ≡ `${a[0]}` for indexed; empty for associative).
- **`Variable { value: VarValue, exported, readonly, integer }`** —
  one shell variable.
- **`Shell`** (`shell_state.rs`) — session state: vars,
  positional_args, functions, aliases, jobs, history, traps,
  local_scopes, shell_options (errexit/nounset), command_hash,
  dir_stack, loop_depth (current loop-nesting depth, for `break`/
  `continue` N), and more. `Shell::ifs()` is the canonical IFS accessor.
- **`ExecOutcome`** (`executor.rs`) — `Continue(i32)`, `Exit(i32)`,
  `FunctionReturn(i32)`, `LoopBreak(u32, i32)` (level, terminal `$?`),
  `LoopContinue(u32)`. Propagates through short-circuit sites in
  sequences/pipelines.
- **`ExpansionResult`** (`param_expansion.rs`) — `Value(String)`,
  `Empty`, `Fatal { status }`, `WordList(Vec<String>)` (for
  `"${a[@]}"`-style multi-word results).
- **`ParamLookup<'a>`** (`param_expansion.rs`) — `Scalar` (consult
  `shell.get(name)`) or `Element(Option<&'a str>)` (caller resolved
  the array element; `None` = missing). Threaded through
  `expand_modifier_with_value` so `${a[i]:-default}` correctly fires
  the default branch on missing elements.
- **`Field`** (`expand.rs`) — one expansion output field with
  per-byte quoting info; combined into the argv `Vec<String>` after
  glob/word-split.

## Cross-cutting conventions

- **Builtins dispatch** — `BUILTIN_NAMES` lists all builtin names.
  `is_builtin(name)` says yes/no. `run_builtin(name, args, out,
  shell)` is the main entry point for non-declaration commands.
  Declaration commands (`declare`/`local`/`readonly`/`export` +
  `typeset`) route through `run_declaration_builtin(name, decl_args,
  out, shell)` with `decl_args: &[DeclArg]` — the executor pre-parses
  assignment-shaped Words via `try_split_assignment` so
  `ArrayLiteral` parts never reach normal `expand()`. The legacy
  scalar-args path through `run_builtin` is guarded by a
  `debug_assert!(!is_declaration_command(name))` tripwire.
- **POSIX special builtins** — `is_special_builtin(name)` lists the
  POSIX special set (`:`, `.`, `break`, `continue`, `eval`, `exit`,
  `export`, `readonly`, `return`, `set`, `shift`, `source`, `trap`,
  `unset`). These DO mutate parent shell state in pipelines.
- **Bash-diff harnesses** — ~160 executable `*_diff_check.sh` scripts under
  `tests/scripts/` run the same fragments through bash and huck and assert
  byte-identical output (e.g. `arrays_diff_check.sh`, `ifs_diff_check.sh`,
  `error_message_diff_check.sh`). Adding a `<feature>_diff_check.sh` is the
  gold standard for bash-compat verification. On a memory-constrained box,
  guard a sweep with `ulimit -v 1500000` + a per-harness `timeout`.
  `tests/scripts/run_diff_checks.sh` runs the whole sweep against each
  harness's default binary (most use `target/debug/huck`; a few need
  `target/release/huck`, so build both first — it never overrides `HUCK_BIN`)
  and CI runs it on every build, so a red harness fails CI. A known-failing
  case is quarantined in-harness with a self-flagging `xfail` (see
  `cmdsub_comment_diff_check.sh`, tracking #109), not dropped from the gate.
- **Bash test-suite integration** — opt-in runner at `tests/bash-test-suite/`
  that consumes upstream bash's own test suite via `$BASH_SOURCE_DIR`;
  baseline triaged at `docs/bash-test-suite-baseline.md`.
- **Test layout** — unit tests live in `#[cfg(test)] mod` blocks at
  the bottom of each source file. Integration tests are binary-driven
  scripts in `tests/*.rs` using the `run_capture` helper pattern
  (spawn `huck_binary()`, write to stdin, capture stdout/stderr/exit).
- **Error emission (the `sh_error!` family)** — since v269 EVERY error
  diagnostic goes through one emitter family in `error_emit.rs`; there are no
  hand-rolled `"huck: "` literals (an `include_str!` invariant test,
  `prologue_literal_invariant`, enforces this). All share
  `Shell::error_prefix(kind: Diag) -> String` (`shell_state.rs`, `pub(crate)`),
  which builds bash's full prologue matrix `<name>: [-c: ][line N: ][cmd: ]`.
  `Diag` is `Runtime(Option<&str> cmd)` or `Syntax { line }`; `<name>` is `$0`
  verbatim (or `"huck"` interactive); `line N:` only when non-interactive; `-c:`
  only for a syntax error under `-c` (and not sourced); `cmd:` only on runtime
  (e.g. `Some("let")`). **Pick the variant by whether a redirect-aware writer is
  in scope** (this is load-bearing — see below):
  - `sh_error_to!(shell, writer, cmd, "…")` / `emit_error_to` — writes to a
    **caller-provided writer**. MANDATORY for builtins and the executor: they
    emit through the `out`/`err` writer the executor hands them (which carries
    per-command `2>&1` and the bare-builtin `route_err_to_out` swap). A
    thread-local emit there would LEAK the diagnostic to the real stderr under
    `$(cmd 2>&1)` capture.
  - `sh_error!(shell, cmd, "…")` / `emit_error` — writes to the **thread-local
    sink** (`err_thread_local.rs::with_err`). Only for writer-less deep helpers
    (expansion, param-expansion, most `shell_state` methods) that emit in the
    outer context.
  - `emit_syntax_error(shell, line, "…")` — `Diag::Syntax`; adds `-c:` per the
    gate above.
  - `emit_cli_error(prog, "…")` — pre-shell CLI errors (`<basename>: msg`, no
    line), before a `Shell` exists.

  Arith error bodies still render via
  `arith::render_error_body(expr, err) -> String` (leading-trimmed expression +
  `(error token is "tok")`), passed as the body to `sh_error!`. The
  `tests/scripts/error_message_diff_check.sh` harness (incl. a `$(err 2>&1)`
  capture matrix) is the objective guard against a mis-routed emitter site.

### Single-threaded execution (invariant, enforced)

huck executes subshells, background jobs, and in-process pipeline stages by
`fork()`ing **without a following `exec`** — the child continues in the same
address space through `run_command`. POSIX allows only async-signal-safe calls
between `fork` and `exec` in a multithreaded process, so this is memory-safe
**only while the process is single-threaded.** huck is, in production.

This is enforced, not assumed. `exec_guard` (`crates/huck-engine/src/exec_guard.rs`)
counts active executions globally and per-thread; `execute_with_sink` holds an
`ExecActive` for its duration, and `fork_and_run_in_subshell` calls
`assert_single_threaded_fork()` before the fork. If another thread is executing,
it **panics** (citing #184) rather than let the forked child deadlock on an
inherited lock. A lone engine never trips it.

Consequences: running two `Engine`s concurrently on different threads is
unsupported and will panic at the first subshell fork. **Any test binary that
drives the in-process `Engine` API AND runs anything that forks in-process (a
`( … )` subshell, a `&` background job, a `coproc`, or a builtin as a non-last
pipeline stage) must expose ONE `#[test]` whose scenarios run sequentially** —
libtest runs multiple `#[test]`s on parallel threads, so a second `#[test]`
executing while the first forks trips the guard (this is #184 in miniature; a
*separate binary* alone does NOT help — its own two tests still race). See
`crates/huck-engine/tests/forking_execution_serial.rs`,
`foreground_wait_latency.rs`, `no_wildcard_reap.rs`. **When verifying such a
binary locally, run it at `--test-threads 2` or more — `--test-threads 1` hides
the guard because nothing runs concurrently.** The guard covers only the
fork/deadlock hazard; the cwd, signal/job-control, and fd-table state are also
process-global and would need per-engine virtualization for true multi-engine
support (out of scope; declined in the #184 design).

## Naming conventions

Function-name verbs follow these conventions (codified v170; see the 2026-06-16
naming review). New code should match them:

- **Retrieval** — `get_*` borrows a stored container (`&T`, e.g. `get_indexed`);
  `lookup_*` computes one resolved value (owned `Option<String>`, e.g.
  `lookup_var`); `resolve_*` follows indirection to a concrete target
  (namerefs, paths — e.g. `resolve_nameref`, `resolve_dir`).
- **Lexing/scanning** — `scan_*` advances a `CharCursor` and collects a bounded
  span (e.g. `scan_ansi_c_quoted`, `scan_braced_name_ext`, `scan_var_name`) —
  none forward-scan across nested structure for a matching delimiter; `split_*`
  partitions an already-collected `&str` (e.g. `split_on_sentinels`); `parse_*`
  (in `parser.rs`) produces AST/structure by pulling atoms from the incremental
  `Lexer` (there is no batch `tokenize` — the lexer yields one atom at a time).
- **Execution** — `run_*` executes an AST node/construct (`run_command`,
  `run_pipeline`); `execute*` are the public crate entry points
  (`execute`/`execute_with_sink`/`execute_capturing`); `eval_*` computes a value
  from an expression; `fire_*_trap` runs a trap.
- **Completion** — `complete_*` produces candidates; `run_spec` evaluates a
  registered compspec; `dispatch::resolve` is the top-level completion entry.
- **Options** — option structs are `*Options` (`LexerOptions`, `ShellOptions`,
  `CompOptions`); their bindings/params are abbreviated `opts`.
- **Array types** — the two array kinds use `indexed` / `associative` as the
  type adjective (matching `VarValue::Indexed`/`Associative`), e.g.
  `replace_indexed`/`replace_associative`, `set_indexed_element`/
  `set_associative_element`, `get_indexed`/`get_associative`. Avoid the bare
  noun `array` (ambiguous — both kinds are arrays).

## Iteration workflow

The project is built one numbered iteration at a time
(`v1`, `v2`, …, `v269` as of this writing). `CLAUDE.md` holds the authoritative
version of this loop; the cadence is:

0. **Take an issue.** Review the open GitHub issues labelled `divergence`
   and take (or open) the one this iteration addresses; the PR will close it.
1. **Brainstorm** the next feature via the `superpowers:brainstorming`
   skill. Produces a design doc at
   `docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md`.
2. **Plan** via the `superpowers:writing-plans` skill. Produces a
   per-task implementation plan at
   `docs/superpowers/plans/YYYY-MM-DD-<topic>.md`.
3. **Implement** via `superpowers:subagent-driven-development`: one
   subagent per task, spec-compliance review + code-quality review
   between tasks, fix-up loops as needed.
4. **Open a pull request** from the iteration branch (`vNN-<topic>`) targeting
   main, body referencing the issue via `Closes #N`; push the branch to origin
   and hand the PR to the user to review and merge. Do NOT self-merge to main.
5. **Update docs**: the merged PR auto-closes its `divergence` issue via
   `Closes #N` — no doc edit is needed unless the resolved item was
   *intentional* (then also remove it from `docs/bash-divergences.md`, which is
   now the intentional-only mirror of the `by-design` issues). Open a new
   `divergence` issue for any follow-on gap discovered; a NEW intentional
   divergence gets added to `docs/bash-divergences.md` + an open+closed
   `by-design` issue.
6. **Update memory**: edit the long-running notes under
   `/home/john/.claude/projects/-home-john-projects-huck/memory/`
   (`project_huck_iterations.md` + the `MEMORY.md` index) to capture the new
   iteration's design choices and gotchas.

Commits use the trailer
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
(update the model version to whichever Claude is doing the work; was 4.7 through
v136, 4.8 from v137). The "(1M context)" parenthetical is canonical — do not
remove it.

## Where to add common features

| Want to add… | Touch these files |
|---|---|
| New builtin | `builtins.rs`: add to `BUILTIN_NAMES`, add `builtin_NAME` function, add arm to `run_builtin` match. If it's POSIX special, also add to `is_special_builtin`. If it's a declaration command, add a `_decl` variant and route via `run_declaration_builtin` + update `is_declaration_command`. |
| New `${...}` modifier | `lexer.rs`: add a `ParamModifier` variant (+ any new atom the `ParamExpansion` mode must emit); `parser.rs`: assemble it when building the `ParamExpansion` word-part. `param_expansion.rs::expand_modifier_with_value`: add the arm. Update array-aware paths in `expand.rs::expand_array_param` / `expand_assoc_param` if the modifier composes with subscripts. |
| Whole-array `${var@OP}` transform (`@A`/`@K`/`@k`/`@a`) | New op variants land in `huck-syntax`'s `TransformOp` enum, then route through `is_whole_array_transform_op` in `expand.rs` for `[@]`/`[*]` subscripts or through `param_expansion.rs`'s `Transform { op }` arm for the scalar / single-element form. Render bodies live in `crates/huck-engine/src/array_transforms.rs`. |
| New `test` operator | `test_builtin.rs`: add to `is_unary_op` or `is_binary_op` + add the matching arm in `apply_unary` / `apply_binary`. The recursive-descent parser handles dispatch automatically. |
| New control-flow construct | `lexer.rs`: emit any new keyword/atom the construct needs. `parser.rs`: parse it (delimiter-matching/recursion) and build the AST node (defined in `command.rs`). `executor.rs`: add a `run_*` walker for the new `Command` variant. |
| New `set -o` option | `shell_state.rs::ShellOptions`: add the bool field. `builtins.rs::builtin_set`: add to the OptionInfo registry and the get/set/print helpers. Wire into the executor at the relevant action site. |
| New trap signal / pseudo-signal | `traps.rs`: add to the signal name table. `executor.rs`: add a `fire_*_trap` call at the appropriate spot. |
| New restricted-mode check | `policy.rs`: add an `Op` variant, then add its arm to `Policy::check` for both `Rbash` and `Sandbox` (the compiler forces both — that's the point of the enum). Call `shell.policy.check(op)?` at the enforcement site and emit the returned message via `sh_error!`/`sh_error_to!`. If it's a *variable* restriction rather than an operation, don't add an `Op` — extend `RESTRICTED_READONLY_VARS` instead so it rides the existing readonly machinery. |
| Array follow-on (e.g. `read -a`) | `builtins.rs`: extend the existing builtin with the flag. Use `Shell::set_indexed_element` / `Shell::extend_indexed` / etc. (or the associative siblings). The expansion side is already wired. |
| Emit an error message | Use the `sh_error!` family (`error_emit.rs`), never a literal `"huck: "` (the invariant test rejects it). If a redirect-aware `out`/`err` writer is in scope (builtins, executor) use `sh_error_to!(shell, writer, cmd, "…")`; otherwise `sh_error!(shell, cmd, "…")`. Syntax errors → `emit_syntax_error`; pre-shell CLI → `emit_cli_error`. See the "Error emission" note in Cross-cutting conventions above. |
| A builtin's write to `out` fails | **Do NOT emit a diagnostic** — return early (stop writing) and let the `run_builtin_with_redirects` epilogue report it. Builtin stdout bound for a real fd goes through `fd_writer.rs`'s unbuffered `FdWriter`, which records the first errno; the epilogue is the SINGLE place a write error is worded (`<name>: write error: <strerror>` + rc 1, matching bash). Emitting at the write site double-reports — `cd` and `export -f` both did (v308, #190). Discarding the `Result` (`let _ = writeln!(out, …)`, what most sites do) is fine and still reports correctly. This is also why builtin output must never go through `io::stdout()`: it swallows EBADF, and its `LineWriter` retains failed bytes that then leak to the restored fd 1 (#186, #191). |

## Pointers for new sessions

- **Pending bash-compat work**: the open GitHub issues labelled `divergence`
  (`gh issue list --label divergence --state open`). Filter by `bug` /
  `enhancement` and `sev:high` / `sev:medium` / `sev:low`. Intentional,
  kept-by-design divergences are the closed `by-design` issues, mirrored in
  `docs/bash-divergences.md`.
- **Past iteration design notes**: `docs/superpowers/specs/` and
  `docs/superpowers/plans/` have the full per-iteration paper trail.
  Search for the M-* / feature name.
- **What shipped in a given `vNN`**: `git log` + the
  `project_huck_iterations.md` memory note (newest at top).
- **Memory across sessions**: the user maintains long-running notes
  at `/home/john/.claude/projects/-home-john-projects-huck/memory/`.
  `MEMORY.md` is the index; `project_huck_iterations.md` has detailed
  per-iteration summaries with the design choices that aren't in the code.
