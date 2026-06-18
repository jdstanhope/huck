# v186: `case` statements inside `$(…)` (case-aware `scan_cmdsub_body`) — Design

**Status:** approved 2026-06-18
**Iteration:** v186
**Origin:** The parse sweep's "unterminated 'case' in command substitution"
cluster (`mlxsw_lib.sh` ×2): `MLXSW_SPECTRUM_REV=$(case $X in pat) … ;; esac)`.
`scan_cmdsub_body` (the `$()` close-finder) counts parens but has no notion of
`case`, so a bare case-pattern terminator `)` (e.g. `mlxsw_spectrum)`) at paren
depth 0 is mistaken for the closing `)` of the substitution.

**Approach decided after a spike.** A spike (branch `spike-parse-driven-cmdsub`,
not merged) proved that fully parse-driven `$()` is correct but has a performance
wall on huck's batch tokenizer (re-tokenizing the rest at every `$()` → O(N²),
106 sweep timeouts). So v186 takes the bounded, bash-faithful path: give
`scan_cmdsub_body` the case-statement state bash's own text matched-pair reader
carries (`LEX_INCASE`). See `project_huck_parse_driven_spike.md`.

## bash contract (verified)

A `case` introduces case-mode ONLY at a COMMAND position; `esac` closes it. While
inside a `case`, a paren-depth-0 `)` is a pattern terminator, not the cmdsub close.

| fragment (inside `"$( … )"`) | bash |
|---|---|
| `case $y in a) echo hit;; *) echo no;; esac` | `hit` |
| `echo case` (case as an ARGUMENT) | `case` |
| `if true; then case $y in a) echo T;; esac; fi` | `T` |
| `case $y in a) case $y in a) echo deep;; esac;; esac` (nested) | `deep` |
| `case $y in a\|b) echo alt;; esac` (alternation) | `alt` |
| `case $y in a) echo A;;& *) echo B;; esac` (`;;&`) | `A`⏎`B` |
| `case $y in (a) echo p;; esac` (parenthesized pattern — already works) | `p` |
| `echo x \| grep case \|\| echo none` (case as an arg) | `none` |

The key subtlety: `case` is a keyword after a command-start position (start, `;`,
`&`, `|`, `(`, `{`, newline, a case-pattern `)`, or a command-introducer keyword
`if`/`then`/`elif`/`else`/`while`/`until`/`do`) but a plain word elsewhere
(`echo case`, `grep case`). `(a)` parenthesized patterns already work (the `(`
balances the `)`); only BARE pattern `)` needs the case state.

## Design

Add a small case-statement state machine to `scan_cmdsub_body` (`src/lexer.rs`).
New state alongside the existing `depth` (paren) and `at_boundary` (comment) :

- `case_depth: usize` — number of open `case` statements (nesting).
- `cmd_pos: bool` — true when the next word begins at a COMMAND position (so a
  bare `case`/`esac` there is a keyword). Initialised `true`.
- `word: String` + `word_cmd_pos: bool` — accumulate the current BARE word (ASCII
  alphanumeric/`_` only) and the `cmd_pos` snapshot at its start, to test it
  against `case`/`esac`/the introducer keywords when the word ends.

### The close condition changes

```rust
            Some(')') if depth == 0 && case_depth == 0 => return Ok(()),  // cmdsub close
            Some(')') if depth == 0 => {                                  // case-pattern terminator
                finalize_word(/* … */);     // the preceding pattern is NOT a keyword
                out.push(')');
                cmd_pos = true;             // a clause body (commands) follows
            }
            Some(')') => { depth -= 1; out.push(')'); cmd_pos = true; }   // nested paren close
```

### Word + command-position tracking (in the catch-all and the structural arms)

`finalize_word`: when the current `word` ends, if `word_cmd_pos` is true and the
word is exactly `case` → `case_depth += 1`; if exactly `esac` →
`case_depth = case_depth.saturating_sub(1)`. Then clear `word`/`word_cmd_pos`.

Per-character `cmd_pos` transitions (after finalizing any pending word):
- **identifier char** (`[A-Za-z0-9_]`): if `word` is empty, snapshot
  `word_cmd_pos = cmd_pos`; append the char to `word`. (Does not change `cmd_pos`.)
- **whitespace** (space/tab): finalize word; `cmd_pos =` *was the just-finished
  word a command-introducer keyword* (`if`/`then`/`elif`/`else`/`while`/`until`/
  `do`)? `true` : `false`. (So `then case` → keyword; `echo case` → not.)
- **`;` / `&` / `|` / newline / `{`**: finalize word; `cmd_pos = true`.
- **`(`** (the existing `(` arm): finalize word; `depth += 1`; `cmd_pos = true`.
- **`)`** (the three arms above): finalize word; `cmd_pos = true` (a command can
  follow a pattern `)` / a closed subshell).
