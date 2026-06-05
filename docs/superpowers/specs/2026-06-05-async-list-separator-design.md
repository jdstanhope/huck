# huck v98 — `&` async list separator (with and-or grouping) Design

**Status:** approved design (full-grouping scope), ready for implementation plan.
**Implements:** `&` as a **list separator** that backgrounds the preceding
and-or group and continues the list — `a & b`, `cmd & cmd2 &`,
`for … do cmd & done`, `if … then cmd & fi`, `{ cmd & cmd2; }`, `( a & b )`,
and bash-correct grouping `a && b &` (backgrounds the whole `a && b` group).
Today huck allows `&` only as a *trailing* token on a whole command (sets
`Sequence.background`); as a separator it errors `'&' not allowed here`
(`UnexpectedBackground`), and the subshell parser silently downgrades `&` to `;`
(a latent bug: `( a & b )` runs `a` in the foreground).
**Primary driver:** `~/.nvm/nvm.sh` line 1192 — `NAME=v … nvm_print_alias_path … &`
backgrounded inside a `for` loop (and lines 37/51 `{ …; } &`). Next nvm blocker.
**Closes:** a new Tier-2 entry (async `&` list separator) `[fixed v98]`.
**Branch (impl):** `v98-async-list-separator`.

## Chosen scope

Full bash-correct semantics, including `&` applied to an `&&`/`||` **group**
(`a && b &` backgrounds `(a && b)` as one async unit). Delivered via
**executor-side grouping**, NOT a restructure of the `Sequence` AST (see
"Implementation approach" — same observable behavior, much lower risk). No
precedence limitation.

## Verified bash 5.2 semantics (the contract)

- `echo a & echo b` → `a` backgrounded, `b` foreground; both print.
- `sleep 0 &` (trailing) → backgrounded, status 0 (unchanged today).
- `a && b &` → the group `a && b` runs as ONE background subshell.
- `a & b && c` → `(a &)` ; `(b && c)`.
- `for i in 1 2; do echo $i & done; wait` → both backgrounded.
- `( a & b )` → `a` backgrounded inside the subshell, `b` foreground (currently
  WRONG in huck — runs `a` foreground).
- `$!` is set to the pid of the most recently backgrounded group.
- A list's exit status is the last command's; a trailing `&` yields status 0.

## Implementation approach (flat AST + executor grouping)

huck's `Sequence { first: Command, rest: Vec<(Connector, Command)>, background:
bool }` is a *flat* list (`&&`/`||`/`;` all flattened into `rest` with
`Connector::{Semi,And,Or}`). Rather than restructure this (which would churn
every `Sequence` consumer — executor, all compound parsers, command-sub, tests),
we add one connector variant and make the executor compute and-or **group
boundaries** at run time. This yields identical bash semantics — including
whole-group backgrounding for `a && b &` — because backgrounding a group reuses
the existing "wrap the group in a synthetic `Subshell` and fork" mechanism that
`execute()` already uses for `(multi-command sequence) &`.

## Section 1 — AST (`src/command.rs`)

Add `Connector::Amp` to `enum Connector { Semi, And, Or, Amp }`. An `Amp`
connector before a `rest` element means "the command to its left is backgrounded;
then continue sequentially (like `;`)." The trailing `background: bool` is kept
unchanged for the final command (`a &`). Examples:
- `a & b` → `first=a, rest=[(Amp, b)], background=false`.
- `a & b &` → `first=a, rest=[(Amp, b)], background=true`.
- `a && b &` → `first=a, rest=[(And, b)], background=true`.
- `a && b & c` → `first=a, rest=[(And, b), (Amp, c)], background=false`.

(Semantically: `Semi`/`Amp` are list-level separators; `And`/`Or` are
and-or-level. The executor uses this distinction to find group boundaries — §3.)

## Section 2 — Parser (`src/command.rs`)

Every list loop treats `&` uniformly as a separator (not an error / not a
downgrade-to-`;`):
- **Top-level list loop** (`:648`): replace the `UnexpectedBackground` logic.
  On `Token::Op(Operator::Background)`: skip trailing newlines; if a command
  follows, push `(Connector::Amp, parse_command(iter)?)` and continue; if nothing
  follows (EOF / a `stop_at` keyword), set `background = true` and break (the
  existing trailing-`&` behavior). `&` before `;`/newline collapses (`&;` ≡ `&`).
- **Subshell body parser** (`:1482`): replace the current fake-`Semi` push with
  `(Connector::Amp, …)` so `( a & b )` correctly backgrounds `a`. The trailing-`&`
  -before-`)` case keeps `background=true`.
- **Compound-body / generic sequence parser**: the same `&` → `Amp` separator
  handling so loop/if/case/brace bodies background correctly. (If all list
  contexts already route through one `parse_sequence`-style helper, fix it once;
  otherwise apply the same change to each.)

`UnexpectedBackground` becomes unused (a bare/misplaced `&` is now always either a
separator or a trailing background); remove it if no longer produced, or keep for
a genuinely-invalid position (e.g. `& cmd` with nothing before — verify what
bash does: `& echo` → bash syntax error; huck should still error there).

## Section 3 — Executor (`src/executor.rs`)

Make foreground sequence execution **group-aware**. A "group" is a maximal run of
commands joined by `And`/`Or` connectors, delimited by `Semi`/`Amp` boundaries
(and the list start/end). Each group is backgrounded iff its terminating
connector is `Amp` (or it is the final group and `Sequence.background` is set).

Rework `execute_sequence_body` (and fold in `execute()`'s trailing-background
special-cases):
1. Walk `first` + `rest`, partitioning into groups at `Semi`/`Amp` connectors.
   For each group, record its commands + internal `And`/`Or` connectors + whether
   it is backgrounded (its terminator is `Amp`, or last-group-and-`background`).
2. For each group in order:
   - **Foreground group**: run it with the existing and-or short-circuit logic
     (run first command; for each `(And/Or, cmd)` apply `&&`/`||` semantics);
     its status updates `$?`. (This is exactly today's per-element logic, just
     scoped to the group.)
   - **Background group**: wrap the group's commands into a synthetic
     `Sequence` (with its internal `And`/`Or` connectors, `background=false`) and
     run it via the existing `run_background_subshell(Command::Subshell { body })`
     path — fork, register a job, set `$!`, do NOT wait. Status contribution is
     `0` (background launch). Reuse `run_background_sequence` for a single-pipeline
     group if that path is cheaper, matching today's single-`&` behavior.
3. Control-flow outcomes (`break`/`continue`/`return`/`exit`) from a FOREGROUND
   group propagate as today; background groups run in a child so they can't
   affect parent control flow (matching bash).
4. `set -e` / ERR-trap behavior on foreground groups is unchanged; a backgrounded
   group's failure does not trigger parent `set -e` (matches bash — async lists
   are exempt, like the existing trailing-`&`).

`execute()`'s current `if seq.background { … wrap-in-subshell … }` block is
subsumed: the trailing background is just "the last group is backgrounded," now
handled uniformly by the group walk. Keep the existing
`run_background_subshell`/`run_background_sequence`/`fork_and_run_in_subshell`
helpers; only the dispatch (per-group) changes.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/command.rs` | `Connector::Amp`; `&`-as-separator in the top-level, subshell, and compound-body list loops; retire/relocate `UnexpectedBackground` |
| `src/executor.rs` | group-aware `execute_sequence_body`; per-group foreground/background dispatch reusing `run_background_subshell`/`run_background_sequence`; fold in `execute()`'s trailing-background path |
| `tests/async_list_integration.rs` | NEW — `a & b`, group backgrounding, `&` in loop/if/brace/subshell bodies, `$!`, status, `wait` |
| `tests/scripts/async_list_diff_check.sh` | NEW — 23rd bash-diff harness (deterministic fragments) |
| `docs/bash-divergences.md`, `README.md` | new Tier-2 entry `[fixed v98]`; note the `( a & b )` fake-`;` bug fixed; changelog; README row |

