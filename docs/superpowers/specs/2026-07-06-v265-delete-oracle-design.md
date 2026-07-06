# v265 — Delete the Oracle (Finale Milestone 2)

**Date:** 2026-07-06
**Status:** Design approved; ready for planning
**Depends on:** v264 (THE FLIP — the atom parser is live in production, 461b585)

## Motivation

v264 repointed production command execution onto the atom path
(`new_live_atoms` → incremental atom scanners → `parser::parse_sequence` /
`parse_one_unit`). The old front-end — the `command.rs` recursive-descent
**oracle** parser and the six forward-scanning lexer functions — was left
**resident** as the reference for the differential test harness and as the
engine of `continuation.rs`.

v265 removes it. This is the finale's second and final milestone: after v265
there is exactly one parser and one lexing discipline (THE RULE: the lexer
emits small atoms and never forward-scans for a matching delimiter; the
parser owns delimiter-matching, recursion, and structure).

## Goals

1. Delete the oracle parser (`command::parse` / `parse_one_unit` /
   `parse_cursor` + the recursive-descent function tree that forward-consumes
   a pre-tokenized `Vec<Token>`).
2. Delete the six forward-scanners and the batch-tokenize path that fed the
   oracle: `scan_step_command` (the non-atom `Mode::Command` branch),
   `scan_dollar_expansion`, `scan_arith_body`, `scan_backtick_body`,
   `scan_braced_param_expansion`, `scan_legacy_arith_body`, plus
   `tokenize` / `tokenize_with_opts` / `from_tokens` and the `Vec<Token>`
   replay machinery, and any code that goes transitively dead with them
   (e.g. `emit_word_with_braces`).
3. Port the one remaining production oracle consumer, `continuation.rs`,
   onto the atom path.
4. Decouple the differential test harness from the oracle by **smoke-converting**
   its helpers. The harness is far larger than a first count suggested — ~882
   assertions (`diff_cmd` alone has 830 call-sites) across ~150 of parser.rs's
   182 test fns, the whole atom-parser AST regression corpus. Rather than
   delete it, change the *helper bodies* so they no longer call the oracle:
   assert the atom parser returns `Ok`/`Err` (as appropriate) instead of
   `atom == oracle`. This keeps every curated input as a parse-level
   regression guard while removing the oracle dependency. The `old_seq` /
   `old_seq_al` / `old_unit` / `old_eg` helpers then go unused and are deleted.
5. Tidy module ownership to the intended end-state (below).
6. Backfill durable, oracle-independent shape coverage: a curated set of
   focused tests asserting exact **atom token-streams** (lexer) and exact
   **ASTs** (parser) for representative inputs across every grammar family.
   This replaces the AST-shape verification the smoke-convert drops, in a
   readable form that depends on nothing being deleted.

## Non-Goals

- No behavior change for the user. Every huck-engine behavioral test and every
  `tests/scripts/*_diff_check.sh` bash-diff harness must stay green. v265 is a
  deletion + relocation, not a feature or a bug-fix iteration.
- No new grammar, no new parser capability. The atom path already has full
  coverage (Stage 2 completed at v258; reconciliation completed v259–v263).
- No rewrite of the atom parser or lexer internals beyond moving whole
  functions between modules.

## End-State Architecture

The organizing principle: **`command.rs` holds the AST; the lexer only
produces tokens; the parser owns all parsing.** The dividing line between
"parsing" and "data" is *does it consume tokens?*

### `command.rs` — the AST module

