# v340 — flip `nquote2` + `nquote3` via positional `${@<op>}` per-element transforms

**Issue:** [#314 — positional `${@<op>}`/`${*<op>}` transforms apply to only the first param](https://github.com/jdstanhope/huck/issues/314)
(the per-element-transform half of L-88; the default-op half `${@-word}` is #26, out of scope).

**Goal:** flip BOTH bash 5.2.21 test-suite categories `nquote2` and `nquote3`
from FAIL to byte-identical (0-diff) PASS — a **double flip** — raising the
runner's PASS count 27 → 29.

## Background

`nquote2` (23-line diff) and `nquote3` (19-line diff) both FAIL against huck.
Their **entire** diffs are the same single root: positional-parameter transforms
`${@<op>}` / `${*<op>}` apply `<op>` to only the FIRST positional parameter, and
the quoted `"${@<op>}"` form joins all params into one word. The array form
`${arr[@]<op>}` already works.

The `$'\001'` (Ctrl-A) bytes throughout the tests are just the test data, not the
cause. The bug reproduces on plain data:

```
set aXa bXb cXc
${@/X/-}     → huck: <a-a> <bXb> <cXc>     bash: <a-a> <b-b> <c-c>   (unquoted)
"${@/X/-}"   → huck: <a-a bXb cXc>         bash: <a-a> <b-b> <c-c>   (quoted)
${arr[@]/X/-}→ huck: <a-a> <b-b> <c-c>     bash: <a-a> <b-b> <c-c>   (array OK)
```

Diff-hunk → command mapping (verified every hunk):
- `nquote2`: `${@/$'\001'/A}`, `"${@/…}"`, `${@//$'\001'/A}`, `${@//w/W}` (test lines 45–53).
- `nquote3`: `${@%$'\001'*}`, `"${@%…}"`, `${@#*$'\001'}`, `${@##*…}` (test lines 84–89).

All are per-element **transform** ops (pattern-removal / substitution). No hunk is
a `${@:-word}` default-op case (that half of L-88 is #26, not exercised here).

## Cause

`expand_modifier_with_value` (`crates/huck-engine/src/param_expansion.rs`) has an
`is_star_at` path (`name == "*" || name == "@"` under `ParamLookup::Scalar`,
line ~105) that resolves `$@`/`$*` to the **IFS-joined** positional string and
applies the modifier once to that scalar. The dispatch in `expand.rs`
(`WordPart::ParamExpansion`, ~line 1258) routes `$@`/`$*` + a non-substring
modifier to this scalar path via `expand_modifier_quoted`:

```rust
} else if matches!((name.as_str(), modifier),
                   ("@" | "*", ParamModifier::Substring { .. })) {
    expand_positional_substring(name, modifier, *quoted, shell)
} else {
    crate::param_expansion::expand_modifier_quoted(name, modifier, *quoted, shell)  // <- scalar; the bug
}
```

The array path (`expand_array_param`, `expand.rs:781`) instead loops over each
element applying `scalar_apply_per_element(name, modifier, element, quoted,
shell)` and returns `ExpansionResult::WordList(words)`, which the caller renders
with correct word boundaries (quoted `@` → separate words; unquoted → IFS-split
each; quoted `*` → join with IFS[0]).

## Fix

Add ONE branch to the `expand.rs:1258` dispatch, before the scalar fallback:
when `name` is `@` or `*`, there is no `subscript`, it is not `indirect`, and
`is_per_element_modifier(modifier)` is true, map the modifier over the positional
parameters and return a `WordList`:

```rust
} else if (name == "@" || name == "*") && is_per_element_modifier(modifier) {
    expand_positional_transform(name, modifier, *quoted, shell)
}
```

New helper — **mirrors the array per-element transform arm verbatim**
(`expand_array_param`, `expand.rs:996-1009`), which returns `WordList` ONLY for
`[@]`+quoted and `Value(join)` otherwise. The `@`↔`[@]` and `*`↔`[*]` mapping:

```rust
/// `${@<op>}` / `${*<op>}` with a per-element transform op: apply `op` to EACH
/// positional parameter (like `${arr[@]<op>}`). Result-shape mirrors the array
/// arm exactly: quoted `@` → WordList (separate words); every other case →
/// Value(IFS[0]-join) (quoted `*` → one field; unquoted `@`/`*` → caller
/// IFS-splits). v340 (#314).
fn expand_positional_transform(
    name: &str,
    modifier: &crate::lexer::ParamModifier,
    quoted: bool,
    shell: &mut Shell,
) -> ExpansionResult {
    let args = shell.positional_args.clone();
    let transformed: Vec<String> = args
        .iter()
        .map(|a| scalar_apply_per_element(name, modifier, a, quoted, shell))
        .collect();
    if name == "@" && quoted {
        ExpansionResult::WordList(transformed)
    } else {
        let sep = ifs_join_sep(&shell.ifs());
        ExpansionResult::Value(transformed.join(&sep))
    }
}
```

The `WordList` result (quoted `@`) is rendered as separate words by the existing
arm at `expand.rs:1290`; the `Value` results (all other cases) are IFS-split
(unquoted) or emitted as one word (quoted `*`) by the existing `Value` arm at
`expand.rs:1271` — identical to how the array path's results flow. Under the
default IFS (space/tab/newline) the nquote2/3 data (Ctrl-A bytes, not IFS chars)
survives the unquoted join→split round-trip unchanged.

## Scope / non-goals

- Fires ONLY for `$@`/`$*` with a per-element transform modifier
  (`is_per_element_modifier`: `RemovePrefix`/`RemoveSuffix`/`Substitute`/`Case`
  and the per-element `Transform` ops). Bare `$@`/`$*` (`AllArgs`), `${@}`
  (`ParamModifier::None`), `${@:-word}` (default/alt ops — L-88 other half, #26),
  and `${@:o:l}` (substring, already routed to `expand_positional_substring`) are
  untouched.
- `${@:-word}`-family per-element field-splitting stays deferred (#26).

## Verification

- Extend a param-expansion diff-check harness (e.g.
  `tests/scripts/param_transform_diff_check.sh` or `param_substitution_diff_check.sh`
  — pick the closest existing one, else add `positional_transform_diff_check.sh`)
  with `${@<op>}`/`${*<op>}` cases: quoted AND unquoted; each transform op class
  (`#`,`##`,`%`,`%%`,`/`,`//`,`^`,`^^`,`,`,`,,`,`@Q`); multi-param; empty
  positional list; a param containing IFS chars; and Ctrl-A data (the nquote
  scenario). Compare byte-identically vs bash.
- Official runners: `HUCK_BASH_TEST_CATEGORY=nquote2` and `=nquote3` must each
  report 0-diff PASS.
- **No-regression via a main-worktree baseline** (v334 lesson): build
  `origin/main` in a `git worktree`, run the big expansion categories
  (`more-exp`, `new-exp`, `exp-tests`, `array`, `array2`, `dollars`, `posixexp`)
  on both binaries, and confirm huck's output for them is unchanged by this fix
  (byte-identical old-vs-new), so the WordList reroute is provably
  behavior-preserving outside the `${@<op>}` path.
- Full `tests/scripts/run_diff_checks.sh` sweep green (esp. `param_*`,
  `array_*`, `ifs_*` harnesses).
- Per-crate `cargo test` for `huck-engine` + the `param_*` / `special_params` /
  `array*` / `ifs` `-p huck` integration bins, single-threaded under `ulimit -v`.
- Full runner PASS 27 → 29 with ONLY `nquote2` + `nquote3` flipped (regression
  scan vs the saved baseline).

## Summary of touched files

- `crates/huck-engine/src/expand.rs` — new dispatch branch + `expand_positional_transform`.
- `tests/scripts/…_diff_check.sh` — positional `${@<op>}` harness cases.
- `docs/bash-test-suite-baseline.md` — baseline update (PASS 27 → 29; nquote2/nquote3 rows).
- Memory: `project_huck_iterations.md` + `MEMORY.md`.
