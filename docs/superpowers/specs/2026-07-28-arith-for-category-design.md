# v340 — flip the `arith-for` bash-suite category

**Issues:** [#64 — arith-for empty-section reconstruction](https://github.com/jdstanhope/huck/issues/64)
and [#313 — arith-for malformed-header error messages](https://github.com/jdstanhope/huck/issues/313).

**Goal:** flip the bash 5.2.21 test-suite `arith-for` category from FAIL to a
byte-identical (0-diff) PASS, raising the runner's PASS count 27 → 28.

## Background

The `arith-for` category (`run-arith-for` → `arith-for.tests`) FAILs against
huck with a 30-line diff. Every diff line is explained by exactly **two**
divergences, both in the arith-for parse/reconstruct path:

| Diff lines | Divergence |
|---|---|
| 17, 21, 28, 34, 43 | Root 1 — empty `for (( ))` section reconstructs empty, not `1` |
| 67–68, 69–71 | Root 2 — malformed-header error messages differ + missing echo line |

The category needs BOTH fixed; each contributes lines to the diff.

## Root 1 — empty-section reconstruction (#64)

### Symptom
`arith-for.tests` defines a function `fx` whose body has C-style loops with
empty header sections (`for (( ; i < 3; i++ ))`, `for ((;;))`, …), then calls
`type fx`. bash's reconstruction normalizes every empty section to the literal
`1`:

| huck | bash |
|---|---|
| `for ((; i < 3; i++ ))` | `for ((1; i < 3; i++ ))` |
| `for ((i=0; ; i++ ))` | `for ((i=0; 1; i++ ))` |
| `for ((i=0; i<3; ))` | `for ((i=0; i<3; 1))` |
| `for ((; ; ))` | `for ((1; 1; 1))` |

### Cause
`crates/huck-syntax/src/generate.rs::arith_for_to_source` renders each section
via a `sec` closure that maps a `None`/empty section to the empty string:

```rust
let sec =
    |w: &Option<crate::lexer::Word>| w.as_ref().map(arith_body_to_source).unwrap_or_default();
```

### Fix
Reconstruction-only: when a section reconstructs to an empty (whitespace-only)
string, emit `"1"` instead:

```rust
let sec = |w: &Option<crate::lexer::Word>| {
    let s = w.as_ref().map(arith_body_to_source).unwrap_or_default();
    if s.trim().is_empty() { "1".to_string() } else { s }
};
```

**Scope decision — reconstruction-only, NOT parse-time AST normalization.** The
`ArithForClause` sections stay `Option<Word>` (empty = `None`); execution is
unchanged (`1` is an inert truthy no-op equivalent to the current empty-section
handling — empty cond is already "always true", empty init/step are already
skipped). This keeps the change to one function and off the hot execution path.
Tradeoff: no category currently tests empty-section *xtrace*, and `set-x`
(v339) passes with non-empty sections, so this is safe today; if a future
category checks empty-section xtrace, it would need a follow-up. The parser test
that encodes "all sections empty → None" reconstruction
(`crates/huck-syntax/src/parser/tests.rs`, the `for ((;;))` round-trip cases)
must have its expected reconstruction string updated to the `1` form.

## Root 2 — malformed-header error messages (#313)

### Symptom
`arith-for.tests` runs two `${THIS_SH} -c` cases with malformed headers and
captures stderr (piped through `sed 's|^.*/||'` to strip the program path):

**Case A — 2 sections** (`for (( i=0; "i < 3" ))`):
```
huck: -c: line 1: syntax error: 'for ((...))' header: expected 3 sections separated by `;`, got 2
```
```
bash: -c: line 1: syntax error: arithmetic expression required
bash: -c: line 1: syntax error: `(( i=0; "i < 3" ))'
```

**Case B — 4 sections** (`for (( i=0; i < 3; i++; 7 ))`):
```
huck: -c: line 1: syntax error: 'for ((...))' header: expected 3 sections separated by `;`, got 4
```
```
bash: -c: line 1: syntax error: `;' unexpected
bash: -c: line 1: syntax error: `(( i=0; i < 3; i++; 7 ))'
```

bash emits **two** lines per error: a specific message, then a verbatim echo of
`(( <raw header content> ))`. The echo is byte-exact — verified that
`for ((   i=0   ;   "x < 3"   ))` echoes `` `((   i=0   ;   "x < 3"   ))' `` with
all interior spacing and quote characters preserved (so it is a **raw source
slice**, not a Word reconstruction).

### Cause
`crates/huck-syntax/src/parser.rs::parse_arith_for_clause` splits the header on
`;` into `sections: Vec<Word>` and, when `sections.len() != 3`, returns a single
`ParseError::ArithForHeader(String)` with huck's own wording and no echo line.

### Fix
Three parts:

1. **Capture the raw header content.** In `parse_arith_for_clause`, after
   consuming the two opening `(` tokens, record the byte offset of the start of
   the header content (immediately after `((`); when the body parser reaches the
   closing `))`, record the end offset. Slice the verbatim content with the
   existing `Lexer::source_span(start_off, close_off) -> &str`. The captured
   string is the raw content between `((` and `))`, including interior
   whitespace and quote characters (e.g. ` i=0; "i < 3" `).

2. **Message by count direction.** When `sections.len() != 3`:
   - fewer than 3 → `arithmetic expression required`
   - more than 3 → `` `;' unexpected ``

3. **Two-line render.** bash emits two full syntax-error lines, each with the
   complete `<prog>: -c: line N: ` prefix:
   - line 1: `syntax error: <message>`
   - line 2: ``syntax error: `(( <raw content> ))' `` (the raw content wrapped in
     `((` … `))`, in bash's `` `…' `` framing).

   huck's diagnostic path (`crates/huck-engine/src/error_emit.rs`) already emits
   this shape: `emit_syntax_error(shell, line, body)` writes
   `{prefix}{body}\n` where `prefix` = `<prog>: -c: line N: ` and the `body`
   carries the literal `syntax error: …` text (verified: Shape-1/2/3 arms pass
   `format_args!("syntax error …")` as the body). So the arith-for error renders
   as TWO `emit_syntax_error` calls (one per line), NOT via the existing
   single-line `echo` parameter of `emit_syntax_error_ex` (whose echo line lacks
   the `syntax error:` text).

**Mechanism.** Replace the string-carrying `ParseError::ArithForHeader(String)`
(`crates/huck-syntax/src/command.rs:828`) with a structured variant carrying the
count direction and the raw content (e.g.
`ArithForHeader { too_many: bool, raw: String }`). Wire it at three sites:
- `parser.rs::parse_arith_for_clause` — construct it with the captured raw span
  and `too_many = sections.len() > 3`.
- `error_emit.rs::render_diag_inner` — add a match arm that emits the two
  `emit_syntax_error` lines (message chosen by `too_many`; second line the raw
  echo). This arm precedes the generic fallback.
- `errors.rs::parse_error_message_impl` — update the `ArithForHeader` arm to a
  reasonable single-string form for non-`render_syntax_diag` contexts (`huck -n`,
  Display), e.g. the direction message alone; the two-line bash-exact output is
  produced only by the `render_diag_inner` arm.

## Verification

- Extend `tests/scripts/arith_for_diff_check.sh` (or add one if absent) with:
  empty-section `declare -f`/`type` reconstruction (all four empty positions),
  and both malformed-header `-c` error cases (2-section and 4-section, plus an
  irregular-spacing variant to lock the byte-exact echo).
- Official category runner: `HUCK_BASH_TEST_CATEGORY=arith-for bash
  tests/bash-test-suite/runner.sh` must report `arith-for` 0-diff PASS.
- Full `tests/scripts/run_diff_checks.sh` sweep green — no regression, with
  attention to `declare_f`, `set_x`/`setx_trace_fidelity` (v339 arith-for
  reconstruction), and any syntax-error-diagnostic harness (`parser`,
  `syntax_error_diag`).
- Per-crate `cargo test` for `huck-syntax` (parser: section capture + error;
  generate: empty→1) and `huck-engine`, plus the `-p huck` integration bins
  `arith_for_integration`, `declare_f_integration`, and any syntax-error/driver
  bin, run single-threaded under a `ulimit -v` guard.
- Confirm only `arith-for` flipped (full runner PASS 27 → 28) with no
  regression, via a targeted re-run of neighbor categories (`parser`, `set-x`,
  `func`, `posix2`, `cprint`).

## Out of scope / follow-ups

- Parse-time AST normalization of empty sections to `1` (this spec does
  reconstruction-only) — revisit only if a future category needs empty-section
  xtrace to read `(( 1 ))`.
- Any arith-for behavioral divergence beyond reconstruction + these two
  malformed-header shapes is not in scope; open a new `divergence` issue if one
  surfaces during implementation.

## Summary of touched files

- `crates/huck-syntax/src/generate.rs` — `arith_for_to_source` `sec` closure
  (empty → `1`).
- `crates/huck-syntax/src/parser.rs` — `parse_arith_for_clause` raw-span capture
  + count-direction error construction.
- `crates/huck-syntax/src/command.rs` — `ParseError::ArithForHeader` variant
  shape (`{ too_many: bool, raw: String }`).
- `crates/huck-syntax/src/errors.rs` — `parse_error_message_impl` `ArithForHeader`
  arm (single-string form for `-n`/Display).
- `crates/huck-engine/src/error_emit.rs` — `render_diag_inner` arm emitting the
  two-line arith-for header error.
- `crates/huck-syntax/src/parser/tests.rs` — update empty-section reconstruction
  expectations (the `for ((;;))` round-trip cases → `1` form) AND the
  section-count error tests (~2319/2365/2630) that assert the old
  `ArithForHeader(String)` "got N" message/shape.
- `tests/scripts/arith_for_diff_check.sh` — harness (extend or add).
- `docs/bash-test-suite-baseline.md` — baseline update (PASS 27 → 28).
- Memory: `project_huck_iterations.md` + `MEMORY.md`.
