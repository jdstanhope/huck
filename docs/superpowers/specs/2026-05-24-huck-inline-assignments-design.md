# v23: Inline Assignments — Design Spec

## Goal

Support `VAR=val cmd args` (and `A=1 B=2 cmd …`) — a leading run of
`NAME=value` words attached to a simple command. Closes M-04 from
`docs/bash-divergences.md`, one of the most-ubiquitous bash idioms.

Pre-v23, huck parses `FOO=bar cmd` as a command literally named
`FOO=bar` and errors with "not found." After v23, the assignments run
before `cmd`, contribute their values to the command's environment (and
possibly to the shell's), then either persist or get restored depending
on the target — see Semantics below.

## Semantics

Inline assignments are **set** with the export flag before the command
runs and either **persist** in the shell or are **restored** to their
prior state after the command finishes, depending on the target:

| Target | Persistence |
| --- | --- |
| No command (just `A=1 B=2`) | Persistent (same as existing `Assign` path) |
| External command | Restored after the command finishes |
| Regular builtin (`cd`, `pwd`, `echo`, `jobs`, `wait`, `fg`, `bg`, `kill`, `disown`, `history`, `test`, `[`) | Restored |
| Special builtin (`break`, `continue`, `exit`, `export`, `return`, `unset`) | Persistent |
| Function call | Persistent |

This mapping matches POSIX 2.14 (special vs. regular builtin) and POSIX
2.9.1 (function calls). The special-builtin set is intersected with
huck's existing builtins; `set`/`shift`/`trap`/`eval`/`exec`/`:`/
`readonly`/`.` will be added to the special list when they're
implemented.

**Left-to-right evaluation.** Assignments are applied in source order;
each one's RHS sees prior assignments' values. `A=1 B=$A cmd` →
`B=1` inside `cmd` (and inside the shell, during execution). This
falls out of the implementation naturally — we expand and set one at
a time.

**Snapshot details.** For each inline assignment, we remember `(name,
prior_value: Option<String>, prior_exported: bool)`. Restoration walks
the snapshot in reverse order:

- If `prior_value` is `Some(v)`: set the var back to `v`; restore the
  prior export flag (re-export or unexport as needed).
- If `prior_value` is `None`: the var was unset before — call
  `shell.unset(name)`.

Reverse order matters when the same name appears twice in the prefix
(e.g. `FOO=a FOO=b cmd`): unwinding LIFO restores the original state
even if intermediate values differed.

**`$?` behaviour.** RHS expansion goes through `expand_assignment`,
which (per B-07, fixed 2026-05-23) snapshots `$?` so `FOO=$? cmd` sees
the pre-prefix `$?` value, not anything mutated by command-subs in
earlier RHSs. After the command runs, `$?` reflects the command's
exit status — restoration of vars doesn't touch `$?`.

## AST changes

`src/command.rs`:

```rust
pub enum SimpleCommand {
    /// `A=1 B=2 …` with no command — every assignment persists.
    /// Replaces the old single-var `Assign { name, value }` form;
    /// callers must update to read `Vec<(String, Word)>`.
    Assign(Vec<(String, Word)>),
    Exec(ExecCommand),
}

pub struct ExecCommand {
    /// Leading `NAME=value` words preceding the command word. Empty
    /// when the user wrote `cmd args` with no assignment prefix.
    pub inline_assignments: Vec<(String, Word)>,
    pub program: Word,
    pub args: Vec<Word>,
    pub stdin: Option<Redirect>,
    pub stdout: Option<Redirect>,
    pub stderr: Option<Redirect>,
}
```

The `Assign` enum-variant change is breaking — every `match` site on
`SimpleCommand::Assign { … }` becomes `SimpleCommand::Assign(items)`
and the iteration applies each `(name, value)` in turn. The Stage::Done
path in `run_multi_stage` still treats Assign stages as "skip with
status 0" — unchanged.

## Parser changes

`parse_simple_command` (or wherever the simple-command boundary is
detected) walks leading tokens:

1. Accumulate a `Vec<(String, Word)>` of assignment words. A token is
   an assignment word if it parses as `identifier=…` — the lexer
   already classifies these via the `in_assignment_value` flag, but
   the parser's check is independent: it inspects the unquoted prefix
   for a valid identifier (per `valid_identifier_text`) followed by
   `=`.
