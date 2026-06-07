# huck v106 — extglob inside command substitutions (M-101) Design

**Status:** approved design, ready for implementation plan.
**Implements:** propagating the lexer's `LexerOptions` (i.e. `extglob`) into every
**recursive body re-tokenization**, so extglob patterns (`!(…)`/`@(…)`/`+(…)`/
`*(…)`/`?(…)`) work inside `$(…)` / `` `…` `` command substitutions, `${…}`
operands, and array-literal elements — not just at top level.
**Why now:** v104/v105 let `~/.bashrc` source far enough that
`bash_completion` line 1232 `local -a svcs=($(printf '%s\n' $xinetddir/!($_backup_glob)))`
and line 1249 fail with `syntax error in command substitution: unexpected token
after command` — huck re-tokenizes the `$(…)` body with extglob **off**, so
`!(…)` lexes as `!` (negation) + `(…)` (subshell).
**Closes:** new bug **M-101** `[fixed v106]` (Tier-1).
**Branch (impl):** `v106-extglob-in-command-sub`.

## Root cause (verified)

Extglob is a `LexerOptions` flag resolved from `shopt extglob` at tokenize time
(`tokenize_with_opts(src, LexerOptions { extglob })`). Top-level extglob works
(v90/v91). But the lexer captures the body of a `$(…)`/`` `…` ``/subscript/array
element as a raw string and **re-tokenizes it with the default `tokenize(body)`
— which is `extglob: false`**. The single command-substitution chokepoint is:

```rust
// src/lexer.rs:1997
fn parse_substitution_body(body: &str) -> Result<Sequence, LexError> {
    let tokens = tokenize(body).map_err(...)?;        // ← extglob LOST here
    let parsed = crate::command::parse(tokens)...?;
    ...
}
```

So `!(…)` inside `$(…)` is mis-lexed and the inner `command::parse` reports
`unexpected token after command`. Verified: `x=$(echo a/!(b))` → huck errors;
bash → `a/!(b)`. Top-level `echo a/!(b)` works in huck. Line 1232 is worse: the
command-sub sits inside an **array literal** `svcs=( $(…) )`, so the path is
`read_array_element_word` → `read_dollar_expansion` → `scan_paren_substitution`
→ `parse_substitution_body` → `tokenize(body)`.

## Fix principle

**Recursive lexing inherits the parent tokenizer's `LexerOptions`.** Thread
`opts: LexerOptions` (it derives `Copy`) from `tokenize_core` down every path that
reaches a recursive `tokenize(...)`, and replace those `tokenize(body)` calls with
`tokenize_with_opts(body, opts)`. Lexer-only; no parser/AST/evaluator change.

## Section 1 — Functions that gain an `opts: LexerOptions` parameter

All are **private to `src/lexer.rs`** (0 external callers), so this is fully
contained. Add `opts: LexerOptions` (last param) to each and pass it through:

| Function | Why | Recursive `tokenize` site to fix |
|----------|-----|----------------------------------|
| `read_dollar_expansion` (1483) | gateway: `$(…)` / `$((…))` / `${…}` | — (passes `opts` onward) |
| `scan_paren_substitution` (1915) | `$(…)` body capture | calls `parse_substitution_body(&body, opts)` |
| `scan_backtick_substitution` (2010) | `` `…` `` body capture | calls `parse_substitution_body(&body, opts)` |
| `parse_substitution_body` (1997) | **the chokepoint** | `tokenize_with_opts(body, opts)` |
| `scan_extglob_group` (986) | calls `read_dollar_expansion` | — |
| `scan_regex_operand` (893, v105) | calls `read_dollar_expansion` | — |
| `scan_expanding_body_line` (1351) | calls `read_dollar_expansion` | — |
| `parse_braced_operand` (1837) | `${…OP…}` operand expansions | (its own re-tokenize, if any) |
| `read_array_element_word` (2391) | array-literal elements (**line 1232**) | the `tokenize(&buf)` at ~2501 → `tokenize_with_opts(&buf, opts)` |
| `parse_subscript_body` (2316) | `[…]` subscript bodies | the `tokenize(src)` at 2317 → `tokenize_with_opts(src, opts)` |

`read_dollar_expansion`'s 12 call sites are inside: `tokenize_core` (has `opts`),
`scan_extglob_group`, `scan_regex_operand`, `scan_expanding_body_line`,
`parse_braced_operand`, and `arith_string_to_word` (see Section 2). Each passes its
own `opts` through.

