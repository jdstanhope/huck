# v260 — Iteration B carry-forward fix (CF1): heredoc-in-word fill-walk

**Status:** design approved (2026-07-05)
**Arc:** Phase C reconciliation — clearing the accumulated live-flip carry-forwards
before the finale (flip `command_atoms` live + delete the forward-scanning scanners).
Iteration A (v259) resolved CF2/CF3/CF4. **This is Iteration B — CF1, the substantive
one: the only carry-forward that silently DROPS real user data.** From the verified
carry-forward inventory (`huck_carryforward_inventory.md`).

## Summary

On the dormant atom-command path (`new_seq`, `command_atoms` default `false`), a heredoc
whose opener sits **inside a Word** — a command substitution `$(…)`/`` `…` ``, a process
substitution `<(…)`/`>(…)`, an arithmetic `$((…))`, a `${…}` operand, an array-literal
element, or a quoted span — has its BODY DROPPED (`body: Word([])`). The oracle
(`old_seq`) and bash fill it. Root cause: the post-parse heredoc-body fill walk
(`fill_command`, parser.rs:2689) attaches bodies only to a command's own
`redirects`; it never descends into the command's Words, so a heredoc nested inside a
Word is never reached by the FIFO body queue.

**Fix (Scope A, chosen):** add a `fill_word` recursion that descends into every nested
`Sequence`/`Word` a `WordPart` can carry, and make `fill_command` walk the command's own
Words (inline-assignment values, program, args) and, for a bare assignment, the
assignment value Words. A fixed visit order (**words then redirects**) fills every
common case correctly; the single order-sensitive case — a heredoc redirect appearing
in source *before* a heredoc-bearing Word on the same command — is PINNED as a remaining
divergence (matching it needs source-position tracking the AST does not carry; that is
the rejected Scope B).

