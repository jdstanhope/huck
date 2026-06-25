# v219 — `Word` quote-span provenance (`WordPart::Quoted`)

## Status

Design approved 2026-06-25. Scope confirmed with the user: implement quote-span
provenance via a `WordPart::Quoted` wrapper variant (option B), targeting a
**cprint** bash-test-suite category flip. herestr's two non-provenance blockers
and xtrace L-21(a) are out of scope.

## Background

huck reconstructs parsed command ASTs back to shell source for `declare -f` /
`type` (the v218 `print_cmd.c` port made the *structure* byte-faithful). The
remaining reconstruction divergence is **quote provenance**: huck's `Word`
stores only `quoted: bool` per `WordPart`, which loses (a) the quote *style*
(`'…'` vs `"…"` vs `$'…'` vs backslash-escape) and (b) the *span boundaries*
(which adjacent parts share one quote pair). bash preserves both exactly.

### What bash actually does (measured against bash 5.2.21)

bash preserves the original quote **structure** verbatim but **re-renders
expansions** through its own command-printer:

| source word | bash `declare -f` reconstruction |
|---|---|
| `"plain"` | `"plain"` (double kept even when unneeded) |
| `'plain'` | `'plain'` (single kept) |
| `\$escaped` | `\$escaped` (backslash kept) |
| `"a b""c d"` | `"a b""c d"` (adjacent spans kept separate) |
| `'a'"b"` | `'a'"b"` (mixed adjacent kept) |
| `ab'cd'ef` | `ab'cd'ef` (bare+single+bare kept) |
| `"$a $b"` | `"$a $b"` (one span, vars+literal) |
| `"$(  date  )"` | `"$(date)"` — quote kept, **command-sub re-rendered** |
| `"${x:-  d }"` | `"${x:-  d }"` — literal modifier text kept |

So matching bash requires preserving the quote *delimiters and spans* as
structure while continuing to render expansion innards (`$(…)`, `${…}`, `$((…))`)
through huck's existing recursive part renderers. **Verbatim-source replay is
wrong** (it would keep `"$(  date  )"`); a structural model is required.

### Why cprint flips and herestr does not

Measured residuals (huck vs bash `.right`), after the v218 format port:

- **cprint** — 10 diff lines, **all** quote/escape-style provenance loss
  (huck emits `"\$"PWD`, `"&""|""()"`, `fu"%"nc` where bash emits `\$PWD`,
  `\&\|'()'`, `fu\%nc`). No time-pipeline or other divergence appears. Quote
  provenance is cprint's **only** blocker → fixing it **flips cprint to PASS**.
- **herestr** — provenance hunks (`"$a $b"`, `'what a fabulous window
  treatment'`, `'double"quote'`) PLUS two non-provenance blockers: `declare -p`
  ANSI-C value quoting (`[3]=$'i\n'` rendered as a literal newline) and a
  runtime `command not found:` (empty command name). Provenance shrinks herestr
  but does not flip it.

cprint is therefore the success target; herestr stays FAIL (shrunk), with its
two extra blockers recorded as the documented next targets.

## Goals

1. Add `WordPart::Quoted { style: QuoteStyle, parts: Vec<WordPart> }` and a
   `QuoteStyle` enum (`Single`, `Double`, `AnsiC`, `Backslash`); the lexer wraps
   each contiguous quoted run in one `Quoted`.
2. `declare -f` / `type` reconstruction is **byte-identical to bash 5.2.21** for
   quoted words (single/double/ANSI-C/backslash, adjacent spans, mixed, and
   expansion-bearing double-quoted runs).
3. The **cprint** bash-test-suite category passes (runner diff = 0).
4. **No expansion-semantics regression**: inner parts keep `quoted: true`, so
   the existing splitting/glob/quoted-null logic is unchanged; the full test
   suite and the IFS/splitting/quoting diff harnesses stay green.
5. Round-trip idempotence preserved (`rt_*` tests).

## Non-goals / Out of scope (deferred, documented)

- **herestr's non-provenance blockers**: `declare -p` ANSI-C control-char value
  quoting (`$'…'`), and the runtime `command not found:` bug. herestr stays FAIL
  (shrunk); both recorded as `[deferred]`.
- **xtrace L-21(a)** quote-provenance residual (`reconstruct_word_source` in
  `expand.rs`): the same `Quoted` structure is now available to it, but
  rewiring xtrace is out of scope unless it falls out for free. xtrace is
  `[intentional]`/low.
