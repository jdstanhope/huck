# v346 — flip the `quote` bash-suite category

**Issue:** [#334 — quote: empty-field generation in `${x:+word}`, backslash
removal, IFS word-splitting](https://github.com/jdstanhope/huck/issues/334).

**Goal:** flip the bash-suite `quote` category to PASS (byte-identical) by
fixing the word-expansion/quoting roots. Target: full runner PASS 33 → 34.

## Background & feasibility spike

`quote` is FAIL. With the suite's `recho`/`zecho` helpers built (from
`$BASH_SOURCE_DIR/support/*.c`, needs `-I$BASH_SOURCE_DIR` for `bashansi.h`),
the clean full-category diff (`quote.tests` + `quote1..4.sub`) is ~48 lines,
decomposing into the roots below — all well-defined POSIX quoting rules (no
walls like the UTF-8 or regex-engine categories). Each was reproduced against
bash 5.2.21.

Field-count reproduction without the helper (`x=x; e=; set -- <frag>;
printf 'n=%d:' "$#"; printf '<%s>' "$@"`):

| fragment | bash | huck |
|---|---|---|
| `${x:+""}` | `n=1:<>` | `n=1:<>` ✓ |
| `${x:+ ""}` | `n=1:<>` | `n=0:` ✗ |
| `${x:+"$e"}` | `n=1:<>` | `n=1:<>` ✓ |
| `${x:+"$e""$e"""}` | `n=1:<>` | `n=1:<>` ✓ |
| `${x:+"$e" "$e"""}` | `n=2:<><>` | `n=0:` ✗ |
| `${x:+''}` | `n=1:<>` | `n=1:<>` ✓ |

## Root R4 — empty-field generation in `${x:±word}` alternates (dominant)

`quote2.sub` (~40 diff lines). In `${x:+word}` / `${x:-word}` (and `:=`/`:?`),
the substituted `word` must undergo the SAME quote-removal + field-splitting a
normal word does: a quoted-empty segment (`""`, `"$e"` with `e=`, `''`)
produces an **empty field**, and IFS whitespace inside the alternate separates
fields. huck collapses these — it drops the empty field(s) when the alternate
has internal IFS whitespace and/or multiple quoted-empty parts (`${x:+ ""}`,
`${x:+"$e" "$e"""}`), while the no-internal-space cases already work.

The fix is in the parameter-expansion path that expands the alternate/default
`word` of `${x:±word}`: it must produce quoted-empty fields and split on IFS
identically to top-level word expansion — likely the alternate is being
expanded through a path that discards empty fields or does not mark the quoted
segments as field-generating. Implementer to locate the `${x:+…}`/`${x:-…}`
alternate expansion and align its field/quoted-empty handling with the normal
word path. Simple single-segment cases must stay correct (no double fields).

## Root R1 — backslash+space / backslash-newline removal

`quote.tests` (lines producing `foo\ bar` / `foo\<newline>bar` where bash gives
`foobar`; and the tail `${foo:-string \\}` / `${foo:-string \\\}}` cases).
huck leaves a backslash that bash removes during quote removal. Align
huck's backslash-removal in the affected quote/echo and `${foo:-word}` contexts
with bash.

## Root R2 — literal control-char (`\^J`) in an argument

`quote.tests` ~line 46 (a backtick / embedded-newline argument). `recho` shows
bash passing the raw byte where huck renders `\^J`; huck is inserting/escaping
where it should pass through. Verify with the built `recho` (the divergence is
only visible through it).

## Root R3 — IFS word-splitting with quotes

`quote.tests` ~line 87: `foo b   c baz` / `foo 'bar baz` (bash) vs huck
`foo  baz`. huck collapses/mis-splits fields under the test's IFS/quote setup.
Align field splitting with bash for the specific construct.

## Root R5 — trailing `${foo:-string \\\}}` backslash count

`quote.tests` end: an off-by-one field/backslash count (`2`/`4` vs huck `3`) on
the `${foo:-string \\\}}` family. Likely a knock-on of R1/R4 (backslash removal
+ field count inside `${foo:-word}`); confirm whether it resolves once R1/R4
land, else fix the `${foo:-word}` backslash handling.

## Verification

- **Official `quote` runner** produces zero diff (the flip signal). The runner
  builds `recho`/`zecho` from `support/*.c`; ensure a C compiler is available.
- **Diff-check harness** `quote_diff_check.sh` with fragments per root using an
  arg-counter that does NOT need the external `recho` (`set -- <frag>; printf
  'n=%d:' "$#"; printf '<%s>' "$@"`) for R4/R3, plus `echo`/`printf %q` for
  R1/R2/R5. Cover the R4 table above (matches + fixes), the R1 backslash tail
  cases, the R3 split case, and regressions (single-segment `${x:+""}` stays
  `n=1`; a normal word still splits normally).
- **Unit tests** in the expansion crate for the `${x:±word}` alternate
  field/quoted-empty generation.
- **No-regression:** full bash-suite runner PASS **33 → 34**, branch PASS-set
  diffed against the v345 baseline (exactly the 33 + `quote`; R4 touches the
  shared parameter-expansion + field-splitting path — verify the
  dollar/param/ifs/nquote* categories explicitly); `run_diff_checks.sh` green;
  per-crate lib tests + the param/ifs/quote `-p huck` integration bins.

## Scope / non-goals

- Only the roots above. Other quoting already passes and must stay passing.
- `recho`/`zecho` are external helpers the runner builds; the huck-side harness
  avoids depending on them (uses `set --`/`printf`).

## Summary of touched files

- `crates/huck-engine/src/` — the parameter-expansion + field-splitting path
  for `${x:±word}` alternates (R4, R5), backslash removal (R1), control-char
  passthrough (R2), IFS splitting (R3). Exact modules pinned during
  implementation (`param_expansion.rs` / `expand.rs` / the IFS split site).
- `tests/scripts/quote_diff_check.sh` (new).
