# v233 — `${…}` Parameter-Expansion Parse Robustness

**Date:** 2026-06-27
**Status:** approved
**Topic:** Close the four `${…}` parse gaps where bash accepts a construct
huck rejects at parse time, and adopt bash's "scan-to-`}` then defer
semantic errors to runtime" model so a lexable-but-invalid `${…}` no
longer aborts the whole script.

## Problem

A parse-only sweep (`huck -n`) over the 471 bash-5.2.21 test-suite
scripts found 33 that huck fails to parse but bash accepts. The largest
cluster — 9 files — is parameter expansion. huck raises a **fatal lex
error** on `${…}` content it does not recognize; bash instead scans to
the matching `}`, builds the expansion, and defers any semantic error to
**runtime** ("bad substitution"). Because a parse error aborts the entire
script, each gap can sink a whole test category regardless of runtime
behavior.

The nine files break into four mechanisms (all confirmed against bash
5.2.21; the two other "nested quote" files parse fine in isolation and
fail for unrelated surrounding-context reasons — out of scope):

| Mechanism | Failing form | Files |
|---|---|---|
| M1 prefix-name expansion | `${!_Q*}`, `${!_Q@}` | new-exp3, nameref20, varenv13 |
| M2 special params as the name | `${##}`, `${!#}`, `${-3}`, `${$x}` | exp, new-exp, errors6, errors2 |
| M3 `@`-transform edges | `${V@}`, `${!v[@]@Q}` | new-exp10, new-exp13 |
| M4 `$'…'` inside a brace pattern | `${x#$'a\t\'\tb'}` | posixexp7 |

### Measured bash semantics (the spec's ground truth)

```
${!_Q*}   with _Qa=1 _Qb=2  -> "_Qa _Qb"  rc 0   (sorted names, $*-style join)
${!_Q@}   with _Qa=1 _Qb=2  -> "_Qa _Qb"  rc 0   ($@-style separate words)
${!NOSUCH*}                 -> ""          rc 0
${##}     set -- a b c      -> "1"         rc 0   (= length of $#, i.e. ${#<#>})
${!#}     set -- a b c      -> "c"         rc 0   (= indirect of $#: var named "3" -> $3)
${#}      no params         -> "0"         rc 0   (already works: = $#)
${-3}                       -> bad substitution, rc 1, parses under `bash -n`
${-3:-x}                    -> bad substitution, rc 1, parses under `bash -n`
${$x}                       -> bad substitution, rc 1, parses under `bash -n`
${V@}     V=42              -> bad substitution, parses under `bash -n`
${!v[@]@Q} v=(a b c)        -> runtime error ("invalid variable name"), parses under `bash -n`
${x#$'a\t\'\tb'} x=aXb      -> "aXb"        rc 0   ($'...' recognized in pattern)
${x#$'f'}        x=foo      -> "oo"         rc 0
```

Bad-substitution exit/continuation behavior is byte-matched to bash by
the diff harness (Testing); the implementer measures bash's exact
non-interactive continuation and replicates it.

## Goal

Make huck parse all four mechanisms the way bash does:

1. **M1** — implement prefix-name expansion `${!pfx*}` / `${!pfx@}`.
2. **M2** — accept special parameters (`# @ * $ ! - ? 0-9`) as the name in
   the length (`${#<sp>}`) and indirect (`${!<sp>}`) forms; route invalid
   combinations to a runtime bad-substitution.
3. **M3** — parse `${var@}` (empty transform op) and `${!arr[@]@OP}`
   (indirect-keys + transform) instead of lex-erroring; defer their
   runtime errors.
4. **M4** — recognize `$'…'` (and `'…'`, `"…"`, nested `${…}`) inside the
   brace body so embedded quotes do not produce a false "unterminated".

And adopt the **scan-to-`}`-then-defer** model: a lexable `${…}` whose
content is semantically invalid produces a deferred bad-substitution
node, not a parse abort. An **unterminated** `${…}` (no matching `}`)
remains a parse error, exactly as in bash.

**Non-goals:** the two surrounding-context "unterminated quote" files
(array6, nquote2 — they parse in isolation); flipping any bash-test
category (parse-unabort is necessary, not sufficient — flips are measured
and reported honestly, not promised). Behavioral parity of every exotic
M3 combination beyond "parses + errors at the right time."

## Design

### A. Parse model: scan to `}`, classify, defer

In `crates/huck-syntax/src/lexer.rs`, the `${…}` path
(`scan_braced_operand` / `parse_braced_operand_opts`, ~2529–2718)
currently returns `Err(LexError::EmptyParamName)` /
`Err(LexError::InvalidBraceModifier(..))` for unrecognized content. New
flow:

1. **Scan to the matching `}`** using a brace-body scanner hardened per M4
   (quotes/`$'…'`/nested `${}` aware). If no matching `}` is found →
   keep the existing **parse error** (unterminated `${…}`).
2. With the full body in hand, **classify**:
   - a recognized form (existing modifiers, plus M1/M2/M3 additions) →
     build the normal `WordPart::Param{…}` node;
   - otherwise → build a new **`WordPart::Param` carrying a `BadSubst`
     marker** that records the raw `${…}` text.

`BadSubst` is represented as a new `ParamModifier::BadSubst { raw: String }`
(raw is the literal `${…}` source, used for the runtime message). This
keeps the change inside the existing `WordPart::Param` shape; no new
top-level `WordPart` variant is required.

### B. M1 — prefix-name expansion

- **Lexer:** in the `${!…}` branch, after reading the prefix name, if the
  next char before `}` is `*` or `@`, emit
  `ParamModifier::PrefixNames { at: bool }` (`at=true` for `@`) with the
  prefix as the param name. (Distinct from the existing bare-indirect and
  `IndirectKeys` paths.)
