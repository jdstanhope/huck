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

**The inversion (this design):** the **LEXER owns** a running nesting **depth** (it is a
lexing-context concern — it changes how the next character tokenizes), and per character
emits a `BeginBacktick`/`EndBacktick`/body atom based on that owned state. The escaping
*level* of each backtick, compared to the current depth, decides begin vs end. Deciding
this needs only a **small local lookahead** — the run of backslashes immediately before a
backtick, bounded by the depth — done at the `CharCursor` level (bounded peek), NOT an
unbounded scan for a matching delimiter. The **parser** consumes the resulting flat atom
stream and assembles the (arbitrarily nested) `WordPart::CommandSub` tree via its own
Begin/End matching. Crucially: **the lexer never reads parser state** (no lexer→parser
dependency — the same coupling we removed with the old `scan_substitution_body` edge); it
reads only its OWN depth. No scan-ahead, no unescape-then-retokenize.

## Scope (in)

- **`` `…` `` command substitution at ARBITRARY nesting depth** (the whole point — nesting
  is handled natively, not deferred). Unquoted and inside `"…"`.
- **`Mode::Backtick { depth }`** — a new `Mode` variant carrying the nesting counter as
  PER-FRAME state (the v241 `seen_name`/`in_dquote` pattern), so it rides the mode stack
  and is `mark`/`rewind`-safe. `depth` = how many backtick levels are currently open. It is
  **LEXER-OWNED**: the lexer mutates it as scanning state (increments when it emits a
  `BeginBacktick`, decrements on `EndBacktick`); the parser never reads or writes it.
- **`TokenKind::BeginBacktick` / `TokenKind::EndBacktick`** — the flat delimiter atoms
  (a `` ` `` opening a child vs closing the current level).
- **`scan_step_backtick`** — dispatched under `Mode::Backtick`; reads the lexer's OWN
  `depth` from that mode and mutates it. Never reads parser state.
  - on a backtick: decode its escaping *level* from the immediately-preceding backslash
    run — a SMALL LOCAL lookahead (bounded by the depth), done at the `CharCursor` level
    (bounded multi-char peek; the run is contiguous, not a distant delimiter) — and compare
    to `depth` → emit `BeginBacktick` (opens a child; the lexer increments its `depth`) or
    `EndBacktick` (closes the current; the lexer decrements). This is state + local peek,
    NOT an unbounded scan for a matching `` ` ``.
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
- **`parse_backtick_sub(iter, quoted) -> Result<WordPart, ParseError>`** — ENTERS backtick
  tokenization (pushes `Mode::Backtick` — the parser→lexer "switch tokenization style"
  signal, the committed direction; the DEPTH inside is lexer-owned, the parser neither sets
  nor reads it), then CONSUMES the flat atom stream: the opening `BeginBacktick`, the body
  parsed as a `Sequence` terminated by `EndBacktick` (the backtick analogue of v243's
  `parse_subshell_sequence` stopping on `Op(RParen)`), recursing into `parse_backtick_sub`
  when a body word hits a nested `BeginBacktick`; on the matching `EndBacktick` it exits the
  mode and returns `WordPart::CommandSub { sequence, quoted }`. The parser's Begin/End
  matching (its recursion) and the lexer's depth counter are INDEPENDENT — they agree on
  the nesting but neither drives the other. Empty `` `` `` → an empty `Sequence` (matching
  `empty_sequence`, as v244). `zero_lines_in_sequence` to match the oracle's line-zeroing.
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
- **The lexer NEVER scans ahead for a matching delimiter** — `scan_step_backtick` emits one
  atom per step; nesting is resolved by the lexer's OWN depth STATE (in `Mode::Backtick`)
  renaming the emitted token, plus the parser's Begin/End matching to build the AST — not by
  looking for a matching `` ` ``. A SMALL, LOCAL, bounded `CharCursor` peek over the
  contiguous backslash run before a backtick (to decode its escaping level) is explicitly
  allowed — it is not the forbidden unbounded scan.
- **The lexer does NOT depend on the parser** — `scan_step_backtick` reads only lexer state
  (its own `depth`); it never calls the parser or reads parser AST. (The parser→lexer mode
  signal is the allowed direction; lexer→parser is not.)
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
- **Who owns `depth` (settled):** the LEXER owns it. It lives in `Mode::Backtick { depth }`
  and is mutated BY THE LEXER (increment on emitting `BeginBacktick`, decrement on
  `EndBacktick`); the lexer reads only its own `depth`, never parser state. The parser
  enters/exits the `Mode::Backtick` (the parser→lexer mode signal) and matches Begin/End to
  build the AST, but does NOT set or track the depth. Resolve the entry/exit choreography in
  the plan (parser pushes `Mode::Backtick` to enter; the lexer sets `depth=1` on the first
  `BeginBacktick` and pops back to `Command` when `depth` returns to 0) — but keep the depth
  strictly lexer-owned. This is deliberately different from v244 (where the `$()` structure,
  not a lexing-context counter, was parser-driven); here the depth is intrinsic to how each
  character tokenizes, so it must be the lexer's.
- **CharCursor bounded peek:** decoding a backtick's escaping level needs to look at the
  contiguous backslash run before it (bounded by depth). Confirm `CharCursor` supports the
  needed multi-char peek, or extend it — this is bounded LOCAL peeking, explicitly allowed,
  and must not be turned into a scan for a matching `` ` ``.
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
