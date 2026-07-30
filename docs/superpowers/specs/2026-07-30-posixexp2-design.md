# v347 — flip the `posixexp2` bash-suite category

**Issue:** [#337 — quote: backslash handling in `${...}` operands](https://github.com/jdstanhope/huck/issues/337).

**Goal:** flip the bash-suite `posixexp2` category to PASS (byte-identical) by
fixing two backslash-in-`${...}`-operand roots. Target: full runner PASS
34 → 35.

## Background & feasibility spike

`posixexp2` stress-tests `${IFS+word}` operand quoting under `set -o posix`.
After v346, the clean full-category diff is **15 lines**, decomposing into
exactly two roots — both "bash removes the backslash, huck keeps it." A code
spike confirmed both live in `scan_step_param_operand`'s backslash arms
(`crates/huck-syntax/src/lexer.rs`, ~2714), **not** the consciously-deferred
param-expansion-lexer refactor. Minimal reproductions (arg values chosen so the
operand is actually taken):

## Root A — `\}` before the `}` delimiter in a double-quoted `${...}` operand

Inside `${...}`, a `\}` escapes the closing-brace delimiter: bash removes the
backslash and keeps `}` as a literal brace in the value. huck keeps the
backslash **when the `${...}` is double-quoted**; the unquoted form already
works.

- `x=1; echo "${x+\}z}"` → bash `}z`, huck `\}z`.
- `x=1; echo "${x+a\}b}"` → bash `a}b`, huck `a\}b`.
- `x=1; echo ${x+\}z}` (unquoted) → both `}z` (already correct — do not regress).

**Regression baselines (must stay):** `x=1; echo "${x+\p}"` → `\p` (a backslash
before a NON-delimiter, non-special char is KEPT), `x=1; echo "${x+a\$b}"` →
`a$b` (`\$` is already special-dropped). So the fix must drop the backslash
**only** before the operand's `end` delimiter char (`}`, and `]` for the
`${a[...]}` subscript operand by analogy), not before arbitrary chars.

**Fix:** in `scan_step_param_operand`, the backslash arm(s) must, when the next
char is the operand `end` delimiter, drop the backslash and emit the delimiter
char as literal (quoted) content — in the context where huck currently keeps
it (the double-quoted-operand path). The scanner already threads `end`,
`in_dquote`, and `enclosing_dquote`; the special-char arm (`$`/`` ` ``/`"`/`\`)
already drops the backslash. Add `end` (and the closing subscript `]`) to the
"drop the backslash" set for the operand context, matching bash. Verify unquoted
and pattern-operand (`${x#...}`, `${x/.../...}`) forms stay correct.

## Root B — `\<newline>` line-continuation in a `${...}` operand

A `\<newline>` inside a `${...}` operand is a line continuation: bash removes
both bytes (the same rule v346 #334 applied to backtick bodies), in every
inner-quote context.

- `x=1; echo "${x+foo\<NL>bar}"` → bash `foobar`, huck `foo\<NL>bar`.
- `x=1; echo ${x+foo\<NL>bar}` (unquoted) → bash `foobar`, huck `foo<NL>bar`.
- `x=1; echo "${x+'foo\<NL>bar'}"` (inner single quotes) → bash `'foobar'`, huck
  `'foo\<NL>bar'`.

**Fix:** in `scan_step_param_operand`, a `\` immediately followed by a newline
consumes both and emits nothing (line continuation), in the default,
inner-single-quote, and inner-double-quote operand sub-scanners. (Note: inside
inner single quotes bash STILL removes `\<newline>` here — the operand's
backslash-newline processing precedes inner-quote literalization, mirroring the
backtick rule.)

## Verification

- **Official `posixexp2` runner** produces zero diff (the flip signal; the
  category needs no external helper — plain `echo`/`printf`).
- **Diff-check harness** `posixexp2_diff_check.sh` with fragments per root plus
  the regression baselines (`\p` kept, `\$` dropped, unquoted `\}` already `}`,
  a normal `${x+word}` with no backslash, a pattern operand `${x#\}}`).
- **Unit tests** in the syntax crate for `scan_step_param_operand` backslash
  handling (`\}` drop in dquoted operand; `\<newline>` removal; `\p` kept).
- **No-regression:** full bash-suite runner PASS **34 → 35**, branch PASS-set
  diffed against the v346 baseline (exactly the 34 + `posixexp2`; the operand
  scanner is shared by ALL `${...}` — verify the nquote*/param/dollars/rhs-exp/
  quote categories explicitly); `run_diff_checks.sh` green; per-crate lib tests
  + the param/quote `-p huck` integration bins.

## Scope / non-goals

- Only the two operand-backslash roots. The broader param-expansion-lexer debt
  (drifted brace scanners, `${'x1'…}` error-model backlog in `posixexp`) is out
  of scope.
- No operand-scanner rearchitecture — the fix adds two backslash cases to the
  existing arms.

## Summary of touched files

- `crates/huck-syntax/src/lexer.rs` — `scan_step_param_operand` backslash arms
  (Root A `\}`/`\]` delimiter escape; Root B `\<newline>` line continuation).
- `tests/scripts/posixexp2_diff_check.sh` (new).