Keeps: the AST type definitions (`Sequence`, `Command`, `Pipeline`,
`ExecCommand`, `Redirection`, `Redirect`, `RedirOp`, `RedirFd`, `ParseError`,
`TestExpr`, `TestUnaryOp`, `Assignment`, …), their inherent impls
(`.name()`, `.default_fd()`, `.target_fd()`, `.program_static_text()`,
`.slot_stdin/stdout/stderr()`, `slots_for_simple_path()`), and the **pure
query/predicate functions over the AST or a `Word`**:
`is_assignment_word`, `try_split_assignment`, `try_split_assignment_ref`,
`valid_function_name_text`, `valid_identifier_text`, `is_function_body_shape`,
`is_compound_opener`, `try_unary_op`, `is_bang_word`, `word_literal_text`,
`lit_word`, `is_redirect_op` (operates on `&Operator`, no token stream).

These pure predicates stay with the AST — they take no `&mut Lexer`, consume
no tokens, and the **engine** (executor/expand) calls several of them at
runtime, so keeping them with the AST avoids making the engine depend on the
parser.

Removes: the entire oracle parser and everything only it reached.

After v265, `command.rs` contains **no code that consumes tokens** and has no
dependency on `parser.rs` or the lexer's scanning loop (it already has no
`parser.rs` dependency today — v265 preserves that).

### `parser.rs` — the sole parser

Keeps: the atom-driven recursive-descent parser (`parse_sequence`,
`parse_one_unit`, and its helpers).

Gains (moved in from `command.rs`): the **parser-internal** helper functions
the parser reuses — those that either take `&mut Lexer` (`next_is_redirect`,
`next_is_test_binary_operator`, `skip_test_newlines`) or build AST nodes
solely during parsing (`build_redirections`, `dup_op`). Candidate set to
move: `next_is_redirect`, `build_redirections`, `next_is_test_binary_operator`,
`skip_test_newlines`, `dup_op`. The distinction from the command.rs-resident
predicates below is caller set, not signature alone: these are used **nowhere
but the parser**, whereas the predicates that stay are pure AST/`Word` queries
the engine also calls. Task 5 verifies each candidate's callers before moving
— any helper the engine (executor/expand/builtins) also consumes stays in
`command.rs` with the AST.

Gains (moved in from `lexer.rs`): `brace_expand_parts` and its helper
`word_contains_unquoted_brace` — pure `Vec<WordPart> → Vec<Vec<WordPart>>`
brace-expansion logic that, once the oracle's `emit_word_with_braces` is
deleted, is called only from `parser.rs` (command-word assembly, array
literals). It operates on produced WordParts, never the char cursor — a
parser/expansion concern.

### `lexer.rs` — token production only

Keeps: the atom scanners (`scan_step_command_atoms` and the mode scanners for
`${…}`, `$(…)`, backtick, `$((…))`, `$[…]`, `[[ ]]` regex, extglob, array
literals, heredoc bodies, …) and the shared leaf helpers they call
(`scan_ansi_c_quoted`, `scan_raw_ansi_c_body`, `decode_ansi_c_escapes`,
`emit_dquote_dollar_atom`, `emit_unquoted_dollar_atom`, `new_live`,
`new_live_atoms`, the atom heredoc queue, …).

Removes: the six forward-scanners, the batch `tokenize`/`from_tokens` path,
`Mode::Command`'s non-atom branch, `emit_word_with_braces`, and any other
function that goes dead once nothing calls the oracle.

## The Deletion Mechanism — Compiler-Guided

The safe way to distinguish oracle-only code from shared code is to let the
compiler do it. The sequence:

1. Make **nothing** call the oracle entry points (`command::parse`,
   `command::parse_one_unit`, `tokenize`, `tokenize_with_opts`,
   `from_tokens`). This means porting `continuation.rs`, repointing the ~6
   test helpers, and smoke-converting the differential harness first.
2. Remove those entry points.
3. `cargo build -p huck-syntax` and `-p huck-engine`. The `dead_code` lint
   flags every function now only-ever-reachable from the deleted oracle.
4. Delete the flagged functions. Rebuild. Repeat until 0 warnings.

What survives step 4 is, by construction, exactly the shared code (AST +
atom scanners + shared leaves). The compiler is the oracle-vs-shared
classifier — no manual line-by-line judgment about what is safe to delete.

