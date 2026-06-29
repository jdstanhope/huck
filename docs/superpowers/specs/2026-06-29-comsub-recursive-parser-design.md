# v236: `$()` recursive-parser command substitution — design

**Status:** approved (brainstorm 2026-06-29)
**Iteration:** v236

## Goal

Replace huck's heuristic byte-scanner for `$( … )` command-substitution
boundaries with a true **recursive parse**: the parser's grammar — not a
parallel character/keyword counter — decides where the substitution ends.
This makes huck accept inside `$()` anything bash's grammar accepts in a
command list, and additionally collects heredoc bodies that span the
substitution boundary, matching bash's warn-and-continue behavior on an
unterminated in-comsub heredoc.

## Background: how it works today

`scan_paren_substitution` (lexer.rs:2795) currently:

1. calls `scan_cmdsub_body` — a heuristic byte-scanner that walks the raw
   input tracking paren depth, quotes, `#` comments, and a hand-rolled
   `case … esac` state machine — to find the matching `)` and copy the body
   to a `String`;
2. calls `parse_substitution_body` to re-tokenize and parse that string into
   a `Sequence`, embedded as `WordPart::CommandSub { sequence, quoted }`.

`scan_cmdsub_body` carries documented limitations (lexer.rs:2293–2298): the
empty case `$(case x in esac)` errors, `case`/`esac` literals in *pattern*
position are mis-counted, and a `VAR=val case` prefix-assignment is
mishandled. More broadly, any construct whose `)` placement the heuristic
mis-models (nested case patterns, `if/then` fragments, bare `)` clause
terminators) is parsed differently from bash — the root cause behind the
`comsub`, `comsub-posix`, `comsub-eof`, `more-exp`, and `new-exp` bash
test-suite failures.

This is exactly the problem bash solves by recursively invoking its parser
(`parse_comsub` / `xparse_dolparen`) over the shared input: the grammar knows
that a `)` after `case x in a` is a pattern terminator, while the `)` in
command-terminator position closes the substitution.

## Scope

**In scope:**

- Rewrite the **primary** `$()`→`Sequence` path (`scan_paren_substitution`)
  to find the boundary by parsing.
- **Heredoc-in-comsub:** bodies that span the boundary are collected
  correctly; an unterminated in-comsub heredoc emits bash's warning and
  treats the body as empty (rather than hard-erroring).

**Out of scope (kept on `scan_cmdsub_body`, recorded as deferred follow-ons):**

- The four *other* `scan_cmdsub_body` callers: verbatim-skipping a `$()`
  inside a `${…}` operand (`consume_paren_cmdsub_verbatim`, lexer.rs:2629 &
  3901), legacy `$[ ]` arith (2241), and subscript scanning (3400). These
  live in the param-expansion operand scanners that the 2026-06-28
  architectural review consciously deferred (see the
  `huck-param-expansion-debt` memory); touching them courts that fragility.
- Backticks (`` `…` ``) — their boundary is purely lexical (next unescaped
  backtick, no nesting), so they need no grammar. Unchanged.
