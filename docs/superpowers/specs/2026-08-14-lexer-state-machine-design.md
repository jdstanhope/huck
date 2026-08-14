# v361 — The lexer's state belongs in one automaton

Issue: [#641](https://github.com/jdstanhope/huck/issues/641)

## Problem

The lexer is a pushdown automaton in principle: `modes: Vec<Mode>` with `Command`
as the floor, the parser pushing a mode and pulling one atom per call. Nesting —
a command substitution inside a double quote — is the stack. That part is sound,
and it is what replaced the older design where the lexer scanned ahead balancing
quotes to build whole words.

A large part of the state never made it into that automaton. It lives beside it,
as booleans, and the parser reaches in to set them.

Measured on `main`:

| | count |
| --- | --- |
| fields on `Lexer` | 39 |
| of those, plain `bool` | 13 |
| `Mode` variants | 13 |
| fields carried by those variants | ~25, mostly bool |

`Mode::Command` is the worst of it: a single fieldless variant whose sub-states
live as parallel lexer bools — `cmd_at_word_start` (27 touches),
`in_assignment_value` (13), `assign_val_tilde_ok` (12), `has_token` (7),
`expect_regex`. Five booleans is 32 representable combinations for what is really
one position in a small state machine; nearly all of them are meaningless, and
nothing prevents reaching one.

Two structural problems ride along with it:

- **The parser pokes lexer state directly** — `set_regex_body_started`,
  `set_force_extglob`, `set_param_start_off_from_cursor`, `set_in_dquote`,
  `set_retokenize_arith_as_cmdsub`, `set_recovery_cmd_word`,
  `set_recovery_redirect_target`. `cmd_at_word_start` has 9 parser references,
  `recovery_cmd_word` 11, `brace_expand` 11. A clean automaton has one entry
  point: push a parameterised mode, pull atoms.
- **Parallel structures.** `push_mode` pushes `modes` and `mode_open_offs`
  together and `pop_mode` pops both, with nothing preventing drift. Separately,
  `pending_heredocs` is vestigial — never written any more, the only non-`atom_`
  mutation site left is an `.iter()` read — while `atom_pending_heredocs` is the
  live queue.

## Why now

Every EOF-reporting divergence chased in v360 ([#635](https://github.com/jdstanhope/huck/issues/635))
came from a nested lexing context encoded as a flag rather than a pushed state.
`Mode::Arith` tracks quoting with `in_squote`/`in_dquote`, so it knows a quote is
open but not *where* — and `quote_open_off`, `span_opener_off` and `err_open_hint`
accreted around that gap. A `'` inside a `${…}` operand has no state at all, so it
can be neither named nor allowed to affect what terminates the expansion.

v360's answer was a `PairStack` pushed in lockstep with `modes`. That is a second
parallel stack — the exact pattern rule 3 below forbids — so it is being dropped
rather than extended. If the nested contexts were frames, the mode stack would
already answer the question.

## Scope

**No detectable behaviour change.** The same tokens, the same errors, the same
messages, the same exit statuses, on every input. This is a re-encoding of state,
verified as inert the way v360's Phase 0 was.

### Rules this iteration establishes

1. **Control state is the automaton.** Exactly one state at a time; nesting is a
   stack of states, never parallel flags. A mode's sub-states become variants or
   parameters of that mode.
2. **Transitions are enforced, not assigned.** State changes through a method that
   knows which transitions are legal. An impossible combination should be
   unrepresentable; where it cannot be made unrepresentable, an illegal transition
   must fail loudly in debug builds.
3. **No parallel structures.** Two vectors pushed in lockstep, two counters
   incremented together, or a counter moved alongside a vector push/pop are the
   same bug waiting to happen. If two values must move together, they are one
   value: one structure whose elements carry both.
4. **Data on frames is fine.** `paren_depth`, `start_off`, `delim` are values a
   frame carries. The rule is that the STATE is the frame, not that frames are
   empty.
5. **Options and instructions are not state.** `brace_expand`, `extglob` are
   shopts; `replay` is a construction kind; `retokenize_arith_as_cmdsub` is a
   one-shot instruction. They live outside the automaton — `LexerOptions` already
   exists as that home.
6. **Inherited context is derived, not copied.** `enclosing_dquote` is duplicated
   into four operand variants and `opts.in_dquote` is context stored as
   configuration. With a real stack, look down it.

### Explicitly out of scope

- **Any behaviour change.** #631 and #640 — a `'` inside a double-quoted `${…}` —
  would fall out of promoting quote spans to frames, and that is tempting, but it
  changes what parses. It is a separate decision and a separate iteration; this
  spec's value depends on being provably inert.
- **[#493](https://github.com/jdstanhope/huck/issues/493)**, huck parsing `${…}`
  eagerly where bash defers. That is the opposite mismatch — frames huck has that
  bash does not — and is far larger than a state re-encoding.
- **The `Delim`/EOF reporting model itself** (#635). Once the state is in one
  place, that question gets easier; it is not answered here.

## Design

### The state, sorted

Each of the 13 lexer bools and ~25 mode fields lands in exactly one bucket. The
sort is the design; the mechanical work follows from it.

| bucket | goes where | examples |
| --- | --- | --- |
| control state | a mode variant or parameter | `cmd_at_word_start`, `in_assignment_value`, `has_token`, `expect_regex`, `assign_val_tilde_ok`, `body_started`, `backtick_raw_started`, `seen_name`, `at_element_start`, `expect_subscript_eq` |
| frame data | a field on the frame | `paren_depth`, `start_off`, `delim`, `subscript_append` |
| not state | `LexerOptions`, or an explicit parameter | `brace_expand`, `extglob`, `replay`, `retokenize_arith_as_cmdsub`, recovery hints |
| inherited context | derived by inspecting the stack | `enclosing_dquote`, `opts.in_dquote` |

### `Mode::Command` becomes a state machine

The five command-position booleans become one `CommandPos` carried by
`Mode::Command`. The legal transitions are the design artifact — the point is not
that the enum is tidier, it is that `at word start AND inside an assignment value`
stops being representable.

The transition method is the only way to move: no assignment to a position field
from outside the state module, so every move is checked in one place.

### The parser's entry points shrink

Each `set_*` the parser calls today is either (a) a mode parameter that should be
supplied when the mode is pushed, (b) an option, or (c) a one-shot instruction.
The end state is that the parser pushes a parameterised mode and pulls atoms, and
`recover.rs` keeps its own hints as parameters rather than lexer fields.

This is where the iteration could grow without limit, so it is bounded: the
`set_*` surface must shrink, and each removal must be justified by the bucket it
lands in, but a `set_*` that resists re-encoding stays and is documented rather
than forced.

### Parallel structures removed

- `mode_open_offs` merges into the mode stack: one stack whose frames carry their
  own opening offset. This is the change v360's Task 3 was reaching for, done
  without a second stack.
- `pending_heredocs` is deleted along with its now-constant reads, once its
  never-written status is confirmed by making it fail loudly rather than by
  inspection.

## Verification

The bar is the one v360's Phase 0 met, because "no detectable behaviour change" is
the entire claim:

- **Zero expected-value edits.** `git diff main -- '*tests.rs' 'tests/'` empty. A
  behaviour-preserving refactor that needed a test changed is not one.
- **`tools/param_corpus.sh`** — 250 `${…}` forms including one unterminated row per
  operand mode — byte-identical between a binary built at the branch point and the
  refactored one. That corpus exists because the earlier single-exit refactor
  passed every other gate while panicking on `echo ${x:1`.
- **`tools/parse_sweep.sh`** — 3103 real scripts, `bash -n` vs `huck -n` —
  identical to the same baseline.
- **`tools/eof_matrix.sh --check`** — 813 EOF-reporting cells: 0 FIXED, 0
  REGRESSED. Any movement means behaviour changed.
- Full `tests/scripts/run_diff_checks.sh` green, every `-p huck` integration
  binary, both `--lib` suites, pinned clippy clean.
- Each step lands separately and is inert on its own. A step that cannot be shown
  inert is the signal that it is a behaviour change in disguise.

## Risks

- **Scope creep through the `set_*` surface.** Mitigated by the bound above: a
  setter that resists re-encoding is documented, not forced.
- **A "tidier" enum that changes behaviour.** The five command-position bools
  encode reachable states that may not be obviously legal; collapsing them can
  silently drop one. The inertness gates are the defence, and the corpus is the
  one that has already caught this class once.
- **`Mode` is `Copy` and cloned by `mark`/`rewind`.** Growing frames has a cost on
  a hot path; if a frame gains enough data to matter, that is a signal the data
  belongs elsewhere, not that the rule should bend.