## Testing

1. **Parser unit tests**: `a & b` → `rest=[(Amp,b)]`; `a && b &` → `[(And,b)]`,
   `background=true`; `a & b &` → `[(Amp,b)]`,`background=true`; bare trailing
   `a &` unchanged.
2. **Integration** (`tests/async_list_integration.rs`) — use deterministic
   ordering (background a command that writes to a file, then `wait`, then read;
   or `sleep`-free commands + `wait`):
   - `echo a & echo b; wait` (both run); collect output deterministically.
   - `for i in 1 2 3; do echo $i >> F & done; wait; sort F` → `1\n2\n3`.
   - `{ echo a & echo b; }; wait`.
   - `( echo a & echo b ); wait` (the previously-foreground `a` now backgrounds —
     assert both print).
   - `true && echo grouped &; wait` (group backgrounded; `grouped` prints).
   - `false && echo no &; wait` (group backgrounded; `no` does NOT print —
     `&&` short-circuit preserved inside the backgrounded group).
   - `sleep 0 & echo "pid=$!"` (`$!` set to the bg pid; non-empty numeric).
   - exit status: `false & echo $?` → `0` (trailing/leading `&` launch is 0).
   - regression: `a && b`, `a || b`, `a; b`, `a & ` (trailing) all unchanged.
3. **bash-diff harness** `tests/scripts/async_list_diff_check.sh` (23rd):
   deterministic fragments (write-to-file + `wait` + `cat`/`sort`; `$?` after a
   trailing `&`; `&&` short-circuit inside a backgrounded group) byte-identical
   to bash 5.2. Avoid timing-dependent interleaving (sort or serialize).
4. **Regression**: full suite — especially the existing background/job tests
   (`jobs`/`bg`/`fg`/`wait`), subshell, pipeline, command-substitution (which
   ignores `&`), and `set -e` tests must pass unchanged.
5. **End-to-end**: a temp script `for i in 1 2; do echo $i & done; wait` works;
   and (manual, in changelog) re-bisect `nvm.sh` — it should parse past line 1192.

## Edge cases & notes

- **`( a & b )` semantics fix**: this iteration changes `( a & b )` from
  "run `a` foreground" (current latent bug) to "background `a`". Documented as a
  bug fix (it was never correct).
- **Command substitution**: `execute_capturing` ignores `&` (substitutions must
  complete before interpolation) — keep that behavior; a backgrounded group
  inside `$(…)` runs foreground-to-completion as today (note: bash also largely
  serializes here; preserve huck's existing capture semantics).
- **`$!` and `$?`**: `$!` ← last backgrounded group's pid; a list's `$?` is the
  last *foreground* group's status, or `0` if the list ends with a background.
- **`&` with nothing before it** (`& echo`): still a syntax error (verify against
  bash and keep erroring).
- **No regression for non-`&` lists**: `Semi`/`And`/`Or`-only sequences execute
  through the same group walk with every group foreground — behavior identical to
  today (the group walk degenerates to the current per-element loop).
