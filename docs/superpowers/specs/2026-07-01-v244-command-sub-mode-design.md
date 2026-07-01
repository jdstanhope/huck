# v244 — command substitution `$( … )` lexer mode (design)

**Status: DESIGN (approved direction).** Date: 2026-07-01.

Inverts `$( … )` command substitution into the parser-driven front-end, following the
template v241 established for `${…}` (ParamExpansion mode): a `CommandSub` lexer mode
emits FLAT atoms, and the parser (`parser.rs`) assembles `WordPart::CommandSub` — the
first word-level mode whose body is a full **command list**, so it integrates with the
v242/v243 command parser. **Dormant + differential** vs the production lexer. Direction:
memory `huck-frontend-parser-driven-direction` / `huck-lexer-rearch-design`; prior:
v241 (`${…}`), v243 (subshell — the reused body primitive).

## Goal

When the parser drives a `CommandSub` lexer mode, `parser::parse_command_sub` must build
the SAME `WordPart::CommandSub { sequence, quoted }` the production lexer builds via its
scan-ahead path (`scan_dollar_expansion` → `scan_paren_substitution` → `scan_cmdsub_body`
→ `parse_substitution_body`) — verified by a differential corpus. The production lexer is
the ORACLE and its word-scanning path stays byte-identical.

## Scope (in)

- **`$( … )` command substitution**, unquoted and inside `"…"`.
- **The comsub STRUCTURE is parser-driven:** a `$( … )` body is a `Sequence` terminated
  by `)` — structurally identical to a subshell body — so `parse_command_sub` reuses
  v243's **`parse_subshell_sequence`** (already stops on `Op(RParen)`) for the body. The
  parser owns the `)` matching; the production `scan_cmdsub_body` scan-ahead does NOT run.
- **A `CommandSub` lexer mode** (fills the already-declared `Mode::CommandSub` stub;
  `scan_step` currently `unreachable!`s it): on entry consume `$(` and emit a `CmdSubOpen`
  atom; then tokenize the body by delegating to the existing Command-mode scanner
  (`scan_step_command`), so the body is a FLAT stream of Command-mode tokens and the
  terminating `)` is a normal `Op(RParen)` the parser consumes. Per-frame "have I emitted
  the open yet" state lives IN the `Mode::CommandSub` variant (the v241 per-frame pattern),
  so it is `mark`/`rewind`-safe.
- **`parse_command_sub(iter, quoted) -> Result<WordPart, ParseError>`** in `parser.rs`:
  push `Mode::CommandSub`, consume `CmdSubOpen`, `parse_subshell_sequence` for the body,
  pop, return `WordPart::CommandSub { sequence, quoted }`.
- **Wire into v241's operand `DeferredExpansion`:** `scan_step_param_operand` currently
  emits `DeferredExpansion` for `$(`, `$((`, and backtick uniformly. Change it to emit a
  distinguishing atom for `$(`-not-followed-by-`(` so `parse_word` recurses into
  `parse_command_sub`; `$((`/backtick still emit `DeferredExpansion` (still deferred). This
  makes `${x:-$(cmd)}` parse end-to-end in the new path (closing a v241 deferral) and
  proves the word↔command↔word recursion composes.

## The word↔command recursion (the novel integration)

`parse_word` (word level) → sees a comsub → `parse_command_sub` → `parse_subshell_sequence`
(command level) → `parse_command`/`parse_simple` (which consume body words) → a body word
may contain another comsub → … . This mutual recursion is normal recursive descent and
already exists implicitly in the production path. The **one-level** boundary: the body's
words arrive lexer-built and pass through opaquely (the v242/v243 interim), so a nested
`$(inner)` or `${x}` *inside a body word* is still fat-lexer-built (Command mode's
`scan_dollar_expansion`) — only the OUTER comsub structure is parser-driven this iteration.

## Non-goals (deferred → follow-ons / `UnsupportedExpansion`)

- **Backtick `` `…` ``** command substitution — a SEPARATE scanner (`scan_backtick_body`)
  with its own `unescape_backtick` step, no paren-nesting, no `case` tracking. Its own
  iteration (v245-ish).
- **`$(( ))` arithmetic expansion** (`WordPart::Arith`) — it is arith, same bucket as the
  arith command; stays on the `unreachable!` path.
- **`$((`-adjacent command sub** (`$( (subshell) )` written WITHOUT the disambiguating
  space): the `CommandSub` mode, on `$(` immediately followed by `(`, emits
  `DeferredExpansion` → `parse_command_sub` returns `UnsupportedExpansion`. The corpus
  writes the comsub-of-subshell form spaced (`$( (echo x) )`, unambiguous). The
  `mark`/`rewind` arith-vs-comsub disambiguation is reserved for its own deliberate
  iteration.
- **Atom-izing the body's words** — needs the general word-atom mode (a large separate
  piece); not v244.
