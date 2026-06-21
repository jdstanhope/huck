# huck Architecture Overview

This is a one-page map of how huck is structured, intended as the
starting point for anyone (human or LLM) extending the shell. It
covers the load-bearing types, the execution pipeline, the
iteration workflow, and where to add common new features. For the
list of pending bash-compat work see `docs/bash-divergences.md`.
For the iteration history see the table in `README.md`.

## Module map

The repo is a **4-member Cargo workspace** (v202 → v203), a layered stack with a
compiler-enforced acyclic dependency direction `syntax ← engine ← cli ← bin`:

- **`huck-syntax`** (`crates/huck-syntax/`) — the Shell-free **frontend**: `lexer`,
  `command` (AST + parser), `brace_expand`, `generate` (AST→source), plus
  `errors.rs` (`lex_error_message`/`parse_error_message`) and `util.rs`
  (`escape_double_quote_value`). No dependencies.
- **`huck-engine`** (`crates/huck-engine/`) — the **terminal-free execution
  core**: expansion, execution, builtins, shell state, traps, jobs, completion
  candidate-generation, the readline keymap *data*, and the headless entry
  (`huck_engine::shell::run_program` / `process_line`). Depends on `huck-syntax`;
  **rustyline-free** (a stray `use rustyline` here won't compile) — it is the
  embeddable, no-terminal interpreter.
- **`huck-cli`** (`crates/huck-cli/`) — the interactive **REPL** (`run` + the
  rustyline `Editor` loop) and the line-editor *adapters*: the `HuckHelper`
  completer (`Candidate`→`rustyline::Pair`) and the readline apply
  (`parse_keyseq`/`function_to_cmd`). Depends on `huck-engine` + `rustyline`.
- **`huck`** (root) — a thin **binary**: `main.rs` → `huck_cli::run(args)`.

`huck-engine/src/lib.rs` re-exports the frontend (`pub use huck_syntax::{lexer,
command, …}`) so `crate::lexer::`/`crate::command::` paths inside the engine
resolve unchanged. **Run the suite with `cargo test --workspace`** — a bare
`cargo test` from the root only runs the `huck` *bin* package's integration tests,
not the unit tests in the member crates.

Two-tier layout within the engine: lexer/parser/AST (the `huck-syntax` crate) at
the bottom, expansion + execution above, builtins at the top.

| Module | Responsibility |
|---|---|
| `lexer.rs` | Tokenize input + scan word-parts. Owns the `Word` / `WordPart` types and all subscript / paramexp / array-literal recognition. Also contains the parser entry point (`tokenize` → `parse` → `Sequence`). |
| `command.rs` | AST types: `Sequence`, `Pipeline`, `Command`, `SimpleCommand`, `ExecCommand`, `AssignTarget`, `Assignment`, `DeclArg`, `IfClause` / `WhileClause` / `ForClause` / `CaseClause`, `TestExpr` (for `[[ ]]`). |
| `shell_state.rs` | `Shell` struct (all session state) + `Variable` + `VarValue` (`Scalar`/`Indexed`/`Associative`) + `ShellOptions` + `AssignErr` / `DeclareErr`. Snapshot/restore primitives (`snapshot_var`/`restore_var`, `snapshot_for_local_scope`). |
| `expand.rs` | Word → Field expansion pipeline. Owns `Field`, `expand`, `emit_split_fields` (IFS-driven), `eval_subscript` (arith), `eval_subscript_key` (string), `slice_word_list`, `expand_array_param`, `expand_assoc_param`. |
| `param_expansion.rs` | Modifier-aware parameter expansion (`${var:-w}`, `${#var}`, `${var/pat/repl}`, `${var^^}`, etc.). `ExpansionResult` enum (`Value` / `Empty` / `Fatal` / `WordList`) + `ParamLookup` enum (`Scalar` / `Element(Option<&str>)`). |
| `executor.rs` | Walks `Command` / `Sequence` / `Pipeline` trees. `run_*` functions per AST shape. `apply_one_assignment` + `apply_inline_assignments` + `restore_inline_assignments`. Pipeline fork/exec; subshell forking for compound stages. ERR-trap + errexit wire-in via `maybe_errexit`. |
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
lex → Word stream         (lexer.rs::tokenize)     — quoting, $-expansions, subscripts, here-doc bodies
    │
    ▼
parse → Sequence          (lexer.rs::parse → command.rs types) — AST
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
- **Bash-diff harnesses** — three executable scripts under
  `tests/scripts/` run the same fragments through bash and huck and
  diff: `arrays_diff_check.sh` (v71/v72 + the element-default fix),
  `ifs_diff_check.sh` (v74), `test_combinators_diff_check.sh` (v75).
  Adding a new feature with a `*_diff_check.sh` is the gold standard
  for bash-compat verification.
- **Test layout** — unit tests live in `#[cfg(test)] mod` blocks at
  the bottom of each source file. Integration tests are binary-driven
  scripts in `tests/*.rs` using the `run_capture` helper pattern
  (spawn `huck_binary()`, write to stdin, capture stdout/stderr/exit).

## Naming conventions

Function-name verbs follow these conventions (codified v170; see the 2026-06-16
naming review). New code should match them:

- **Retrieval** — `get_*` borrows a stored container (`&T`, e.g. `get_indexed`);
  `lookup_*` computes one resolved value (owned `Option<String>`, e.g.
  `lookup_var`); `resolve_*` follows indirection to a concrete target
  (namerefs, paths — e.g. `resolve_nameref`, `resolve_dir`).
- **Lexing/scanning** — `scan_*` advances a `CharCursor` and collects a span
  (e.g. `scan_cmdsub_body`, `scan_subscript`); `split_*` partitions an
  already-collected `&str` (e.g. `split_modifier_operand`); `parse_*` produces
  AST/structure from tokens; `tokenize` turns source into tokens. The thin
  `consume_…_verbatim` wrappers re-emit a closing delimiter around a `scan_*`.
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
(`v1`, `v2`, ..., `v75` as of this writing). The cadence is:

1. **Brainstorm** the next feature via the `superpowers:brainstorming`
   skill. Produces a design doc at
   `docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md`.
2. **Plan** via the `superpowers:writing-plans` skill. Produces a
   per-task implementation plan at
   `docs/superpowers/plans/YYYY-MM-DD-<topic>.md`.
3. **Implement** via `superpowers:subagent-driven-development`: one
   subagent per task, spec-compliance review + code-quality review
   between tasks, fix-up loops as needed.
4. **Merge** the iteration branch (`vNN-<topic>`) to main via
   `--no-ff` after user confirmation; push to origin.
5. **Update docs**: flip the corresponding `M-*` / `B-*` entry in
   `docs/bash-divergences.md` from `[deferred]` to `[fixed vNN]`,
   add a change-log entry, add a row to README's iteration table.
6. **Update memory**: edit
   `/home/john/.claude/projects/-home-john-projects-shuck/memory/`
   files to capture the new iteration's design choices and gotchas.

Commits use the trailer
`Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`.
The "(1M context)" parenthetical is canonical — do not remove it.

## Where to add common features

| Want to add… | Touch these files |
|---|---|
| New builtin | `builtins.rs`: add to `BUILTIN_NAMES`, add `builtin_NAME` function, add arm to `run_builtin` match. If it's POSIX special, also add to `is_special_builtin`. If it's a declaration command, add a `_decl` variant and route via `run_declaration_builtin` + update `is_declaration_command`. |
| New `${...}` modifier | `lexer.rs`: add a `ParamModifier` variant + scanner. `param_expansion.rs::expand_modifier_with_value`: add the arm. Update array-aware paths in `expand.rs::expand_array_param` / `expand_assoc_param` if the modifier composes with subscripts. |
| New `test` operator | `test_builtin.rs`: add to `is_unary_op` or `is_binary_op` + add the matching arm in `apply_unary` / `apply_binary`. The recursive-descent parser handles dispatch automatically. |
| New control-flow construct | `lexer.rs`: add token recognition + AST construction (in `command.rs`). `executor.rs`: add `run_*` walker for the new `Command` variant. |
| New `set -o` option | `shell_state.rs::ShellOptions`: add the bool field. `builtins.rs::builtin_set`: add to the OptionInfo registry and the get/set/print helpers. Wire into the executor at the relevant action site. |
| New trap signal / pseudo-signal | `traps.rs`: add to the signal name table. `executor.rs`: add a `fire_*_trap` call at the appropriate spot. |
| Array follow-on (e.g. `read -a`) | `builtins.rs`: extend the existing builtin with the flag. Use `Shell::set_indexed_element` / `Shell::extend_indexed` / etc. (or the associative siblings). The expansion side is already wired. |

## Pointers for new sessions

- **Pending bash-compat work**: see `docs/bash-divergences.md`. Search
  for `[deferred]` to find every open M-/B-/L-/I- entry. Severity tag
  (high/medium/low) on each.
- **Past iteration design notes**: `docs/superpowers/specs/` and
  `docs/superpowers/plans/` have the full per-iteration paper trail.
  Search for the M-* / feature name.
- **What's in v22 vs v75**: README's iteration table is the index.
- **Memory across sessions**: the user maintains long-running notes
  at `/home/john/.claude/projects/-home-john-projects-shuck/memory/`.
  The `project_huck_iterations.md` file has detailed per-iteration
  summaries with the design choices that aren't in the code.
