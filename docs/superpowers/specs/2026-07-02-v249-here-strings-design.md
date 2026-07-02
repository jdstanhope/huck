# v249 — Here-strings (`<<<`) on the atom-command path (dormant, differential) — Design

**Status: APPROVED (2026-07-02).** Second Phase-C **Stage 2** "port a deferred
construct onto the atom-command path" iteration (after v248 function
definitions). Direction:
`2026-06-30-phase-c-parser-driven-frontend-roadmap.md` (Stage 2) + memory
`huck-frontend-parser-driven-direction` / `huck-lexer-rearch-design`.

## 1. Goal & context

huck's front-end is being inverted so the lexer emits small atoms and the
PARSER (`crates/huck-syntax/src/parser.rs`, entry `parse_sequence` /
`parse_command`) assembles words + structure — a DORMANT path (gated by a
`command_atoms` lexer flag defaulting to `false`; production still uses the batch
Word-lexer + `command.rs` oracle) that must produce ASTs byte-identical to the
oracle, gated by the differential harness `new_seq` (atoms) vs `old_seq`
(oracle), with `diff_cmd(s)` asserting equality. Each Stage-2 iteration removes
one construct family from the atom path's deferred set. v249 ports **here-strings
(`<<< word`)**.

Here-strings are the lowest-risk remaining deferred family: the roadmap classes
`<<<` as "light — a new operator + a no-split word; not a full mode." Everything
needed already exists on the atom path — the port is removing two explicit
deferral guards.

## 2. What already exists (so the port is small)

- The atom scanner already emits `TokenKind::Op(Operator::HereString)` for `<<<`
  (v247 T5, `lexer.rs:2788`, in `scan_command_operator_atom`), distinct from the
  `<<`/`<<-` `Heredoc` token.
- `crate::command::is_redirect_op(Operator::HereString)` is already `true`, so
  `crate::command::next_is_redirect` already returns `true` for a leading `<<<`
  — `parse_simple` already routes it to the atom-path `parse_one_redirect`.
- The atom-path `parse_one_redirect` (`parser.rs:1250`) already: reads an
  optional fd-prefix, consumes the operator, skips an inter-token `Blank`,
  ASSEMBLES the target word from atoms via `parse_word_command` (or takes a
  legacy `Word`), and calls `crate::command::build_redirections(op, target,
  fd_prefix)`.
- `crate::command::build_redirections` already maps
  `Operator::HereString => vec![Redirection { fd: plain_fd(), op:
  RedirOp::HereString(target) }]` (`command.rs:1972`) — the exact AST the oracle
  builds. `plain_fd()` is `fd_prefix.unwrap_or(RedirFd::Default)` (stdin).

So the ONLY things stopping here-strings are two `UnsupportedCommand` guards.

## 3. Scope

**In scope.** Both positions a `<<<` redirect appears, producing the same
`Redirect::HereString(Word)` (slotted to stdin) as the oracle:

- As a redirect on a command: `cmd <<< word` — e.g. `cat <<< hello`,
  `wc -l <<<foo` (glued), `cat <<< "$x"`, `cat <<< 'lit'`, `cat <<< $'a\tb'`,
  `cat <<< $var`, `cat <<< a b` (target is the single word `a`; `b` is an arg to
  `cat`), interleaved with other redirects (`cmd <<< x > out`, `cmd 2>&1 <<< x`),
  and repeated (`cmd <<< a <<< b` — source-ordered list; executor applies
  last-wins).
- At command position (leading, no program): `<<< word` — the oracle falls
  through to `parse_simple`, producing an empty-words command with a stdin
  `HereString` redirect. Leading *file* redirects (`> out`) already work on the
  atom path, so this comes for free once the guard is relaxed.

Target word: assembled by the existing `parse_word_command` — literals, quoting,
and all expansions (`$x`, `${…}`, `$(…)`, `` `…` ``, `$((…))`) are handled as in
any redirect target; expansion happens at runtime, unchanged.

**Out of scope / stays deferred.**

- **Heredocs (`<<EOF … EOF`, `<<-`)** — the body-collection family is the hard,
  roadmap-unresolved-in-the-pull-model case; it stays `UnsupportedCommand` on the
  atom path (both guards keep deferring `TokenKind::Heredoc`).
- No live flip: `command_atoms` stays `false`; production untouched.
- No lexer changes and NO `command.rs` changes (`is_redirect_op` /
  `next_is_redirect` / `build_redirections` are already `pub(crate)` and already
  used by the atom path). v249 edits ONLY `parser.rs`.

## 4. Design

Two deferral removals in `parser.rs`, plus a differential test corpus.

### 4.1 `cmd <<< word` — remove the redirect-path guard

In `parse_one_redirect` (`parser.rs:1266–1306`), delete the HereString deferral
(currently `parser.rs:1269–1272`):

```rust
// HereString (`<<<`) — deferred.
if matches!(op, Operator::HereString) {
    return Err(ParseError::UnsupportedCommand);
}
```

