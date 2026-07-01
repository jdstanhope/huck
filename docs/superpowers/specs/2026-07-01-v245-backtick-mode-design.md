# v245 — backtick `` `…` `` command substitution lexer mode (design)

**Status: DESIGN (approved direction).** Date: 2026-07-01.

Inverts backtick command substitution (`` `…` ``), **including arbitrary-depth
escaping-based nesting**, into the parser-driven front-end. Unlike v244's `$( … )`
(which delegated the body to `scan_step_command` and stopped on a natural `)`),
backticks need char-level unescaping and their nesting is escaping-encoded — so the
lexer tracks a **backtick depth** and lets that state *rename the token it emits*
(`BeginBacktick`/`EndBacktick`/body-token) for the character under the cursor, WITHOUT
ever scanning forward. The parser owns the matching (mode push/pop). Dormant +
differential vs the production lexer. Direction: memory
`huck-frontend-parser-driven-direction` / `huck-lexer-rearch-design`; prior: v241
(`${…}`), v244 (`$( … )` — the reused harness + `WordPart::CommandSub` AST).

## Why this is harder than `$( … )` (and why the depth-tracking approach)

The production path is two-phase — `scan_backtick_body` collects the raw body up to
the first UNESCAPED `` ` ``, `unescape_backtick` strips `` \` ``→`` ` `` / `\\`→`\` /
`\$`→`$`, then `parse_substitution_body` re-tokenizes; nesting works because a `` \` ``
survives the unescape as a bare `` ` `` and the re-tokenize recurses. That two-phase
collect-then-unescape-then-retokenize is exactly the scan-ahead the committed direction
removes, AND `scan_step_command` cannot scan a backtick body directly (it treats `` ` ``
as a nested OPENER, and turns every `\c` into a `Quoted{Backslash}` literal instead of
backtick's `` \` ``/`\\`/`\$` unescaping — e.g. `` `echo \$x` `` must yield the VARIABLE
`$x`, which requires `\$`→`$` BEFORE tokenizing).

**The inversion (this design):** the lexer carries a running nesting **depth** and, per
character, emits a `BeginBacktick`/`EndBacktick`/body atom based on that state — never
looking past the current character. The escaping *level* of each backtick, compared to
the current depth, decides begin vs end. The parser assembles the (arbitrarily nested)
structure by pushing/popping a `Backtick` mode. No scan-ahead, no unescape-then-retokenize.

## Scope (in)

- **`` `…` `` command substitution at ARBITRARY nesting depth** (the whole point — nesting
  is handled natively, not deferred). Unquoted and inside `"…"`.
- **`Mode::Backtick { depth }`** — a new `Mode` variant carrying the nesting counter as
  PER-FRAME state (the v241 `seen_name`/`in_dquote` pattern), so it rides the mode stack
  and is `mark`/`rewind`-safe. `depth` = how many backtick levels are currently open.
- **`TokenKind::BeginBacktick` / `TokenKind::EndBacktick`** — the flat delimiter atoms
  (a `` ` `` opening a child vs closing the current level).
