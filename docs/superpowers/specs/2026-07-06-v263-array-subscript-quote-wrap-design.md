# v263 — subscript quote-wrap fix (array-literal + param-expansion `[sub]`)

**Status:** design approved (2026-07-06)
**Arc:** the LAST reconciliation carry-forward before the finale. A→B→C
(v259/v260/v261) + F2 (v262) are done; this clears the final must-fix item
surfaced by the v259 whole-branch review, unblocking the `command_atoms` flip.
From the verified carry-forward inventory (`huck_carryforward_inventory.md`).

## Summary

On the dormant atom-command path (`new_seq`, `command_atoms` default `false`), a
bare (non-`$`) quoted span inside a **subscript operand** — the `[sub]` of an
array-literal `a=([sub]=v)` or a parameter expansion `${a[sub]}` — is inlined
FLAT, but the oracle wraps it in a `Quoted{…}` part:

- `a=(["k"]=v)` → atom subscript `[Literal{"k",true}]` (flat) vs oracle
  `[Quoted{Double,[Literal{"k",true}]}]`
- `a=(['k']=v)` → atom flat vs oracle `[Quoted{Single,…}]` (single-quote too)
- `${a["k"]}` / `${a['k']}` → same divergence in the param-expansion subscript
  (same `Mode::ParamSubscriptOperand`)

**Root cause:** the subscript operand is scanned by `scan_step_param_operand`
under `Mode::ParamSubscriptOperand`. Its bare-`"` arm inlines the double-quote
span flat (emits `Lit{quoted:true}`, no `BeginDquote`), and its `'…'` arm emits a
flat `Lit{quoted:true}` — so the parser never sees a signal to wrap. The v259 F3
fix already made the *double*-quote wrap work for `$"…"` (the `$"` arm emits
`BeginDquote`, and `parse_word`'s F3 arm wraps `Quoted{Double}` for subscript
mode / flattens for value families). Bare `"`/`'` bypass that path. The oracle
re-tokenizes the subscript via `scan_subscript`/`parse_subscript_body` — the
general tokenizer — which keeps the `Quoted{…}` wrapper for any quoted span.

**Scope (approved — full set):** both quote kinds (`"…"`→`Quoted{Double}`,
`'…'`→`Quoted{Single}`) in **both** subscript contexts (array-literal `[sub]=`
and param-expansion `${a[sub]}` — the same mode). Value-family operands
(`${x:-…}`/`${x/…/…}`/`${x:o:l}`) keep their flat inlining, untouched.

**Fix (F3-consistent, gated to subscript mode `end == ']'`):** the bare-`"`
operand arm emits a zero-width `BeginDquote` (like `$"`) so the existing F3
`parse_word` arm wraps `Quoted{Double}`; the `'…'` operand arm emits a
`QuoteRun{Single,text}` so a new `parse_word` operand arm wraps `Quoted{Single}`.
`command.rs` EMPTY-diff; `command_atoms` stays `false`.

## Background — the confirmed surface (probed)

Probed `old_seq` (oracle) vs `new_seq` (atom); every part `quoted:true` inside a
`Quoted{…}` wrapper, the enclosing `Index`/`ArrayLiteralElement.subscript` unchanged:

| input | oracle subscript | atom today |
|---|---|---|
| `a=(["k"]=v)` | `[Quoted{Double,["k"]}]` | `[Lit "k" true]` (flat) |
| `a=(['k']=v)` | `[Quoted{Single,["k"]}]` | `[Lit "k" true]` (flat) |
| `a=([""]=v)` | `[Quoted{Double,[Lit "" true]}]` | flat |
| `a=(['']=v)` | `[Quoted{Single,[Lit "" true]}]` | flat |
| `a=(["k$x"]=v)` | `[Quoted{Double,[Lit k, Var x]}]` | `[Lit k, Var x]` (flat) |
| `a=([x"y"z]=v)` | `[Lit x false, Quoted{Double,[y]}, Lit z false]` | `[Lit x, Lit y true, Lit z]` |
| `a=([x'y'z]=v)` | `[Lit x, Quoted{Single,[y]}, Lit z]` | flat |
| `a+=(["k"]=v)` | wrapped (append:true) | flat |
| `${a["k"]}` | `Index([Quoted{Double,["k"]}])` | `Index([Lit "k" true])` |
| `${a['k']}` | `Index([Quoted{Single,["k"]}])` | flat |
| `${a[x"y"]}` | `Index([Lit x, Quoted{Double,[y]}])` | flat |
| `declare -A m=(["k"]=v)` | wrapped (arg word) | flat |

**Regression guards (already EQ=true — must STAY):**
- `${x:-"y"}` → value operand FLAT `[Literal{"y",true}]` (NOT wrapped)
- `${x:-'y'}` → value single-quote FLAT
- `a=([$"k"]=v)` / `${a[$"k"]}` → `Quoted{Double}` (v259 F3, already wraps)
- `a=([k]=v)` / `${a[k]}` → plain `[Literal{"k",false}]`

`Mode::ParamSubscriptOperand` is uniquely identified by `end == ']'` (value
families use `end == '}'`), so the fix touches subscripts only.

## Architecture

**Files:**
- `crates/huck-syntax/src/lexer.rs` — two arms in `scan_step_param_operand`
  (bare-`"` and `'…'`), gated on `end == ']'`.
