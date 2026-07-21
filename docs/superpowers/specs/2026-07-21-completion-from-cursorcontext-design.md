# Completion from `CursorContext`: delete `analyze_full`

Issue: [#248](https://github.com/jdstanhope/huck/issues/248)

## Problem

huck determines completion context — is the cursor completing a command, a
variable, or a filename? — with a hand-rolled character scanner, `analyze_full`
in `crates/huck-engine/src/completion.rs`. It re-derives shell structure
(quote/backtick/paren tracking, command-vs-argument position, opener
disambiguation) and keeps needing hand-patches as edge cases surface. It has
real bash divergences a daily user hits immediately (all currently produce **no
completion** where bash completes):

| input | bash completes | huck today |
|---|---|---|
| `if whi` | commands | nothing |
| `echo "$(whi` | commands (inside the quoted comsub) | nothing |
| `echo $(( HO` | variables (arithmetic) | nothing |
| `for x in whi` | words / files | nothing |

Iteration 1 ([#246](https://github.com/jdstanhope/huck/issues/246), merged) gave
the parser an error-recovery capability: `huck_syntax::parse_recover(src)` parses
a line truncated at the cursor and returns a `CursorContext { enclosing,
position, word, word_start }` derived structurally. This iteration (2) deletes
`analyze_full` and drives completion from `CursorContext`, fixing the four
divergences by construction.

## Design

### Architecture

`dispatch::resolve` (huck-engine) stops calling `analyze_full` and instead calls
`huck_syntax::parse_recover(&line[..pos])`, then maps the returned
`CursorContext` to a completion source. `analyze_full`, `analyze`, the
`CompletionContext` enum, and the `~30` `analyze_*` unit tests are deleted. Two
pieces:

1. **huck-syntax** — extend the `CursorContext` capture to emit
   `WordPosition::RedirectTarget` when the cursor word is a redirect operand.
2. **huck-engine** — a new mapper `cursor_to_completion` replacing `analyze_full`
   plus the context-branching in `dispatch::resolve`.

The existing candidate builders (`complete_command` / `complete_variable` /
`complete_file`), the programmable-spec path (`run_spec_with_empty_fallback`),
the basename-display logic, and the trailing-space decoration all stay unchanged
— only the *context determination* changes source.

### huck-syntax: `RedirectTarget` capture

The parser already parses redirects, so it knows when the cursor word is a
redirect operand. Extend the position capture to set
`WordPosition::RedirectTarget` for that case.

**`RedirectTarget` is the LOWEST-priority position.** A redirect target is a
*word*, and a word can contain expansions; the cursor's position is whatever
construct it is *innermost* in. The priority order (already how iteration 1
captures position — `RedirectTarget` slots in at the bottom):

1. inner `ParamExpansion` / `Arith` / `$name` → `VariableName`
2. inner `CommandSub` / backtick / subshell → `Command`
3. grammar slot: word 0 of a simple command → `Command`; later word →
   `Argument`; `NAME=(` element → `AssignRhs`; **redirect operand →
   `RedirectTarget`**

So `cat foo > whi` → `RedirectTarget`, but `cat foo > ${HOM` → `VariableName`,
`cat foo > $(whi` → `Command`, `cat foo > $(( HO` → `VariableName` — the inner
expansion wins. Verified against the merged iteration-1 capture: `$HOM`, `$(whi`,
`$(( HO` inside a redirect already report the inner-mode position; only the bare
`cat foo > whi` case (currently `Argument`) becomes `RedirectTarget`, and both
map to file completion, so this is additive, not a behavior change for the
mapper.

### huck-engine: the `cursor_to_completion` mapper

A function in `completion.rs`:

```rust
fn cursor_to_completion(
    cursor: &huck_syntax::CursorContext,
    line: &str,
    pos: usize,
    shell: &mut Shell,
) -> (usize, Vec<Candidate>);   // (replacement anchor, candidates)
```

Rules — each pinned against bash 5.2.21:

| `position` | `word` contains `/`? | source |
|---|---|---|
| `Command` | no | command completion (PATH + builtins + functions + aliases) |
| `Command` | yes | file completion (a command word with `/` is a path — `./whi`, `/usr/bin/l`) |
| `Argument` | — | file completion **+ programmable-spec lookup** (`run_spec_with_empty_fallback` by command name) |
| `VariableName` | — | variable completion |
| `RedirectTarget` | — | file completion |
| `AssignRhs` | — | file completion (array element / value) |

**Anchoring and the dir/prefix split.** For every file-completion case the
mapper splits `cursor.word` on the last `/`: `dir` is everything through it,
`prefix` is the basename, and the replacement anchor is
`cursor.word_start + (byte offset past the last '/')`. This reproduces the second
offset `analyze_full` returned, so the existing basename-display and
trailing-space logic keep working unchanged. Command / variable completions
anchor at `cursor.word_start`.

**Programmable completion is unchanged.** The `Argument` path still runs
`run_spec_with_empty_fallback` with the command name (`extract_command_name` on
the line) and COMP_WORDS from the line — the mapper only decides *that* we are in
argument position, not how the spec executes.

**`enclosing` is not read by the mapper** for these cases: `position` already
encodes command / variable / file, and `word` / `word_start` give the prefix and
anchor. It stays in `CursorContext` for future richer use.

### The four fixes fall out

- `if whi` → `position == Command` → command completion.
- `echo "$(whi` → `position == Command` (the double-quote no longer blocks the
  `$(` — the parser sees the command substitution) → command completion.
- `echo $(( HO` → `position == VariableName` (`enclosing` ends `Arith`) →
  variable completion.
- `for x in whi` → `position == Argument` → file completion.

## Testing

The gate is **bash 5.2.21 fidelity**, not scanner-preservation. `analyze_full`'s
deleted unit tests are not carried forward verbatim; the ones encoding correct
behavior are re-expressed as mapper/dispatch tests, and the four encoding the
divergences are dropped (they were wrong).

1. **Mapper/dispatch unit tests** (`completion.rs`) — the primary gate. Each runs
   the real `parse_recover` → `cursor_to_completion` path in-process via
   `dispatch::resolve` (or the mapper directly) and asserts the candidate kind +
   anchor:
   - the four fixes: `if whi` → a command candidate (e.g. `while`/`which`);
     `echo "$(whi` → a command candidate; `echo $(( HO` → a variable candidate
     (e.g. `HOME`); `for x in whi` → file candidates.
   - must-not-regress: `echo > whi` → files; `./whi` → files (path); `cat src/fo`
     → dir-split file candidates anchored after `src/`; `echo $HO` → variables;
     bare-word command → commands; `cat foo > ${HOM` → variable (the redirect-
     target-with-inner-expansion priority case); an argument with a registered
     `-W` spec → the spec's words.
2. **`-p huck` completion integration binaries** (`completion_integration`,
   `complete_actions_integration`, `arith_completion_integration`) — must stay
   green. They mostly assert `compgen`/`complete` **stdout** (unaffected — the
   mapper is tab-dispatch only); update any that drive interactive Tab context to
   the corrected behavior.
3. **PTY spot-check vs bash 5.2.21** — the four fixes and the key regressions
   (`echo > whi`, `./whi`, `cat foo > ${HOM`) driven through the real binary
   under a pty (as the #236/#237/#244 fixes were verified), confirming byte-level
   parity.

Per the repo's constraints: run tests per-crate
(`cargo test -p huck-syntax` for the capture extension;
`cargo test -p huck-engine` / `-p huck-cli` for the consumer;
`--jobs 1 --lib -- --test-threads 1`), run the `-p huck` completion integration
binaries single-threaded before pushing, and the full
`tests/scripts/run_diff_checks.sh` sweep must stay green (unaffected — completion
is interactive, not `-c`).

## Scope

**In scope.** The `RedirectTarget` capture extension (huck-syntax); the
`cursor_to_completion` mapper (huck-engine); deletion of `analyze_full` /
`analyze` / `CompletionContext` and their unit tests; rewiring
`dispatch::resolve`; the four divergence fixes; the tests above.

**Out of scope / deferred (follow-up issues, NOT regressions — huck already has
these gaps).**
- Scalar assignment-value completion (`x=my` → complete the value as a file).
  huck currently command-completes `x=my`; the mapper keeps that (it is
  `position == Command`, word `x=my`), so no regression. bash completes the
  value.
- Nested-command programmable-spec lookup inside `$(…)` — using `git` (not the
  outer `echo`) as the command for `echo $(git <TAB>`. `analyze_full` did not do
  this either.
- The other iteration-1 deferred limitations that this iteration does not need:
  backtick-body coarse capture, `case $x in <partial-pattern>` → `tree == None`
  (the cursor context is still captured, so completion still works there).

## Documentation

`docs/architecture.md`'s completion section: note that completion context is now
derived from `huck_syntax::parse_recover` (the recovery parser) rather than a
hand-rolled scanner, and that the four command-substitution/arithmetic/compound
divergences are fixed. Remove any lingering reference to `analyze_full`. #248 is
tracked in the GitHub issue tracker and auto-closes via the PR body.