2. After the run, peek the next token:
   - **Operator or end-of-pipeline** → emit `SimpleCommand::Assign(list)`.
   - **A non-assignment word** → that word is the program; the rest
     are args; emit
     `SimpleCommand::Exec(ExecCommand { inline_assignments: list, program, args, … })`.
3. After the program word, assignment-shaped tokens are passed through
   as ordinary `args`. POSIX explicitly says only the *leading* run
   of assignments preceding the command word is recognised.

Reserved keywords (`if`, `while`, `for`, `case`, `function`, etc.)
break the leading run and surface as parse errors at the
compound-command level. So `A=1 if true; then …; fi` is rejected
(out of scope for v23). This matches v23's "simple commands only"
scope and yields a clean ParseError rather than silently dropping the
prefix.

## Executor changes

**One new helper:**

```rust
// src/builtins.rs
pub fn is_special_builtin(name: &str) -> bool {
    matches!(name, "break" | "continue" | "exit" | "export" | "return" | "unset")
}
```

**One new shell-state helper:**

```rust
// src/shell_state.rs
impl Shell {
    /// Read the current export flag without consuming the value.
    pub fn is_exported(&self, name: &str) -> bool { … }
}
```

(`unset`, `set`, and `export_set` already exist.)

**Per-stage application** in `run_exec_single` (and the builtin-stage
path in `run_multi_stage` that calls `run_builtin` in-process):

```rust
// pseudo-code; concrete locations: run_exec_single in executor.rs
let snapshot: Vec<(String, Option<String>, bool)> = cmd
    .inline_assignments
    .iter()
    .map(|(name, rhs)| {
        let prior_value = shell.get(name).map(str::to_string);
        let prior_exported = shell.is_exported(name);
        let value = expand_assignment(rhs, shell);
        shell.export_set(name, value);
        (name.clone(), prior_value, prior_exported)
    })
    .collect();

let persistence = match &resolved {
    None => Persistence::Persistent, // no command — same as Assign-path
    Some(r) if r.is_function => Persistence::Persistent,
    Some(r) if is_special_builtin(&r.name) => Persistence::Persistent,
    Some(_) => Persistence::Temporary,
};

let outcome = run_the_command(...);

if persistence == Persistence::Temporary {
    for (name, prior_value, prior_exported) in snapshot.into_iter().rev() {
        match prior_value {
            Some(v) if prior_exported => shell.export_set(&name, v),
            Some(v) => {
                shell.set(&name, v);
                // ensure not exported — needs a small helper or
                // remove + re-add
            }
            None => shell.unset(&name),
        }
    }
}

outcome
```

The exact resolution path (`resolve(cmd, shell)`) already distinguishes
function / builtin / external — the new branch just decides
persistence based on the resolved kind.

**Empty-program case** is the `SimpleCommand::Assign(list)` path. Each
assignment is applied with the persistent (no-snapshot) flow —
identical to today's single-var Assign, just iterated.

**Pipeline stages**: each `Exec` stage carries its own
`inline_assignments`. The builtin-stage path in `run_multi_stage`
(non-forking, in-process) applies and restores assignments around the
builtin call exactly like `run_exec_single`. The external-stage path
spawns via `process.envs(shell.exported_env())` — assignments are
already exported into the shell snapshot, so they reach the child
naturally; restoration happens after `waitpid` returns.

## Edge cases

- **`FOO= cmd`** (empty RHS): `FOO` is set to `""` and exported;
  child sees `FOO=`. Restoration applies normally.
- **`FOO=$? cmd`**: `$?` snapshot via `expand_assignment` (B-07);
  reads pre-prefix value.
- **`PATH=~/bin cmd`**: tilde expansion in RHS — already handled by
  `expand_assignment`'s tilde path (via the lexer's
  `tilde_eligible_in_assignment`).
