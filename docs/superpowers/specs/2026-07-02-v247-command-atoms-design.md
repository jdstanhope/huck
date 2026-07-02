# v247 — Command-mode-emits-atoms (pure mechanical inversion, dormant) — design

**Date:** 2026-07-02
**Status:** approved (brainstorm), pending implementation plan
**Arc:** Phase C, **Stage 2, step 1 of 3**. Stage 1 (dormant word-level modes) is
complete: v240 mode-stack + mark/rewind, v241 `${…}`, v242 flat command list,
v243 compounds, v244 `$(…)`, v245 backtick, v246 `$((…))`. This is the first step
of Stage 2 (the cutover). See [[huck-lexer-rearch-design]] /
[[huck-frontend-parser-driven-direction]].

## Goal

Make `Command` mode able to emit small **atoms** (instead of pre-built `Word`
tokens), and make `parser.rs`'s command parser assemble command-position Words
from those atoms — producing ASTs **byte-identical** to the production
`command.rs` path. This is a **pure mechanical inversion**: v247 adds NO new
grammar. It is DORMANT (production stays on Word-mode + `command.rs`) and gated by
a comprehensive old-==-new differential harness. It lays the atom foundation that
later steps build on (step 2: complete the deferred constructs; step 3: flip the
live path + delete `command.rs` and the forward-scanning scanners + return to the
bash test suite).

## Why (the arc context)

`command.rs` is built around whole `Word` tokens; the fat forward-scanning
scanners (`scan_dollar_expansion`, `scan_arith_body`, `scan_backtick_body`,
`scan_braced_param_expansion`, …) pre-build Words and pre-disambiguate `((`. The
end-state (delete those scanners, one parser-driven front-end) requires `Command`
mode to emit atoms and the parser to own word assembly. v247 builds that
capability behind the differential gate without touching the live path — the same
dormant+differential discipline every Stage-1 iteration used.

## Core rule (unchanged, non-negotiable)

The lexer emits small atoms and NEVER scans ahead for a matching delimiter;
forward-only, at most a bounded local peek. The PARSER owns word assembly,
delimiter-matching, and recursion (via mode push/pop + mark/rewind). Production
scanners stay UNTOUCHED; the atom path is parallel and dormant.

## Scope

**In scope — must be byte-identical to `command.rs` (covered by `diff_cmd`):**
- Simple commands: bare and multi-word, every quoting form (`"…"`, `'…'`, `$'…'`,
  backslash escapes), every command-position expansion (`$x`, `${…}`, `$(…)`,
  `` `…` ``, `$((…))`, `$?`/`$@`/`$*`/positional, tilde).
- Scalar assignments as command-prefix and standalone: `x=v`, `x+=v`, `a[i]=v`
  (the value is an ordinary word; the array-literal RHS `a=(…)` is deferred).
- Pipelines (`|`), and-or lists (`&&`/`||`), separators (`;`, `&`, newline).
- Redirects in source order, including fd-prefixes `3>` / `{fd}>` and all
  operators (`>`, `>>`, `<`, `<&`, `>&`, `<>`, `&>`, `>|`, …). Heredoc *openers*
  are recognized but the body stays deferred (see below).
- Comments (`#…`), line continuations (`\<newline>`), blank lines.
- The compounds `parser.rs` already handles: `if`/`while`/`until`/`for`
  (list form)/`case`/`select`/subshell `( )`/brace-group `{ }` — with their
  command words now assembled from atoms.

**Deferred in v247 — atom path returns `UnsupportedCommand` (same as today);
asserted via `diff_unsupported`, excluded from the byte-identical corpus:**
- The arith command `(( ))` and C-style `for (( ))` (the `((` unlock — next step).
- `[[ ]]` test expressions.
- Heredoc bodies and here-strings (`<<<`).
- Function definitions, coproc.
- **Array literals `a=(…)`** (own element sub-grammar; needs an `ArrayLiteral`
  atom mode — follow-on).
- **Command-position alias expansion** (read-time alias re-lex vs the atom pull —
  follow-on).

## Architecture

