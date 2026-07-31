# v351 — flip `case` design

**Issue:** [#350](https://github.com/jdstanhope/huck/issues/350) — "backslash from an
unquoted expansion is not treated as a pattern escape in `case`/`[[ ]]`/`${x#pat}`".

## Goal

Flip the bash 5.2.21 test-suite **`case`** category to PASS (byte-identical to
`case.right`) by fixing two roots, both self-contained in pattern
matching/evaluation (no byte-wall, no environment artifacts).

## Root 1 (dominant) — a backslash from an unquoted expansion is a pattern escape

In bash, a backslash in a pattern (fnmatch, `FNM_NOESCAPE` off) escapes the
next character, making it literal: `\x` matches `x`, `\*` matches a literal `*`,
`\\` matches `\`. When the backslash comes from an **unquoted** expansion, it
stays pattern-active; a **quoted** `"$x"` is literal.

huck's `glob::Pattern` matcher has no backslash escape — it deliberately encodes
quoted metacharacters as bracket literals (`[*]`, `[?]`, …) via
`escape_pattern_literal`, and pushes **unquoted** expansion text (including data
backslashes) into the pattern verbatim. So a data backslash reaches
`glob::Pattern` as a literal `\`, and `\x` fails to match `x`.

Verified (all `bash` matches, `huck` fails):

```
x='\x';  case x in $x) …          # bash matches, huck no-match
p='\*';  case '*' in $p) …        # bash matches (literal *), huck no
p='\\';  case '\' in $p) …        # bash matches (literal \), huck no
x='\x';  [[ x == $x ]]            # bash matches, huck no
v=xy; p='\x'; echo "${v#$p}"      # bash "y", huck "xy"
```

The same root spans `case`, `[[ … == pat ]]`, and `${var#pat}`/`%`/`//`
(all go through `expand_pattern`/pattern expansion).

### Fix

The quote/unquote distinction is known at expansion time in
`expand_word_with_quote_escape` (`expand.rs:2109`), which drives both
`expand_pattern` (glob) and `expand_regex_operand` (regex). For a **quoted**
part it already `escape`s the text; for an **unquoted** part it pushes text raw.

Change the **unquoted** branch to process data backslashes as escapes: walk the
unquoted text and, for each `\c`, emit the *literal* form of `c` via the same
`escape` callback (so `\*` → `escape("*")` = `[*]`, `\x` → `escape("x")` = `x`,
`\\` → `escape("\\")`), and drop a trailing lone backslash per bash. A
non-escaped unquoted metacharacter still passes through raw (stays active).
Because the fix runs the escaped character through the *same* `escape` used for
quoted parts, it works for both the glob pattern path and the regex path
(`\.` in an unquoted `[[ =~ ]]` operand → literal `.`, matching bash).

This is per-part at the quote boundary — the only place the distinction exists
(a post-hoc pass over the final string cannot tell a quoted `\` from an
unquoted one, since both render as a bare `\`). Cross-part escapes (`$a$b` with
`a='\'`, `b='*'`) are a rare edge bash itself treats idiosyncratically; per-part
processing is acceptable and matches the category.

### Blast radius

`expand_word_with_quote_escape` feeds ONLY `expand_pattern` and
`expand_regex_operand` — i.e. `case`, `[[ == ]]`/`[[ =~ ]]`, and
parameter-expansion pattern operators. Filename globbing uses a different
expansion path and is NOT touched (any equivalent gap there is a separate
follow-up). A full no-regression sweep is still mandatory (extglob/glob-test/
nquote/dbracket categories exercise these matchers) — the spike must confirm the
existing pattern tests stay green.

### Matcher backslash handling

After the fix, `expand_pattern` emits a clean glob string (bracketed literals,
no raw data backslashes), so both `glob::Pattern` and `extglob_match` receive
the same well-formed pattern. Confirm `escape("\\")` yields a form the matcher
treats as a literal backslash (may need `[\\]`); add a unit case.

## Root 2 (minor) — arithmetic side-effect pattern on a readonly variable

`readonly xx=1; case 1 in $((xx++)) ) … ;; *) … esac; echo ${xx}.$?`:
- bash: one `xx: readonly variable` error, `$?` = 1 → prints `1.1`.
- huck: emits an EXTRA, differently-worded line `xx++: xx: readonly variable
  (error token is "")`, and `$?` = 0 → prints `1.0`.

Fix: when a `$(( … ))` inside a `case` pattern fails (e.g. readonly), huck must
(a) not double-report / must match bash's single readonly error for this path,
and (b) leave `$?` = 1. Scope to the case-pattern arithmetic-expansion error
path; verify the non-error arithmetic-pattern cases (`$((y=0))`, `$((x=1))`,
`;&` fall-through in lines 33–38 of `case.tests`) still match bash (`1.0` there).

## Tests

- Unit/behavior: the Root-1 matrix (`\x`→x, `\*`→literal, `\?`, `\a`, `a\*b`,
  `\\`→`\`) across `case`, `[[ == ]]`, and `${x#pat}`; the quoted-`"$x"`
  literal KEEP cases; Root-2 readonly-arith `1.1` + single error, and the
  non-error arith-pattern KEEP (`1.0`).
- `tests/scripts/case_diff_check.sh` — bash↔huck byte-identical over the pattern
  matrix + KEEP guards (quoted patterns literal, extglob still works,
  filename-glob unaffected sample).

## Verification

- Official `case` runner → PASS, `diff case.right <huck>` == 0.
- Full bash-suite runner: PASS-set == v350 baseline (38) **+ `case`** = 39, zero
  regressions (spot-check extglob/glob-test/nquote*/dbracket — Root 1 touches
  shared pattern expansion).
- `run_diff_checks.sh` green (incl. new harness); engine lib green; the
  `[[ ]]`/glob/param-expansion integration bins green.

## Out of scope

Filename-globbing backslash-from-expansion (separate path; open a follow-up if
the category doesn't need it). The `;&`/`;;&` fall-through and `esac`-as-pattern
main-test lines already pass.
