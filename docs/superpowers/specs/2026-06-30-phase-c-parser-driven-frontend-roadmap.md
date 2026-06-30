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
- **No new lexer→parser edges.** The existing one is removed in v241; until
  then the `ParseError::Lex(Box<LexError>)` stopgap stays.
- Each iteration is **independently shippable and reviewed** (subagent-driven
  development), and merges only when the oracle is green.

## 5. Iteration sequence

Ordering decision (approved): build the **mode stack + mark/rewind machinery
first** (no behavior change), then make the parser drive it — so comsub/subshell
go straight to inline parsing with no throwaway intermediate.

### v240 — Internal mode stack + `mark`/`rewind`-with-cursor-reset
**Scope:** Refactor the ~10 ad-hoc `Lexer` flags (`has_token`,
`in_assignment_value`, `dbracket_depth`, `expect_regex`, `brace_expand`, the
three dquote representations `quoted`/`enclosing_dquote`/`opts.in_dquote`,
`alias_trailing_eligible`) into a `modes: Vec<Mode>` stack with per-mode scan
logic, and implement `mark()` / `rewind(mark)` with `CharCursor` byte-offset
reset + history truncation.
**Behavior:** none changes — modes are INTERNAL, the parser is uninvolved, the
public API still hands the parser a token stream that produces identical
`Word`s. This is pure scaffolding, but it is where the rewind-and-re-lex
capability (the `((` enabler) is born and unit-tested in isolation.
**Risk:** high-care (touches the whole lexer) but de-risked by being
behavior-preserving with the full byte-identical oracle.
**Done when:** flags are gone, the mode stack drives `scan_step`, `mark`/`rewind`
have direct unit tests (including a re-lex-under-different-mode test), suite +
harnesses byte-identical.

### v241 — Parser drives comsub / subshell inline (removes the edge, drops the Box)
**Scope:** The lexer emits opener tokens for `$(` / `` ` `` / subshell `(`; the
parser pushes `CommandSub` / `Subshell` mode and parses the body **inline as a
command list**, assembling `WordPart::CommandSub` / `ProcessSub` / a subshell
command itself. Delete `parse_substitution_body` and `scan_paren_substitution` /
`scan_backtick_substitution`'s parser call; remove `LexError::SubstitutionParseError`
(and `LexError::Substitution`); un-box `ParseError::Lex`.
**Behavior:** byte-identical (same `Sequence` built, now by the parser).
**Risk:** medium — the first real parser-driven Word assembly; must handle
comsub embedded mid-word (`a$(b)c`), inside `"…"`, in regex/extglob operands,
and the `case…esac`-aware `)` boundary that `scan_cmdsub_body` tracks today.
**Done when:** the lexer no longer references `command::parse`; `Box` gone;
suite + harnesses green.

### v242 — `Arith` mode + the `((` disambiguation
**Scope:** `$((` / `((` / `$[` lex under `Arith` mode; the parser parses the
arithmetic body (today's `ArithBlock` + `arith_string_to_word` folds into this).
Implement the `((` arith-vs-nested-subshell resolution via the §3.3
checkpoint/rewind path.
**Risk:** medium — the rewind path is exercised for real here; nested `$( )` /
`${ }` inside arith must compose.
**Done when:** `((3+3))` and `( (a); b )` both byte-identical; arith no longer a
deferred raw string.

### v243+ — `ParamExpansion` mode: collapse the 6 drifted `${}` scanners
**Scope:** `${…}` lexes under one `ParamExpansion` mode, assembled by the parser,
replacing `scan_braced_param_expansion` / `scan_braced_operand` /
`scan_braced_name` / `scan_braced_name_ext` / `scan_braced_skip` /
`scan_substitution_operand` and the three dquote flags. Retires the v233–235
bug-chain subsystem and the open items in `huck-param-expansion-debt`.
**Risk:** highest correctness risk in the roadmap (the most bug-prone area);
**likely several sub-iterations** (e.g. name/subscript first, then each modifier
family). Each sub-iteration byte-identical.
**Done when:** one brace scanner remains; the param-expansion error-model
backlog (`${#$'x1'}`, `${x@QQ}`, `${x[i}`) is revisited under the unified path.

### v24x — Finalize separation (residual parser-AST type deps)
**Scope:** Remove the lexer's remaining compile-time dependencies on parser AST
types where they no longer belong (e.g. relocate shared AST, or make
`CommandSub`/`ProcessSub` carry a parser-built type without the lexer naming it).
Achieve true module separation: `huck-syntax`'s lexer no longer depends on its
parser.
**Risk:** low–medium, mechanical once assembly has moved.
**Done when:** the lexer compiles without referencing `command::` parser types
(beyond shared leaf types intentionally kept).

## 6. Open design questions (resolve in the iteration that hits each)

- **Heredoc bodies in a pull model.** Heredoc bodies are line-oriented and
  *deferred* (collected after the redirect line's newline). How the
  `HeredocBody` mode interacts with the pull + mark/rewind model — when bodies
  are scanned, how back-patching works without a batch pass — is unresolved.
  Tackled when heredocs move into the mode stack (not v240–v242). Until then the
  current `collect_heredoc_bodies` back-patch is preserved unchanged.
- **Exact `mark`/`rewind` flush semantics across push/pop.** Rule of thumb:
  pushing/popping a mode invalidates buffered lookahead produced under a
  different mode. Pin the precise rule (and interaction with heredoc deferral)
  in v240.
- **Here-string operand.** Confirm it can ride `Command`-mode word lexing with a
  no-split flag rather than a dedicated mode.
- **Speculative constructs beyond `((`.** `case` pattern lists, `for`/`select`
  headers, and function-def `NAME()` detection: confirm whether each needs a
  mode, a mark/rewind speculation, or stays in `Command` mode. Inventory before
  v242/v243.
- **Residual type deps strategy** (v24x): relocate shared AST vs. introduce a
  lexer-local body type the parser finalizes.

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
