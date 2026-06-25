# v222 — function-def reconstruction fidelity (nested `function` keyword + redirected brace-body)

## Status

Design approved 2026-06-25. Two related `declare -f` / `type` reconstruction
fidelity fixes, both entirely within `crates/huck-syntax/src/generate.rs`. No
parser/AST changes.

## Background

L-61(c) lists two `func` bash-test-category reconstruction blockers. Re-measuring
against bash 5.2.21 corrected the first one's description:

### Blocker 1 — nested function defs render with a `function` keyword

bash renders EVERY nested function definition (a function-def command appearing
inside another function/command body) as `function NAME () `, regardless of how
it was written in source. All three forms render identically:

```
function f3() { … }   ┐
function f3   { … }   ├─ bash declare -f outer →   function f3 () \n{ … }
f3() { … }            ┘   (the keyword and the `()` are ALWAYS added)
```

The OUTER named function — the one `declare -f NAME` / `type NAME` reconstructs —
renders WITHOUT the keyword (`NAME () `). `type` uses the same rule as `declare -f`.

This is NOT "preserve the source keyword": a paren-form `p4() { … }` nested in a
body still becomes `function p4 () `. So no parser flag / AST field is needed —
the distinction is purely outer-vs-nested at render time.

huck currently drops the keyword everywhere, so nested defs render as `f3 () `.

### Blocker 2 — redirected brace-body is double-wrapped

A function whose body is a brace group with a redirect (`f() { …; } 1>&2`) is
parsed as `FunctionDef { body: Redirected { inner: BraceGroup(seq), redirects } }`
(confirmed: `parse_function_def` → `parse_command` → `attach_redirects`). bash
uses that brace group AS the function's braces and hoists its redirect to the
function's closing brace:

```
f () \n{ \n    … \n} 1>&2
```

huck's `Command::FunctionDef` arm only unwraps a bare `BraceGroup` body; a
`Redirected { BraceGroup }` falls through to the generic `other` arm and is
rendered as a single nested command, double-wrapping:

```
f () \n{ \n    { \n        … \n    } 1>&2\n}
```

The hoist applies ONLY to brace-group bodies. A subshell body with a redirect
(`funcc() ( … ) 2>&1`) is NOT hoisted — bash keeps `{ ( … ) 2>&1 }` (verified).

## Goals

1. Nested function-def commands reconstruct as `function NAME () ` (keyword + `()`
   always), for all three source forms, in `declare -f` / `type` output.
2. The outer named function keeps `NAME () ` (no keyword).
3. A brace-group function body with a redirect reconstructs with the body unwrapped
   into the function braces and the redirect hoisted after the closing `}`
   (`} 1>&2`), for both the outer function and nested defs.
4. A non-brace-group body (subshell, simple command) is unchanged: wrapped in fresh
   `{ }` with any redirect kept inside.

## Non-goals / Out of scope

- The other three `func` blockers (L-61 a/b/d: `declare -xF`/`-xf` export filter,
  `FUNCNAME` assignment protection, `FUNCNEST` enforcement). func will NOT flip to
  PASS this iteration — these two fixes clear only L-61(c).
- Multi-redirect best-effort: a brace body with multiple/`fd>2` redirects is
  hoisted through the existing `slots_for_simple_path` 0/1/2 collapse (same
  best-effort as every other reconstruction path; not widened here).
- No parser or AST changes. No POSIX-mode special-casing.

## Design

All changes in `crates/huck-syntax/src/generate.rs`.

### Shared helper

Extract the function-def rendering into one helper carrying the keyword flag and
the unwrap+hoist logic:

```rust
fn render_function_def(name: &str, body: &Command, indent: usize, with_keyword: bool) -> String {
    let kw = if with_keyword { "function " } else { "" };
    // Peel a redirect wrapper around a brace-group body: the brace group becomes
    // the function's braces and its redirects hoist to the closing `}`.
    let (group_seq, hoisted): (Sequence, String) = match body {
        Command::BraceGroup(seq) => (*seq.clone(), String::new()),
        Command::Redirected { inner, redirects } if matches!(inner.as_ref(), Command::BraceGroup(_)) => {
            let Command::BraceGroup(seq) = inner.as_ref() else { unreachable!() };
            (*seq.clone(), render_hoisted_redirects(redirects))   // e.g. " 1>&2"
        }
        other => (Sequence { first: other.clone(), rest: Vec::new(), background: false }, String::new()),
    };
    format!(
        "{kw}{name} () \n{p}{{ \n{}{p}}}{hoisted}",
        group_body(&group_seq, indent + 1),
        p = pad(indent),
    )
}
```

`render_hoisted_redirects` reuses the existing `slots_for_simple_path` +
`redirect_to_source` machinery already used by the `Command::Redirected` arm
(lines 88-101), emitting a leading space before each slot redirect.

### Wiring

- `function_to_source(name, body)` (the `declare -f`/`type`/`export` entry point,
  line 18) → call `render_function_def(name, body, 0, false)` directly (stop
  building a `Command::FunctionDef` and recursing through the arm).
- `Command::FunctionDef { name, body }` arm in `command_to_source` (line 59) →
  `render_function_def(name, body, indent, true)`. Only nested defs reach here.

`exported_function_value` (body-only, line 24) is unaffected.

## Testing / Verification

- Unit tests in `generate.rs`:
  - outer named function → `NAME () ` (no keyword), all three definition forms.
  - nested def (built as a `FunctionDef` inside an outer body) → `function NAME () `
    for all three forms (`function f3()`, `function f3`, `f3()`).
  - redirected brace body (`Redirected{BraceGroup,[1>&2]}`) on the OUTER function →
    `} 1>&2` hoist, no inner `{ }`.
  - redirected brace body on a NESTED def → `function NAME () \n{ … } 1>&2`.
  - subshell body with redirect → NOT hoisted (`{ ( … ) 2>&1 }`).
- Extend bash diff harnesses (`declare_f_diff_check.sh` and/or
  `function_keyword_diff_check.sh`) with: an outer+nested mix, each nested form,
  a redirected brace body (outer and nested), and a subshell-with-redirect guard
  — all byte-identical to live bash 5.2.21.
- `cargo test --workspace` green (~3692).
- Re-run the `func` category: confirm its diff SHRANK (the `function f3`/`f5`
  keyword hunks and the `} 1>&2` brace hunks are gone) and that NO currently-PASS
  category (esp. `cprint`) regresses. Record whether any category incidentally
  flips (not predicted).

## Risks

- **Outer/nested split must be exact.** Accidentally adding `function ` to the
  outer named function would break every `declare -f`/`type`/`export` consumer.
  The split is structural (entry point vs `command_to_source` recursion) and
  covered by the no-keyword outer unit tests + the full declare_f harness.
- **Hoist scope.** Restrict the redirect-hoist to `Redirected { BraceGroup }`;
  leave subshell/other bodies untouched (guarded by the subshell-redirect test).

## Divergence-doc bookkeeping (on merge)

- Update `docs/bash-divergences.md`: remove blocker (c) from L-61 (resolved);
  L-59(b) (the brace-group-redirect reconstruction gap) is also resolved — remove
  it, leaving L-59 scoped to the arith-for empty-`for ((` section (a). Note the
  two remaining func blockers (a/b/d minus c) in L-61.
- Update the iteration memory.