- **`FOO=$(cmd1) cmd2`**: command substitution in RHS runs as part of
  expansion; its `$?` updates flow into the same expansion's snapshot
  (so a later same-RHS `$?` reads the snapshot, but a *later
  assignment's* RHS sees the updated `$?`).
- **`1FOO=bar cmd`**: LHS `1FOO` isn't a valid identifier — the word
  is not an assignment; it becomes the program word.
- **`cd FOO=bar`**: `cd` is the program; `FOO=bar` is an arg. `cd`
  errors with "too many arguments" — matches bash.
- **Repeated names**: `FOO=a FOO=b cmd` — final value is `b`;
  reverse-order restoration unwinds correctly because each snapshot
  entry holds the value seen *just before* its own application.
- **Pipeline with builtin and external stages**: each stage's
  assignments are scoped to that stage. `FOO=1 echo $FOO | FOO=2 cat`
  — the `echo` stage runs in-process with FOO=1 temporarily; the
  `cat` stage forks with FOO=2 in its env.

## Out of scope

- `local`-style function-internal scoping (that's M-06).
- Inline assignments preceding compound commands (`A=1 if …; fi`);
  parser will reject these. Bash allows them and applies the
  assignment to the whole compound, but the surface is significantly
  larger and adds nothing to the script-compat goal here.
- `set`/`shift`/`trap`/`eval`/`exec`/`:`/`readonly`/`.` builtins —
  they'd extend `is_special_builtin`, but they aren't implemented yet.

## Tests

| Test | Layer | Covers |
| --- | --- | --- |
| `parse_inline_assignments_collect_into_exec` | unit (command.rs) | `A=1 B=2 cmd arg` parses with 2 inline_assignments, program, args |
| `parse_assign_only_single_var` | unit | `A=1` → `Assign(vec![("A", …)])` — backward-compat shape |
| `parse_assign_only_multiple_vars` | unit | `A=1 B=2` (no command) → `Assign(vec of 2)` |
| `parse_mid_command_assignment_word_stays_literal` | unit | `cmd A=1` → program=cmd, args=["A=1"] |
| `parse_invalid_identifier_lhs_is_not_assignment` | unit | `1FOO=bar cmd` → program=`1FOO=bar`, no inline_assignments |
| `parse_assignment_before_compound_command_errors` | unit | `A=1 if true; then echo hi; fi` → ParseError |
| `inline_assignment_external_command_sees_var` | integration | `FOO=hi env \| grep ^FOO=` outputs `FOO=hi` |
| `inline_assignment_external_command_restores_after` | integration | `unset FOO; FOO=hi /bin/true; echo "[$FOO]"` → `[]` |
| `inline_assignment_left_to_right_visibility` | integration | `A=1 B=$A env \| grep ^[AB]=` shows `A=1` and `B=1` |
| `inline_assignment_unset_before_restores_to_unset` | integration | `unset FOO; FOO=hi /bin/true; printenv FOO; echo $?` → no output, exit 1 |
| `inline_assignment_set_unexported_before_keeps_unexported_after` | integration | `FOO=outer; FOO=inner /bin/true; env \| grep ^FOO= \|\| echo "not exported"` |
| `inline_assignment_regular_builtin_restores` | integration | `FOO=outer; FOO=inner test -n "$FOO"; echo $FOO` → `outer` |
| `inline_assignment_special_builtin_persists` | integration | `FOO=val export FOO; echo $FOO` → `val` |
| `inline_assignment_function_call_persists` | integration | `myfunc() { :; }; FOO=val myfunc; echo $FOO` → `val` |
| `inline_assignment_function_mutation_persists` | integration | `myfunc() { FOO=$FOO-modified; }; FOO=initial myfunc; echo $FOO` → `initial-modified` |
| `inline_assignment_dollar_question_snapshot` | integration | `false; FOO=$? true; echo $FOO` → `1` (B-07 reuse) |
| `inline_assignment_empty_rhs` | integration | `FOO= env \| grep ^FOO=` → `FOO=` |
| `inline_assignment_tilde_expands` | integration | `FOO=~ /bin/true; echo $FOO` (after restoration `$FOO` is whatever it was; check via env-passthrough instead) — `HOME=/tmp/x FOO=~ env \| grep ^FOO=` → `FOO=/tmp/x` |
| `inline_assignment_repeated_name_restores_original` | integration | `FOO=outer; FOO=a FOO=b /bin/true; echo $FOO` → `outer` |
| `inline_assignment_in_pipeline_stage_scoped_to_stage` | integration | `FOO=stage1 env \| FOO=stage2 grep ^FOO=` → `FOO=stage2` (only the last stage's env is observed by grep) |

Plus targeted regression check: existing `expand_assignment` snapshot
test (`expand_assignment_last_status_after_command_sub_reads_snapshot`)
must still pass.

## Change log

- **2026-05-24**: Spec drafted, semantics aligned with user
  (POSIX-correct + functions persist + left-to-right).
