# v266 — Sever the last two atom→oracle bridges, then delete the oracle

**Date:** 2026-07-06
**Status:** Design approved; ready for planning
**Depends on:** v264 (THE FLIP — the atom parser is live in production, 461b585); v265 (shrink-oracle-to-leaf-sub-lexer + decouple-harness, merge 7271959)
**Supersedes the deferred goal of:** v265 T4/T5 (delete the oracle + module tidy)

## Motivation

Since v264 THE FLIP, the atom parser (`parser::parse_sequence` over
`Lexer::new_live_atoms`) is the production front-end. But the oracle — the old
`command.rs` recursive-descent parser (`command::parse`), the whole-buffer Word
lexer (`tokenize`/`tokenize_with_opts`/`tokenize_no_brace`/`from_tokens` + the
Word scanner `scan_step_command` + the 6 forward-scanners), and the old word-part
scanners — is still **live production code**, not a dead differential reference.
v265 attempted to delete it (T4) and discovered why it could not: the atom
production path still delegates fragment lexing to the oracle at a small number
of leaf sites. See `huck-oracle-not-self-contained` and the RE-SCOPED banner on
`docs/superpowers/plans/2026-07-06-v265-delete-oracle.md`.

This iteration removes that delegation, which makes the entire oracle subtree
unreachable from production, and then deletes it. End state: **one parser, one
lexing discipline** — `lexer.rs` = token production, `parser.rs` = all parsing,
`command.rs` = AST types only.

## Corrected scope: two bridges, not four

The v265 memory listed **four** leaf sub-lexers as the prerequisite
(`parse_subscript_body`, `maybe_expand_command_alias`, `scan_array_element_word`,
`parse_substitution_body`). A full **transitive audit** — every production call
site of an oracle entry point (`tokenize*`/`from_tokens`/`command::parse`) in
`crates/huck-syntax/src/lexer.rs`, each classified by whether the *atom* path (not
the oracle) actually reaches it — shows only **two** are genuine atom→oracle
bridges:

| Sub-lexer / site | Atom-reachable? | Verdict |
|---|---|---|
| `maybe_expand_command_alias` @~5072 (`tokenize(&body)`) | ✅ parser's `expand_command_alias` calls it | **PORT — bridge 1 (alias)** |
| `parse_subscript_body` via `try_scan_assign_prefix` @~4231 (`a[i]=v` lvalue) | ✅ atom `scan_command_word_atom` → `try_scan_assign_prefix` | **PORT — bridge 2 (subscript)** |
| `scan_array_element_word` @~7383 (`a=(…)` elements) | ❌ only from oracle `scan_step_command`; atom path uses `Mode::ArrayLiteral` | delete with oracle |
| `parse_substitution_body` + `command::parse` @~6832 (`$(…)`) | ❌ only via oracle word-part scanners / transitively through the two bridges above | delete with oracle |
| `scan_param_subscript` @~6991/7102/7155 (`${a[i]}`) | ❌ under oracle `scan_dollar_expansion` (`scan_braced_param_expansion`) | delete with oracle |
| `with_in_dquote` / `fd_prefix_of_text` (dquote helpers) | ❌ oracle-only | delete with oracle |
| `pub fn remaining` @~4960 | ❌ test-only caller | delete with oracle |

Porting **alias** + **subscript-lvalue** severs the last two atom→oracle edges;
the rest become an unreachable island reachable only from tests, and get deleted
wholesale.

### The wrinkle that shapes both ports

Neither bridge can be ported by "re-lex the fragment with a fresh atom lexer and
drive it to EOF." The atom lexer emits **zero-width opener signals**
(`$(`→`CmdSubOpen`, `$((`→`ArithOpen`, `${`→`ParamOpen`, backtick, extglob) that
only the **parser** consumes; a standalone driver past one of them spins forever
(the T6 lesson — `command_atoms_of` hangs on `"a b"`/`${…}`/`$(…)`). Therefore
both ports must keep the **parser in the loop** for fragment assembly. This is
THE RULE restated: the parser owns delimiter-matching, recursion, and word
assembly.

## Bridge 1 — alias via an input-source stack

Replace history-token-splicing (`tokenize(&body)` → splice Word tokens into
`self.history`) with a **stack of input sources** in the lexer.

Today the lexer's `CharCursor` reads a single input. Give the lexer an ordered
stack of char sources; `next()` reads characters from the top source and pops
back to the parent source at that source's EOF.

- **Firing an alias** — at command position, when the current command word is a
  bare literal `name` with `name ∈ aliases` and `name ∉ active`: **push** the
  alias body text as a new input source and insert `name` into `active`. No
  pre-tokenizing, no history splicing. The lexer reads the body's characters
  inline, emitting normal atoms and the zero-width opener signals the **parser**
  consumes exactly as for any other input. This is what sidesteps the
  standalone-driver spin: the parser is always the entity draining the lexer, so
  `alias now='echo $(date)'` lexes correctly.