- **`scan_step_backtick(depth)`** — reading forward ONE character at a time:
  - on a backtick (with its preceding backslash run): decode its escaping *level* and
    compare to `depth` → emit `BeginBacktick` (level opens a child) or `EndBacktick`
    (level closes the current). NO lookahead — the decision is `f(current char + run of
    preceding backslashes already consumed, current depth)`.
  - on `\$` / `\\` (at the current depth's escaping): depth-aware unescape to `$` / `\`.
  - on other `\c`: preserve (matching `unescape_backtick`).
  - otherwise: tokenize body content as ordinary Command tokens (so a `$(…)` / `${…}` in
    the body fat-builds and passes through, exactly as in v244's one-level boundary).
  - EOF before the matching close → the same unterminated error the oracle returns.
- **The escaping-level decode** — bash's backtick escaping COMPOUNDS per level (level-1
  delimiter `` \` ``, level-2 `` \\\` ``, i.e. a level-L delimiter carries `2^L − 1`
  backslashes; body `$`/`\` compound the same way). `scan_step_backtick`'s decode must
  MATCH the production oracle (`scan_backtick_body` + `unescape_backtick` applied
  recursively, one level per nesting depth). The differential across depths 0/1/2 pins it
  exactly; the mechanism is uniform for deeper levels.
- **`parse_backtick_sub(iter, quoted) -> Result<WordPart, ParseError>`** — on
  `BeginBacktick`: push `Mode::Backtick { depth }`, parse the body as a `Sequence`
  terminated by `EndBacktick` (the backtick analogue of v243's `parse_subshell_sequence`
  stopping on `Op(RParen)`), recursing into `parse_backtick_sub` when a body word hits a
  nested `BeginBacktick`; on `EndBacktick` pop and return `WordPart::CommandSub {
  sequence, quoted }`. Empty `` `` `` → an empty `Sequence` (matching `empty_sequence`, as
  v244). `zero_lines_in_sequence` to match the oracle's body line-zeroing.
- **Primary contexts:** the incremental-lexer sites `scan_step_command` unquoted-word
  (lexer.rs ~1705) and inside-`"…"` (~1639), mirroring v244. `parse_backtick_sub` takes
  `quoted` and threads it onto the `WordPart::CommandSub`.
- **Operand wiring** (parallel to v244 T4): where v241's `${…}` operand
  `DeferredExpansion` fires for a backtick, dispatch `parse_word` into `parse_backtick_sub`
  so ``${x:-`cmd`}`` parses end-to-end.

## Non-goals (deferred → follow-ons / stay fat-lexer)

- **The non-incremental char-scanner backtick sites** (`scan_regex_operand`,
  `scan_extglob_group`, expanding-heredoc body, `arith_string_to_word`,
  `parse_braced_operand_opts` — lexer.rs 2490/2550/2906/2945/2987/3921/3960) stay on the
  production fat-lexer path until later Phase C iterations (same as v244 left them). The
  dormant differential covers the primary unquoted + `"…"` contexts.
- **Body-word atomization** — a nested `$(inner)` or `${x}` in a backtick body word stays
  fat-built (the v244 interim); only the backtick structure (incl. its own nesting) is
  parser-driven.
- **`$(( ))` arith** stays deferred (its own future mode).

## Global constraints

- **Byte-identical / dormant:** the PRODUCTION path (`scan_backtick_body`,
  `unescape_backtick`, `scan_backtick_substitution`, `parse_substitution_body`,
  `scan_step_command`'s `` ` `` arm) is UNCHANGED; nothing in production pushes
  `Mode::Backtick`. The new mode + atoms + `scan_step_backtick` + `parse_backtick_sub` +
  the operand dispatch are reached ONLY by tests and the dormant parser path. `cargo test
  --workspace` green, 0 warnings; release harness byte-identical.
- **The lexer NEVER scans ahead** — `scan_step_backtick` emits one atom per step; the
  nesting is resolved by the depth STATE (in `Mode::Backtick`) modifying the emitted token
  kind, and by the PARSER's push/pop — not by looking for a matching `` ` ``.
- **`command.rs` untouched**; reuse `WordPart::CommandSub { sequence, quoted }` (no AST
  change), `ParseError::UnsupportedExpansion` (for any residual defer), and the v244
  differential harness. Changes live in `lexer.rs` (mode, atoms, `scan_step_backtick`,
  dispatch arm, the operand-scanner backtick branch) and `parser.rs` (`parse_backtick_sub`,
  operand dispatch, corpus).
- **The production lexer is the ORACLE** — on any differential mismatch, fix the NEW path
  to match; never weaken the comparison.

## Testing (the proof)

Reuse v244's harness shape with backtick-source helpers: `old_bt(s, quoted)` = the
production `WordPart::CommandSub` from a real backtick (the ORACLE, via `find_command_sub`
descending into `Quoted{Double}`), `new_bt(s, quoted)` = `parse_backtick_sub` on a live
lexer; `diff_bt` asserts equality for unquoted + `"…"`-quoted.

**In-scope corpus** (`diff_bt`, arbitrary depth is the star): depth-0 (`` `echo hi` ``,
`` `a | b` ``, `` `a && b` ``, `` `if x; then y; fi` ``, empty `` `` ``); body escapes
(`` `echo \$x` `` → var, `` `echo \\` `` → literal `\`); `$(…)`/`${…}` in a body word
(`` `echo $(date)` ``, `` `echo ${x}` `` — fat-built, pass through); quoted
(``"`echo hi`"``); **depth-1 nesting** (`` `echo \`date\`` ``, `` `a \`b\` c` ``);
**depth-2 nesting** (`` `a \`b \\\`c\\\` d\` e` ``) to prove the escaping-compounding
decode; the operand case (``${x:-`echo d`}``).

**Error parity** (`assert_eq!` on the `Err`): unterminated `` `echo `` (matches the
oracle's `UnterminatedSubstitution`/parse error).

## Open / edges (resolve in the plan)

- **The escaping-level decode is the hard technical core.** Build it incrementally
  (depth-0 → depth-1 → depth-2), each level's decode pinned by the differential vs the
  recursive oracle; do NOT hand-derive a closed-form and skip the differential. Confirm the
  `2^L − 1`-backslash intuition against the oracle per level rather than assuming it.
- **Who owns `depth`:** the PARSER pushes `Mode::Backtick { depth+1 }` on consuming
  `BeginBacktick` and pops on `EndBacktick` (as v244's parser owns push/pop); the lexer
  READS `current_mode`'s `depth` to pick the token kind. Confirm the sequencing (lexer
  reads depth D, emits atom, parser adjusts the stack, next step reads the new depth).
- **Begin-vs-end rule:** at depth D, a backtick at escaping-level `D` opens a child
  (`BeginBacktick`), at level `D−1` closes the current (`EndBacktick`); at depth 0 a bare
  `` ` `` is `BeginBacktick`. Verify this stack rule against the oracle for depths 0/1/2.
- **Body content unescaping vs a nested-backtick body word:** confirm `\$`/`\\` unescape
  depth-aware while a nested `` \` `` becomes a `BeginBacktick` (not a literal), and a
  body `$(…)` fat-builds — i.e. the three `\`-cases (`` \` ``/`\\`/`\$`) and the bare
  backtick are the only depth-sensitive chars.
- **Empty + unterminated** parity with the oracle (`empty_sequence`, unterminated error).
- **Operand hand-off:** reuse v244's zero-width-signal pattern (the operand scanner
  signals a backtick without consuming it; `parse_word` calls `parse_backtick_sub`), or the
  analogous approach; confirm no v241/v244 regression.