- `crates/huck-syntax/src/parser.rs` — one new `QuoteRun` arm in the operand-parse
  function; the `BeginDquote` F3 arm already handles double-quote. New corpus in
  `mod tests`.
- `crates/huck-syntax/src/command.rs` — UNTOUCHED (EMPTY diff).

### Lexer — `scan_step_param_operand` (gated `end == ']'`)

- **Bare-`"` arm** (the `Some('"')` "outside dquote" arm that currently inlines
  the span flat): when `end == ']'`, emit a zero-width `BeginDquote` at the `"`
  (do NOT consume the `"`; leave it for `parse_dquote`), then `return` — mirroring
  the `$"` arm (which consumes `$`, leaves `"`, emits `BeginDquote`). Forward
  progress is guaranteed by the mode switch: `parse_word`'s `BeginDquote` arm
  calls `parse_dquote`, which pushes `Mode::DoubleQuote` and consumes the `"`.
  When `end != ']'`, keep the current flat inline unchanged.
- **`'…'` arm** (currently scans the span and emits `Lit{text,quoted:true}`):
  when `end == ']'`, emit `QuoteRun{style:QuoteStyle::Single, text}` instead of
  the flat `Lit`. When `end != ']'`, keep the flat `Lit` unchanged.

### Parser — operand-parse function

- `BeginDquote` arm: UNCHANGED — the v259 F3 arm already does
  `in_subscript ? push(dq) : flatten`. A bare-`"`-emitted `BeginDquote` in
  subscript mode reaches it and wraps `Quoted{Double}`.
- NEW `QuoteRun{style, text}` arm: `parts.push(WordPart::Quoted { style, parts:
  vec![WordPart::Literal { text, quoted: true }] })`. Only reached from subscript
  mode (the scanner emits `QuoteRun` only when `end == ']'`), so it always wraps.
  Must be placed before the catch-all `_ => UnsupportedExpansion`.

### Why this matches, precisely

- Double-quote: the F3 `parse_word` arm wraps `Quoted{Double}` for
  `Mode::ParamSubscriptOperand`; `parse_dquote` scans the interior (handling
  `$x`/expansions and the empty-`""` marker) → `[Quoted{Double,…}]`. Value
  families are never routed through `BeginDquote` for a bare `"` (the gate keeps
  them flat).
- Single-quote: the new `QuoteRun` arm wraps `Quoted{Single,[Literal{text,true}]}`
  (single-quote is literal — no expansion), matching the oracle. Empty `''` →
  `QuoteRun{Single,""}` → `Quoted{Single,[Literal{"",true}]}`.
- Mixed (`[x"y"z]`): plain-literal atoms push `Literal{quoted:false}`; the
  `BeginDquote`/`QuoteRun` spans push their `Quoted{…}` → `[Lit x, Quoted, Lit z]`.

## Differential corpus

**Fixed — new `diff_cmd`:** `a=(["k"]=v)`, `a=(['k']=v)`, `a=([""]=v)`,
`a=(['']=v)`, `a=(["k$x"]=v)`, `a=([x"y"z]=v)`, `a=([x'y'z]=v)`, `a+=(["k"]=v)`,
`${a["k"]}`, `${a['k']}`, `${a[x"y"]}`, `declare -A m=(["k"]=v)`.

**Regression guards — stay `diff_cmd` byte-identical:** `${x:-"y"}`,
`${x:-'y'}` (value families flat), `a=([$"k"]=v)`, `${a[$"k"]}` (F3 dquote),
`a=([k]=v)`, `${a[k]}` (plain).

## Testing & gates

- Differential harness in `parser.rs mod tests`: `diff_cmd` for the fixed corpus
  and the regression guards.
- Full `huck-syntax` lib suite is the non-regression net (value-family operand
  tests are the guard that the `end == ']'` gate is correct).
- `command.rs` diff-vs-main = EMPTY.
- Both `command_atoms` sites stay `false`.
- `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` green (box is
  1 core/1.9GB — never `--workspace`, never multi-threaded).
- `cargo build -p huck-syntax` → 0 warnings; `scan_step_param_operand` and the
  operand-parse match stay exhaustive.

## Task decomposition (SDD)

- **T1 — the two lexer arms + the parser arm + the corpus.** Single task: gating
  the bare-`"` (→`BeginDquote`) and `'…'` (→`QuoteRun{Single}`) arms on
  `end == ']'`, adding the operand-parse `QuoteRun` arm, and the `diff_cmd`
  corpus. One coherent change to one code path; splitting the lexer signal from
  the parser wrap would leave a non-passing intermediate.

## Live-flip carry-forwards

RESOLVES the last must-fix reconciliation carry-forward
(`atoms_array_literal_subscript_dquote_wrap_divergence`, extended to single-quote
+ the param-expansion subscript). No new pin expected — the fix only changes
cases that were wrong, and is gated to subscript mode so value families are
untouched. Whole-branch review to probe siblings routed through
`Mode::ParamSubscriptOperand` not in the corpus (nested subscripts, `${a[i]}`
in a nested operand, indirect `${!a[…]}`, arithmetic subscripts `[0+1]` with
quotes). After merge, mark this resolved and record v263. **This UNBLOCKS THE
FINALE** — re-verify the keep-intentional pins still assert, then flip
`command_atoms` LIVE and delete the six forward-scanning scanners. No
`bash-divergences.md` change (dormant).
