# v182: backslash-escapes in `${var/pat/repl}` operand splitting — Design

**Status:** approved 2026-06-17
**Iteration:** v182
**Origin:** The parse sweep's "unterminated quote" cluster — the kernel
`scripts/config` file (two header copies, one root cause). Line 209
`V="${V//\\\"/\"}"` is a global substitution whose pattern (`\\\"`) and
replacement (`\"`) contain a backslash-escaped backslash followed by a
backslash-escaped double-quote. huck rejects it with `syntax error:
unterminated quote`; bash parses and runs it (it un-escapes `\"`→`"` in `$V`).

## Problem

`${var/pat/repl}` (and `${var//pat/repl}`, anchored `/#`, `/%`) is scanned in
three phases in `src/lexer.rs`:

1. `scan_braced_operand` (`:2221`) extracts the operand body up to the matching
   `}`, preserving backslash escapes VERBATIM (`\x` → `\x`). Correct.
2. `scan_substitution_operand` (`:3201`) → `split_substitution_body` →
   `split_modifier_operand(body, '/')` (`:3221`) splits the body into
   `(pattern_src, replacement_src)` on the first top-level `/`.
3. `parse_braced_operand_opts` (`:2314`) parses each segment into a `Word`,
   doing the real un-escaping (`\x` → literal `x`).

`split_modifier_operand`'s backslash handling (`:3229-3242`) is wrong:

```rust
'\\' => {
    let dst = if delim_seen { &mut second } else { &mut first };
    match chars.peek().copied() {
        Some(d) if d == delim => { chars.next(); dst.push(delim); }   // \delim → delim
        Some('\\')            => { chars.next(); dst.push('\\'); }     // \\ → \
        _                     => dst.push('\\'),                       // \x → push '\', LEAVE x
    }
}
```

Two defects compound:

- **The `_` arm doesn't consume the escaped char.** For `\"` it pushes `\` and
  leaves the `"`, which the next iteration treats as a quote opener (`:3253`),
  scanning forward and SWALLOWING the `/` delimiter into an unterminated quoted
  span. The split then reports "no delimiter" and the whole body lands in
  `pattern_src` with an unbalanced `"`.
- **The `\\`→`\` un-escape collapses the backslash count.** Even consuming the
  `"` isn't enough: for `\\\"` the leading `\\` is collapsed to a single `\`,
  shifting the pairing so the `"` ends up unescaped in the segment.

The corrupted segment (e.g. `\\"/Z`) then reaches `parse_braced_operand_opts`'s
`"` handler (`:2351`), which scans to EOF and returns
`LexError::UnterminatedQuote` (`:2356`) — the observed error.

Confirmed minimal trigger (parse-only): `${V//\\\"/Z}` errors in huck, parses
in bash. `${V//\"/Z}` (single backslash) is fine; the `\\\"` run (≥3
backslashes before a `"`) is the trigger. Independent of surrounding double
quotes and of single (`/`) vs global (`//`).

## Goal

Make huck parse the construct and produce bash-identical substitution results.

## Design

Make `split_modifier_operand` a **pure splitter**: preserve every `\x` escape
VERBATIM (consume both chars, push both, no un-escaping). All un-escaping is
already done — once — by `parse_braced_operand_opts` downstream. The `\\`
handler arm collapses to:

```rust
'\\' => {
    // Preserve an escaped char VERBATIM (backslash + the char) and CONSUME the
    // char so it cannot act as a delimiter or open a quote/backtick span. The
    // real un-escaping happens once, downstream, in parse_braced_operand_opts;
    // pre-un-escaping here would double-process backslashes (corrupting runs
    // like `\\\"`). An escaped delimiter (`\/`) is thus preserved AND not seen
    // as a split point.
    let dst = if delim_seen { &mut second } else { &mut first };
    dst.push('\\');
    if let Some(nc) = chars.next() { dst.push(nc); }
}
```

(A trailing `\` at end of body pushes just `\`, as before — the `if let`
handles the no-next-char case.)

### Why the end results are unchanged for working cases

`parse_braced_operand_opts` already maps `\x` → literal `x` for every `x`
(`:2320-2328`), so the FINAL pattern/replacement `Word`s are identical:

| operand body | old `split` output | new `split` output (verbatim) | parsed pattern (both) |
|---|---|---|---|
| `a\/b/x` | `("a/b", "x")` | `("a\/b", "x")` | `a/b` |
| `a\\b` | `("a\b", None)` | `("a\\b", None)` | `a\b` |
| `\\\"/Z` | `("\\"/Z", None)` ⟶ **UnterminatedQuote** | `("\\\"", "Z")` | pattern `\"`, repl `Z` |

