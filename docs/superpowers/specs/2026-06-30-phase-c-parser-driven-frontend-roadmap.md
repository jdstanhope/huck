# Phase C — Parser-Driven Front-End Roadmap

**Status: ROADMAP (approved direction; per-iteration specs follow).** Date: 2026-06-30.

This is the multi-iteration roadmap for the final stage of huck's front-end
re-architecture: inverting the current "fat lexer / thin parser" into a
**parser-driven** front end where the parser builds words, command
substitutions, arithmetic, and subshells, and *informs the lexer* which
tokenization rules (mode) to apply for each region.

It extends the living re-arch design
(`2026-06-29-incremental-lexer-rearch-design.md`, Sections 2–4 + Migration
Phase C). Read that first for the settled S1/S2 shape and the Phase A/B history.
Companion memory: `huck-frontend-parser-driven-direction` (committed direction)
and `huck-lexer-rearch-design` (living-doc pointer).

---

## 1. Goal & committed direction

The parser owns **structure**; the lexer owns **lexing rules**. Parameters
(`${…}`), command substitution / subshells (`$(…)`, `` `…` ``, `(…)`), and
arithmetic (`$((…))`) each need *real parsing* under *different lexing rules*,
so parsing must drive, pushing/popping lexer **modes**.

End-state properties:
- The lexer makes **no calls into the parser** (the one
  `parse_substitution_body → command::parse` edge is gone; `ParseError::Lex`'s
  `Box` is dropped).
- The lexer **stops pre-building expansion structure**; it emits
  opener/closer/piece tokens under the current mode, and the parser assembles
  the `WordPart` / `Sequence` / arith AST.
- The ~10 ad-hoc lexer state flags become a **mode stack**.
- The 6 drifted `${…}` scanners collapse into one `ParamExpansion` mode (retires
  the v233–235 parameter-expansion bug-chain subsystem).
- The **engine-facing AST is unchanged** (`WordPart`, `Sequence`, etc.): only
  *who builds it* flips from lexer to parser. The engine (`expand_part` and the
  ~17 `WordPart` match arms in `expand.rs`, plus `generate.rs`/`executor.rs`
  consumers) is not touched by the inversion.

This is a *direction*, executed as independent byte-identical iterations; we are
not committing to land it all in one change.

## 2. Starting point — the fat lexer / thin parser (current reality)

Confirmed by a full front-end audit (2026-06-30). Today:

- `lexer.rs` (~9,500 lines) builds **fully-structured `Word` tokens** with
  `WordPart` trees. ~30 `scan_*` functions; ~10 ad-hoc `Lexer` state fields.
- The parser (`command.rs`) treats `Word` as **essentially opaque** — it only
  looks inside for keyword detection (`keyword_of`) and assignment splitting
  (`try_split_assignment` / `WordPart::AssignPrefix`). It never decomposes
  expansions.
- The only **lexer→parser call edge** is `parse_substitution_body`
  (lexer.rs ~3057), reached by `scan_paren_substitution` /
  `scan_backtick_substitution`, used for `$(…)`, backticks, **and process
  substitution** (all share that chokepoint). It re-lexes the body and calls
  `command::parse`, storing a finished `Sequence` in `WordPart::CommandSub` /
  `ProcessSub`.
- `ArithBlock(text, opts)` is the one token already carrying a **deferred raw
  string** parsed parser-side (via `arith_string_to_word`) — arithmetic is
  "half" parser-driven already.
- The lexer also has **compile-time type dependencies** on parser AST:
  `Sequence` (in `CommandSub`/`ProcessSub`), `AssignTarget`, `RedirFd`,
  `ParseError` (in `LexError::SubstitutionParseError`). Removing the *call* does
  not by itself remove these *type* deps (addressed last).

## 3. End-state architecture

### 3.1 Mode-driven lexer

The lexer scans **one token at a time under a current mode** that the parser
controls. A mode is just the lexing rules for a region. `Lexer` gains a
`modes: Vec<Mode>` stack; `next`/`peek` lex under `modes.last()`.

The mode set:

