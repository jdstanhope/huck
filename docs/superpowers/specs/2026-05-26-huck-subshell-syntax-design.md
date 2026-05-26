# v28: Subshell Syntax `(list)` — Design Spec

## Goal

Support POSIX subshell syntax: `(list)` runs the inner sequence in a
forked subshell with isolated side effects. Closes M-11 from
`docs/bash-divergences.md`.

Pre-v28: `(cmd)` is a parse error (bare `LParen` at command position
isn't a recognized construct — `(` is only valid inside case-pattern
lists).

After v28:
```sh
$ FOO=outer
$ (FOO=inner; echo $FOO)
inner
$ echo $FOO
outer
```

The subshell forks; assignments, `cd`, function defs, `exit`, and all
other side effects are local to the fork and discarded when the
subshell exits.

## Scope

User-confirmed:
- **Just `(list)` syntax** — the inner sequence runs in a forked
  subshell. Inner redirects work as today (`(cmd > out.txt)`); outer
  redirects on the subshell itself (`(cmd) > out.txt`) are OUT OF
  SCOPE and remain a separate gap (redirects-on-compound-commands
  more broadly).
- **`Command::Subshell { body: Box<Sequence> }`** AST shape.
- **Always fork** on each `Command::Subshell` — nested `((cmd))`
  double-forks (POSIX-correct, slightly wasteful, simplest impl).

## Semantics

**Fork-on-execute**: every `Command::Subshell` evaluation calls
`libc::fork()`. The child runs the body's `Sequence` via `execute(...)`
then `_exit`s with the resulting status.

**Side-effect isolation**: var assignments, `cd`, `export`, `unset`,
function definitions, inline assignments, `set` (when implemented),
and all other mutations to shell state are local to the subshell — POSIX
2.12 / bash. Parent inherits NOTHING from the subshell.

**State inheritance via fork**: the child sees the parent's full state
at fork time — vars, exported env, functions, positional parameters,
`$?`, `$!`, `$0`/`$$` (cached at startup; same value in subshell).
Standard libc::fork() semantics.

**`exit N`**: exits the subshell with status `N` (masked to 8 bits per
B-05). Parent shell sees the subshell's exit as the `(...)` command's
status and continues. POSIX.

**`return N`**: only meaningful inside a function. Inside a function
inside a subshell, `return` exits the function (still inside the
subshell). Stray `return` at the top level of a subshell is neutralized
to status 0 (matches huck REPL's I-03 behavior; bash errors). Document
the slight divergence.

**Pipeline composition**: `(cmd) | grep` and `echo | (cat)` both work
via v25's per-stage classification: a Subshell stage is `InProcess`,
runs via `fork_and_run_in_subshell`, which forks AGAIN to run the
Subshell body. Two forks for `(cmd) | other`. Wasteful but correct.

**Backgrounded subshell**: `(cmd) &` runs the whole subshell as a
background job via the existing `run_background_sequence` machinery
(v25). The bg job's pid is the subshell's fork pid.

**Job control**: subshells take part in the foreground pgrp just like
external pipeline stages. Ctrl-Z stops the subshell; `fg` resumes.
B-09's `wait_pipeline_raw` handles this without modification (since
subshells appear identical to external stages from the parent's
waitpid perspective).

## Lexer changes

**None.** `(` and `)` are already tokenized as `Operator::LParen` and
`Operator::RParen` (added in v21 for case-clause patterns). No new
operator, no new context-sensitive recognition.

## AST changes

`src/command.rs`:

```rust
pub enum Command {
    Pipeline(Pipeline),
    Simple(SimpleCommand),
    If(IfClause),
    While(WhileClause),
    For(ForClause),
    Case(CaseClause),
    BraceGroup(BraceGroup),
    Subshell { body: Box<Sequence> },    // NEW (v28)
    FunctionDef { name: String, body: Box<Command> },
}
```

Body is a `Sequence` (not a `Command`) so it can hold `;`/`&&`/`||`-
separated lists and backgrounded sub-commands. Mirrors `BraceGroup`'s
shape. Boxed for ABI reasons (Command enum size).