- **Engine** (`param_expansion.rs`): collect the names of all set shell
  variables whose name starts with the prefix, **sorted** (bash orders
  them); join `$*`-style for `*` (single field, IFS-joined in the usual
  contexts) and `$@`-style (separate words) for `@`. Empty match → empty
  result, rc 0. Honors quoting like `$*`/`$@`.

### C. M2 — special parameters as the name

- **Lexer:** the name reader accepts a single special-parameter char
  (`# @ * $ ! - ? 0-9`, with multi-digit runs for positional params) as
  the parameter name in the `${#name}` (length) and `${!name}` (indirect)
  forms. So `${##}` parses as length-of-`$#`, `${!#}` as indirect-of-`$#`.
- **Engine:** length and indirect operators evaluate against the special
  parameter's value (reusing existing special-parameter resolution).
- **Invalid combos** (`${-3}`, `${-3:-x}`, `${$x}`) — where the leading
  char cannot begin a valid name/operator sequence — classify as
  `BadSubst` (per A) rather than lex-error.

### D. M3 — `@`-transform edges

- `${var@}` (transform sigil with **no** operator letter before `}`) →
  `BadSubst` (bash: bad substitution).
- `${!arr[@]@OP}` — the existing `IndirectKeys` parse must tolerate a
  trailing `@OP` transform: parse into a `Param` node carrying both the
  indirect-keys flag and the `Transform { op }`. Evaluation applies keys
  then transform; bash's runtime errors for nonsensical combinations are
  reproduced where they occur (or surfaced as the engine's existing
  error), but the construct **parses**.

### E. M4 — quote/`$'…'`-aware brace-body scan

The brace-body scanner used to find the closing `}` must treat the
following as opaque spans (their internal `}`/`'`/`"` do not terminate or
confuse the scan):

- `'…'` single-quoted (no escapes),
- `"…"` double-quoted (with `\` escapes),
- `$'…'` ANSI-C quoted (with `\'`, `\t`, `\\`, … escapes — reuse the
  existing `$'…'` decoder's scan logic),
- nested `${…}` (recurse / depth-count).

This lets `${x#$'a\t\'\tb'}` and similar parse: the `'` inside `$'…'` is
consumed by the ANSI-C span, not mistaken for an unbalanced quote.

### F. Runtime "bad substitution" error

`ParamModifier::BadSubst { raw }` evaluates (in `param_expansion.rs`) to a
runtime error rendered as:

```
huck: <error_prefix>${raw}: bad substitution
```

reusing v216's `Shell::error_prefix` (non-interactive →
`<src>: line N: `, interactive → `huck: `), to match bash's
`bash: line N: ${raw}: bad substitution`. The exit status and
non-interactive continuation behavior are matched to bash exactly (the
implementer measures: bash aborts the current command's expansion, sets
status, and continues the script non-interactively) and pinned by the
diff harness. No new error variant is added if an existing expansion-error
channel already carries a message + status; otherwise a `BadSubstitution`
expansion error is introduced alongside the current ones.

## Testing

- **Lexer unit tests:** each M1–M4 form parses to the expected node;
  `${!pfx*}`/`${!pfx@}` → `PrefixNames`; `${##}`/`${!#}` → length/indirect
  with special-param name; `${V@}`/`${-3}`/`${$x}` → `BadSubst`;
  `${x#$'a\t\'\tb'}` parses with the `$'…'` intact; an **unterminated**
  `${x` still errors.
- **Engine unit tests:** prefix listing is sorted and respects `*` vs `@`;
  length/indirect of `$#` match the measured values; `BadSubst` produces
  the bash-matching message + exit.
- **`tests/scripts/param_expansion_diff_check.sh`** (new): bash↔huck
  byte-identical (stdout+stderr+exit, file mode) across all M1–M4 forms
  and the bad-substitution forms, including the short-circuit context
  (`[[ … || … ${H*} ]]`) and the `trap EXIT` continuation case.
- **Re-parse gate:** `huck -n` over all nine real suite files
  (new-exp.tests, new-exp3/10/13.sub, nameref20.sub, varenv13.sub,
  exp.tests, errors2/6.sub, posixexp7.sub, cond.tests) → **zero**
  parse-aborts.
- **Full sweep:** `cargo test --workspace` green.
- **Measure (honest):** re-run the bash-test categories most likely
  affected — `new-exp`, `posixexp`, `posixexp2`, `errors`, `cond`,
  `varenv`, `nameref`, `exp-tests` — and report flip/shrink per
  measure-first discipline. Parse-unabort is necessary, not sufficient.

## Risks

- **Model-shift regressions:** moving validation from lex-time to runtime
  could let a construct that *should* be a syntax error slip through.
  Mitigation: only lexable-with-matching-`}` content defers; unterminated
  `${…}` stays a parse error; the workspace + diff-harness sweep guards
  existing behavior.
- **Brace-scan over-reach (M4):** a hardened scanner must not change where
  a valid `}` is found for already-working expansions. Mitigation: the
  quote/`$'…'`/nested-`${}` spans are additive; existing tests pin the
  prior boundaries.
- **Prefix listing ordering/quoting (M1):** must match bash's sort and
  `$*`/`$@` field semantics. Mitigation: diff harness covers quoted and
  unquoted, `for`-loop iteration, and no-match cases.
- **Bad-subst continuation/exit:** bash's non-interactive behavior is
  subtle (abort current command, continue script). Mitigation: measured
  empirically and pinned byte-for-byte by the harness, not assumed.