- **`'` / `"` / `` ` `` / `\` / `$`** (start of a quoted/expansion span — the
  existing quote/escape arms, and `$` falls through the catch-all): finalize word;
  `cmd_pos = false` (we are inside a word, not at a command start). The quote/
  escape spans are consumed as today (they cannot contain a bare keyword).
- **`#` word-start comment** (existing arm): unchanged; the trailing newline (next
  char) sets `cmd_pos = true`.
- **other chars** (`<`, `>`, `=`, glob `*`/`?`, etc.): finalize word;
  `cmd_pos = false` (mid-command). A glob pattern like `spectrum*` is thus a
  non-keyword word, correct.

### Why this matches bash for the contract

- `$(case $y in a) … esac)`: `case` at start (`cmd_pos`) → depth 1; the bare `a)`
  / `*)` terminators at paren-depth 0 are skipped (case_depth > 0); `esac` (after
  a `;;`/newline → `cmd_pos`) → depth 0; the final `)` closes. ✓
- `$(echo case)`: `echo` (cmd_pos, not a keyword) → after the space `cmd_pos =
  false` (echo isn't an introducer); `case` is a non-keyword word; `)` closes. ✓
- `$(if true; then case …)`: `if`(introducer)→space→`cmd_pos`; `true`; `;`→cmd_pos;
  `then`(introducer)→space→`cmd_pos`; `case`→keyword. ✓
- nested / alternation / `;;&` / `(a)` / arg-after-pipe: all follow from the rules.

### Known limitation (documented, not fixed)

A pattern that is LITERALLY `case` or `esac` appearing after `;;` (where `cmd_pos`
is true) — e.g. `case x in a) :;; case) :;; esac` — would be mis-counted. This is
pathological (a bare `case`/`esac` as a case pattern is itself a bash gotcha,
normally written `(case)`), matches bash's own LEX_INCASE imperfections, and does
not occur in real code. Note it in the function doc comment.

### Scope of the change

`scan_cmdsub_body` is the single `$()` close-finder used by
`scan_paren_substitution` (parsed cmdsubs), `consume_paren_cmdsub_verbatim`
(verbatim cmdsub skip in array elements / `${…}` operands), and
`split_modifier_operand`. The case-awareness benefits all of them uniformly. The
verbatim users only need the correct close position (which the new logic yields);
they don't re-interpret the body.

## Verification

- **New bash-diff harness** `tests/scripts/case_in_cmdsub_diff_check.sh`
  (executing, byte-identical stdout+exit): the 8 contract rows above, the
  `mlxsw_lib` real shape (`$(case $C in spectrum) echo 1;; spectrum*) echo
  ${C#spectrum};; esac)`), a `case` whose clause body contains a nested `$(…)`,
  and a control where the cmdsub has no case (`$(echo a; echo b)`).
- **Lexer unit tests** (`src/lexer.rs` `mod tests`, near the `scan_cmdsub_body_*`
  tests): `scan_cmdsub_body` over `case $y in a) echo hit;; esac)` returns
  `Ok("case $y in a) echo hit;; esac")` and stops at the FINAL `)`; over
  `echo case)` returns `Ok("echo case")` (case-as-arg, stops at the first `)`);
  over `case $y in a) case $y in a) :;; esac;; esac)` (nested) stops at the final
  `)`. The 4 existing `scan_cmdsub_body_*` tests (basic / nested-arith / quoted-
  paren / unterminated) and the v183 comment tests stay green.
- **Parse-sweep payoff:** re-run `tools/parse_sweep.sh`; confirm both `mlxsw_lib.sh`
  copies parse (`huck -n` rc 0). Report `HUCK_GAP` from the 8 baseline;
  `HUCK_LENIENT`/`HUCK_CRASH`/`HUCK_TIMEOUT` stay 0 (esp. NO timeouts — unlike the
  spike). No `case` rows remain.
- **Full `cargo test`** (0 failures). UP-FRONT grep `tests/` + `src/` for
  `scan_cmdsub_body` / cmdsub tests; update only genuine old-behavior tests (none
  expected — the change only affects `$(case …)` which previously errored).
- All `tests/scripts/*_diff_check.sh` green; clippy clean.

## Docs / close-out

No tracked `M-*`/`L-*` divergence covers this (sweep-found). No
`bash-divergences.md` change. Record the iteration in
`project_huck_iterations.md` + `MEMORY.md`; update the backlog note.

## Scope boundary

In scope: the case-statement state machine in `scan_cmdsub_body`, the new harness
+ lexer unit tests. **Not** in scope: the fully parse-driven `$()` rewrite (spiked,
deferred — needs streaming lex+parse); the `scan_arith_block`/`$((` paths; the
`case` pattern-named-`case`-after-`;;` pathological edge (documented); the other
sweep clusters (`unexpected token after command`, `parameter expansion with empty
name`, `function definition`). No `bash-divergences.md` change.