`tokenize_core` already owns `opts`; update its `read_dollar_expansion(...)`,
`scan_extglob_group(...)`, `scan_regex_operand(...)`, and any other now-`opts`-taking
helper calls to pass `opts`.

## Section 2 — The `arith_string_to_word` boundary (do NOT change its signature)

`arith_string_to_word` (1406) is `pub(crate)` with **3 external callers** (runtime
arith via `expand.rs`). To keep the change contained to `src/lexer.rs`, leave its
signature unchanged and have its two internal `read_dollar_expansion(...)` calls
(1422, 1464) pass `LexerOptions::default()` (extglob off). Consequence: a command
substitution nested *inside* `$(( … ))` arithmetic (e.g. `$(( $(echo !(x)) ))`)
does not get extglob — a negligible edge (extglob inside an arithmetic-nested
command sub is essentially never used). Record as a low `L-` note. Arithmetic
itself does no globbing, so this loses nothing else.

## Section 3 — Not changed

- The parser, AST, evaluator, and `command::parse` — untouched.
- `tokenize` / `tokenize_with_opts` public API — unchanged (we add `opts` only to
  private helpers and route `tokenize(body)` → `tokenize_with_opts(body, opts)`).
- v90/v91 top-level extglob, v105 `=~` regex operands — unaffected (the helpers
  keep their existing behavior; they just forward `opts`).

## Files & responsibilities

| File | Change |
|------|--------|
| `src/lexer.rs` | thread `opts: LexerOptions` through the 10 private helpers in Section 1; `tokenize(body)` → `tokenize_with_opts(body, opts)` at the recursive sites; `arith_string_to_word` passes `LexerOptions::default()` (Section 2) |
| `tests/extglob_command_sub_integration.rs` | NEW — extglob in `$()`/backtick/array-literal/`${}` |
| `tests/scripts/extglob_command_sub_diff_check.sh` | NEW — 31st bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-101 `[fixed v106]`; changelog; README row; arith-nested L-note |

## Testing

1. **Lexer unit tests**: with `LexerOptions { extglob: true }`,
   `tokenize_with_opts("echo $(echo !(x))", opts)` produces a `CommandSub` whose
   inner sequence parsed without error (no stray `Op(LParen)`/negation in the body);
   the same with backtick `` `echo !(x)` ``, and with an array literal
   `a=($(echo !(x)))`. With `extglob: false`, the body still lexes extglob-off
   (unchanged).
2. **Integration / behavior** (vs bash, extglob enabled on a PRIOR line — never the
   same line, since same-line `shopt` isn't active at parse time in either shell):
   - `shopt -s extglob` then `x=$(echo a/!(b)); echo "$x"` → `a/!(b)`.
   - `printf '%s\n' $(echo @(README|LICENSE))`-style with real files in a temp dir.
   - the line-1232 shape: `shopt -s extglob; d=/tmp/somedir; local -a s=($(printf '%s\n' $d/!(x)))` reduced to a runnable form (array literal containing a `$()` with `!(…)`).
   - backtick form `` shopt -s extglob; echo `echo !(x)` ``.
   - a `${var:-$(echo @(a))}`-style default-operand command sub.
   - **Regression**: with extglob OFF, `echo $(echo hi)` and normal command subs
     are byte-unchanged; `$(( 1+1 ))` arithmetic unchanged.
3. **bash-diff harness** `tests/scripts/extglob_command_sub_diff_check.sh` (31st):
   deterministic fragments (create temp files, `cd` to a known dir) byte-identical
   to bash 5.2 — `$()`+extglob, backtick+extglob, array-literal+extglob, and an
   extglob-off control.
4. **Regression**: full suite (2691+), all 30 existing harnesses, and **the payoff**
   — sourcing `/usr/share/bash-completion/bash_completion` no longer errors at
   lines 1232/1249 (report the next gap, if any).

## Edge cases & notes

- **Arith-nested command sub** (Section 2): `$(( $(echo !(x)) ))` keeps extglob off
  for the inner sub — documented low `L-` note.
- **Nullglob/dotglob/etc.** are runtime glob behaviors applied at expansion, not
  lex options, so they are unaffected (and already inherited via the shell). Only
  `extglob` changes *lexing*, which is why only it must be threaded.
- **Same-line `shopt -s extglob; <use>`** stays a (shared-with-bash) limitation —
  not addressed here.
