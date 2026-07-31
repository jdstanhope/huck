# v349 — flip the `attr` bash-suite category

**Issue:** [#343 — attr: readonly builtin lacks -a, readonly: error prefix, quoted name=(val) arg](https://github.com/jdstanhope/huck/issues/343).

**Goal:** flip `attr` to PASS (byte-identical) by fixing the `readonly`-builtin
divergences. Target: full runner PASS 36 → 37.

## Background & feasibility spike

`attr` stress-tests variable attributes (`readonly`/`declare -a`/`-A`/`-r`/
`-x`). Local bash 5.2.21 matches `attr.right` exactly; the clean residual (`diff
attr.right <huck-output>`, `<` = bash/correct) is ~37 lines, decomposing into
`readonly`-builtin roots. **⚠️ diff-direction:** this category's error lines
carry huck's binary-path prefix, so the RAW runner diff reads inverted —
always confirm with `diff attr.right <huck-output>`. A spike confirmed all
roots live in `builtin_readonly_decl` (`crates/huck-engine/src/builtins.rs`
~1816) + the executor's declaration-arg construction — no rearchitecture.

## Root A — `readonly -a` (indexed-array attribute)

`readonly -a x=(1 2)` → huck `readonly: -a: invalid option`; bash accepts (a
readonly indexed array). `builtin_readonly_decl`'s flag loop handles `-p` and
`-A` but not `-a`, so `-a` hits the `invalid option` arm.

**Fix:** add a `-a` arm (a `want_indexed` flag mirroring `want_associative`).
Where `want_associative` ensures the name is associative before marking
readonly / before `apply_one_assignment`, `-a` should ensure the name is an
indexed array (huck has an indexed-array declare path — reuse it). Then
`readonly -a name=(…)` assigns + marks readonly, and `declare -p` renders
`declare -ar name=(…)`. Verify: `readonly -a x=(1 2); declare -p x` →
`declare -ar x=([0]="1" [1]="2")`; `readonly -a y` (no value) marks an empty
indexed array readonly; `-a` and `-A` together match bash.

## Root B — `readonly name=(array)` value

Plain `readonly r=(7)` ALREADY works (`declare -ar r=([0]="7")` matches bash) —
the category's residual `declare -ax r=([0]="(7)")` comes from a `readonly -a`
path that currently errors, so it is expected to resolve once Root A lands.
Confirm after Root A; if a standalone `readonly -a` array-value case still
mis-parses `(7)` as a literal, fix it there.

## Root C — spurious `readonly:` prefix on the readonly-variable error

`readonly x=1; readonly x=2` → bash `x: readonly variable`, huck
`readonly: x: readonly variable`. (The plain `x=1; readonly x; x=5`
assignment-to-readonly path already matches bash's bare form.)

**Fix:** in `builtin_readonly_decl`'s `DeclArg::Assign` arm, change the error
from `readonly: {name}: readonly variable` to `{name}: readonly variable`
(bash omits the builtin-name prefix for this error). Do NOT change other
`readonly:`-prefixed errors (invalid option, not-a-valid-identifier) — those
keep the prefix, matching bash.

## Root D — quoted `readonly 'name=(val)'` parsed as an assignment

`readonly 'a=(3)'` → huck `` readonly: `a=(3)': not a valid identifier `` (the
quoted arg reaches `builtin_readonly_decl` as `DeclArg::Plain("a=(3)")`); bash
parses it as an assignment to `a` (which, when `a` is already readonly, then
errors `a: readonly variable` — matching Root C's form). bash: a `name=value`
argument to a declaration builtin is an assignment even when quoted, as long as
`name` is a valid identifier.

**Fix:** the executor's declaration-builtin arg construction must treat an arg
of the form `<valid-ident>=<value>` as `DeclArg::Assign` even when the word was
quoted (currently a quoted word becomes `Plain`). Scope carefully: only when
the part left of the first `=` is a valid identifier; otherwise stays `Plain`
(bash: `readonly '3x=1'` → not a valid identifier). Verify the quoted-value
semantics match bash (`readonly 'c=(3)'` with `c` a pre-existing array →
`declare -ar c=([0]="(3)")` — the quoted `(3)` is a literal scalar value
assigned to element 0, not an array literal).

## Verification

- **Official `attr` runner** produces zero diff (the flip signal; confirm huck
  output == `attr.right`).
- **Diff-check harness** `attr_diff_check.sh` (no external helper): Root A
  (`readonly -a x=(1 2); declare -p x`; `readonly -a`+`-A`); Root C
  (`readonly x=1; readonly x=2`); Root D (`readonly 'a=(3)'` after `readonly a`);
  regressions (`readonly -A`, plain `readonly r=(7)`, invalid-option still
  errors with prefix, `readonly 3x` invalid identifier).
- **Unit/integration tests** for `readonly -a` (attribute + array value +
  readonly), the error-prefix change, and the quoted-assignment parse.
- **No-regression:** full bash-suite runner PASS **36 → 37**, branch PASS-set
  diffed against the v348 baseline (exactly the 36 + `attr`; Root D touches the
  shared declaration-arg construction — verify declare/export/local/readonly
  categories explicitly); `run_diff_checks.sh` green; the
  declare/readonly/export `-p huck` integration bins.

## Scope / non-goals

- Only the `readonly` roots above. Do not touch `declare`/`export` semantics
  beyond the shared DeclArg-construction change Root D needs (and verify it
  doesn't regress them).

## Summary of touched files

- `crates/huck-engine/src/builtins.rs` — `builtin_readonly_decl` (Root A `-a`,
  Root C error prefix).
- The executor's declaration-builtin arg construction — Root D quoted
  `name=value` → `DeclArg::Assign`.
- `tests/scripts/attr_diff_check.sh` (new).