With it gone, `Operator::HereString` flows through the existing generic path:
skip a `Blank`, assemble the target via the existing target `match` (which
already returns `RedirectTargetIsOperator` / `MissingRedirectTarget` for a
missing/operator target, matching the oracle), then
`build_redirections(Operator::HereString, target, fd_prefix)` yields
`RedirOp::HereString(target)`. No other change needed here — the
process-substitution guard (`<(`/`>(`) directly above is unaffected (it matches
only `RedirIn`/`RedirOut`).

### 4.2 Leading `<<<` — relax the command-position guard

In `parse_command` (`parser.rs:1552–1556`), change the guard so it defers only
heredocs, letting a leading `<<<` fall through to `parse_simple`:

```rust
// Heredoc at command position — deferred (heredoc BODIES are future work).
// `<<<` (here-string) is NOT deferred here: it flows to parse_simple as a
// leading redirect (an empty-words command reading stdin from the here-string),
// matching the oracle (which falls through to parse_pipeline → parse_simple_stage).
if matches!(iter.peek_kind()?, Some(TokenKind::Heredoc { .. })) {
    return Err(ParseError::UnsupportedCommand);
}
```

Trace for leading `<<< word`: the arith/`((`/bare-`(` guards don't match;
`peek_leading_keyword` returns `None` (a `<<<` is an `Op`, not a keyword);
funcdef detection is skipped (peek is an `Op`, not `Lit`/`Word`); `parse_simple`
runs, its loop's `next_is_redirect` fires on the `<<<`, `parse_one_redirect`
builds the `HereString` redirect, and with no words it finalizes an empty-words
command carrying the redirect — the same AST the oracle produces.

## 5. Testing (differential gate)

All in `parser.rs` `mod tests`, using `diff_cmd` (asserts `new_seq == old_seq`)
and the deferred/error patterns already in the file.

- **`atoms_here_string_redirect`** (`diff_cmd`): `cat <<< hello`, `wc -l <<<foo`
  (glued, no space), `cat <<< "$x"`, `cat <<< 'lit'`, `cat <<< $'a\tb'`,
  `cat <<< $var`, `cat <<< a b` (target `a`, `b` an arg), `cmd <<< x > out`
  (here-string + file redirect, source order), `cmd 2>&1 <<< x`
  (fd-dup + here-string), `cmd <<< a <<< b` (two here-strings, ordered list).
- **`atoms_here_string_leading`** (`diff_cmd`): `<<< word`, `<<<foo`,
  `<<< "$x"`, `<<< word > out`, and a here-string as a pipeline stage
  (`<<< x | cat`) if the oracle accepts it (verify — else drop that one).
- **`atoms_here_string_fd_prefix`** (`diff_cmd`): `3<<< word` — pin whatever the
  oracle produces (fd-prefixed here-string). If the oracle rejects it at the
  lexer level (`old_seq` would panic via `.expect("lex")`), move it to the
  error-parity bucket asserting only `new_seq(s).is_err()` — determine by
  observation, not assumption.
- **`atoms_here_string_errors`** (error parity — compare `new_seq` to `old_seq`
  normalized to `Ok(())`/error-debug, mirroring the existing `atoms_error_parity`
  split of lexer-level vs parser-level rejects): `cat <<<` (EOF → missing
  target), `<<<` (leading, EOF), `cat <<< |` (target is operator),
  `cat <<< <` (target is operator), `cat <<< ;`.
- **`atoms_here_string_heredoc_still_deferred`** (`new_seq` → `UnsupportedCommand`):
  `cat <<EOF\nx\nEOF` and a leading `<<EOF\nx\nEOF` — heredocs remain deferred
  (proves the guard change is `<<<`-only, not a heredoc regression).
- **Non-interference:** the existing redirect corpus (`atoms_structure` etc.)
  stays green — the change must not affect file redirects / fd-dups.
- Full `huck-syntax` lib green single-threaded
  (`cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1`), doctests
  green, `cargo build -p huck-syntax` 0 warnings.

## 6. Non-goals / follow-ons

- Heredoc bodies (`<<EOF`) — a later, harder iteration (the pull-model
  interaction is roadmap-open).
- The live flip + scanner deletion (the finale) — unchanged by v249.
- Other deferred families (process sub `<(…)`, array literals, `[[ ]]`, arith
  command, coproc, `$[ ]`) — future port iterations.

## 7. Invariants

- Byte-identical: every in-scope here-string input parses to the SAME AST /
  same error on the atom path as the oracle (`diff_cmd` / error parity). A
  well-formed in-scope divergence is a v249 BUG to fix, not to pin.
- Production untouched: `command_atoms` defaults `false`; NO `command.rs` or
  `lexer.rs` changes (both already expose what's needed); `scan_step_command` /
  `process_line` unchanged.
- Heredocs stay deferred (both guards keep `TokenKind::Heredoc`).
- 0 warnings; every commit carries the `Co-Authored-By: Claude Opus 4.8 (1M
  context)` trailer; branch `v249-here-strings`, not `main`.
