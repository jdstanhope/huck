# v174: command substitution inside array literals (backtick + `$()`-with-quotes) — Design

**Status:** approved 2026-06-17
**Iteration:** v174
**Origin:** Found by the parse-compat sweep (`tools/parse_sweep.sh`) over 4191 real
scripts on the box: `huck -n` rejected ~13 scripts (the whole pyenv/rbenv family)
with "unterminated command substitution", all on the line shape
`IFS=$'\n' scripts=(\`pyenv-hooks …\`)` — a backtick command substitution used as
an array-literal element.

## Problem

`scan_array_element_word` (`src/lexer.rs:2764`) scans a single array-literal
element character-by-character into a raw `String`, then re-tokenizes it. It
**breaks the element on whitespace**, with bespoke "consume the whole construct"
handling only for `$(…)` and `${…}`:

- **Reported bug (backtick):** there is NO case for a backtick. So in
  `` a=(`echo hi`) `` the opening backtick falls into the default arm (pushed as a
  plain char), and the scanner then hits the **space inside the backticks**,
  treats it as an element separator, and breaks — collecting only `` `echo ``. 
  Re-tokenizing `` `echo `` yields `LexError::UnterminatedSubstitution` →
  "syntax error: unterminated command substitution". bash accepts
  `` a=(`echo hi`) ``.
- **Latent sibling bug (`$()` with quotes):** the `$(…)` handling
  (`lexer.rs:2819`) is a **hand-rolled paren-counter** that only counts `(`/`)`
  and ignores quotes, so `a=($(echo ')'))` miscounts the `)` inside the single
  quotes and fails ("unterminated quote"). bash accepts it.

Meanwhile the codebase already has the correct, shared boundary scanners,
introduced precisely to be the single source of truth (v167 `$()` kernel, v168
backtick kernel):

- `scan_cmdsub_body(chars, out, unterminated)` — appends a `$(…)` body to `out`
  up to the matching `)` (NOT including it), correctly skipping over single- and
  double-quoted spans and nested parens.
- `scan_backtick_body(chars, out, unterminated)` and its wrapper
  `consume_backtick_verbatim(chars, out)` — appends a backtick body up to the
  closing backtick (handling `\` escapes) and re-adds the closing backtick.

The array-element scanner simply does not route through them.

## Goal

Make command substitutions inside array literals parse like bash, by routing the
array-element scanner's command-substitution handling through the existing
kernels — fixing both the reported backtick bug and the latent `$()`-with-quotes
bug in one consolidating change.

## Design

In `scan_array_element_word` (`src/lexer.rs`), within the per-character `match`:

1. **Replace** the hand-rolled `$(…)` block (the `Some('(') => { … manual paren
   depth loop … }` arm under the `'$'` case, ~lines 2823–2843) with a call to the
   kernel:
   ```rust
   Some('(') => {
       buf.push('(');
       chars.next();
       scan_cmdsub_body(chars, &mut buf, LexError::UnterminatedSubstitution)?;
       buf.push(')');
   }
   ```
   (`scan_cmdsub_body` consumes through the matching `)` without pushing it, so
   the caller pushes the `)`.) The `${…}` brace arm is left unchanged.

2. **Add** a backtick arm to the outer `match c`, mirroring the `$(…)` shape:
   ```rust
   '`' => {
       buf.push('`');
       chars.next();
       consume_backtick_verbatim(chars, &mut buf)?;
   }
   ```
   `consume_backtick_verbatim` scans the whole backtick body (including internal
   whitespace) and appends the closing backtick, so the element no longer breaks
   at the space.

The collected raw text (e.g. `` `echo hi` `` or `$(echo ')')`) is then
re-tokenized by the existing `tokenize_no_brace(&buf, opts)` call, which parses
the command substitution correctly — exactly as it already does for the working
`a=($(echo hi))` case.

### Why this shape

- Reuses the v167/v168 "single source of truth" scanners instead of growing a
  third hand-rolled copy — eliminates drift and the quote-miscount class of bug.
- Minimal: a ~10-line change in one function; no new types, no signature changes,
  no change to how elements are re-tokenized or to brace expansion.

### Behavior

- `` a=(`echo hi`) ``, `` a=(`a` `b`) ``, `a=($(echo ')'))`, `a=($(f) \`g\`)`,
  `IFS=$'\n' a=(\`cmd\`)` → parse and evaluate identically to bash.
- Existing cases unchanged: `a=($(echo hi))`, `a=(x{1,2}$v)`, quoted/spaced
  elements, subscripted `a[i]=…` (a different path), etc.

## Verification

- **New bash-diff harness** `tests/scripts/array_cmdsub_diff_check.sh` (the gold
  standard): byte-identical bash↔huck on executing fragments — single backtick
  element, two backtick elements, `$()`-with-`)`-in-quotes, mixed `$()`+backtick
  elements, the `IFS=$'\n' a=(\`…\`)` pyenv shape, a nested
  `a=($(echo \`echo hi\`))` case, and a regression case (`a=($(echo hi))`).
  Asserts element values (`echo "${a[@]}"`, `${#a[@]}`).
- **Parse-sweep payoff:** re-run `tools/parse_sweep.sh tools/scripts.tsv` and
  confirm the "unterminated command substitution" `HUCK_GAP` cluster drops from
  ~13 to ~0 and the overall `HUCK_GAP` count falls accordingly, with no new
  regressions in other buckets.
- Full `cargo test` (0 failures), all existing `tests/scripts/*_diff_check.sh`
  harnesses green (especially the array-literal ones), clippy clean.

## Scope boundary

In scope: routing the `$(…)` and backtick command-substitution handling in
`scan_array_element_word` through `scan_cmdsub_body` / `consume_backtick_verbatim`;
the new harness; the parse-sweep confirmation. **Not** in scope: the `${…}` brace
handling in the same function (its own construct, not surfaced by the sweep — a
possible follow-on if a `${…}`-with-quotes-in-array case ever appears); subscripted
array-element assignment paths; any other parse-sweep `HUCK_GAP` cluster (function
names, arithmetic termination, etc. — separate iterations). No new
`bash-divergences.md` entry (this was never a tracked divergence; the sweep found
it). Record the iteration in `project_huck_iterations.md` + `MEMORY.md`.
