# v198: `set -x` xtrace for compound commands — Design

**Status:** approved 2026-06-20
**Iteration:** v198
**Resolves:** L-21(a) ("Finer compound traces not emitted")
**Origin:** The v197 RUN_HUCK_DIFF runtime-sweep triage re-confirmed in the wild
(e.g. `[[ '' =~ ^[0-9.]+$ ]]` in jackc/pgx's `setup_test.bash`) that huck's
`set -x` emits a trace line only for simple commands and assignments — it emits
**nothing** for compound-command headers. bash traces each one.

## bash contract (verified against bash 5.x)

Under `set -x`, bash emits a `$PS4`-prefixed line for these compound forms. The
prefix (`+ ` by default, depth-repeated for nesting) is produced by the existing
`ps4(shell)` machinery and is identical to the simple-command case.

| Form | bash trace | Notes |
|---|---|---|
| `case W in …` | `+ case <W> in` | `W` shown **raw/unexpanded**; once at entry; emitted even if no pattern matches |
| `for V in WORDS; …` | `+ for <V> in <WORDS>` | WORDS **raw**; emitted at the **top of each iteration** (same text each pass); zero iterations → zero lines |
| `select V in WORDS; …` | `+ select <V> in <WORDS>` | WORDS **raw**; once at entry |
| `(( expr ))` | `+ (( <body> ))` | body **verbatim** (incl. internal spacing): `(( ` + body + ` ))` |
| `for ((i;c;s)); …` | `+ (( <i> ))` once, then `+ (( <c> ))` / `+ (( <s> ))` per pass | each arith clause shown verbatim |
| `[[ … ]]` | `+ [[ <leaf> ]]` **per evaluated leaf** | operands **expanded**; short-circuits (untaken `&&`/`||` branch not traced); `!` folds into its leaf line |
| `while`/`until`/`if` | (no own line) | the condition *commands* trace themselves — handled for free once `[[ ]]`/`(( ))` do |

Verified specifics:

- `[[ $a == 1 && $b == 9 ]]` (a=1,b=2) → `+ [[ 1 == 1 ]]` then `+ [[ 2 == 9 ]]`
  (both evaluated). `[[ $a == 1 || $b == 9 ]]` (a=1) → only `+ [[ 1 == 1 ]]`
  (the `||` branch is not evaluated, so not traced).
- `[[ ! -e /nope ]]` → one line `+ [[ ! -e /nope ]]` (the `!` stays with the leaf).
- `[[ ( 1 == 1 ) ]]` → `+ [[ 1 == 1 ]]` (grouping parens are structural, not traced).
- `[[ -z "" ]]` → `+ [[ -z '' ]]` (empty operand shown as `''`).
- `(( v=3, v+1 ))` → `+ ((  v=3, v+1  ))`; `((1+1))` → `+ (( 1+1 ))` (uniform
  `(( ` + verbatim-body + ` ))`).
- `case "$x" in` → `+ case "$x" in`; `for i in $xs` → `+ for i in $xs` (raw).
- Nesting depth uses the existing `ps4` depth machinery (a `[[ ]]` inside a
  traced function is still single-`+` at depth 1, like the current simple-command
  traces).

## Architecture

All changes are confined to `src/executor.rs` plus one reconstructor helper in
`src/expand.rs`. **No** lexer/parser/AST changes and **no** new `Command`
variants.

### Component 1 — `reconstruct_word_source(&Word) -> String` (new, `expand.rs`)

Re-renders a parsed `Word` back to source text (unexpanded), mirroring the
existing `reconstruct_array_literal` precedent. Per `WordPart`:

- `Literal { text, quoted }` → `text` (the parser already stripped the quote
  characters; `quoted` is preserved for the rare cases that need re-quoting, but
  the common rendering is the literal text verbatim — see Residual 3).
- `Var { name, quoted }` → `$name` (or `"$name"` when `quoted`).
- `ParamExpansion { … }` → `${name<modifier/subscript>}` reusing the existing
  param-expansion source rendering where available.
- `CommandSub { sequence, .. }` → `$(<rendered sequence>)`.
- `Arith { body, .. }` → `$((<reconstruct_word_source(body)>))`.
- `AllArgs { joined, .. }` → `$*` (joined) / `$@`.
- `Tilde`, `LastStatus`, `ProcessSub`, `AssignPrefix`, `ArrayLiteral` →
  best-effort source rendering (rare inside the traced headers; never panics).

Used for `case`/`for`/`select` headers and the `(( ))` / C-for arith bodies.
Independently unit-testable.

### Component 2 — `xtrace_compound(shell, body: &str)` (new, thin, `executor.rs`)

```rust
fn xtrace_compound(shell: &mut Shell, body: &str) {
    if shell.shell_options.xtrace {
        let p4 = ps4(shell);
        xtrace_emit(&format!("{p4}{body}"));
    }
}
```

Single emit path → depth/PS4/single-`write(2)` behavior stays identical to the
simple-command traces.

### Component 3 — emit sites (one call per form)

- `run_case` (executor.rs:1646): at entry, before pattern matching —
  `xtrace_compound(shell, &format!("case {} in", reconstruct_word_source(&clause.subject)))`.
- `run_for` (1218): at the **top of each iteration body**, before running the
  body — `for <var> in <space-joined reconstructed words>`. (Placing it inside
  the per-value loop yields bash's per-iteration repetition and the
  zero-iteration → zero-lines behavior for free.)
- `run_select` (1491): once at entry — `select <var> in <words>`.
- `run_arith` (1378): `(( <reconstruct_word_source(body)> ))`.
- `run_arith_for` (1393): emit `(( <init> ))` once before the loop; `(( <cond> ))`
  before each condition test; `(( <step> ))` after each body pass — matching
  bash's init-once / cond+step-per-pass ordering.
- `eval_test_expr` (the `[[ ]]` leaf hook): see Component 4.
- `run_while` / `if`: **no change**.

For-words are space-joined: `clause.words.iter().map(reconstruct_word_source).collect::<Vec<_>>().join(" ")`.

### Component 4 — `[[ ]]` leaf hook in `eval_test_expr` (executor.rs:1753)

A new `render_test_leaf(expr, shell) -> String` builds the `[[ … ]]` body for a
leaf, expanding operands with the **same** expansion the evaluator already
performs (computed once, reused for both the trace and the comparison):

- `Unary { op, operand }` → `<op-str> <expanded-operand>` (empty → `''`).
- `Binary { op, lhs, rhs }` → `<expanded-lhs> <op-str> <expanded-rhs>`
  (each empty → `''`).
- `Regex { lhs, pattern }` → `<expanded-lhs> =~ <reconstruct_word_source(pattern)>`.

Op→string maps (small static `match` tables):

- `TestUnaryOp`: `FileExists`→`-e`, `IsRegFile`→`-f`, `StringNonEmpty`→`-n`,
  `StringEmpty`→`-z`, `VarSet`→`-v`, … (one arm per variant).
- `TestBinaryOp`: `StringEq`→`==`, `StringNe`→`!=`, `StringLt`→`<`,
  `StringGt`→`>`, `IntEq`→`-eq`, `IntGt`→`-gt`, `NewerThan`→`-nt`, … (one arm
  per variant).

Emission threads a `suppress: bool` through `eval_test_expr` (a sibling
`eval_test_expr_traced(expr, shell, suppress)`, with `eval_test_expr` calling it
with `suppress=false`):

- `Unary`/`Binary`/`Regex`: if `!suppress && shell.shell_options.xtrace`, emit
  `+ [[ <render_test_leaf> ]]` **before** computing the result.
- `Not(inner)`: if `inner` is a leaf (`Unary`/`Binary`/`Regex`), emit
  `+ [[ ! <render_test_leaf(inner)> ]]` and recurse with `suppress=true` (one
  combined line, like bash). A non-leaf `Not` (rare: `[[ ! ( a && b ) ]]`)
  recurses with `suppress=false` (its leaves trace individually — a documented
  edge, see Residual 1).
- `And`/`Or`: recurse with `suppress=false`. Because the existing evaluator
  already short-circuits (`And`: skip rhs if lhs false; `Or`: skip rhs if lhs
  true), an untaken branch is never reached, so its leaf line is never emitted —
  bash's short-circuit behavior falls out for free.

PS4 is rendered with xtrace suppressed during its own expansion (the existing
`ps4` already does this), so a `$(…)` in `PS4` does not recurse into these new
emit sites.

## Residuals (documented, narrow, all under the existing low-tier L-21)

1. **`[[ ]]` pattern-side quoting.** bash renders the rhs *pattern* of
   `==`/`!=`/`=~` with per-character quote-provenance escaping (`[[ $x == "p q" ]]`
   → `+ [[ p q == \p\ \q ]]`). Reproducing that requires tracking which characters
   came from quoted vs unquoted `WordPart`s. huck renders the lhs raw (matches
   bash) and the rhs as its expanded value (empty → `''`, globs/regex raw), so the
   common cases (`[[ '' =~ ^[0-9.]+$ ]]`, `[[ $x == foo ]]`, `[[ hi == h* ]]`,
   `[[ -n $v ]]`) match byte-for-byte; an exotic mixed-quote *pattern* may differ.
   Also `Not` of a non-leaf compound is an edge (above).
2. **`[[ ]]` operator spelling.** `StringEq` collapses source `=` and `==` to one
   AST variant; huck renders it canonically as `==`. (`<`/`>` likewise canonical.)
   bash echoes the source spelling. Cosmetic, rare.
3. **Quote style in reconstructed headers.** `'foo'` / `"foo"` / `foo` in a
   `case`/`for`/`select` word all parse to the same `Literal`; quoted-vs-unquoted
   is recoverable from the `quoted` flag, single-vs-double is not. The expression
   text matches; the quote character may differ in exotic cases.

Everything else — structure, leaf-splitting, short-circuit, per-iteration
repetition, depth/PS4 prefix, no-match `case`, empty `for` list, verbatim arith
bodies, `while`/`until`/`if` condition tracing — matches bash exactly.

## Out of scope

The broader RUN_HUCK_DIFF "error-message FORMAT" theme (argv0/`line N:` prefix,
command-not-found word order, `.: … file not found` wording, raw `(os error N)`
leaks, `trap -p` SIG-prefix) is a separate, larger iteration. The other L-21
residuals (b: decl-RHS-cmdsub double-exec; c: `2>` does not suppress trace per
M-90; d: pipeline-stage trace order) are unchanged by this work.

## Verification

- **Unit tests** (`expand.rs` / `executor.rs`):
  - `reconstruct_word_source`: literal, `$var`, `"$var"`, `${x:-d}`, `$(cmd)`,
    `$((a+b))`, `$@`/`$*`, mixed.
  - op→string maps: a representative `TestUnaryOp` / `TestBinaryOp` each.
  - `render_test_leaf`: each leaf kind, empty operand → `''`, a `Not(leaf)`.
- **New harness `tests/scripts/xtrace_compound_diff_check.sh`** (byte-identical
  bash↔huck, combined stderr): `[[ ]]` single / `&&` / `||` short-circuit / `!` /
  `=~` / grouping parens; `(( ))` (incl. internal spacing); `case` match +
  no-match; `for-in` multi-word + empty-list + per-iteration; C-style `for`;
  `select` (fed via heredoc); and `while` / `if` regression (condition lines).
  The 3 residuals get explicit known-divergence **comments** (not assertions).
- **Non-tautology:** the pre-fix binary emits nothing for these forms, so the
  harness fails pre-fix and passes post-fix (proven by rebuilding at the merge
  base in a throwaway worktree).
- Full `cargo test` (0 failures), all `*_diff_check.sh` harnesses, and
  `cargo clippy --all-targets` green.

## Docs / close-out

Update **L-21(a)** in `docs/bash-divergences.md`: the per-construct compound
traces are now emitted; narrow the entry to the three documented residuals
(pattern-side quoting, operator spelling, quote style) instead of "emits
nothing". Record v198 in `project_huck_iterations.md` + `MEMORY.md`.

## Task decomposition (for the plan)

1. `reconstruct_word_source` + unit tests.
2. `xtrace_compound` helper + the raw-header emit sites (`case`, `for`, `select`,
   `(( ))`, C-for) + harness cases for those.
3. The `[[ ]]` leaf hook (`render_test_leaf`, op maps, `suppress` threading) +
   unit tests + `[[ ]]` harness cases.
4. Full-suite/clippy/harness sweep, docs (L-21 update), memory.
