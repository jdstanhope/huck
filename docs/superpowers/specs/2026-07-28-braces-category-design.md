# v341 — flip the `braces` bash-suite category

**Issues:** [#44 — brace expansion ordering vs parameters](https://github.com/jdstanhope/huck/issues/44)
(Root 1) and [#318 — negative step / nested non-comma / backslash char-range](https://github.com/jdstanhope/huck/issues/318)
(Roots 2–4).

**Goal:** flip the bash 5.2.21 test-suite `braces` category from FAIL to a
byte-identical (0-diff) PASS, raising the runner's PASS count 29 → 30.

## Background

The `braces` category FAILs with a 34-line diff. Every diff line is one of
**four** brace-expansion divergences, all in `crates/huck-syntax/src/brace_expand.rs`
plus the brace-reconstruction path in `lexer.rs`:

| Diff lines | Root |
|---|---|
| 23 | Root 1 — bare `$var{x,y}` name-merge (#44) |
| 36–37 | Root 2 — backslash in a char range not emptied |
| 44–45 | Root 3 — nested non-comma brace `a-{b{d,e}}-c` not recursed |
| 50–62 | Root 4 — negative step `{10..1..-2}` left literal |

The category needs ALL four. None is fundamentally blocked (Root 1 is a targeted
merge, NOT the full #44 pipeline rearchitecture).

## Root 4 — negative step (`parse_range`)

### Symptom
`{10..1..-2}`, `{z..a..-2}`, `{100..0..-5}`, `{-1..-10..-2}`, `{10..0..-2}` are
left literal; bash expands them (`{10..1..-2}` → `10 8 6 4 2`).

### Cause
`parse_range` (`brace_expand.rs:181`) rejects a negative explicit step in both
the integer arm (~201) and char arm (~270): `_ => return None`.

### Fix
bash ignores the step's SIGN — it uses `|step|` and takes direction from the
endpoints (`{10..1..-2}` and `{10..1..2}` both give `10 8 6 4 2`). In both arms,
replace the step match's `Ok(n) if n > 0 => …` / `_ => return None` with:

```rust
Some(s) => match s.parse::<i64>() {
    Ok(0) => return None,
    Ok(n) => {
        let m = n.abs();
        if r >= l { m } else { -m }
    }
    Err(_) => return None,
},
```

## Root 3 — nested non-comma brace (`expand_into`)

### Symptom
`a-{b{d,e}}-c` is left literal; bash expands the inner `{d,e}` even though the
outer `{b{d,e}}` has no top-level comma → `a-{bd}-c a-{be}-c`. Likewise
`a-{bdef-{g,i}-c` (unbalanced) → `a-{bdef-g-c a-{bdef-i-c`.

### Cause
When `parse_body(body)` returns `None` (outer body isn't a valid brace expr —
no top-level comma/range), `expand_into` (`brace_expand.rs:59-72`) treats the
whole `{body}` as literal WITHOUT recursing into `body` to expand inner braces.

### Fix
In that `None` branch, recurse into `body` for inner braces, wrap each result in
literal `{`/`}`, and cross with the suffix expansion (the prefix is literal — it
precedes the first top-level `{`):

```rust
None => {
    // Outer {body} is not a brace expr (no top-level comma/range) → the braces
    // are LITERAL, but inner braces inside body still expand (bash:
    // `a-{b{d,e}}-c` → `a-{bd}-c a-{be}-c`). Recurse into body and suffix and
    // cross them, re-wrapping body in literal braces. (Do NOT re-feed
    // `{be}` through expand_into — the literal braces would be re-parsed as a
    // top-level brace and, having no comma/range, recurse forever.)
    let mut body_exp = Vec::new();
    expand_into(body, &mut body_exp)?;
    let mut suffix_exp = Vec::new();
    expand_into(suffix, &mut suffix_exp)?;
    for be in &body_exp {
        for se in &suffix_exp {
            out.push(format!("{prefix}{{{be}}}{se}"));
            if out.len() > MAX_ELEMENTS {
                return Err(BraceError::TooManyElements);
            }
        }
    }
    return Ok(());
}
```

No regression for a body with no inner braces (`{foo}` → `expand_into("foo")` →
`["foo"]` → `{foo}`, identical to today), or a non-brace body with spaces
(`{a b}` → `{a b}`).

## Root 2 — backslash in char range → empty element (`parse_range` char arm)

### Symptom
`{A..a}` (byte range 0x41–0x61, spanning `\` at 0x5C): huck emits a literal `\`;
bash emits an EMPTY element there (`… Z [ <empty> ] ^ _ ` a`). Verified only the
backslash (0x5C) is emptied — `[`, `]`, `^`, `_`, `` ` `` are all present.

### Cause
The char-range loop (`brace_expand.rs:284-306`) pushes `c.to_string()` for every
character in the byte range, including `\`.

### Fix
In the char-range loop, when the generated character is `\` (0x5C), push an
empty string instead of the backslash:

```rust
if let Some(c) = char::from_u32(cur as u32) {
    if c == '\\' {
        out.push(String::new());
    } else {
        out.push(c.to_string());
    }
} else {
    return None;
}
```

(The existing `is_ascii_alphabetic()` endpoint guard is unchanged — bash only
expands a char range when BOTH endpoints are ASCII letters, e.g. `{A..a}`;
`{X..^}` and `{!..~}` stay literal, matching huck already.)

## Root 1 — bare `$var{x,y}` name-merge (#44, targeted)

### Symptom
`$var{x,y}` (var=baz, varx=vx, vary=vy): bash → `vx vy`; huck → `bazx bazy`.
ONLY the bare `$name` form diverges — `${var}{x,y}` and `"${var}"{x,y}` already
match bash (`bazx bazy`).

### Cause
bash brace-expands TEXTUALLY before parameter expansion: `$var{x,y}` → `$varx
$vary` → param-expand → `vx vy`. huck lexes `$var` into a `WordPart::Var{var}`
and `{x,y}` into a separate `Literal`, brace-expands the parts
(`brace_expand_parts` in `lexer.rs`), and reconstructs `[Var{var}, Literal{"x"}]`
— so `$var` expands to `baz` independently and `x` is appended → `bazx`. Bash
merges `varx` into one name.

Crucially, bare `$var` is `WordPart::Var` while braced `${var}` is a
`WordPart::ParamExpansion` — so the two are structurally distinguishable, and the
fix targets `Var` only (leaving `${var}{x,y}` correct).

### Fix (targeted — no pipeline rearchitecture)
After brace expansion reconstructs a word's `Vec<WordPart>` (the
`split_on_sentinels` results inside `brace_expand_parts`, `lexer.rs:6825`), run a
post-pass: whenever a bare `WordPart::Var{name, quoted:false}` is immediately
followed by an unquoted `WordPart::Literal{text, quoted:false}`, split `text`
into its leading name-continuation run (`[A-Za-z0-9_]*`) and the rest; append the
run to the Var's `name`; drop the literal if the rest is empty, else replace it
with `Literal{rest}`. Applied iteratively so `$a{b,c}{d,e}`-style chains merge
left-to-right.

This fires ONLY in the brace-reconstruction path (a word reaches
`brace_expand_parts` only if it contains an unquoted brace; a bare `$name`
otherwise always lexes its maximal name, so `[Var, name-cont Literal]` only
arises from a brace split), and ONLY for `Var` (bare `$name`), never
`ParamExpansion` (braced `${…}`) — so:
- `$var{x,y}` → `[Var{varx}]`, `[Var{vary}]` → `vx vy` ✓
- `${var}{x,y}` → `[ParamExpansion{var}, Literal{"x"}]` (untouched) → `bazx bazy` ✓
- `$var{-,+}` → `[Var{var}, Literal{"-"}]` (`-` not name-cont, no merge) → `baz-`/`baz+` ✓

### Scope note
This is the narrow, category-flipping slice of #44. The general
brace-before-param ordering (e.g. re-lexing arbitrary brace output) is NOT
attempted; #44 stays open for the broader cases if any surface.

## Verification

- Extend `tests/scripts/brace_expansion_diff_check.sh` with cases for all four
  roots: negative step (int + char, ascending/descending), nested non-comma
  (`a-{b{d,e}}-c`, deeper nesting), backslash char range (`{A..a}`, `{Z..a}`),
  and bare-`$name` merge (`$var{x,y}` vs `${var}{x,y}` vs `"${var}"{x,y}` vs
  `$var{-,+}`). Byte-identical vs bash.
- Official runner: `HUCK_BASH_TEST_CATEGORY=braces bash
  tests/bash-test-suite/runner.sh` → 0-diff PASS.
- **No-regression via a main-worktree baseline** (Root 1 touches word
  reconstruction; Roots 2–4 touch sequence expansion): build `origin/main`, run
  the brace/expansion categories (`braces` itself, plus `dollars`, `more-exp`,
  `new-exp`, `exp-tests`, `array`, `assoc`) on both binaries and confirm no
  category's output regresses.
- Full `tests/scripts/run_diff_checks.sh` sweep green (esp. `brace_*`,
  `param_*`, `array_*`).
- Per-crate `cargo test` for `huck-syntax` (brace_expand unit tests + lexer
  reconstruction) and `huck-engine`, plus the `-p huck` `brace_expansion` /
  `braced_special_params` / `special_params` integration bins, single-threaded
  under `ulimit -v`.
- Full runner PASS 29 → 30, only `braces` flipped.

## Out of scope / follow-ups

- The broader #44 brace-before-param ordering beyond the bare-`$name` merge.
- Any brace divergence not in this category's diff — open a new issue if found.

## Summary of touched files

- `crates/huck-syntax/src/brace_expand.rs` — `parse_range` (Roots 4 + 2),
  `expand_into` None branch (Root 3).
- `crates/huck-syntax/src/lexer.rs` — `brace_expand_parts` post-pass merge (Root 1).
- `tests/scripts/brace_expansion_diff_check.sh` — harness cases.
- `docs/bash-test-suite-baseline.md` — baseline (PASS 29 → 30; `braces` row).
- Memory: `project_huck_iterations.md` + `MEMORY.md`.