## Parser changes

`src/command.rs::parse_command` (or the equivalent dispatcher that
sees the first token of a Command):

Add a dispatch arm: first token is `Token::Op(Operator::LParen)` (in
command-start context — not inside a case-pattern):

```rust
Token::Op(Operator::LParen) => {
    iter.next();   // consume LParen
    // Parse the inner sequence; bounded by RParen.
    let body = parse_sequence_until_rparen(iter)?;
    // Expect closing RParen.
    match iter.next() {
        Some(Token::Op(Operator::RParen)) => {},
        _ => return Err(ParseError::UnterminatedSubshell),
    }
    if body.is_empty() {
        return Err(ParseError::EmptySubshell);
    }
    Command::Subshell { body: Box::new(body) }
}
```

`parse_sequence_until_rparen` is either a new helper or the existing
`parse_sequence` with an RParen-aware stop token. Implementer's choice;
the existing `parse_sequence` may take a "stop on these tokens" set —
adapt accordingly.

**Empty body** (`()`): `ParseError::EmptySubshell`. POSIX-required.

**Unterminated subshell** (`(cmd`): `ParseError::UnterminatedSubshell`.
The continuation classifier (v19 + v21 era) needs a corresponding
`Incomplete(Subshell)` reason so the REPL prompts for more lines until
the closing `)`. This is a small classifier addition.

**Disambiguation from function-def `()`**: the function-def syntax
`name() body` requires an IDENT immediately followed by `(`. The
subshell syntax `(cmd)` has `(` at command-start position with no
preceding IDENT. The parser's existing dispatch logic distinguishes
these correctly via lookahead. Verify no conflict.

**Disambiguation from case-pattern `(`**: case patterns appear inside
`case X in <pat>) ...` clauses, which are parsed by the dedicated
`parse_case` function. The new Subshell dispatch only fires at
command-start position, not inside `case` clauses. No conflict.

## Continuation classifier

`src/continuation.rs`:

```rust
pub enum ContinuationReason {
    Backslash,
    Operator,
    OpenQuote,
    Compound,
    Heredoc,
    Subshell,   // NEW (v28)
}
```

`classify` maps `Err(ParseError::UnterminatedSubshell)` to
`Incomplete(Subshell)`. The continuation classifier already has the
"compound command unterminated" path; Subshell joins that family.

`joiner_for(Subshell, _)`: returns `"; "` (same as other multi-line
compounds — semicolons separate consecutive lines that the user
typed at the continuation prompt). Subshells don't have the heredoc
quirk of needing literal newlines.

## Executor

`src/executor.rs`:

**Top-level `Command::Subshell` dispatch** in `run_command`:

```rust
Command::Subshell { body } => {
    // Wrap body in a Command for the helper. Or: call a Sequence-aware
    // path in fork_and_run_in_subshell.
    fork_and_run_subshell_at_top_level(body, shell)
}
```

Where `fork_and_run_subshell_at_top_level` forks, runs the body via
`execute(body, shell, &mut StdoutSink::Terminal)` in the child,
`_exit`s with the status, and the parent `waitpid`s.

**Pipeline-stage `Command::Subshell`**: existing v25 path. `classify_stage`
returns `StageKind::InProcess`. The pipeline-fork helper
`fork_and_run_in_subshell(cmd, ...)` is called with `cmd = Command::Subshell`.
To avoid infinite recursion in the child (where `run_command(Command::Subshell)`
would fork AGAIN), the helper gains a small dispatch at the top:

```rust
fn fork_and_run_in_subshell(cmd: &Command, ...) -> Result<i32, io::Error> {
    // ... existing fork() setup ...
    if pid == 0 {
        // CHILD: dup2, signal reset, etc...
        let outcome = match cmd {
            Command::Subshell { body } => execute(body, shell, &mut sink),
            other => run_command(other, shell, &mut sink),
        };
        // ... _exit translation ...
    }
    // ... existing parent setup ...
}
```

This makes the helper's child path execute the Subshell's body directly
(not via `run_command`), avoiding the recursive fork. Net: one fork
per Subshell, even when used as a pipeline stage.

