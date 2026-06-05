# huck v96 — `${var@OP}` parameter transforms (scalar subset) Design

**Status:** approved design, ready for implementation plan.
**Implements:** the scalar `${var@OP}` parameter-transformation operators —
`@P` (prompt expand), `@Q` (shell-quote), `@U` (uppercase), `@L` (lowercase),
`@u` (uppercase first char), `@E` (expand backslash escapes). Today huck rejects
any `${var@...}` with `syntax error: invalid parameter-expansion modifier: @`.
**Primary driver:** **oh-my-posh** uses `${prompt@P}` / `${_omp_secondary_prompt@P}`
— the top remaining `~/.bashrc` block (lines 69/96 + their parse cascades).
**Defers:** the array/attribute forms `@A` / `@K` / `@k` / `@a` (assignment-form,
key-value, attribute flags) — need array/`declare` plumbing and are unused by the
bashrc.
**Closes:** **M-86** (`[deferred]` → `[fixed v96]`, scalar subset; `@A`/`@K`/`@k`/`@a`
remain a logged follow-on).
**Branch (impl):** `v96-param-transform`.

## Verified bash 5.2 semantics (the contract)

- `v='a b'; "${v@Q}"` → `'a b'` (shell-quoted, reusable as input; `'…'` for
  simple strings, `$'…'` when special chars require it).
- `v=hello; "${v@U}"` → `HELLO`; `v=HeLLo; "${v@L}"` → `hello`;
  `v=hello; "${v@u}"` → `Hello` (only the first char).
- `v='a\tb'; "${v@E}"` → `a<TAB>b` (backslash escapes expanded as `$'…'` would).
- `v='[\u]'; "${v@P}"` → `[<username>]` (prompt expansion of the value).
- `v=x; "${v@Z}"` (unknown operator) → `bash: ${v@Z}: bad substitution`, rc 1.
- Empty/unset `v`: `@P`/`@Q`/`@E`/case operators apply to the empty string
  (`@Q` of empty → `''`; others → empty).

## Section 1 — Lexer + AST (`src/lexer.rs`)

Add a `Some('@')` arm to `dispatch_braced_modifier` (`src/lexer.rs:~2190`, beside
the `#`/`%`/`/`/`^`/`,` arms): consume `@`, read the single operator letter, and
push `ParamModifier::Transform { op }`. Define:

```rust
pub enum TransformOp { PromptExpand, Quote, Upper, Lower, UpperFirst, EscapeExpand }
// ParamModifier gains:
Transform { op: TransformOp },
```

- Letter → op map: `P`→PromptExpand, `Q`→Quote, `U`→Upper, `L`→Lower,
  `u`→UpperFirst, `E`→EscapeExpand.
- **Unknown letter** (`@Z`, `@A`/`@K`/`@k`/`@a` for now) → a lex error that the
  message renderer surfaces as a `bad substitution`-class diagnostic (bash:
  `${v@Z}: bad substitution`). Reuse/extend the existing `InvalidBraceModifier`
  path or add a dedicated variant — the implementer picks whichever yields a
  clean message; exact text need not byte-match bash (error path).
- The `${!name@P}` indirect-then-transform combination is out of scope (rare);
  the `@` arm is reached on the normal (non-indirect) path. If `indirect` is set,
  current behavior is unchanged. (Note for the implementer: confirm the `@` arm
  threads the existing `name/quoted/subscript/indirect` through like its
  siblings.)

## Section 2 — Eval (`src/param_expansion.rs`)

A `ParamModifier::Transform { op }` arm computes the expanded scalar value of the
parameter (same lookup the `Case`/`Length` arms use — honoring `subscript` for
`${a[i]@U}`), then applies `op`, reusing existing huck helpers:

- **`PromptExpand` (`@P`)** → `crate::prompt::expand_prompt(&value, shell)`.
  Note: `expand_prompt` processes the PS1 backslash escapes (`\u`,`\h`,`\w`,
  `\[`,`\]`,`\n`,…) and `$VAR`. Bash gates `$VAR`/command-substitution on the
  `promptvars` shopt (default on); oh-my-posh sets `shopt -u promptvars` first.
  v96 reuses `expand_prompt` as-is (matches bash's *default* promptvars-on
  behavior); the promptvars-off suppression of `$VAR` in `@P` is a documented
  low-impact sub-divergence (oh-my-posh's value is pre-rendered ANSI with no
  `$VAR`, so unaffected).
- **`Quote` (`@Q`)** → shell-quote the value. Reuse `escape_alias_value`
  (`src/builtins.rs:5052`) or a shared quoter; the plan verifies byte-match with
  bash's `@Q` (simple → `'…'`). If `escape_alias_value`'s output diverges for
  some inputs, factor a small `shell_quote` helper that matches bash.