Only the INTERMEDIATE `split_modifier_operand` strings change (now verbatim),
so its two `split_modifier_operand_quotes_and_escapes` assertions and the
function doc comment update. The `$(…)`/backtick/`{…}`/quoted-span skipping in
`split_modifier_operand` is unchanged. This also fixes a latent sibling bug:
old `${x/\\/Z}` un-escaped `\\`→`\`, and `parse_braced_operand_opts` then
dropped the trailing `\` → EMPTY pattern; the verbatim path yields pattern `\`.

### Behavior after the fix (bash-verified results)

- `V='a\"b'; echo "${V//\\\"/\"}"` → `a"b` (the real `scripts/config` idiom).
- `V='a\"b\"c'; echo "${V//\\\"/X}"` → `aXbXc`.
- `V=a/b/c; echo "${V//\//_}"` → `a_b_c` (escaped-delimiter control, unchanged).
- `V='x\y'; echo "${V//\\/Z}"` → `xZy` (escaped-backslash control, unchanged).
- `V=foobar; echo "${V/o/O}"` → `fOobar` (plain control, unchanged).
- `scripts/config`: `huck -n` silent, rc 0 = bash.

## Verification

- **New bash-diff harness** `tests/scripts/dollar_subst_escape_diff_check.sh`
  (executing, byte-identical bash↔huck stdout+exit): the un-escape idiom
  `${V//\\\"/\"}` → `a"b`; `${V//\\\"/X}` → `aXbXc`; the escaped-delimiter
  control `${V//\//_}`; the escaped-backslash control `${V//\\/Z}`; an anchored
  form `${V/#\\\"/Q}`; the plain control `${V/o/O}`; and a `:` substring control
  to confirm that path is unaffected (e.g. `V=abcdef; echo "${V:1:3}"` → `bcd`).
  These assert the substitution RESULT, not merely that the fragment parses.
- **Unit tests** (`src/lexer.rs` `mod tests`): update
  `split_modifier_operand_quotes_and_escapes` to the verbatim outputs
  (`split_modifier_operand("a\\/b/x", '/')` → `("a\\/b", Some("x"))`;
  `split_modifier_operand("a\\\\b", '/')` → `("a\\\\b", None)`); the quoted-span
  assertion (`"\"a/b\"/x"`) is unchanged. Add a regression:
  `split_modifier_operand("\\\\\\\"/Z", '/')` → `("\\\\\\\"", Some("Z"))` (the
  Rust literal for body `\\\"/Z` splitting to `\\\"` and `Z`). Keep the other
  `split_modifier_operand_*` tests green (command-sub / backtick / brace
  skipping unchanged).
- **Parse-sweep payoff:** re-run `tools/parse_sweep.sh tools/scripts.tsv
  tools/parse_results.tsv`; confirm BOTH `scripts/config` copies now parse
  (`huck -n` rc 0, no stderr); report `HUCK_GAP` movement from the 22 baseline;
  `HUCK_LENIENT`/`HUCK_CRASH` stay 0.
- **Full `cargo test`** (0 failures). UP-FRONT (v178/v180/v181 lesson) grep all
  of `tests/` + `src/` for tests asserting the OLD `split_modifier_operand`
  un-escaping (`split_modifier_operand` callers/assertions, and any
  `${var/.../...}` integration test that encodes a backslash result) — update
  only those that encode the pre-fix intermediate behavior; do not weaken
  result-level tests (those must stay identical, proving the end results are
  unchanged).
- All `tests/scripts/*_diff_check.sh` green; clippy clean.

## Docs / close-out

No tracked `M-*`/`L-*` divergence covers this (it was sweep-found; the
"unterminated-quote (4)" backlog note in the iteration memory over-counted —
the literal `unterminated quote` cluster is only `scripts/config`). So no
`bash-divergences.md` deletion. Record the iteration in
`project_huck_iterations.md` + `MEMORY.md`, and correct the backlog note (the
`unterminated '((' arithmetic` / `unterminated 'case` gaps are SEPARATE
clusters, not this one).

## Scope boundary

In scope: `split_modifier_operand`'s `\\` arm (verbatim preservation), its two
test assertions + doc comment, the new harness + regression unit test. **Not**
in scope: `scan_braced_operand`, `parse_braced_operand_opts`, the
command-sub/backtick/brace skipping in `split_modifier_operand`, the runtime
pattern-matching engine, the OTHER sweep clusters (`unterminated '((' arith`,
`unterminated 'case`, `expected a command`, etc.). No `bash-divergences.md`
change.