- **Replacing `quoted: bool`** as the expansion signal (option C) — not done;
  the bool stays, the wrapper is additive (decision B1 below).

## Design

### AST representation (`crates/huck-syntax/src/lexer.rs`)

```rust
pub enum QuoteStyle {
    Single,    // '…'   — literal content, no expansion
    Double,    // "…"   — expansions active; literal chars double-quote-escaped on render
    AnsiC,     // $'…'  — ANSI-C escapes decoded at lex time; re-escaped on render
    Backslash, // \c    — a single backslash-escaped character
}

pub enum WordPart {
    // … existing variants unchanged …
    /// One contiguous quoted run, preserving its source style and span.
    /// Inner `parts` keep their own `quoted: true` flag (decision B1) so the
    /// expansion path is unchanged; the wrapper exists for reconstruction.
    Quoted { style: QuoteStyle, parts: Vec<WordPart> },
}
```

A bareword (unquoted) segment stays a flat, unwrapped `WordPart` with
`quoted: false`. Only quoted runs are wrapped. Worked examples:

- `ab'cd'ef` → `[Literal{"ab",false}, Quoted{Single,[Literal{"cd",true}]}, Literal{"ef",false}]`
- `"$a $b"` → `[Quoted{Double, [Var{a,true}, Literal{" ",true}, Var{b,true}]}]`
- `\$PWD` → `[Quoted{Backslash,[Literal{"$",true}]}, Literal{"PWD",false}]`
- `\&\|'()'` → `[Quoted{Backslash,[Literal{"&",true}]}, Quoted{Backslash,[Literal{"|",true}]}, Quoted{Single,[Literal{"()",true}]}]`

### Decision B1: inner parts keep `quoted: true`

The wrapper is **additive**: it records style+span for reconstruction, but the
inner parts retain `quoted: true`, which remains the load-bearing signal for
expansion (word-splitting, globbing, quoted-null). This is what keeps the ~288
`quoted`-reading sites in `expand.rs` unchanged — expansion simply recurses into
`Quoted` and the existing per-part logic runs on the inner parts. `generate.rs`
uses the wrapper's `style` for delimiters (and double-quote-escapes literal
content in a `Double` run); it does not rely on the inner flag for delimiters.

### Lexer changes (`lexer.rs`)

The word-scanning quote blocks each currently push run parts directly into the
flat `parts` vec. Restructure each to collect the run into a local sub-vec and
push one `WordPart::Quoted`:

- **Single `'…'`** (~line 540): collect the literal into one inner
  `Literal{quoted:true}`; wrap `Quoted{Single, …}`. Empty `''` →
  `Quoted{Single, [Literal{"",true}]}` (preserve the empty-token contract).
- **Double `"…"`** (~line 558): collect into a sub-vec —
  `flush_literal`/`scan_dollar_expansion`/backtick push into the sub-vec
  (`scan_dollar_expansion` already takes `&mut Vec<WordPart>`, pass the sub-vec);
  wrap `Quoted{Double, …}`. Empty `""` → `Quoted{Double,[Literal{"",true}]}`.
- **Backslash `\c`** (~line 601): wrap the one-char escaped literal as
  `Quoted{Backslash, [Literal{c,true}]}`. (`\<newline>` line-continuation
  behavior is unchanged — it produces no part.)
- **ANSI-C `$'…'`** (`scan_ansi_c_quoted`, ~line 1889): the decoded value
  becomes the inner `Literal{quoted:true}`; wrap `Quoted{AnsiC, …}`.

Quote handling **inside** a `Double` run (escaped `\"`, `\$`, `` \` ``, `\\`,
`\<NL>`) is unchanged — those produce literal chars within the run's sub-vec, as
today; `generate.rs` re-escapes them when rendering the `Double` run.

### Expansion changes (`expand.rs`)

Add one arm to the central part-processing path: `WordPart::Quoted { parts, .. }`
⇒ process each inner part with the existing per-part logic (they carry
`quoted: true`), appending to the **same** field accumulator so concatenation
(`"$a"b`), the empty-quoted field (`""`), `"$@"`/`"$*"`, and quoted-null
(`${x:+}`) all behave as before. Any other site that iterates a `Word`'s parts
and matches specific variants gets a `Quoted` arm too — the compiler enumerates
them (exhaustive `match` on the new variant fails to compile until handled).

### `generate.rs` changes (the reconstruction payoff)

Render `Quoted` by style:
- `Single` → `'` + inner literal text verbatim + `'`.
- `Double` → `"` + inner parts (literals via existing `escape_double_quote_value`;
  `Var`/`CommandSub`/`Arith`/`ParamExpansion` via their normal recursive
  renderers — NOT individually re-quoted) + `"`.
