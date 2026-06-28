# v234: `${…}` indirect-with-subscript-modifier + extquote name — Design

**Status:** approved (brainstorm 2026-06-28)
**Closes:** M-148 (the two remaining `${…}` parse residuals from v233) — both parts
plus the related M3-combo (`${!arr[@]@OP}`) message divergence.

## Goal

Close the two remaining `${…}` parse gaps so they **parse and behave like bash**
(evaluate or error at runtime) instead of aborting with `syntax error:
unterminated '${...}'`. This extends v233's "scan-to-`}` then evaluate/defer at
runtime" model to the last two `${…}` forms huck still parse-rejects.

Two independent features:

- **Feature 1 — `${!name[sub]<modifier>}`**: indirect expansion whose target has
  a subscript AND a trailing modifier.
- **Feature 2 — `$'…'` as the name inside `${…}`** (bash `extquote`): the ANSI-C
  quote form decodes into the parameter name.

## Background: measured bash 5.2.21 ground truth

### Feature 1 — the keys-vs-indirect distinction

`${!name[@]}` / `${!name[*]}` with **nothing after `]`** is the **array-keys
operator** (indices/keys). With a **trailing operator** it is instead **indirect
expansion** through `${name[sub]}`'s value, then the operator:

| input (`v=arr; arr=(aa bb)` unless noted)        | bash               | note |
|--------------------------------------------------|--------------------|------|
| `${!v[@]}`                                        | `0`                | keys of `v` (scalar = degenerate array, single key 0) |
| `${!arr[@]}`                                      | `0 1 2`            | keys of a real array |
| `${!v[@]%b}`                                      | `aa`               | indirect `v[@]`→"arr"→`arr[0]`="aa", then `%b` |
| `${!v[@]@Q}`                                      | `'aa'`             | indirect + `@Q` transform |
| `${!arr[@]%b}` (real array)                       | `aa bb cb: invalid variable name` (rc 1) | `arr[@]` values joined → invalid name |
| `${!v%b}` (no subscript)                          | `aa`               | **already works in huck** |

The presence of a trailing operator flips the interpretation from keys to
indirect. huck currently: `${!v[@]%b}` → `unterminated '${...}'`; `${!v[@]@Q}` →
(v233) a wrong `bad substitution`.

### Feature 2 — `$'…'` as the name (extquote)

| input                                  | bash                              | note |
|----------------------------------------|-----------------------------------|------|
| `x1=not; ${$'x1'}`                      | `not`                             | `$'x1'`→name "x1" |
| `ab=Z; ${a$'b'}`                        | `Z`                               | concatenation "a"+"b"="ab" |
| `${$"x1"}`                              | `${"x1"}: bad substitution`       | locale `$"…"` is NOT a valid name source |
| `${$'x\ty'}`                            | `${x\ty}: bad substitution`       | decoded name invalid (contains TAB); message shows DECODED form |
| `x=foo; ${x#$'f'}`                      | `oo`                              | modifier OPERAND `$'…'` — **already works** (word expansion) |
| `declare -f` of `${$'x1'}`             | reconstructs as `${x1}`           | bash **normalizes** the decoded name |

So: only `$'…'` (ANSI-C) decodes to a name; `$"…"` in name position bad-substs.
bash normalizes the name in `declare -f`, which means a **lex-time decode** is both
the simplest implementation and the reconstruction-faithful one.

## Architecture

All parsing lives in `crates/huck-syntax/src/lexer.rs`
(`scan_braced_param_expansion`, its `${!…}` branch, the brace-body name scan, and
`dispatch_braced_modifier`). Evaluation reuses
`crates/huck-engine/src/expand.rs::expand_indirect` (which **already** accepts
`subscript` + `modifier` and computes a through-value) and the existing
`param_expansion.rs` scalar modifier evaluator. Reconstruction in
`generate.rs`/`expand.rs::reconstruct_param_expansion` needs no new arm (the AST
shapes are existing variants). No new `ParamModifier` variant is introduced.

ANSI-C decoding reuses `crates/huck-syntax/src/lexer.rs::decode_ansi_c_escapes`
(public).

### Feature 1 — lexer change

In the `${!…}` branch of `scan_braced_param_expansion`, after scanning
`name[sub]` (`scan_param_subscript` returns the `SubscriptKind`), decide on the
char following `]`:

- `}` → **array-keys operator** (`IndirectKeys` for `[@]`/`[*]`; existing
  scalar-indirect for `[n]`) — UNCHANGED.
- a modifier introducer (`%` `#` `:` `/` `^` `,` `@`) → call the existing
  `dispatch_braced_modifier` with `indirect = true` and `subscript = Some(sub)`,
  emitting a normal `ParamExpansion { name, subscript: Some(sub), modifier,
  indirect: true, quoted }`.
- anything else → `recover_bad_subst` (defer to runtime; replaces today's
  `UnterminatedBrace` for this position).

This retires v233's `recover_bad_subst` routing of the `[@]`-then-`@` combo
(`${!arr[@]@OP}`) — it now parses as indirect+subscript+transform.

### Feature 1 — engine