**Dormant + differential.** `command_atoms` stays `false`. `command.rs` is EMPTY-diff
(the change is entirely in the atom parser's fill walk). `parser.rs` only.

## Background — why bodies are dropped (probed)

The atom lexer emits each heredoc body as atoms right after the `Newline` ending the
opener's line, and the parser stashes each parsed body into a Lexer-owned FIFO queue
(`parsed_heredoc_bodies`; `take_heredoc_bodies` at lexer.rs:839). After the full
sequence parses, `parse_sequence` drains the queue and calls `fill_sequence` →
`fill_command`, which walks the command tree and, at each still-empty
`RedirOp::Heredoc { body: Word([]) }` placeholder, pops the next queued body
(`fill_redirects`, parser.rs:2671). The queue is FIFO in **lexer emission order =
source order of the openers** (left-to-right per physical line).

`fill_command` (parser.rs:2689) is exhaustive over `Command` variants and recurses into
nested `Sequence`s (if/while/for/case/brace/subshell/coproc/…), but for
`Simple(Exec)` it only does `fill_redirects(&mut exec.redirects, bodies)` — it never
looks at `exec.program`/`exec.args`/`exec.inline_assignments`, and for
`Simple(Assign)` it does nothing. So a heredoc opener inside any Word is never visited,
and its body stays queued (or, with multiple, mis-attaches). Probed EQ=false for:
`echo $(cat <<X…)`, `x=$(cat <<X…)`, `a=($(cat <<X…))`, `echo ${y:-$(cat <<X…)}`,
`echo $(( $(cat <<X…) + 2 ))`. Plain outer-redirect heredocs (`cat <<A <<B`) already
work (EQ=true).

**No source positions in the AST.** `WordPart::CommandSub`/`ProcessSub`, `Word`, and
`Redirection` carry no offset/column; only `ExecCommand` has a `line`. So the fill walk
cannot sort placeholders by source column — it must visit them in an order that
reconstructs emission order. A fixed AST-field visit order reconstructs it correctly
except when a command has BOTH a Word-nested heredoc AND its own redirect heredoc whose
columns interleave against that order (see the pin).

## Architecture

**Files:**
- `crates/huck-syntax/src/parser.rs` — new `fill_word` + `fill_param_modifier`; extend
  `fill_command`'s `Simple(Exec)` and `Simple(Assign)` arms. Update the corpus + flip
  the v250 pin.
- `crates/huck-syntax/src/command.rs` — UNTOUCHED (EMPTY diff).

### `fill_word(word, bodies)`

Walks `word.0` left-to-right; for each `WordPart` recurses into every nested
`Sequence`/`Word` **in source order**, EXHAUSTIVE (no `_ =>` wildcard, mirroring
`fill_command`):

- `CommandSub { sequence, .. }` → `fill_sequence(sequence, bodies)`
- `ProcessSub { sequence, .. }` → `fill_sequence(sequence, bodies)`
- `Arith { body, .. }` → `fill_word(body, bodies)`
- `Quoted { parts, .. }` → recurse each inner part (a shared inner helper walks a
  `&mut [WordPart]`)
- `ParamExpansion { subscript, modifier, .. }` → walk the **subscript first** (if
  `Some(SubscriptKind::Index(w))` → `fill_word(w)`; `All`/`Star` carry no Word), then
  `fill_param_modifier(modifier, bodies)` (source order: `${a[i]:-word}` has the
  subscript before the modifier word)
- `ArrayLiteral(elems)` → each element in order: its `subscript` (`Option<Word>`) then
  its `value` (`Word`) — `[idx]=val`, subscript before value
- `Literal`/`Tilde`/`Var`/`LastStatus`/`AllArgs`/`AssignPrefix` → no-op (no nested Word),
  listed explicitly

### `fill_param_modifier(modifier, bodies)`

Exhaustive over `ParamModifier`; recurses into each variant's Word(s) in source order:
- `UseDefault{word,..}`/`AssignDefault{word,..}`/`ErrorIfUnset{word,..}`/
  `UseAlternate{word,..}` → `fill_word(word)`
- `RemovePrefix{pattern,..}`/`RemoveSuffix{pattern,..}` → `fill_word(pattern)`
- `Substitute{pattern, replacement, ..}` → `fill_word(pattern)` then
  `fill_word(replacement)` (pattern precedes replacement in `${x/pat/rep}`)
- `Substring{offset, length}` → `fill_word(offset)` then, if `Some(l)`, `fill_word(l)`
- `Case{pattern: Some(p), ..}` → `fill_word(p)` (`None` → nothing)
- `None`/`Length`/`IndirectKeys`/`PrefixNames`/`Transform`/`BadSubst` → no-op (no Word)

### `fill_command` — extended arms

- `Simple(SimpleCommand::Exec(exec))` → in source order:
  1. for each `a` in `exec.inline_assignments`: if `a.target` is
     `AssignTarget::Indexed { subscript, .. }` → `fill_word(subscript)`, then
     `fill_word(&mut a.value)` (`A=$(cat <<X) cmd`)
  2. `fill_word(&mut exec.program)`
  3. for each arg in `exec.args` → `fill_word(arg)`
  4. `fill_redirects(&mut exec.redirects, bodies)` ← **redirects LAST** (the visit-order
     choice)
- `Simple(SimpleCommand::Assign(items, _))` → for each `a` in `items`: walk
  `a.target` Indexed subscript (if any) then `fill_word(&mut a.value)`. (Bare
  assignments carry no redirects — `x=$(cat <<X)`, `a=($(cat <<X))`.)

All other `fill_command` arms are unchanged.

### Visit order & the pin

**Order = words then redirects** (chosen). Rationale:
- Plain outer-redirect heredocs (`cat <<A <<B`, `cat foo <<A`) still fill correctly —
  no Word heredocs, `fill_redirects` runs on the in-order `redirects` list.
- Matches the oracle for the **idiomatic trailing-redirect** interleaving
  `cat $(sh <<B…) <<A…`: emission order B (arg) then A (redirect); words-first fills the
  arg (B) then the redirect (A) → correct.

**PINNED remaining divergence:** a heredoc redirect appearing in source *before* a
heredoc-bearing Word on the same command — `cat <<A $(sh <<B)`: emission order A
(redirect, col 5) then B (arg-cmdsub); words-first visits the arg (B) first and pops
body-A into it, then the redirect (A) gets body-B → both mis-attached. Matching this
needs per-node source positions the AST does not carry (Scope B, rejected as
disproportionate for an unusual construct). Pin the observed atom output; document that
the oracle is correct. (Bare-assignment commands cannot hit this — they have no
redirects — so all the common `x=…`/`a=(…)` cases are unaffected.)

## Differential corpus

**Fixed — new `diff_cmd` (all EQ=false today, must become byte-identical):**
- `echo $(cat <<X\nhi\nX\n)` (arg cmdsub)
- `x=$(cat <<X\nhi\nX\n)` (assignment RHS)
- `a=($(cat <<X\nhi\nX\n))` (array-literal element value)
- `echo ${y:-$(cat <<X\nhi\nX\n)}` (param-expansion operand)
- `echo $(( $(cat <<X\n1\nX\n) + 2 ))` (arith body)
- `` echo `cat <<X\nhi\nX\n` `` (backtick)
- `echo $(a <<X\nxx\nX\n)$(b <<Y\nyy\nY\n)` (two Word-nested heredocs, queue order)
- `cat $(sh <<B\nbb\nB\n) <<A\naa\nA\n` (Word heredoc THEN trailing outer redirect — the
  idiomatic interleaving the chosen order handles)
- `FOO=$(cat <<X\nhi\nX\n) echo hi` (inline assignment value)

**Regressions — stay green:** `cat <<A\naa\nA\n`, `cat <<A <<B\naa\nA\nbb\nB\n`,
`cat <<A; cat <<B\n…` (plain/sequence heredocs already work).

**Pinned — documented divergence:** `cat <<A $(sh <<B\nbb\nB\n)\naa\nA\n` (redirect
before a Word-nested heredoc). Assert the actual atom output (mis-ordered) and note the
oracle is correct; matching requires Scope B source positions.

## Testing & gates

- Differential harness in `parser.rs mod tests`: `diff_cmd` for the fixed corpus; a
  pinned test for the interleaved case.
- Replace the v250 pin `atoms_heredoc_in_cmdsub_body_drop_divergence` (parser.rs) with
  `diff_cmd` (CF1 resolves it).
- `command.rs` diff-vs-main = EMPTY.
- Both `command_atoms` sites stay `false`.
- `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` green (box is
  1 core/1.9GB — never `--workspace`, never multi-threaded).
- `cargo build -p huck-syntax` → 0 warnings.

## Task decomposition (SDD)

- **T1 — `fill_word` + `fill_param_modifier` (the recursion):** the exhaustive Word/
  modifier walk + the Word-nesting corpus that has NO redirect interleaving (arg cmdsub,
  assignment RHS, array value, param operand, arith, backtick, two Word-nested, inline
  assignment). `fill_command` Exec/Assign arms wired to call it (words-then-redirects).
- **T2 — visit-order & interleaving + pins:** the `cat $(sh <<B) <<A` diff_cmd (trailing
  redirect handled), the `cat <<A $(sh <<B)` pin (redirect-first, documented), the
  outer-redirect regressions, and flipping the v250
  `atoms_heredoc_in_cmdsub_body_drop_divergence` pin to `diff_cmd`.

(T1 and T2 could be one task; splitting keeps the recursion and the source-ordering
concerns separately reviewable — the ordering is the subtle part.)

## Live-flip carry-forwards

RESOLVES CF1 (the substantive data-drop). Also folds the v257 carry-forward "heredoc in
a coproc body nested in `$()`" — coproc bodies recurse via `fill_command` and the cmdsub
now descends. NEW pin: the redirect-before-Word-heredoc interleaving
(`cat <<A $(sh <<B)`) — needs Scope B source positions; recorded in
`huck_carryforward_inventory.md`. After merge, mark CF1 resolved and record v260 in the
iteration log. Remaining before the finale: Iteration C (CF6+CF7 arith quote sub-mode),
plus F2 and the array-literal-subscript bare-dquote divergence; CF5/CF8/CF9/CF10 stay
keep-intentional. No `bash-divergences.md` change (dormant).