| Mode | Region | Notes |
|---|---|---|
| `Command` | default | operators, keywords, words; brace-expand + glob/extglob literalness |
| `Subshell` | `( … )` | command list |
| `CommandSub` | `$(…)` / `` `…` `` | recursive `Command` until matching close |
| `ParamExpansion` | `${…}` | collapses the 6 drifted brace scanners + the 3 dquote flags |
| `Arith` | `$((…))` / `((…))` / `$[…]` | `((` disambiguation via mark/rewind (§3.3) |
| `ArrayLiteral` | `a=(…)` | element words, `[i]=` subscripts |
| `DoubleBracket` / `Regex` | `[[ … ]]`, RHS of `=~` | replaces `dbracket_depth` / `expect_regex` |
| quote sub-rules | `'…'`, `"…"`, `$'…'` | literalness / expansion gating |
| `HeredocBody` | `<<EOF` … | **line-oriented + deferred**; expansion gated on delimiter quoting; see §6 |
| (here-string) | `<<< word` | *light* — a new operator + a no-split word; not a full mode |

### 3.2 Parser drives assembly

Building a `Word`, the parser scans pieces in `Command` mode; on `${` / `$(` /
`$((` / `` ` `` it **pushes the matching mode**, parses the inner structure into
the AST, pops, and continues the word. Comsub and subshell bodies are parsed
**inline as command lists** — no body-string extraction, no recursive re-lex.

### 3.3 Checkpoint / rewind-and-re-lex (the `((` ambiguity)

`((` may open an arithmetic command `((3+3))` *or* two nested subshells
`( (sub); sub )`; which one is unknowable until the body is parsed. The mode
stack therefore needs **`mark` / `rewind` that resets the char cursor to a byte
offset and re-lexes from there under a (possibly different) mode** — not mere
token replay, because the same bytes tokenize differently per mode.

- `mark()` captures `(cursor byte-offset, history index, current mode)`.
- `rewind(mark)` resets the `CharCursor` to that offset, **discards buffered
  lookahead at/after it**, and restores the mode; the next pull re-scans those
  bytes under whatever mode is now active.

`((` walkthrough:
1. Parser at command position sees `((` → takes a `mark` at the first `(`.
2. Optimistically pushes `Arith`, tries to parse `((expr))`.
3. **Success** (matching `))`): arithmetic command.
4. **Failure** (`((cat); ls)` — invalid arith / no depth-0 `))`): `rewind` to
   the mark, push `Subshell`, re-lex/parse the same bytes as `( (cat); ls )`.

This mirrors bash's own optimistic-arith-then-reparse-as-subshell behavior.
The same machinery serves other speculative points (e.g. assignment-vs-command,
`function NAME` vs `NAME()`), so `mark`/`rewind`-with-cursor-reset is a
**foundational capability**, built first (§5, v240).

## 4. Invariants / global constraints

Binding on every iteration in this roadmap:

- **Byte-identical** output with the prior huck for all inputs. Oracle:
  `cargo test --workspace` green + the full `tests/scripts/*_diff_check.sh`
  release harness sweep byte-identical. 0 warnings.
- The **engine-facing AST stays fixed** during the inversion (`WordPart`,
  `Sequence`, `ParamModifier`, etc.). An iteration that must change an AST shape
  flags it explicitly and updates every consumer (`expand.rs`, `generate.rs`,
  `executor.rs`, `procsub.rs`) in the same change.
- **No new lexer→parser edges.** The existing one is removed in the Stage-2
  parser rewrite; until then the `ParseError::Lex(Box<LexError>)` stopgap stays.
- Each iteration is **independently shippable and reviewed** (subagent-driven
  development), and merges only when the oracle is green.

## 5. Implementation strategy & iteration sequence

**Approved strategy (2026-06-30): build and fully validate the stacked lexer
FIRST, driven by tests that simulate the parser; THEN rewrite the parser in one
pass to drive it and delete the old scanners.** Rationale: shell grammar nests
arbitrarily (anything under anything), so the parser migration does NOT decompose
cleanly by expansion type — but the lexer can be proven correct independently,
and the parser-simulating tests become the executable specification the parser
rewrite follows.

### Stage 1 — build the stacked lexer (additive, DORMANT in production)

Add, *alongside* the existing batch `Word` path (not replacing it):
- the `modes: Vec<Mode>` stack + `mark`/`rewind`-with-cursor-reset (§3.3);
- new `TokenKind` variants for word-part-level tokens (expansion
  openers/closers and pieces), **additive to the current `Word(Word)` token and
  not in conflict** with the existing Word/WordPart AST;
- per-mode "scan one token" logic for each mode in the set (§3.1).

Each new mode is **dormant in production**: the parser does not push it yet, so
the default `Command` path still produces the exact old `Word` tokens. Therefore
**every Stage-1 iteration is byte-identical** (the full oracle stays green; the
new code is exercised only by unit tests). Validation is **parser-simulating
tests** — tests that push/pop modes and pull tokens, asserting the token stream
for deeply nested structures: `$( ${x:-$(…)} )`, `$(( (a) + $b ))`,
`${x/$(…)/$y}`, comsub inside `"…"`, the `((`-vs-`( ( )` ambiguity via
mark/rewind, heredocs. These tests are the worked examples Stage 2 implements
against.

Indicative iterations (each adds modes + parser-simulating tests; dormant /
byte-identical):
- **v240** — mode-stack infrastructure + `mark`/`rewind`-with-cursor-reset + the
  new word-part `TokenKind`s; first modes proven by tests: the `Command` mode
  (the ~10 ad-hoc flags — `has_token`, `in_assignment_value`, `dbracket_depth`,
  `expect_regex`, `brace_expand`, the three dquote reps, `alias_trailing_eligible`
  — re-expressed as mode state) and `Subshell`.
- **v241+** — one or two modes per iteration with parser-simulating tests:
  `CommandSub`/backtick, `Arith` (incl. the `((` mark/rewind disambiguation),
  `ParamExpansion` (the single collapsed brace scanner replacing the 6 drifted
  ones), `ArrayLiteral`, `DoubleBracket`/`Regex`, `HeredocBody`. Stage 1 ends
  when the stacked lexer can produce every nesting structure under test.

### Stage 2 — rewrite the parser onto the stacked lexer (one pass) + delete old scanners

- Rewrite the parser to assemble Words / comsub / arith / subshells / params by
  **driving the stacked lexer** (push/pop modes, mark/rewind), using the Stage-1
  parser-simulating tests as the worked examples. Comsub/subshell bodies become
  inline command-list parses. Remove `parse_substitution_body` and the
  lexer→parser edge; remove `LexError::SubstitutionParseError` /
  `LexError::Substitution`; **un-box `ParseError::Lex`**.
- **De-risk the one-shot switch with differential testing**: keep the old
  fat-lexer+parser path available behind a flag during the transition and assert
  **old AST == new AST** across the entire `*_diff_check.sh` corpus (plus a fuzz
  pass) *before* deleting the old code. The parser-simulating tests prove the
  lexer; the differential gate proves the parser rewrite; together they make the
  big-bang switch safe.
- Once green, **delete the old scanners** (`scan_braced_*`, the substitution
  scanners, `scan_array_literal`/`scan_regex_operand`/`scan_extglob_group` and
  the other context scanners the modes subsume) and the dormant-path scaffolding.

This stage is necessarily large (the parser rewrite is monolithic — see
rationale), but it is bounded: it ships only when the differential gate and the
full oracle are green, and it is the ONLY non-byte-identical-by-construction step
(it is made safe by construction instead).

### Stage 3 — finalize separation (residual parser-AST type deps)
Remove the lexer's remaining compile-time dependencies on parser AST types
(`CommandSub`/`ProcessSub` carrying `Sequence`, `AssignTarget`, `RedirFd`,
`ParseError`) — relocate shared AST or introduce a lexer-local body type the
parser finalizes — so `huck-syntax`'s lexer no longer depends on its parser.
Mechanical once assembly has moved.

## 6. Open design questions (resolve in the iteration that hits each)

- **Heredoc bodies in a pull model.** Heredoc bodies are line-oriented and
  *deferred* (collected after the redirect line's newline). How the
  `HeredocBody` mode interacts with the pull + mark/rewind model — when bodies
  are scanned, how back-patching works without a batch pass — is unresolved.
  Tackled in the Stage-1 `HeredocBody` mode iteration. Until then the current
  `collect_heredoc_bodies` back-patch is preserved unchanged.
- **Exact `mark`/`rewind` flush semantics across push/pop.** Rule of thumb:
  pushing/popping a mode invalidates buffered lookahead produced under a
  different mode. Pin the precise rule (and interaction with heredoc deferral)
  in v240.
- **Here-string operand.** Confirm it can ride `Command`-mode word lexing with a
  no-split flag rather than a dedicated mode.
- **Speculative constructs beyond `((`.** `case` pattern lists, `for`/`select`
  headers, and function-def `NAME()` detection: confirm whether each needs a
  mode, a mark/rewind speculation, or stays in `Command` mode. Inventory before
  the Arith / ParamExpansion mode iterations.
- **Residual type deps strategy** (Stage 3): relocate shared AST vs. introduce a
  lexer-local body type the parser finalizes.
- **Differential-testing harness (Stage 2).** How the old and new
  lexer+parser paths coexist behind a flag, and what the AST-equality comparison
  + fuzz corpus look like — designed at the start of Stage 2.

## 7. Appendix — current-state inventory (audit 2026-06-30)

For implementers; cite when scoping an iteration.

- **The single lexer→parser edge:** `parse_substitution_body` (lexer.rs ~3057),
  via `scan_paren_substitution` (~3045) / `scan_backtick_substitution` (~3076).
  Covers `$(…)`, backticks, process substitution, and the `$((`-reparse-as-cmdsub
  fallback. The only `command::parse` call in `lexer.rs`.
- **The 6 drifted brace scanners:** `scan_braced_param_expansion` (~3113, the
  only one that emits `WordPart`s), `scan_braced_operand` (~2828, raw-text
  collect with `$'…'`/`$"…"`/`$(…)` awareness), `scan_braced_name` (~3797,
  identifier only), `scan_braced_name_ext` (~3763, + `$'…'` extquote, returns
  `NameScan`), `scan_braced_skip` (~2424, the *less-complete* legacy-arith copy),
  `scan_substitution_operand` (~4074, raw-collect then re-parse twice).
- **The ~10 ad-hoc flags:** `has_token`, `in_assignment_value`, `dbracket_depth`,
  `expect_regex`, `brace_expand`, `opts.in_dquote`, `enclosing_dquote` (param),
  `quoted` (param), `alias_trailing_eligible`, plus alias `active`/`aliases`.
- **Token model:** `TokenKind = Word(Word) | Op(Operator) | Newline | Heredoc{…}
  | ArithBlock(String, LexerOptions) | RedirFd(RedirFd)`.
- **`WordPart` variants:** `Literal, Tilde, Var, LastStatus, CommandSub{sequence},
  ProcessSub{sequence}, Arith{body:Word}, ParamExpansion{name,modifier,quoted,
  subscript,indirect}, AllArgs, AssignPrefix{target}, ArrayLiteral, Quoted{style,
  parts}`. All built by the lexer today.
- **Parser chokepoints:** `parse_simple_stage` consumes `TokenKind::Word`;
  `ArithBlock` consumed in `parse_command_inner` / `parse_next_stage` via
  `arith_string_to_word`; `keyword_of` / `try_split_assignment` are the only
  intra-`Word` peeks. v239 made the parser pull via `peek`/`next`.
- **Engine consumers (ripple targets for any AST change):** `expand.rs`
  `expand_part` (~1051, ~17 arms) + `expand_assignment` (~1685) + xtrace
  reconstruct (~1471); `generate.rs` `part_to_source` (~555, exhaustive, no
  wildcard); `executor.rs` (~3831/6501/6544); `run_substitution` (~1906) and
  `procsub::realize` consume `Sequence`.