Expected to be **no-change**: `expand_indirect` already computes
`through = scalar value of (name, sub)` then applies the modifier. For a real
multi-element array the through-value is the IFS-joined values → the existing
v233 `"<value>: invalid variable name"` fatal path fires (matching bash). For the
scalar-degenerate case (`v=arr` → `v[@]`="arr" → `arr[0]`="aa") the modifier
applies to "aa". The plan must VERIFY this with integration tests and only touch
the engine if a case diverges.

**In-scope cleanup:** the existing `invalid variable name` runtime error in
`expand_indirect` uses the bare `huck:` prologue; bash uses
`script: line N: <value>: invalid variable name`. Align it to
`shell.error_prefix(None)` while here (small, removes a divergence). If aligning
proves to ripple, defer it and note the prologue gap instead.

### Feature 2 — lexer change

In the brace-body **name** scan (the path that builds `name` before
subscript/modifier dispatch in `scan_braced_param_expansion`):

- Accumulate the name as a `String`. While scanning name characters, if the
  cursor is at `$'`, scan the full ANSI-C span (escape-aware, embedded `}`/`'`
  safe — reuse the M4 span logic / `scan_ansi_c_quoted`), **decode** the body via
  `decode_ansi_c_escapes`, and append the decoded text to the name. Continue
  accumulating (so `${a$'b'c}` → `abc`).
- If the cursor is at `$"` in name position → `recover_bad_subst` (bash
  bad-substs `$"…"` as a name).
- After the name is assembled, **validate** it with `is_valid_name`. Valid →
  proceed to normal subscript/modifier scanning with the decoded `name`. Invalid
  (e.g. decoded name contains a TAB) → `recover_bad_subst`.

### Feature 2 — engine

None. The decoded name flows through the existing scalar/indirect/modifier paths.
Modifier operands/patterns containing `$'…'`/`$"…"` (e.g. `${x#$'f'}`) are already
handled by normal word expansion and need no change.

### Feature 2 — reconstruction

None. The `name` field holds the decoded `x1`, so `declare -f` / `set -x` emit
`${x1}`, matching bash's normalization. (No source-form round-trip — acceptable
and bash-faithful.)

## Edge cases & interactions

- Single-element-subscript indirect with modifier: `${!v[1]%x}` — same path.
- Substring after `[@]`: `${!v[@]:1:2}` — `:` is a modifier introducer → indirect
  + subscript + substring.
- `$'…'` name with embedded `}`: `${$'a}b'}` — the span scan consumes the inner
  `}`; only the real terminator closes the brace.
- `$'…'` name then modifier: `${$'x1'%$'t'}` — name `x1`, then `%`, operand `$'t'`
  scanned as a normal modifier operand (already decodes).
- Both features touch the same name/subscript scan path; they compose
  (`${!$'arr'[@]%b}`) but exotic combinations are not special-cased.
- Both features REDUCE the set of inputs routed to `BadSubst`; genuinely
  unterminated `${…}` still parse-errors (`UnterminatedBrace`).

## Documented residuals (low, acceptable)

1. For an **invalid decoded name** (`${$'x\ty'}`), huck's `recover_bad_subst`
   captures the raw source `${$'x\ty'}` while bash's message shows the decoded
   form `${x\ty}`. Esoteric (invalid name + escape in name position).
2. Parser-stage error-prologue gaps elsewhere remain part of the separate staged
   error-prologue rollout, not this iteration.

## Testing

- **Lexer unit tests** (`lexer.rs` `mod tests`): `${!v[@]%b}`→
  `ParamExpansion { indirect, subscript: Some(All), modifier: RemoveSuffix }`;
  `${!v[@]@Q}`→indirect+`Transform`; `${!v[@]}`→still `IndirectKeys`;
  `${$'x1'}`→`name == "x1"`; `${a$'b'}`→`name == "ab"`; `${$"x1"}`→`BadSubst`;
  `${$'x\ty'}`→`BadSubst`.
- **Integration tests** (new `tests/param_indirect_extquote_integration.rs`, same
  `run_file` harness as `param_expansion_badsubst_integration.rs`): runtime values
  `${!v[@]%b}`→`aa`, `${!v[@]@Q}`→`'aa'`, real-array `${!arr[@]%b}`→
  `invalid variable name` (rc 1), `${$'x1'}`→value, `${x#${$'x1'%$'t'}}`→`tOK`,
  and `declare -f`→`${x1}`.
- **New diff harness** `tests/scripts/param_indirect_extquote_diff_check.sh` —
  byte-identical bash↔huck across both features (mirrors the existing
  `param_expansion_diff_check.sh` shape).
- **Re-parse gate:** `new-exp13.sub` and `posixexp7.sub` parse past lines 72/58
  (the M-148 lines) under `huck -n`.
- **Full `cargo test --workspace`** green; warning-clean build.
- **Category measurement** (controller, post-review): re-run the affected bash
  test categories (`new-exp`, `posixexp`, `errors`) and record flip/no-flip
  honestly. A flip is not expected; the value is parse robustness.

## Out of scope

- `extquote` as a real `shopt` gate (huck has no `extquote` option; always-on
  matches default bash). POSIX-mode disabling is not modeled.
- Locale translation for `$"…"` (no-op in huck, consistent with the C locale).
- The other M-148-adjacent parse gaps already tracked elsewhere (L-45 same-line
  extglob, L-48, L-53, etc.).