- `Backslash` → `\` + the single inner char.
- `AnsiC` → `$'` + ANSI-C re-escape of the inner value (control chars →
  `\n`/`\t`/`\xNN` etc.) + `'`.

The existing per-part `quote_if(quoted, …)` wrapping in `part_to_source` is
removed for the wrapped case (the wrapper now owns quoting); unwrapped parts
with `quoted: false` render bare as today. A `Word` whose parts contain no
`Quoted` wrapper (e.g. a synthetic/parser-rebuilt word) renders via the existing
path — graceful fallback, no panic.

### Other `WordPart` consumers (compiler-enumerated)

Each exhaustive `match WordPart` must gain a `Quoted` arm. Known sites to expect
(the compiler is authoritative): case-pattern rendering (`pattern_word_to_source`
in generate.rs), `declare -p` value rendering, xtrace `reconstruct_word_source`,
brace expansion, `try_split_assignment`/`AssignPrefix` handling, and any
`Word`-walking utility. Each recurses into `parts` (treating inner as quoted) —
no new behavior beyond reaching the inner parts.

## Testing / Verification

- **`generate.rs` exact-match tests** (bash 5.2.21-captured strings) for each
  form: `"plain"`, `'plain'`, `\$x`, `"$a $b"`, `"a b""c d"`, `'a'"b"`,
  `ab'cd'ef`, `\&\|'()'`, `fu\%nc`, double-quoted with `$(…)`/`${…}` inside,
  and `$'…'` ANSI-C.
- **`tests/scripts/declare_f_diff_check.sh`** — add the quoting fragments;
  assert byte-identical vs live bash (harness stays green, count grows).
- **cprint category PASS** — run the runner
  (`BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_BASH_TEST_HELPERS=/tmp/bash-test-helpers
  HUCK_BASH_TEST_CATEGORY=cprint bash tests/bash-test-suite/runner.sh`); expect 0
  diff. This is the headline success criterion.
- **No expansion regression** — `cargo test --workspace` green (~3672+), and the
  quoting/splitting diff harnesses (`ifs_diff_check.sh`,
  `operand_dquote_context_diff_check.sh`, `alternate_word_quoting_diff_check.sh`,
  `dollar_quote_forms_diff_check.sh`, `pattern_operand_quoting_diff_check.sh`,
  and the lexer's own quote tests) pass unchanged.
- **Round-trip** — existing `rt_*` tests pass; the new wrapped forms re-parse to
  the same AST (add `assert_rt_ast_eq` cases for representative quoted words).

The success criterion: cprint flips to PASS with the full suite + harnesses
green and no expansion-behavior change.

## Risks

- **Expansion regression** is the primary risk. Mitigated by decision B1 (inner
  parts keep `quoted: true`; expansion only gains a recursion arm) and by the
  large existing expansion test suite + quoting diff harnesses. Any harness
  regression blocks the change.
- **Missed `match` site** — mitigated by exhaustive-match compile errors (a new
  variant cannot be silently ignored).
- **ANSI-C re-escaping fidelity** (`$'…'` render) — only needed for the few
  `AnsiC` cases; covered by exact-match tests. If a control-char escape diverges
  it is a contained generate.rs fix.
- **Lexer sub-vec plumbing** (`scan_dollar_expansion` targeting a sub-vec) —
  contained to the quote blocks; the empty-token and concatenation contracts are
  covered by existing lexer tests.
- cprint may reveal a residual the measurement missed — if so, report it; the
  flip is the criterion, not a partial improvement.

## Divergence-doc bookkeeping

- On merge: DELETE the resolved portion of **L-57** (quote provenance in
  reconstruction is fixed). REPLACE it (or add a successor entry) scoped to
  herestr's remaining non-provenance blockers: (1) `declare -p` ANSI-C
  control-char value quoting, (2) the herestr runtime `command not found:` bug.
- Note in **L-21(a)** that the `Quoted` structure is now available to the xtrace
  path should it be wired up later.
- Update `docs/bash-test-suite-baseline.md` (cprint → PASS; herestr note
  trimmed to its two remaining blockers) and the iteration memory.