## Task Design

Each task ends green on all three verification layers (huck-syntax,
huck-engine, bash-diff sweep) so the deletion never leaves a broken tree.

### Task 1 — Port `continuation.rs` off the oracle

The **only non-mechanical production change.** `continuation::classify(buffer,
extglob)` currently:

- calls `lexer::tokenize_with_opts(buffer, {extglob, ..})`, catching
  `LexError::UnterminatedHeredoc` → `Heredoc` and `is_unterminated_lex(e)` →
  `OpenQuote` *before* parsing;
- runs `command::parse` once to detect `UnterminatedDoubleBracket` (so a
  buffer ending in `&&`/`||` is classified `DoubleBracket`, not `Operator`);
- checks `tokens.last()` for `Op(Pipe|And|Or)` → `Operator`;
- runs `command::parse` again for `UnterminatedSubshell` → `Subshell`,
  `UnterminatedIf|Loop|Case|Brace|Function` → `Compound`, `Ok` → `Complete`,
  else `Error`.

Port to a **single** atom parse. Lex errors now arrive through the parser as
`ParseError::Lex(Box<LexError>)`, so one `parse_sequence` yields both the lex
and parse signals:

```rust
pub fn classify(buffer: &str, extglob: bool) -> Completeness {
    if ends_with_continuation_backslash(buffer) {
        return Completeness::Incomplete(ContinuationReason::Backslash);
    }
    let opts = lexer::LexerOptions { extglob, ..Default::default() };
    let empty = std::collections::HashMap::new();
    let mut lx = lexer::Lexer::new_live_atoms(buffer, &empty, opts);
    let parsed = parser::parse_sequence(&mut lx);

    // DoubleBracket must win over a trailing connector (a `[[ … &&` at EOL).
    if let Err(ParseError::UnterminatedDoubleBracket) = parsed {
        return Completeness::Incomplete(ContinuationReason::DoubleBracket);
    }
    if buffer_ends_with_connector(buffer, extglob) {
        return Completeness::Incomplete(ContinuationReason::Operator);
    }
    match parsed {
        Ok(_) => Completeness::Complete,
        Err(ParseError::Lex(e)) if matches!(*e, LexError::UnterminatedHeredoc) => {
            Completeness::Incomplete(ContinuationReason::Heredoc)
        }
        Err(ParseError::Lex(e)) if is_unterminated_lex(&e) => {
            Completeness::Incomplete(ContinuationReason::OpenQuote)
        }
        Err(ParseError::UnterminatedSubshell) => {
            Completeness::Incomplete(ContinuationReason::Subshell)
        }
        Err(ParseError::UnterminatedIf
            | ParseError::UnterminatedLoop
            | ParseError::UnterminatedCase
            | ParseError::UnterminatedBrace
            | ParseError::UnterminatedFunction) => {
            Completeness::Incomplete(ContinuationReason::Compound)
        }
        Err(_) => Completeness::Error,
    }
}
```

`buffer_ends_with_connector` replaces the `tokens.last()` check. Since
`tokenize` is being deleted, it drives `new_live_atoms` over the buffer and
returns whether the last significant atom is `Op(Pipe|And|Or)`. It lives in
`continuation.rs` (a small token-production query) and must reproduce the
current semantics exactly.

**Verification (this is the risk task):** every `ContinuationReason` variant
must map identically. The gate is the existing `continuation` unit tests plus
a guarded interactive multiline-REPL spot-check (`if`/`while`/`for`/`case`,
`{ }`, `(`, `[[`, open quote, heredoc, trailing `&&`/`|`, line-continuation
`\`). If any signal does not map cleanly — in particular an unterminated
heredoc or open quote surfacing as the right `ParseError::Lex` variant — that
is where implementation care goes. If a variant genuinely cannot be produced
by the atom path, that is a blocker to surface, not to paper over.

Passing an **empty alias map** is correct: completeness is structural, and
the current oracle path (`from_tokens`, no aliases) does not alias-expand
either.

### Task 2 — Repoint the test-only `command::parse` helpers

All non-`continuation` `command::parse` call-sites are inside test functions:
`builtins.rs` (`type_default_function`, `type_prints_function_body`,
`define_fn`), `expand.rs` (`first_arg_word`), `executor.rs`
(`render_test_leaf_forms`, `run_exec_single_function_call_inline_assignment…`).
Repoint each to the atom parser
(`parser::parse_sequence`/`parse_one_unit` over `new_live_atoms`), matching
the production pattern in `shell.rs:384`. Mechanical; each file's test suite
is the gate.

### Task 3 — Smoke-convert the differential harness

The differential harness is ~882 assertions (`diff_cmd` × 830, `diff_eg` × 22,
`diff_al` × 15, `diff_unit` × 15) across ~150 of parser.rs's 182 test fns —
the atom parser's entire fast AST regression corpus. Do **not** delete the
call-sites. Instead change only the **helper bodies** so they stop calling the
oracle, keeping every curated input alive as a parse-level guard:

- `diff_cmd(s)`: was `assert_eq!(new_seq(s).unwrap(), old_seq(s).unwrap())` →
  `assert!(new_seq(s).is_ok(), "expected Ok for {s:?}, got {:?}", new_seq(s))`
- `diff_err(s)`: was `assert_eq!(new_seq(s), old_seq(s))` →
  `assert!(new_seq(s).is_err(), "expected Err for {s:?}, got {:?}", new_seq(s))`
- `diff_al(s, pairs)`: → assert `new_seq_al(s, pairs).is_ok()`
- `diff_unit(s)`: → assert `new_unit(s).iter().all(|r| r.is_ok())`
- `diff_eg(s)`: → assert `new_eg(s).is_ok()`
- `diff_unsupported(s)`: unchanged (already checks only `new_seq`).

Then delete the now-unused oracle helpers `old_seq` / `old_seq_al` /
`old_unit` / `old_eg` (the `command::parse` callers). `new_seq` / `new_seq_al`
/ `new_unit` / `new_eg` stay. Every existing call passes unchanged: every
`diff_cmd` input already has `new_seq(s) == Ok` (it currently `.unwrap()`s it)
and every `diff_err` input already errors, so no test flips. This trades
AST-*shape* verification (now carried by bash-diff + huck-engine) for
panic/`Ok`↔`Err`-regression guards, while removing the oracle dependency —
the actual goal.

### Task 4 — Delete the oracle (compiler-guided)

Remove `parse` / `parse_one_unit` / `parse_cursor` and the recursive-descent
oracle functions from `command.rs`; remove `tokenize` / `tokenize_with_opts` /
`from_tokens` + the `Vec<Token>` replay + `Mode::Command`'s non-atom branch +
the six forward-scanners from `lexer.rs`. Rebuild both crates; follow the
`dead_code` warnings to delete transitively-dead code (`emit_word_with_braces`
and likely `collect_heredoc_bodies` at lexer.rs:5549 — the atom path uses
`collect_heredoc_bodies_after_newline` in parser.rs). Iterate to 0 warnings.

### Task 5 — Module tidy

First confirm caller sets: `grep` each candidate helper's uses across
huck-syntax **and** huck-engine. Move to `parser.rs` only those used nowhere
but the parser — the candidate set is `next_is_redirect`, `build_redirections`,
`next_is_test_binary_operator`, `skip_test_newlines`, `dup_op`; any that the
engine also calls stays in `command.rs`. Drop the now-internal
`crate::command::` prefix at the moved helpers' call-sites. Move
`brace_expand_parts` + `word_contains_unquoted_brace` from `lexer.rs` to
`parser.rs` (verified parser-only after Task 4). Confirm the end-state:
`command.rs` has no parse-time builder/consumer code, `lexer.rs` has no
word-assembly/expansion code, `parser.rs` owns both. May be committed as two
sub-steps (command→parser, then lexer→parser) for reviewability.

### Task 6 — Focused, oracle-independent lexer + parser tests

Backfill the AST-shape coverage the smoke-convert drops, in a durable form
that depends on nothing being deleted. Add two focused test modules with
**explicit expected values** (no oracle, no snapshot magic):

- **Lexer token-stream tests** — assert the exact atom `Vec` (via
  `new_live_atoms` driven to completion, or the existing atom-collection test
  helper) for representative inputs in each lexer mode: plain command words,
  single/double/ANSI-C quotes, `${…}` param expansion (+ operators), `$(…)`,
  backtick, `$((…))`, `$[…]`, `[[ ]]` regex, extglob, array literals, heredoc
  bodies (literal + expanding), process substitution, brace expansion,
  redirect operators. A handful of cases per mode, each asserting the precise
  atom sequence including boundaries.
- **Parser AST tests** — assert the exact `Sequence`/`Command` AST for
  representative inputs across every grammar family: simple command, pipeline
  (+ negation), redirects (all ops + fd-dup + heredoc + here-string), and-or
  lists, subshell, brace group, `if`/`while`/`until`/`for`/`select`/`case`,
  C-style `for ((…))`, arith command `(( ))`, `[[ ]]` (+ `=~`), function
  defs (both forms), coproc, assignments + array literals, and the word-part
  structures (param expansion, cmdsub, arith, backtick nesting). Construct the
  expected AST by reasoning about the grammar, cross-checked against the
  production parser's output.

**Bounded scope:** this is a curated *representative* set (target on the order
of dozens per module), not a re-encoding of the 882-input smoke corpus. The
smoke corpus + bash-diff sweep carry breadth; these tests carry readable,
oracle-free shape verification for the constructs most worth locking down.
Written last, against the final module structure. They must pass on the
delivered code (the parser/lexer behavior is unchanged by Tasks 4–5, which
only move code).

## Testing Strategy

Three-layer verification after **every** task, single-threaded and guarded on
this box (1 core / 1.9 GB):

- `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1`
- `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`
- Build the binary with `cargo build -p huck`, then run the
  `tests/scripts/*_diff_check.sh` bash-diff sweep guarded per harness
  (`ulimit -v 1500000` + `timeout`). Expected: the same 1688 pass / 1 fail
  (`funcnest`, pre-existing intentional L-63) baseline as end-of-v264.

The bash-diff sweep is now the **primary** correctness oracle (the
differential no longer verifies AST shape after the smoke-convert). This is
safe precisely because the atom path is already validated against bash — a
stronger ground truth than the oracle ever was. The Task 6 focused tests add
back explicit, oracle-independent shape verification for the constructs most
worth locking down.

`cargo build` must end at **0 warnings** (the deletion's completion signal).
Trust `cargo`, not rust-analyzer (phantom `dead_code` diagnostics have
recurred across this arc).

## Risks

- **Continuation completeness (Task 1)** — the only real risk. Mitigated by
  the existing continuation tests + the interactive spot-check, and by the
  fact that the atom parser already produces every `Unterminated*` variant
  the classifier reads. Surface any unmappable signal as a blocker.
- **Over-deletion** — mitigated by the compiler-guided mechanism: only
  `dead_code`-flagged functions are removed, so nothing the atom path still
  needs can be deleted without a build failure.
- **Hidden shared helper in a "6-scanner" function** — if a forward-scanner
  shares a leaf the atom path needs, deleting the scanner leaves the leaf
  live (still referenced), so it survives. Only the scanner shell dies.

## Follow-Ons

None expected. v265 is the finale's terminal milestone: one parser, one
lexing discipline, no oracle. Any divergence the bash sweep surfaces during
the port is either fixed in-iteration or recorded as a new `[deferred]` entry
in `docs/bash-divergences.md`.