- The error-message-prefix divergence (huck uses its own name as prefix
  rather than bash's script-and-line form) — tracked separately as the
  staged error-prologue rollout.

## Definition of done

Behavior-correct and verified, flipping bash-suite categories
**opportunistically** (not required):

- A new `tests/scripts/comsub_diff_check.sh` asserts byte-identical
  bash-vs-huck output across the target constructs.
- `cargo test --workspace` stays green.
- The `comsub*` / `more-exp` / `new-exp` categories are re-run and their new
  status recorded. If an unrelated error-prologue blocker keeps a category
  non-PASS, that is a separate deferred item, not a v236 failure.

## Architecture & data flow

`scan_paren_substitution(chars, opts)` — with `chars` positioned just after
the `$(` — becomes:

```text
body_start = chars.offset()                       # byte offset of the body's first char
rest       = &input[body_start..]
(toks, offs, lines, tail_err) = tokenize_partial(rest, opts)   # tolerant tokenization
(sequence, n_consumed)        = command::parse_comsub(toks, lines)?
end        = offs[n_consumed]                      # byte offset just past ')' within `rest`
chars.seek(body_start + end)                       # reposition shared cursor past ')'
return sequence
```

Key properties:

- **Tolerant tokenization.** `tokenize_partial` (lexer.rs:401) already
  returns the tokens produced *before* any lex error plus
  `Some((error, offset))`, and guarantees `offsets.len() == tokens.len() + 1`
  (a trailing sentinel). So tokenizing `rest` never fatally fails on a
  trailing unterminated construct that belongs to the *outer* context
  (e.g. the closing `"` of `"pre $(cmd) post"`); it simply stops, and every
  token's end-offset is recoverable.
- **Parser is the boundary oracle.** `parse_comsub` parses a command list and
  stops at the command-level `RParen`. A `)` that the grammar consumes as a
  case-pattern terminator (or inside a nested `(…)`/quotes) is never the
  close.
- **`tail_err` is ignored when a `)` is found.** It is consulted only when
  `parse_comsub` exhausts the tokens without closing (a real unterminated
  comsub) or to detect an unterminated in-comsub heredoc (see Error
  handling).
- **Nesting recurses naturally.** An inner `$(` encountered during the
  tolerant tokenize of `rest` re-enters `scan_paren_substitution`.
- **`quoted` is threaded** onto `WordPart::CommandSub` exactly as today.

## Components

1. **`command::parse_comsub(tokens: Vec<Token>, lines: Vec<u32>) ->
   Result<(Sequence, usize), ParseError>`** — new public parser entry. Parses
   a command list and stops at the command-level `RParen`, returning the
   parsed `Sequence` and the number of tokens consumed **through and
   including** that `)`. Reuses the existing command-list-until-`RParen`
   logic (the internal routine at lexer.rs:1839 that "breaks on
   `Token::Op(Operator::RParen)` (consuming it)"). This is the only new
   parser surface.

2. **`CharCursor::seek(pos: usize)`** — reposition the cursor to byte offset
   `pos`, recomputing `line` (count newlines in the skipped span). The one
   new `CharCursor` capability; `pos`/`line` are existing fields.

3. **Rewired `scan_paren_substitution`** — the orchestration above. The
   primary `scan_cmdsub_body` call (2795) and the helper
   `parse_substitution_body`'s use from this path are removed for `$()`;
   `parse_substitution_body` stays for the backtick path (2821).

4. **Comsub-mode heredoc handling** — see Error handling.

## Error handling & heredocs

- **Unterminated comsub** (`$(echo`): `parse_comsub` consumes every token
  without finding a command-level `)` → `LexError::UnterminatedSubstitution`.
  Message and behavior unchanged from today.
- **Real syntax error in body** (`$(if)`): `parse_comsub` returns
  `Err(ParseError)` → surfaced via `LexError::SubstitutionParseError`, as
  today.
- **Heredoc across the boundary**
  (`$(cat <<EOF`⏎`body`⏎`EOF`⏎`)`): no special handling needed —
  `tokenize_partial` tokenizes the whole `rest`, so the existing
  pending-heredoc queue collects the body from the later lines, the `)` after
  the terminator line is found by the parser, and `seek` jumps the cursor
  past the entire span.
- **Unterminated heredoc in comsub** (`$(cat <<EOF)` with no terminator
  before EOF): today huck hard-errors. New behavior — when `parse_comsub`
  finds its `)` but the pending-heredoc queue / `tail_err` indicates a
  heredoc reached EOF, emit bash's warning
  (`huck: warning: here-document … delimited by end-of-file …`) and treat
  the heredoc body as empty. This is the one intentional behavior change, and
  it matches bash's warn-and-continue.

## Edge cases & behavior parity

| Case | Behavior |
|---|---|
| `$(case x in a) echo;; esac)` | `)` after `a` stays in the body; only the final `)` closes. The headline fix. |
| `$(case x in esac)` (empty case) | Parser handles empty `case`; today's heuristic errors. Fixed. |
| `$(echo "a) b")` / `$(echo 'a)')` | `)` inside quotes lexes as string content; never a candidate close. |
| `$( (subshell) )` and `$(( … ))` arith | Untouched: the `$((`→try-arith-else-rewind logic in `scan_dollar_expansion` (lexer.rs:1812) sits above `scan_paren_substitution`; only the post-rewind path changes. |
| `"pre $(cmd) post"` | comsub-in-dquotes: tolerant tokenize of `cmd) post"` stops at the unterminated `"`; parser closes at `)`; `seek` lands on `"`; the outer dquote loop resumes. |
| `$(echo \) )` (escaped paren) | `\)` lexes as an escaped literal, not `RParen` → not a close. |
| `` `…` `` backtick | Unchanged (lexical boundary). |
| `$()` (empty) | Parser returns an empty `Sequence`; today's behavior preserved. |
| `$(# comment`⏎`)` | The `#`-comment and its `)` are handled by the real tokenizer/parser, not the heuristic comment-skip. |

Because the body now flows through the real tokenizer + parser, the
`case`/`esac`/`in` heuristics in `scan_cmdsub_body` and their documented
limitations **disappear** rather than getting patched.

## Testing

1. **Characterization net first (regression firewall).** Before touching
   `scan_paren_substitution`, capture huck's *current* comsub output across a
   corpus — the existing `scan_cmdsub_body` unit tests (lexer.rs:6122–6216)
   plus the comsub-relevant bash `.tests` fragments — as golden output. The
   refactor must keep every currently-passing case passing; only
   previously-failing cases may change. Explicit guard against the
   fix-one-break-another pattern.
2. **`tests/scripts/comsub_diff_check.sh`** (gold standard, new).
   Byte-identical bash-vs-huck over: `case`-in-comsub, empty case, nested
   comsub, comsub-in-dquotes, comsub-with-heredoc, unterminated-heredoc-in-
   comsub (warning + output), `$(( ))` still arith, escaped parens.
3. **Unit tests** for `parse_comsub` (correct `Sequence` + `n_consumed` for
   representative bodies, including case-pattern and nested cases) and
   `CharCursor::seek` (offset + line correctness across newlines).
4. **Full `cargo test --workspace`** green, plus a **bash-suite re-run** of
   `comsub`, `comsub-eof`, `comsub-posix`, `more-exp`, `new-exp` recording the
   new status (opportunistic flips noted, not required).

## Risks & migration

- **Primary risk: `seek`/offset-remap off-by-one or line miscount** would
  silently corrupt everything after a comsub. Mitigated by `CharCursor::seek`
  unit tests and the characterization net; the offset comes directly from
  `offs[n_consumed]`, whose existence the `offsets.len() == tokens.len() + 1`
  sentinel guarantees.
- **Perf:** tolerant tokenization of the discarded post-`)` tail is O(tail)
  per comsub. Tails on a line are short; acceptable at parse time. Noted, not
  optimized (YAGNI).
- **Migration:** `scan_cmdsub_body` stays for its 4 remaining callers. Its
  primary-path unit tests (6122–6216) are repurposed to target `parse_comsub`
  where they assert comsub *semantics*, and kept where they assert the
  verbatim-skip callers' behavior.
- **Follow-on `[deferred]` entries** for `docs/bash-divergences.md`:
  (a) verbatim-skip callers still heuristic (case/esac-in-`${…}`-operand);
  (b) any comsub category left non-PASS solely by the error-prologue prefix
  divergence.