For TOP-LEVEL `(cmd)` (not in a pipeline), `run_command` calls the same
helper with inherited stdio fds and empty parent_fds_to_close — clean
reuse of the v25 machinery.

**Backgrounded `(cmd) &`**: goes through `run_background_sequence` (v25
pattern). That path already handles InProcess stages via
`fork_and_run_in_subshell`. The new helper dispatch above applies
uniformly. No further changes.

**No double-fork** (pipeline case): the new helper dispatch ensures
the pipeline's stage-fork directly runs the Subshell body. Nested
subshells (`( ( cmd ) )`) DO double-fork — but that's a different
construct (explicit nested syntax) and the double-fork is correct per
POSIX.

## Edge cases

- **Empty body** (`()`): `ParseError::EmptySubshell`.
- **Unterminated** (`(cmd`): `ParseError::UnterminatedSubshell` →
  `Incomplete(Subshell)` → REPL prompts for more.
- **`(cmd1; cmd2)`**: both commands run in the same fork; sequence
  semantics hold.
- **`(cmd1 && cmd2)`**: short-circuit works inside the fork.
- **`(cmd &)`** (bg inside subshell): the inner `cmd &` backgrounds
  within the subshell; the subshell collects the job. When the subshell
  exits, the bg child is orphaned (re-parented to init).
- **`(cmd) &`** (subshell backgrounded): the whole subshell is bg'd.
  Parent's `$!` becomes the subshell's pid.
- **`(exit 5)`**: subshell exits 5; parent's `$?` is 5; parent
  continues.
- **`((cmd))`**: parses as Subshell containing Subshell; double-forks.
- **`(cd /tmp); pwd`**: parent's cwd unchanged. (Already true for the
  pipeline form via v25; this confirms the explicit-subshell form too.)
- **`(FOO=val; func)` where func references `$FOO`**: func sees FOO=val
  inside the fork.
- **`(f() { :; }); f`**: parent doesn't have `f` defined; `f` errors
  with "not found".
- **Heredoc inside subshell** (`(cat <<EOF\nbody\nEOF\n)`): heredoc body
  flows through the v24 machinery; the subshell fork captures the
  expansion in the child. Works through existing composition.
- **Here-string inside subshell** (`(cat <<< body)`): same, v27
  machinery.
- **Inline assignment + subshell** (`FOO=hi (echo $FOO)`): bash treats
  this oddly — inline assignment before a compound command isn't POSIX-
  standard. Out of scope for v28; document as a separate gap if it
  fails.

## Out of scope

- **Redirects on the subshell** (`(cmd) > out.txt`): would require adding
  a redirect field to `Command::Subshell` (and ideally to all compound
  commands). Separate feature; defer.
- **Bash arithmetic command** (`((expr))` without leading `$`): bash
  extension; out of scope. Note that `((cmd))` will parse as nested
  subshells, not as arithmetic — which differs from bash but is
  consistent with huck's POSIX focus.
- **`{ ; }` brace-group with explicit semicolon vs `(;)` subshell**:
  syntactic parallel; out of scope to bridge any gaps in BraceGroup
  parsing.

## Tests

### Parser (`src/command.rs::tests`)

| Test | Covers |
| --- | --- |
| `parse_subshell_simple` | `(echo hi)` → Subshell variant with single-command body |
| `parse_subshell_with_sequence` | `(cmd1; cmd2)` → body has 2 commands |
| `parse_subshell_with_and_or` | `(true && echo hi)` → body is Sequence with And connector |
| `parse_subshell_nested` | `((echo hi))` → Subshell containing Subshell |
| `parse_subshell_empty_errors` | `()` → ParseError::EmptySubshell |
| `parse_subshell_unterminated_errors` | `(cmd` → ParseError::UnterminatedSubshell |
| `parse_subshell_as_pipeline_first_stage` | `(echo hi) \| cat` → Pipeline; stage 0 is Subshell |
| `parse_subshell_as_pipeline_later_stage` | `echo hi \| (cat)` → Pipeline; stage 1 is Subshell |
| `parse_subshell_does_not_conflict_with_function_def` | `f() (cmd)` → FunctionDef with Subshell body, NOT a Subshell |
| `parse_subshell_does_not_conflict_with_case_pattern` | `case x in (a) :; ;; esac` parses as case (existing) |

