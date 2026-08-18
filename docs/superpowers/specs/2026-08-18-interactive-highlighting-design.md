# v363 — Interactive syntax highlighting

Issue: [#666](https://github.com/jdstanhope/huck/issues/666)

## Problem

huck's line editor shows the command line as undifferentiated text. Everything
that would help while typing — that a command does not exist, that a quote is
still open, where an expansion begins and ends, which `)` closes which `(` — is
invisible until the line is submitted and fails.

This is also the first feature whose purpose is to be BETTER than bash rather
than identical to it, which changes how it must be verified (see Verification).

The compat floor is good enough to build on: `tools/runsweep` (2026-08-17)
puts 95.9% of testable real scripts byte-identical with bash.

## What the user sees

Only the PROBLEM is signalled. A valid command stays plain — a line where
everything is coloured reads as noise, and fish's restraint here is the reason
its highlighting is legible.

| region | treatment |
| --- | --- |
| **invalid command word** | red |
| valid command word | plain (deliberate) |
| single-quoted run | quote colour A |
| double-quoted run | quote colour B |
| `$var`, `${var…}` | region tinted, **variable NAME bold** |
| operators, redirections (`\|`, `&&`, `>`, `2>&1`) | structural, dim |
| reserved words (`if`, `for`, `do`, `done`, …) | distinct from commands |
| comments | dim |
| `$( … )`, `` ` … ` ``, `$(( … ))`, `<( … )` | region tinted; the command INSIDE is itself validity-checked |
| globs (`*`, `?`, `[a-z]`) | distinct — colour only, no match-checking |
| escapes inside `"…"` (`\$`, `\"`, `` \` ``, `\\`) | distinct |
| bracket matching the one under the cursor | emphasised |
| the dangling opener of an unterminated construct | marked |

The last row is the one no other shell can do as precisely: v362 built a model
of exactly which pair an unterminated construct belongs to and where it opened.

## Architecture

### The token source is a PARSE, not a lex

The lexer cannot be driven standalone. This is documented in `lexer.rs` and was
re-confirmed empirically for this design: a probe pulling tokens without a
parser re-emitted `BeginDquote` forever, because the opener signals (`$(`,
`${`, `$((`, `"`, `` ` ``) are zero-width and the PARSER is what consumes them
and pushes the matching mode.

So the highlighter calls `parse_sequence` and reads the token stream that parse
drove. Two properties make this cheap and safe:

- `parse_sequence(iter: &mut Lexer)` takes ONLY the lexer, and `huck-syntax`
  has no dependency on `huck-engine`. The syntactic pass is a pure function of
  `(text, aliases)` — no `Shell`, no borrow, no side effects.
- An incomplete line (the usual state while typing) stops parsing early, so the
  common case is the cheapest.

**Aliases are passed EMPTY.** Read-time alias expansion would splice tokens
whose spans point into the alias body rather than the typed line, producing
garbage offsets. Highlighting shows what was typed.

### What the stream already carries, and the two gaps

Available directly, verified by dumping tokens against source offsets:

- `QuoteRun { style: Single, text }` — a single-quoted run arrives as ONE token
  WITH its style;
- `BeginDquote` … `EndDquote` — a double-quoted run is a FRAME around its
  contents, so expansions inside it remain separately visible (this asymmetry
  with single quotes is load-bearing for the design, not an accident);
- `DollarName { name }`, `ParamOpen`/`ParamName`/`ParamOp`/`ParamClose`,
  `CmdSubOpen`, `ArithOpen`, `LegacyArithOpen`, `BeginBacktick`/`EndBacktick`,
  `ProcSubOpen`, `Op(Operator)`, `RedirFd`, `Tilde`, `Blank`, `Newline`.

Two gaps, both closed at the one place that already knows:

1. **Command position.** Nothing marks "this token is the command word". The
   lexer tracks it in v361's `CommandPos` state machine. It is EXPOSED rather
   than reconstructed in the highlighter — reconstructing it would be a second
   copy of that state machine (#641's rule against parallel structures), and it
   would drift.
2. **Comments are not tokenised at all** — they are skipped. They need a token.

### The side channel

An opt-in recorder, off by default, that the lexer/parser append to while
parsing: per token a `(span, role)`, and per CLOSED pair an `(open_off,
close_off)`.

The pair records are what make bracket matching possible, and they are nearly
free: v361 put `open_off` on every `ModeFrame` and frames are popped at their
closer, so both ends are in hand at pop time.

`Span` carries only a START (no length), so **extents come from consecutive
token starts**; `Blank` and `Newline` are real atoms, so the stream tiles the
line and the last token runs to end-of-input. This is a new invariant and is
tested directly.

### Roles, not colours

The recorder emits semantic ROLES (`CommandWord`, `QuotedSingle`,
`QuotedDouble`, `VarName`, `Operator`, `Comment`, `Glob`, `Escape`, …). The
mapping from role to SGR lives in the CLI crate, so the palette is one table
and `huck-syntax` never learns about colour.

## The semantic layer

Validity colouring needs shell state, so the helper holds the same
`Rc<RefCell<Shell>>` the completer already does — but takes `borrow()` only:
looking a command up must not mutate (no hit-count bump).

Measured costs make the caching policy, not guesswork:

| | |
| --- | --- |
| command hit (hashed, or early in PATH) | ~0 us |
| **miss** (any prefix still being typed) | 90-160 us |
| 6-stage pipeline, all unknown | ~940 us per keystroke |

42 `PATH` segments are stat'd on every miss, and #655's command hash table does
NOT help: it knows commands that have been RUN, while the highlighter sees
names that have only been TYPED — of `g`, `gi`, `git`, two are misses.

So the highlighter keeps **its own cache, positive AND negative**:

- invalidated when `PATH` changes (#655 already added the single chokepoint,
  `invalidate_command_hash_if_path`), and
- cleared at each prompt, which bounds staleness to one command line so a
  freshly installed program is picked up on the next line.

**Slow-filesystem guard.** An NFS or automount entry in `PATH` turns 42 stats
into visible lag. If a lookup exceeds a threshold, validity colouring for that
word is skipped rather than stalling the keystroke — highlighting degrades, the
editor never does.

## Cost budget (measured, release build)

Syntactic pass:

| | |
| --- | --- |
| typical line (29-69 chars) | 4-13 us |
| worst single keystroke, 80-char line | 18 us |
| whole typing pass, 80 keystrokes | 716 us total |
| pasted 8000-char line | 1.8 ms |
| incomplete line (usual while typing) | 1.1-5.3 us |

Linear, ~0.2 us/char. Against a 16 ms frame that is ~800x headroom, so the
design is SYNCHRONOUS: no async, no debounce, no incremental reparse. The
semantic layer dominates by ~60x and is where the caching effort goes.

## rustyline constraints

- `Highlighter::highlight(&self, line, pos) -> Cow<str>` must return the same
  DISPLAY WIDTH — SGR sequences only, never inserted or removed characters.
- `highlight_char(&self, line, pos, kind)` defaults to FALSE; unimplemented,
  nothing ever re-renders. It also carries `CmdKind::MoveCursor`, which is what
  makes bracket matching on cursor movement possible.
- `HuckHelper` already implements `Highlighter` as an empty default impl, so
  the wiring point exists.

## Verification

**This is the part with no precedent.** Every quality gate in this project
diffs against bash: the 309-harness sweep, the runtime sweep, the parse sweep.
NOTHING here has a bash to diff against.

The only existing infrastructure that can test rendered output is the 11
`tests/*_pty.rs` expectrl tests. So a pty harness asserting on RENDERED output
— escape sequences and cursor position — is the FIRST deliverable, not an
afterthought. Without it the differentiating features become the least-tested
code in a project whose culture is measurement.

Three layers:

1. **Unit** — `(text) -> Vec<(span, role)>` is a pure function and is tested as
   one, including the extent-tiling invariant and every construct in the table.
2. **pty** — the rendered line for a given input, byte for byte, including the
   cursor-movement bracket case.
3. **Inertness** — highlighting must not change what PARSES or RUNS. The parse
   sweep (3103 scripts) and the full diff-check sweep must be unchanged, and
   the side channel must be provably off when not highlighting.

Also required and absent today: `NO_COLOR`, not-a-tty suppression, and a shell
option to disable.

## Out of scope

- **Glob match-checking** (fish reddens a glob that matches nothing) — a
  directory read per keystroke, on top of a cost that already dominates.
- **The completion popup.** rustyline has no menu widget, and replacing it
  would cost the `bind` compatibility that `readline_apply.rs` and
  `bind_pty.rs` exist to provide. That decision is deliberately separate.
- Themes/user-configurable palettes. One built-in palette; `NO_COLOR` off.

## Risks

- **The side channel touches the lexer and parser**, which are the most
  load-bearing code in the project. It must be provably inert when off — the
  parse sweep is the gate, and it has caught exactly this class before.
- **Extent-by-consecutive-starts** is a new invariant. A token that does not
  tile (a zero-width signal, a synthesized token with `Span::unknown()`) must
  be handled explicitly rather than producing a zero-length or overlapping
  paint region.
- **Command-position exposure** must not become a second state machine. If it
  cannot be exposed cleanly from `CommandPos`, that is a signal to stop and
  reconsider rather than to reconstruct it in the highlighter.
