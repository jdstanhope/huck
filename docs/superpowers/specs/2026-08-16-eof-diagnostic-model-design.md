# v362 — The EOF diagnostic model

Issue: [#643](https://github.com/jdstanhope/huck/issues/643)

## Problem

When input ends inside a construct, bash names the innermost still-open **matched
pair** and reports it at that pair's line. huck names whichever `LexError`
variant happened to be raised (`lex_is_shape3` maps variant → `Delim`) and takes
the line from the innermost mode frame. Those are different questions, and every
divergence here is a place where the answers differ.

The space was measured exhaustively in
[v360's spec](2026-08-13-eof-delimiter-model-design.md): 165 depth-1 cells and
648 depth-2 cells against bash 5.2.21, **78 divergent**. That measurement stands
— `tools/eof_matrix.sh --check` has reported 0 FIXED / 0 REGRESSED on every task
of v361, which re-confirms the baseline.

v360 proposed a `PairStack` maintained alongside `modes`. That was rejected as a
parallel structure ([#641](https://github.com/jdstanhope/huck/issues/641) rule 3)
and the iteration parked. v361 then moved each construct's opening offset **onto
its frame**, which is what makes this iteration possible without a second stack:
the model becomes a function over the frame stack that already exists.

## Scope

**In**, ~66 of the 78 divergent cells plus one shape no cell reaches:

| issue | what | cells |
| --- | --- | --- |
| [#627](https://github.com/jdstanhope/huck/issues/627) | inside an arithmetic body, `${` and `$[` open no pair — bash names the arithmetic delimiter | ~30 |
| [#634](https://github.com/jdstanhope/huck/issues/634) | `${` with `$(`/`$((`/`${` in name position: the inner pair is uncounted, and validation runs before EOF is noticed | ~21 |
| [#633](https://github.com/jdstanhope/huck/issues/633) | an unterminated `name=( … )` — message shape, line, and exit status | ~15 |
| [#629](https://github.com/jdstanhope/huck/issues/629) | a `$((` re-read as a command substitution loses the arithmetic's opening line | none |

**Out**, and each for a reason rather than for convenience:

- [#631](https://github.com/jdstanhope/huck/issues/631) /
  [#640](https://github.com/jdstanhope/huck/issues/640) — a `'` inside a
  double-quoted `${…}` is a matched pair that swallows the `}`. `echo "${x:-'y}"`
  prints `'y` in huck and is a syntax error in bash. That changes what PARSES,
  not what is reported; it is a separate decision. 6 cells.
- [#628](https://github.com/jdstanhope/huck/issues/628) — a `for (( … )` closed
  by one `)` is a near-token error, a different message shape entirely.
- The `[[ … ]]` conditional wording and `echo (` read as a function definition —
  neither is Shape 2 nor a matched pair. 6 cells.

## Design

### The walk

`reported_pair()` walks the frame stack from the innermost outward and returns
the pair bash would name. **No new storage**: every input it needs is already on
the frames after v361.

The suppression table is the one v360 measured; it is restated here only as the
rules the walk applies:

1. Inside an arithmetic body (`$((`, `$[`, `((`, `for ((`), `${` and `$[` open no
   pair — but `$((` does. Measured, not assumed: `$[1+$((2+` names the inner `)`
   while `$((1+$[2+` names the *outer* one.
2. Inside `'…'`, nothing opens a pair.
3. Inside `"…"`, a `'` opens no pair.
4. A backslash-escaped `\"` or `\'` never opens a pair, in any context.
5. At EOF the pair report wins over construct validation.

### The two frame-less pairs

A `'` run is scanned as one atom and has no frame. A quote span inside an
arithmetic body is not a frame either — it is `in_squote`/`in_dquote`/
`quote_open_off` **on** `Mode::Arith`.

Both are handled without new state:

- On reaching an `Arith` frame with a live quote span, the walk reports **the
  quote**, using the offset the frame already carries.
- When the walk finds no pair at all, the failing atom reports itself: its own
  start offset, delimiter from the error variant. That is exactly what a
  top-level `'` run does today, now stated as a rule rather than a fallback
  someone has to infer.

`err_open_hint` — added in #621 as a side channel for the arith quote span — is
**deleted**. `span_opener_off` stays: it is what keeps `quote_open_off` correct
for an escaped quote, which rule 4 depends on.

### Variant decides the shape, pair decides the delimiter

`lex_is_shape3` keeps answering *is this an open-delimiter EOF* — the Shape 3
(`unexpected EOF while looking for matching X`) versus Shape 2
(`syntax error: unexpected end of file`) split — and **stops mapping variants to
delimiters**. The lexer records the reported pair at raise time, exactly as it
records `err_open_off` today, and the four callers that already ask it for the
line ask for the delimiter too: one added parameter on `render_syntax_diag`.

No `LineRule` type is needed, unlike v360's design. "`$(` reports where input ran
out, everything else reports where it opened" is a property of the delimiter and
stays keyed on `Delim` in the renderer.

### The three that need more than the walk

- **#633.** `ArrayLiteral` becomes a reportable pair. It needs a `Delim` that
  spells `)` **and** is Shape 3 — `Delim::Paren` exists but is deliberately Shape
  2, because a subshell `( echo` is `syntax error: unexpected end of file` in
  both shells. The exit status (bash 1, huck 2) is in scope; *why* bash says 1 is
  not yet known and must be measured before it is implemented. If it reaches
  further into the v358 fatality classifier than expected, stop and report rather
  than widening the iteration silently.
- **#629.** The `$((` → `$( (` re-read rewinds away the arithmetic frame and
  pushes `CommandSub` at the same offset — so the offset is already right and
  only the rule is wrong, because `$(` reports where input ran out. This needs a
  marker on that frame saying it came from a re-read `$((` and reports its
  opening line. That gives `Mode::CommandSub` a field again, one iteration after
  v361 made it a unit variant; it is frame data and belongs there.
- **#634.** Two causes behind one symptom. The inner pair being uncounted is the
  walk's job. "Validation runs before EOF is noticed" needs somewhere to hang a
  drain: on a validation failure, scan to the pair's close, and if input runs out
  the lex error wins. That requires a single exit in `parse_param_expansion`,
  which today has eleven.

### The refactor, first and separately

The single-exit conversion is **not** a behaviour change and must not be mixed
with ones that are. It lands as the first task with its own inertness gates, and
only then does anything behavioural build on it.

It was done once already, on the parked v360 branch, and is redone fresh here
rather than cherry-picked: the branch has diverged, and a fresh conversion is
easier to review than a cross-branch pick. Two traps are known from that attempt
and are called out in the plan:

- two operand arms (substitute-pattern, substring-offset) pop the frame
  themselves with a bare `pop_mode(); pop_mode();`. They do not match the
  `restore_dq!()` pattern a search finds, and leaving them in place after the pop
  moves takes out the `Command` floor — `echo ${x:1` panicked with *"Command is
  the floor and must never be popped"*.
- that panic passed every gate except the corpus, which had no unterminated row
  for those operand modes. It has one per operand mode now.

## Verification

- **`tools/eof_matrix.sh --check`** — reports what left the 78-cell divergent set
  and what joined. **Nothing may join**, on any task.
- **`tools/param_corpus.sh`** — 250 `${…}` forms, byte-identical for the refactor
  task; expected to change only where an in-scope issue says it should afterward.
- **`tools/parse_sweep.sh`** — 3103 real scripts, identical for the refactor task.
- **Hand-written rows** for what 813 single-line cells cannot see: which line each
  pair reports (multi-line inputs), #629's `$((1+2)` shape, #633's exit status,
  and the piped-stdin driver, which re-lexes the buffer through a different
  top-level path.
- **Shape 2 must not move** — `if`, `while`, `case`, `{ }`, `( )` and function
  bodies keep `syntax error: unexpected end of file`. The same `(` proves the
  boundary is real: a subshell is Shape 2, an array literal is Shape 3.
- Full sweep, both `--lib` suites, every `-p huck` integration binary, pinned
  clippy.

## Risks

- **The exit-status half of #633** is the one item whose blast radius is unknown
  until measured. It is explicitly allowed to become its own issue if the
  classifier resists.
- **`Delim` gains a variant**, and `is_matching_delim` decides Shape 2 vs Shape 3
  from it. A wrong answer there moves a construct between message shapes, which
  the Shape 2 control rows exist to catch.
- **The walk is read on every lex error**, including ones that are not EOF at all.
  It must be cheap and must not change what non-EOF errors report — the corpus,
  whose 26 nonzero-status rows are mostly non-EOF, is the check.