### Continuation classifier (`src/continuation.rs::tests`)

| Test | Covers |
| --- | --- |
| `classify_subshell_unclosed_is_incomplete` | `(cmd` → `Incomplete(Subshell)` |
| `classify_subshell_closed_is_complete` | `(cmd)` → `Complete` |
| `joiner_for_subshell_is_semi` | `joiner_for(Subshell, _)` → `"; "` |

### Executor unit (`src/executor.rs::tests`)

| Test | Covers |
| --- | --- |
| `subshell_runs_body_in_fork_isolates_vars` | Set FOO=outer; run Subshell `FOO=inner`; parent's FOO still outer |
| `subshell_exit_does_not_exit_parent` | Run `(exit 5)`; parent continues; `$?` is 5 |
| `fork_and_run_in_subshell_handles_subshell_command_directly` | New dispatch in the v25 helper avoids re-fork |

### Integration (new `tests/subshell_integration.rs`)

| Test | Script | Expected |
| --- | --- | --- |
| `subshell_basic_echo` | `(echo hi)\nexit\n` | `hi` |
| `subshell_isolates_cd` | `pwd; (cd /tmp); pwd\nexit\n` | first pwd == second pwd (both original dir) |
| `subshell_isolates_var_assignment` | `FOO=outer\n(FOO=inner; echo in:$FOO)\necho out:$FOO\nexit\n` | `in:inner` then `out:outer` |
| `subshell_isolates_function_def` | `(f() { echo defined; }; f)\nf\nexit\n` | `defined` then (function-not-found stderr) |
| `subshell_exit_status_propagates` | `(exit 7)\necho $?\nexit\n` | `7` |
| `subshell_with_sequence` | `(echo a; echo b)\nexit\n` | `a\nb` |
| `subshell_with_and_or` | `(true && echo ok)\nexit\n` | `ok` |
| `subshell_in_pipeline_first_stage` | `(echo hi) \| cat\nexit\n` | `hi` |
| `subshell_in_pipeline_last_stage` | `echo hi \| (cat)\nexit\n` | `hi` |
| `subshell_nested_double_fork` | `((echo nested))\nexit\n` | `nested` |
| `subshell_backgrounded` | `(echo bg) > /tmp/v28_bg_$$ &\nwait\ncat /tmp/v28_bg_$$\nrm -f /tmp/v28_bg_$$\nexit\n` | `bg` |
| `subshell_inherits_vars_from_parent` | `FOO=hi\n(echo got:$FOO)\nexit\n` | `got:hi` |
| `subshell_in_function_body` | `f() { (echo from-subshell-in-func); }\nf\nexit\n` | `from-subshell-in-func` |
| `subshell_with_heredoc_inside` | `(cat <<EOF\nbody\nEOF\n)\nexit\n` | `body` |
| `subshell_with_here_string_inside` | `(cat <<< hi)\nexit\n` | `hi` |

### PTY interactive (`tests/pty_interactive.rs`)

| Test | Covers |
| --- | --- |
| `pty_subshell_continuation_prompt` | After `(cmd<ENTER>`, prompt is `> `; closing `)<ENTER>` runs |
| `pty_subshell_ctrl_c_aborts_body_collection` | Mid-subshell Ctrl-C → main prompt, partial discarded |

**Total**: ~10 parser + 3 classifier + 3 unit + 15 integration + 2 PTY = ~33 new tests.

### Doc updates

- `docs/bash-divergences.md`: M-11 → `[fixed (2026-05-26)]`; Tier 2 count drops by 1; change-log entry.
- `README.md`: v28 status row.

## Change log

- **2026-05-26**: Spec drafted; scope = just `(list)` syntax; AST
  `Command::Subshell { body: Box<Sequence> }`; always-fork strategy.
  Reuses v25's fork machinery with a small helper-dispatch change to
  avoid recursive fork in pipeline-stage Subshells.