- **Recursion guard** — `active: HashSet<String>` is re-anchored to **source-stack
  frames**: `name` is inserted when its body source is pushed and removed when
  that source is popped (body exhausted). At command position, expand `name` only
  if `name ∉ active` ("we are not currently lexing inside this name's own
  replacement text"). Semantics are identical to today's guard and to bash: a
  name is marked while its expansion is in progress and cannot re-expand itself,
  so `alias ls='ls --color'` expands once and `a→b→a` chains terminate. This is
  NOT new logic — `active` already does exactly this; only the insert/remove
  points move from around the recursive call to source push/pop.
- **Trailing-blank eligibility** — `alias_trailing_eligible` (bash's "body ends in
  whitespace ⇒ the next command word is also alias-eligible") rides along: set
  when a source is pushed, read by the next command-position check. Preserve
  current behavior.
- **Ownership / lifetime** — alias bodies are owned `String`s from the alias map,
  so the source stack holds **owned** sources (e.g. `Cow<'a, str>` or an enum of
  borrowed-base + owned-alias-body), not just `&'a str`. This is a contained
  `CharCursor`/`Lexer` adjustment; the base input stays borrowed.

Removed once this lands: the `tokenize(&body)` call and the token-splice loop in
`maybe_expand_command_alias`. `expand_command_alias`/`take_trailing_eligible`
public API is preserved (the parser's two call sites are unchanged).

## Bridge 2 — subscript `Word` assembled by the parser

Keep assignment **detection** in the lexer (it already speculatively scans
`name[…]` to confirm a `]=`/`]+=` follows — unchanged), but move subscript-body
**word assembly** out of the oracle and into the parser.

**The AST type is unchanged.** `command.rs`'s `AssignTarget::Indexed { name,
subscript: Word }` stays exactly as-is — the engine/executor is untouched. What
changes is the *lexer-side* representation and *where* the `Word` is built:

- Today the lexer emits `TokenKind::AssignPrefix { target: AssignTarget, append }`,
  reusing the AST `AssignTarget` directly — so for the indexed case it must build
  the subscript `Word` at lex time (via `parse_subscript_body`→`tokenize`). Because
  the lexer cannot assemble atoms without spinning (the opener-signal wrinkle
  above), it cannot build that `Word`.
- Instead, the lexer carries the **raw `[…]` text** in a lexer-only representation
  for the indexed case (e.g. a new `TokenKind::AssignPrefix` indexed variant, or a
  lexer-side `AssignTargetRaw` holding `subscript_raw: String`) — it does **not**
  put a half-built AST `AssignTarget::Indexed` on the token stream.
- When the **parser** consumes that atom to build the command, it assembles the
  subscript `Word` via a new reusable helper `parse_fragment_word(raw, opts) ->
  Result<Word, ParseError>` (construct `Lexer::new_live_atoms(raw, …)` and run the
  parser's existing word-part assembly loop to EOF), then constructs the unchanged
  AST `AssignTarget::Indexed { name, subscript: Word }`. Because the parser drives
  it, `$i` / `${j}` / `$((n))` / `$(…)` / quotes inside the subscript all work, and
  there is **no lexer→parser dependency** (assembly lives in `parser.rs`).
- The same treatment applies wherever a `WordPart::AssignPrefix` carries an indexed
  target (command-word-position `a[i]=v`): lexer carries raw, parser assembles the
  final `Word`.
- **Preserve the current fallback** — an empty or multi-`Word` assembly collapses
  to a single unquoted `Literal` carrying the raw text (exactly what
  `parse_subscript_body` does today for arithmetic subscripts like `a[1 + 2]`, so
  arithmetic evaluation sees the joined text).
- `parse_fragment_word` is the reusable "bounded fragment → one `Word`" primitive,
  not a one-off.

**Deliberately out of scope:** rewriting subscripts to full "parser matches `]`"
RULE-compliance (lexer emits `[`/body-atoms/`]`, parser matches the close). That
is a larger correctness change — it would also fix the latent `]`-inside-`$(…)`
subscript-boundary edge in the current forward-scan — and is orthogonal to
deletion. It is flagged as a **follow-up divergence**, not done here, to keep this
iteration's blast radius small. Detection stays in the lexer; only assembly moves.

## Deleting the oracle (compiler-guided, but manual)

Once both bridges are severed the oracle is unreachable from production.

**Caveat from the v265 post-mortem:** `dead_code` will **not** reliably fire —
rustc has a false-negative for mutually-/self-recursive dead cycles. This is
compiler-*assisted*, not compiler-*driven*: after each removal, grep-assert zero
remaining references to the deleted symbol rather than trusting the lint.

Order:

1. Delete the now-unused oracle entry points: `command::parse` (+ `parse_one_unit`
   and the recursive-descent body), the `tokenize` family
   (`tokenize`/`tokenize_with_opts`/`tokenize_no_brace`/`tokenize_partial_inner`),
   `from_tokens`, and the test-only `pub fn remaining`.
2. Work outward, deleting what is now unreferenced: the Word scanner
   `scan_step_command` non-atom branch, the 6 forward-scanners,
   `parse_substitution_body`, `scan_paren_substitution`, `scan_dollar_expansion`,
   `scan_braced_param_expansion`, `scan_param_subscript`, `scan_array_literal`,
   `scan_array_element_word`, `scan_subscript`, the old `parse_subscript_body`,
   `with_in_dquote`, `fd_prefix_of_text`, and the old word-part scanners
   (`scan_dquote_expansion_body`, `scan_regex_operand`, `scan_extglob_group`,
   `scan_expanding_body_line`, `scan_arith_body`) plus their private helpers.
3. Retire `Mode::Command`'s non-atom branch and the now-always-true
   `command_atoms` flag (the flag becomes unconditional; remove it and the dead
   branch).
4. Module tidy (v265's deferred T5): `command.rs` keeps the **AST types**
   (`Command`/`Sequence`/`Word`/`SimpleCommand`/…) but loses the recursive-descent
   parser. End state — `command.rs` = AST, `lexer.rs` = token production,
   `parser.rs` = all parsing.

## Testing & verification

Behavior preservation is the bar. Safety nets, in priority order:

- **Bash-diff sweep** (`tests/scripts/*_diff_check.sh`) must stay **1688 pass / 1
  fail** (funcnest, pre-existing intentional L-63) — the real behavioral gate. Run
  guarded per-harness (`ulimit -v 1500000` + `timeout` in a subshell), especially
  the alias/subscript/array/command-sub/param-expansion harnesses.
- **Both crate suites green**, single-threaded per the OOM constraint:
  `cargo test -p <crate> --jobs 1 --lib -- --test-threads 1`. Baseline:
  huck-syntax 1059, huck-engine 1739 (counts shift as oracle-only tests are
  deleted; the bash-diff 1688/1 must not regress).
- **New focused tests per ported bridge:**
  - Alias: body containing `$(…)` / `${…}` / a pipe / a trailing space; recursion
    guard (`alias ls='ls --color'` expands once); `a→b→a` chain terminates;
    trailing-blank makes the next word eligible.
  - Subscript: `a[$i]=`, `a[$((n))]=`, `a[$(echo 2)]=`, quoted `a["$x"]=`,
    multi-word `a[1 + 2]=` collapse-to-literal, `+=` append.
- **Oracle-referencing tests:** any remaining `atoms_*_matches_oracle` differential
  tests not already converted in v265 T3 are converted to atom-only assertions or
  deleted, since the oracle reference disappears.
- **0 warnings** from `cargo build -p huck-syntax` and `-p huck-engine` at each
  task end. Trust `cargo`, not rust-analyzer.

## Task breakdown

Subagent-driven-development: one fresh subagent per task, spec + code-quality
review between tasks. Hard dependency chain — T3–T5 require both bridges landed.

1. **T1** — Alias input-source stack: source stack in `CharCursor`/`Lexer`,
   `active` re-anchored to push/pop, trailing-blank preserved; remove the
   `tokenize(&body)` splice. + tests.
2. **T2** — Subscript: lexer carries raw `[…]` text (lexer-only indexed
   representation; AST `AssignTarget::Indexed { subscript: Word }` unchanged) +
   `parse_fragment_word` parser helper (assembles the `Word`, with the
   empty/multi-word→raw-`Literal` fallback) + remove the `parse_subscript_body`
   call from `try_scan_assign_prefix` and the command-word `WordPart::AssignPrefix`
   indexed path. + tests.
3. **T3** — Audit the oracle is now a test-only island (grep every oracle entry
   point has zero production callers); convert/delete remaining oracle-referencing
   tests.
4. **T4** — Delete the oracle (entry points → outward, grep-verified per removal).
5. **T5** — Module tidy: `command.rs` = AST only.
6. **T6** — Final bash-diff sweep + both suites green + fill any coverage gap.

T1 and T2 are independent; run sequentially with a review between (project
workflow). T4 is the large deletion (~several thousand lines); expect manual
cycle-breaking where `dead_code` stays silent.

## Constraints (carried from v265)

- **Test runner (box is 1 core / 1.9 GB):** ONLY
  `cargo test -p <crate> --jobs 1 --lib -- --test-threads 1`. NEVER `--workspace`
  or multi-threaded (OOM-kills the session). Crates: `huck-syntax`, `huck-engine`.
- **Build the binary** with `cargo build -p huck` (root package). `huck-cli` is a
  lib and does NOT build the binary.
- **Guard every bash-diff harness / binary run** with `ulimit -v 1500000` +
  `timeout` in a subshell.
- **THE RULE:** the lexer emits small atoms and NEVER forward-scans for a matching
  delimiter across nesting; the parser owns delimiter-matching, recursion, and
  word assembly. (The retained subscript detection forward-scans only a
  bracket-balanced `[…]` to confirm the assignment shape — a bounded,
  non-recursive scan — and is explicitly noted as a follow-up for full
  RULE-compliance.)
- **Commit trailer, verbatim on every commit:**
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **Do not change AST types or shell behavior** except as each task specifies.
  v266 is bridge-port + deletion + relocation — behavior-preserving, verified by
  the bash-diff sweep.
