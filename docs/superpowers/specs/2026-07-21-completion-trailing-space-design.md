# Completion trailing-space + meaningful `-o nospace`

Issue: [#42](https://github.com/jdstanhope/huck/issues/42)

## Problem

bash appends a single trailing space after a **unique** completion of a word,
so the cursor lands ready for the next word. huck never does this: rustyline
(`CompletionType::List`) inserts the candidate's `replacement` verbatim, and the
only decoration huck adds is a `/` for directories. Because there is no default
space, `complete -o nospace` has nothing to suppress — the option parses into
`CompOptions.nospace` but is unread at tab-dispatch, so it is a silent no-op.

Closing this means implementing bash's default trailing-space behavior first,
then honoring `nospace` against it.

## Ground truth (bash 5.2.21, PTY-probed — not assumed)

All measured directly against the real shell (`--norc`, so core readline, no
bash-completion framework):

| completion at Tab | result | trailing |
|---|---|---|
| unique command (`zzuniqcmd`) | `zzuniqcmd ` | **space** |
| unique non-dir file (`uniquefile.txt`) | `uniquefile.txt ` | **space** |
| unique non-dir variable (`$MYXVAR`) | `$MYXVAR ` | **space** |
| directory (`subdir`) | `subdir/` | `/`, no space |
| ambiguous (`alpha1`/`alpha2`) | `alpha` + beep | none (common prefix only) |
| `complete -o nospace -W foobar` | `foobar` | **none** |
| `complete -W foobar` (default) | `foobar ` | **space** |
| `complete -o nospace -o filenames` on a dir | `mydir/` | `/`, **no space** |

Two rules fall out:

1. The trailing space appears only on a **unique** completion of a
   **non-directory** word. Directories always get `/` and never a space.
2. `-o nospace` suppresses **only** the trailing space. It does **not** suppress
   the directory `/` — a directory under `nospace` still gets `/`.

A third behavior is **out of scope** (deferred to a follow-up issue): when a
completed variable's value is an existing directory, core readline appends `/`
instead of a space (`echo $HOM<TAB>` → `$HOME/`). Replicating it means
expanding and `stat`-ing each variable's value during completion, it is
orthogonal to this issue, and variables completing with a space (the
non-dir-value case) is the common path. Variables will get a space when unique.

## Design

### Mechanism

Append a single trailing space to the `replacement` of **non-directory**
completion candidates. `display` is left untouched.

This rides on rustyline's existing insertion logic, confirmed by reading
`rustyline-18.0.0/src/lib.rs:153-166`:

- For a **single** candidate, `longest_common_prefix` returns that candidate's
  whole `replacement`, and the `candidates.len() == 1` arm inserts it — so a
  trailing space in the replacement is inserted verbatim.
- For **multiple** candidates, only the common prefix of their replacements is
  inserted. Two space-suffixed replacements (`alpha1 ` / `alpha2 `) share the
  prefix `alpha` — the space is excluded — then rustyline beeps and (on the
  second Tab) lists the clean `display` values.

So the space surfaces exactly when bash's does, with no separate "is this
unique?" bookkeeping in huck. It is symmetric with the directory `/`, which
already lives in `replacement` today.

`display` carries no space, so the Tab-Tab candidate list is visually
unchanged.

The user chose (during brainstorming) to bake the decoration into
`replacement` rather than add a `Candidate` field. Consequence, accepted: the
public `Engine::complete` API now returns a trailing space in `replacement` for
word-final candidates — consistent with the existing `/`-in-`replacement`
convention for directories.

### The four candidate-building sites

`crates/huck-engine/src/completion.rs` and
`crates/huck-engine/src/completion_spec.rs`:

| builder | change |
|---|---|
| `completion.rs::complete_command` | append `' '` to every `replacement` (command names are always word-final and non-directory) |
| `completion.rs::complete_variable` | append `' '` to every `replacement` (the value-is-a-dir corner is deferred) |
| `completion.rs::complete_file` | append `' '` to `File` (non-directory) replacements; `Directory` keeps its `/` and gets no space |
| `completion_spec.rs::run_spec_with_empty_fallback` | append `' '` to non-directory replacements **unless `effective_options.nospace`**; directories keep `/` in both the `filenames` and non-`filenames` branches, even under `nospace` |

The space is the **outermost** decoration, applied after `-P`/`-S` and after any
`/`. The three built-in paths always add it; only the spec path consults
`nospace`.

**Non-leakage.** The spec path's `-A command` / `-A file` / `-A function`
actions consume each candidate's `display`, not its `replacement`
(`completion_spec.rs::complete_action`, e.g. `Action::Command` maps
`complete_command(...)` results through `.display`). So baking the space into
`complete_command`'s `replacement` does not add a stray space to `-A command`
results. `complete_file`'s space applies only to the no-spec plain-file path
(`dispatch::resolve`'s `None` branch); the spec path builds its own file
candidates.

### `nospace` semantics (spec path)

Reading `effective_options.nospace` (already computed at
`completion_spec.rs`, including any `compopt -o nospace` mutation applied by a
running `-F` function) gates only the trailing space:

- `nospace == false` → non-directory replacements get `' '`; directories get `/`.
- `nospace == true`  → non-directory replacements get no space; directories
  still get `/`.

This is the point at which `CompOptions.nospace` becomes load-bearing, closing
the issue.

## Testing

**Unit tests** (the mechanism gate), one per rule:

- `complete_command`: a unique-prefix match's `replacement` ends in `' '`.
- `complete_variable`: a match's `replacement` ends in `' '`.
- `complete_file`: a regular file's `replacement` ends in `' '`; a directory's
  ends in `'/'` and has no trailing space.
- spec path, default: a `-W` word's `replacement` ends in `' '`.
- spec path, `nospace`: the same word's `replacement` has **no** trailing space.
- spec path, `nospace` + directory: the directory's `replacement` ends in `'/'`
  (the `/` survives `nospace`).

**Existing tests.** `completion_integration` and `complete_actions_integration`
assert on `replacement` values and will need the trailing space added to their
expectations — this is the intended signal that the default behavior changed.
Sweep the completion test corpus and update every `replacement` assertion that a
word-final candidate now affects.

**End-to-end.** The "unique → insert full replacement (with space)" step is
rustyline's internal logic, not huck code, so it is verified by manual PTY
spot-check against bash 5.2.21 (as the #236/#237 fixes were), not a new
automated harness: huck's bash-diff harnesses drive non-interactive `-c` and
cannot exercise interactive Tab insertion. Spot-checks to run: unique command,
unique file, directory (no space), ambiguous (no space), and
`complete -o nospace -W …` (no space) vs `complete -W …` (space).

Per the repo's constraints, run tests per-crate
(`cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`) and the
`-p huck` integration binaries single-threaded before pushing; the full
`tests/scripts/run_diff_checks.sh` sweep must stay green (it is unaffected —
non-interactive).

## Scope

**In scope.** Default trailing space on unique non-directory completions across
command, variable, plain-file, and programmable (`-F`/`-W`/`-A`) completion;
`-o nospace` suppressing that space on the spec path; directories keeping `/`
(and no space) everywhere, including under `nospace`.

**Deferred (follow-up issue).** Core readline's behavior of appending `/`
instead of a space when a completed variable's value is an existing directory
(`$HOME/`).

**Not applicable.** No new automated interactive-completion harness — outside
the existing non-interactive `-c` diff-harness toolset.

## Documentation

`docs/architecture.md` if it describes completion decoration; the
`CompOptions.nospace` field comment (drop any "parsed but unread" note). #42 is
tracked in the GitHub issue tracker, not `docs/bash-divergences.md` (already
verified absent there), and auto-closes on merge via the PR body.