- **`Upper` (`@U`)** → `case_modify(value, CaseDirection::Upper, /*all*/ true, None, …)`.
- **`Lower` (`@L`)** → `case_modify(value, CaseDirection::Lower, /*all*/ true, …)`.
- **`UpperFirst` (`@u`)** → `case_modify(value, CaseDirection::Upper, /*all*/ false, …)`.
  (Confirm `case_modify`'s signature at `src/param_expansion.rs:424`;
  `(Upper,true)`→`HELLO`, `(Upper,false)`→`Hello`, `(Lower,true)`→`hello` are
  already unit-tested there.)
- **`EscapeExpand` (`@E`)** → decode backslash escapes exactly as `$'…'` does;
  reuse the ANSI-C escape decoder from the v39 M-28 work (the `$'…'` lexer path —
  `decode_ansi_c_escape` / `read_ansi_c_quoted`; the implementer locates the
  reusable entry point, factoring a `decode_ansi_c_escapes(&str) -> String`
  helper if the existing one is iterator-bound).

The result returns through the normal `ExpansionResult::Value` path (quoting/
field-splitting handled by the caller exactly as for other modifiers).

## Section 3 — Wiring

Both `WordPart::ParamExpansion` eval arms in `src/expand.rs` already route a
non-subscript modifier to `crate::param_expansion::expand_modifier`; the new
`Transform` arm lives inside `expand_modifier` (and `expand_array_param` for
`${a[i]@U}`), so no `src/expand.rs` dispatch change is needed beyond what the
compiler flags. The `@` arm in the lexer is the only new construction site.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/lexer.rs` | `TransformOp` enum; `ParamModifier::Transform`; `Some('@')` arm in `dispatch_braced_modifier`; unknown-operator → bad-substitution error |
| `src/param_expansion.rs` | `Transform { op }` eval arm reusing `expand_prompt`/`escape_alias_value`/`case_modify`/ANSI-C decode |
| `src/builtins.rs` or a shared util | `@E` decoder helper if one must be factored from the `$'…'` path; `@Q` quoter if `escape_alias_value` needs aligning |
| `src/shell.rs` | message rendering for the new unknown-operator error variant, if added |
| `tests/param_transform_integration.rs` | NEW — `@P`/`@Q`/`@U`/`@L`/`@u`/`@E` + unknown-op error |
| `tests/scripts/param_transform_diff_check.sh` | NEW — 21st bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-86 `[fixed v96]` (scalar subset; `@A`/`@K`/`@k`/`@a` deferred); promptvars-off `@P` sub-divergence note; changelog; README row |

## Testing

1. **Unit** (lexer): `${v@P}`/`${v@Q}`/`${v@U}`/`${v@L}`/`${v@u}`/`${v@E}` parse to
   `Transform { op: … }`; `${v@Z}` → error.
2. **Unit** (param_expansion): each op against known inputs (`@U` hello→HELLO,
   `@u` hello→Hello, `@L` HeLLo→hello, `@Q` `a b`→`'a b'`, `@E` `a\tb`→`a\tb`,
   `@P` `[\u]`→`[user]`).
3. **Integration** (`tests/param_transform_integration.rs`): the six operators via
   the binary; unknown operator → nonzero/stderr; empty var (`@Q`→`''`).
4. **bash-diff harness** `tests/scripts/param_transform_diff_check.sh` (21st):
   `v=hello; echo "${v@U}" "${v@u}"`; `v=HeLLo; echo "${v@L}"`;
   `v='a b'; echo "${v@Q}"`; `v='a\tb'; echo "${v@E}"`; (and a `@P` fragment whose
   prompt-expanded output is deterministic across bash/huck — e.g.
   `v='x\ny'; echo "${v@P}"` if `\n` expands identically; pick fragments whose
   `@P`/`@Q` output is environment-independent so they byte-match). Investigate
   any divergence; bash is the oracle.
5. **Regression**: existing `${var^^}`/`${var,,}` Case + all param-expansion tests
   pass (the `Case` modifier is unchanged; `Transform` is additive).
6. **End-to-end**: sourcing the oh-my-posh init no longer emits the
   `invalid parameter-expansion modifier: @` errors (lines 69/96) or their `}`/
   `fi` cascades (manual note in the changelog).

## Edge cases & notes

- **`@Q` quoting form**: bash uses `'…'` for simple strings and `$'…'` for strings
  containing newlines/control chars/single quotes. The plan verifies the chosen
  quoter matches bash for the harness inputs; complex-quoting parity (`$'…'` form)
  beyond the tested cases is best-effort and noted if it diverges.
- **`@P` promptvars-off**: documented low-impact sub-divergence (see §2).
- **`@E` vs `$'…'`**: should produce identical escape decoding (same helper).
  Unknown escapes follow the `$'…'` rule (backslash + char preserved).
- **Subscripted `${a[i]@U}`** works via the array-param path; `${a[@]@Q}`
  (transform over a whole array) is part of the deferred array-aware set — only
  the scalar/`[i]` path is in scope.
- **No regression**: `Transform` is a new additive `ParamModifier` variant; every
  existing modifier path is unchanged.