- **A comsub body containing a construct the command parser DEFERS** (arith command,
  `[[ ]]`, function-def, coproc): `parse_subshell_sequence` returns
  `UnsupportedCommand`, so `parse_command_sub` propagates it and the comsub defers. The
  corpus keeps bodies within the v242/v243-supported grammar.

## Global constraints

- **Byte-identical / dormant:** the PRODUCTION word-scanning path
  (`scan_step_command`/`scan_dollar_expansion`/`scan_paren_substitution`) is UNCHANGED;
  nothing in production pushes `Mode::CommandSub`. The new mode + `parse_command_sub` +
  the operand-atom change are reached ONLY by tests (the differential) and the dormant
  `parser.rs` path. `cargo test --workspace` green, 0 warnings; release harness sweep
  byte-identical.
- **`command.rs` untouched** (this iteration reuses `parse_subshell_sequence` and the
  command parser as-is; no `command.rs` change). Changes live in `lexer.rs` (the new
  mode + atom + `scan_step` dispatch arm) and `parser.rs` (`parse_command_sub`, the
  operand wiring, the corpus).
- **`WordPart::CommandSub { sequence: command::Sequence, quoted: bool }`** reused verbatim
  — NO AST change, engine untouched.
- **The lexer NEVER scans ahead for the matching `)`** — the `CommandSub` mode emits the
  open atom then one Command-mode token at a time; the PARSER (`parse_subshell_sequence`)
  owns the `)` matching. This is the whole point (removing `scan_cmdsub_body`'s scan-ahead
  from the new path).
- Reuse `ParseError::UnsupportedExpansion` (v241) for the deferred cases; no new variant.

## Testing (the proof)

Extend v241's differential harness (`parser.rs`): `old_part(s, quoted)` = the production
lexer's `WordPart::CommandSub` (the ORACLE), `new_part(s, quoted)` = `parse_command_sub`
via a live lexer with `Mode::CommandSub` pushed; `diff_ok` asserts `new_part == old_part`
for both unquoted and `"…"`-quoted.

**In-scope corpus** (`diff_ok`): `$(echo hi)`, `$(a; b)`, `$(a | b)`, `$(a && b || c)`,
`$(if x; then y; fi)` (compound body — reuses v243), `$(for i in a b; do echo $i; done)`,
`$( (echo x) )` (comsub of subshell, spaced), `$(echo $(date))` (nesting — outer
parser-driven, inner passes through), `$(cat <<<w)`-style redirect / `$(<file)`
(bodies that are just redirects), `$()` (empty → empty `Sequence`), `"$(echo hi)"`
(quoted), `${x:-$(echo d)}` (comsub inside a `${…}` operand — the wiring), a comsub
whose body has trailing `;`/newline.

**Deferred corpus** (assert `Err(UnsupportedExpansion)`): `$((1+2))` (arith),
`` `echo hi` `` (backtick), `$((echo x))`-adjacent (the `$((` no-space case),
`$([[ -n x ]])` (body defers), `$(f() { x; })` (body defers).

**Error parity** (`assert_eq!` on the `Err`): `$(echo` (unterminated — matches the
production `UnterminatedSubstitution`/parse error the oracle returns).

## Open / edges (resolve in the plan)

- **`CommandSub` mode entry + per-frame state** — the exact "emit `CmdSubOpen` once, then
  delegate to `scan_step_command`" mechanism (a `body_started`-style flag in the
  `Mode::CommandSub` variant); mirror v241's `ParamExpansion { seen_name }`.
- **`$((` defer cursor state** — on `$(` + `(`, emit `DeferredExpansion`; the cursor
  position after the defer only needs to satisfy the `Err` path (the differential asserts
  `Err`), but confirm it doesn't wedge the operand re-entry.
- **`quoted` propagation** — `parse_command_sub(iter, quoted)` sets `WordPart::CommandSub
  .quoted` from its argument (the enclosing-dquote context), matching how the production
  path threads `quoted` through `scan_dollar_expansion`. The body itself lexes
  standalone (no enclosing-dquote), same as production.
- **Operand-atom change** — emit a distinguishing atom (e.g. reuse `CmdSubOpen`) for `$(`
  in `scan_step_param_operand`, keeping `DeferredExpansion` for `$((`/backtick; confirm
  `parse_word` dispatches and the four operand modes all compose.
- **Empty body** — `$()` → the oracle yields an empty `Sequence` (`empty_sequence`);
  `parse_subshell_sequence` on an immediate `)` must produce the SAME (an empty
  `Sequence`, not `EmptySubshell` — comsub differs from subshell here). Match the oracle.
- **Nesting depth / mutual recursion** — confirm `$(echo $(date))` parses (outer via the
  new path, inner fat-built inside the body Word) and the sequences match the oracle.