### Coexistence: a construction-time selector

A `Lexer` gains a construction-time flag `command_atoms: bool` (default `false`).
`scan_step`'s `Mode::Command` arm dispatches on it:
- `false` → today's `scan_step_command` (emits `Word` tokens) — PRODUCTION, untouched.
- `true` → new `scan_step_command_atoms` (emits atoms + structural tokens).

`Mode::Command` remains the stack floor; only the scanner implementation is
selected. The flag threads through `scan_step_command_sub` (so a `$(…)` body under
the atom path also scans as atoms) — carried on the `Lexer`, not per-mode, since
it is a whole-lexer policy. A new `Lexer` constructor / builder sets it (the
differential harness + the eventual live flip use it; production does not).

### New atom: the `Blank` boundary

`TokenKind::Blank` — emitted by `scan_step_command_atoms` for a run of UNQUOTED
inter-word whitespace (spaces/tabs). It marks a word boundary so the parser can
tell `a b` (two words: `Lit(a) Blank Lit(b)`) from `ab`/`a"b"$c` (one word: no
`Blank` between parts). It NEVER appears in the production Word stream. Operators,
newlines, and redirect tokens are already boundaries and need no `Blank`.

No new `Mode` variant is required: the existing word-level modes (`ParamExpansion`,
`CommandSub`, `Backtick`, `Arith`) are reused verbatim when the atom-command
scanner hits `${`/`$(`/`` ` ``/`$((` (it emits the same opener signals the operand
scanner does; the parser pushes the sub-mode).

### `scan_step_command_atoms`

Mirrors `scan_step_command`'s control flow but, where the current scanner would
build and emit a whole `Word`, it instead emits atoms one at a time:
- Ordinary word text → accumulate into a `Lit { text, quoted }` atom (respecting
  `'…'`/`"…"`/`$'…'`/backslash quoting, exactly as the fat scanner does — same
  `quoted` bookkeeping).
- `$name`/`${`/`$(`/`$((`/`` ` ``/`~` → the corresponding atom / opener signal,
  reusing the established v241–v246 atoms (`DollarName`, `ParamOpen`, `CmdSubOpen`,
  `ArithOpen`, `BeginBacktick`, `Tilde`).
- Unquoted whitespace run → `Blank`.
- Operators / newlines / redirect operators / fd-prefixes / heredoc openers /
  comments / line-continuations → the SAME structural tokens `scan_step_command`
  emits today (`Op(…)`, `Newline`, `RedirFd`, `Heredoc` opener, etc.).

Structural tokenization is shared with the Word path wherever practical, so the
two scanners differ ONLY in word-building (atoms vs whole `Word`). The `((`/`[[`
that the deferred constructs need are emitted as their raw atoms/operators (the
parser resolves them to the current deferral — no pre-disambiguation needed, but
also no new grammar in v247).

### Parser: command-position word assembly

- A command-context `parse_word` (shared assembler with a caller-provided stop
  set) assembles atoms into one `Word` until `Blank` / any `Op(…)` / `Newline` /
  a redirect token / EOF, WITHOUT consuming the terminator. The operand
  `parse_word` is unchanged (its terminators — `ParamClose`/`RBracket`/`ParamSep`
  — never occur mid-command-word, and `Blank`/`Op`/`Newline` never occur
  mid-operand, so the shared assembler is safe).
- Every command-parser site that currently reads a whole `TokenKind::Word`
  switches to this assembler: `parse_simple`'s word loop, `parse_for`'s loop
  variable + `in`-list, `parse_case_item`'s pattern words, redirect targets, etc.
  The command parser skips `Blank` tokens between words.
- Keyword / reserved-word recognition stays TEXT-based but inspects the assembled
  Word: a leading word is a keyword only if it is a single unquoted `Literal`
  whose text matches (`if`/`while`/`for`/`case`/`{`/`[[`/…) — so `if` dispatches
  to the compound while `'if'` / `i$x` do not, identical to today.
- Downstream logic that already operates on assembled Words is UNCHANGED:
  `try_split_assignment` (assignment detection + `+=`/subscript), the keyword
  tables, redirect parsing. Byte-identical assembly is what keeps them correct.

### `((` handling in v247

No new grammar: at command position a `((`/`(` is tokenized as raw atoms/operators
and the parser resolves it to the SAME result the current path gives (subshell
where valid; the arith command `(( ))` remains `UnsupportedCommand`). The
disambiguation *machinery* is not required in v247 — it arrives with the arith
command in the next step. v247 only guarantees the atom path reproduces the
current path's classification.

## Error handling

- The atom path must return the SAME `ParseError` as `command.rs` for the same
  malformed input across the in-scope grammar (error parity, gated by a
  `diff_err`-style harness where the oracle errors).
- Deferred constructs return `UnsupportedCommand` on the atom path (asserted).
- Any well-formed in-scope input where the atom path diverges from the oracle is a
  BUG to fix (not to pin) — v247 is pure inversion; there should be no legitimate
  divergence. If a genuine, unavoidable divergence is found, it is pinned as a
  `*_deferred` test + a `docs/bash-divergences.md` `[deferred]` entry and reported
  prominently (as in v245's L-70), but the expectation is zero.

## Testing

- **Differential (the gate):** `diff_cmd(s)` — `new_seq` = atom-Lexer +
  `parse_sequence`; `old_seq` = Word-Lexer + `command::parse`; assert AST equal.
  `diff_unsupported(s)` — atom path returns `UnsupportedCommand` for deferred
  constructs. `diff_err(s)` — atom path returns the same error as the oracle for
  in-scope malformed input.
- **Corpus (broad):** bare/multi-word commands; all quoting forms; every
  command-position expansion; scalar assignments (`x=`, `x+=`, `a[i]=`); pipelines;
  and-or; separators; redirects + fd-prefixes + every redirect operator; comments;
  line-continuations; tilde; each in-scope compound; and ADVERSARIAL
  word-splitting/gluing (`a"b"$c`, `a\ b`, leading/trailing/multiple blanks,
  `x=$y"z"`, glued expansions).
- **Lexer unit tests:** assert the atom stream shape directly for representative
  inputs (e.g. `echo $x | wc` → `Lit(echo) Blank DollarName(x) Blank ... Op(Pipe)
  Blank Lit(wc)`), and that `Blank` never appears in the Word-mode stream.

## Task shape (for the plan)

Incremental SDD tasks, each dormant + differential + independently testable:
- **T1** — scaffolding: the `command_atoms` flag + selector, `TokenKind::Blank`,
  a `scan_step_command_atoms` skeleton, the command-context `parse_word` stop-set,
  the repointed `diff_cmd`/`diff_unsupported` harness, first tests.
- **T2** — ordinary words + `Blank` word-splitting/gluing (literals + quoting),
  byte-identical for bare/multi-word commands.
- **T3** — command-position expansions (`$x`/`${…}`/`$(…)`/`` `…` ``/`$((…))`/
  tilde) assembled via the reused sub-modes.
- **T4** — scalar assignments (`x=`, `x+=`, `a[i]=`) as prefix and standalone.
- **T5** — redirects + fd-prefixes + operators/separators + comments/continuations.
- **T6** — the in-scope compounds (if/while/until/for-list/case/select/subshell/
  brace) on the atom path.
- **T7** — broaden the differential corpus (adversarial splitting/gluing + error
  parity) + `diff_unsupported` coverage of every deferred construct + the lexer
  atom-stream shape tests.

## Constraints

- Test ONLY with `cargo test -p huck-syntax --jobs 1 -- --test-threads 1` (and
  narrower filters). The workspace suite OOM-kills this box (1 core / ~1.9 GiB)
  under parallel fan-out. NEVER run `cargo test --workspace`.
- Production path (`scan_step_command` Word emission, `command.rs`, the fat
  scanners, `process_line`) stays UNTOUCHED and live. v247 changes nothing that
  runs in production.
- No backwards-compat constraints on internal APIs (no external users), but ALL
  existing tests must still pass (guaranteed by construction — production path
  untouched).
- Every commit ends with the trailer
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
