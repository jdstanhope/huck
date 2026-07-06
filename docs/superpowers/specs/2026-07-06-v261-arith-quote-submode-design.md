# v261 — Iteration C carry-forward fix (CF6+CF7): arith quote sub-mode

**Status:** design approved (2026-07-06)
**Arc:** Phase C reconciliation — clearing the accumulated live-flip carry-forwards
before the finale (flip `command_atoms` live + delete the forward-scanning scanners).
Iteration A (v259) resolved CF2/CF3/CF4; Iteration B (v260) resolved CF1. **This is
Iteration C — CF6+CF7, the shared `scan_step_arith` quote cluster.** From the verified
carry-forward inventory (`huck_carryforward_inventory.md`).

## Summary

On the dormant atom-command path (`new_seq`, `command_atoms` default `false`), the shared
arithmetic body loop `scan_step_arith` (lexer.rs:1990) has **no `'`/`"`/`\` handling and
no in-quote state**, so it diverges from the `command.rs` oracle in two ways:

- **CF6 — quote retention (Paren delims `$((`/`((`/`for ((`).** The atom retains quote
  characters in the body Word; the oracle removes them (`arith_string_to_word` performs
  bash quote-removal). `$(( "x" ))` → atom body `" \"x\" "` vs oracle `" x "`;
  `$(( x="5" ))` → atom `" x=\"5\" "` vs oracle `" x=5 "`; `1"2"3` → oracle `123`. Also
  single-quotes wrongly EXPAND `$` (`$(( '$x' ))` → atom emits `Var x` vs oracle keeps
  literal `" $x "`). **This is a must-fix, not cosmetic:** huck's runtime arith tokenizer
  (`huck-engine/arith.rs:386`) errors on a `"`/`'` byte, so a retained-quote body would be
  a runtime PARSE ERROR post-flip.
- **CF7 — early close inside quotes / after backslash (Bracket delim `$[`).** `$[` closes
  on a single depth-0 `]`, but the atom fires it regardless of quote/backslash state:
  `$[ "]" ]` → atom `UnterminatedQuote` vs oracle Arith `" ] "`; `$[']']` → atom
  `UnterminatedQuote` vs oracle Arith `"]"`; `$[ \] ]` → atom splits into two args vs
  oracle Arith `" \] "`.

**Also folded (approved):** a pre-existing structural divergence in the same loop — a
**bare `$`** (one not starting an expansion) is retained/merged by the atom
(`$(( 1 $ 2 ))` → `" 1 $ 2 "`) but the oracle emits it as its OWN literal part
(`[" 1 ", "$", " 2 "]`, from `arith_string_to_word`'s `flush_lit` + a separate `$`
literal). This surfaces in `$(( $'x' ))` (there is NO ANSI-C `$'…'` in arithmetic — the
`$` is a useless literal and `'…'` is quote-removed), so resolving the `$`/quote matrix
cleanly requires it.

**Fix (Approach A, chosen):** add a quote/backslash **sub-mode** to `scan_step_arith`,
driven by new persisted `Mode::Arith` state, reproducing the oracle's two functions
(delimiter-finding in `scan_arith_body`/`scan_legacy_arith_body` AND quote-removal in
`arith_string_to_word`) in one incremental pass. No new atoms, no parser change,
`command.rs` EMPTY-diff, `command_atoms` stays false.

## Background — the confirmed oracle model (probed)

Probed `old_seq` (oracle target) vs `new_seq` (atom today) for the full matrix. Confirmed
targets (arith body Word; every part `quoted: true`, the whole `Arith` `quoted: false`):

| input | oracle body | atom today |
|---|---|---|
| `$(( "x" ))` | `" x "` | `" \"x\" "` (CF6) |
| `$(( x="5" ))` | `" x=5 "` | `" x=\"5\" "` (CF6) |
| `$(( 1"2"3 ))` | `" 123 "` | `" 1\"2\"3 "` (CF6) |
| `$(( '$x' ))` | `" $x "` (literal) | `" '` + `Var x` + `' "` (CF6, wrong expand) |
| `$(( "$x" ))` | `" "`,`Var x`,`" "` | `" \""`,`Var x`,`"\" "` |
| `$(( "a\"b" ))` | `" a\"b "` (esc) | `" \"a\\\"b\" "` |
| `$(( "`echo 1`" ))` | `" "`,`CmdSub`,`" "` | retained-quote (CF6) |
| `$(( "${x:-]}" ))` | `" "`,`Param`,`" "` | retained-quote (CF6) |
| `$(( "a$(( 1 ))b" ))` | `" a"`,`Arith` 1,`"b "` | retained-quote (CF6) |
| `$(( "" ))` | `"  "` (empty dropped) | `" \"\" "` |
| `$(( "a"'b' ))` | `" ab "` | retained-quote (CF6) |
| `$(( 1 $ 2 ))` | `[" 1 ","$"," 2 "]` | `" 1 $ 2 "` (bare-$) |
| `$(( $'x' ))` | `[" ","$","x "]` | `" $'x' "` (bare-$) |
| `$[ "]" ]` | `" ] "` | `UnterminatedQuote` (CF7) |
| `$[']']` | `"]"` | `UnterminatedQuote` (CF7) |
| `$[ \] ]` | `" \] "` | 2 args (CF7) |
| `$[ ${x:-]} ]` | `" "`,`Param`,`" "` | already EQ |
| `$[ $(echo ]) ]` | `" "`,`CmdSub`,`" "` | already EQ |

**Regression guards (already EQ — must STAY):** `$(( ")" ))`, `$(( ')' ))`, `$(( "(" ))`
all fail `scan_arith_body` (quote-blind) and **bail** to cmdsub-of-subshell on BOTH paths.
Quote-removal must NOT make quotes protect `)`/`(`, or these regress.

**The Paren/Bracket asymmetry (genuine bash quirk, faithfully reproduced):**
- **Paren** (`$((`/`((`/`for ((`): `scan_arith_body` counts `()` **quote-blind** (a `)` or
  `;` inside a quote still counts); quote-removal is a separate second pass. → In the atom
  loop, `(`/`)`/`;` fire **regardless of quote state**.
- **Bracket** (`$[`): `scan_legacy_arith_body` skips `'…'`/`"…"` spans and honors `\c` so a
  `]` inside them does not close. → In the atom loop, `[`/`]` fire **only when not in a
  quote** (and a `\`-protected char is consumed raw).

## Architecture

**Files:**
- `crates/huck-syntax/src/lexer.rs` — `Mode::Arith` gains `in_squote: bool`;
  `scan_step_arith` gains `in_squote` (and wires the currently-dead `_in_dquote`) +
  the quote/backslash arms + the bare-`$` flush; dispatch at ~964 updated.
- `crates/huck-syntax/src/parser.rs` — differential corpus + pin flips only.
- `crates/huck-syntax/src/command.rs` — UNTOUCHED (EMPTY diff).

### State

`Mode::Arith { paren_depth, in_squote, in_dquote, body_started, for_header, delim }`.
`in_squote` is NEW; `in_dquote` already exists (was dead for arith — the oracle hardcodes
`quoted: true`; now it carries the double-quote span). No `after_backslash` field —
backslash always resolves within one loop iteration.

`scan_step_arith(paren_depth, in_squote, in_dquote, body_started, for_header, delim)`:
seed locals from the params; a `sync_state!` macro (replacing `sync_depth!`) writes
`paren_depth` **and** `in_squote`/`in_dquote` back to the frame before EVERY
`return Ok(Step::Produced)` (the quote flags must survive a `$`/backtick sub-parse
round-trip, exactly like `paren_depth`).

### Char-handling rules (body loop; `body_started == true`)

Every emitted `Lit`/`DollarName`/`ParamOpen`/… stays `quoted: true`.

**Inside single-quote (`in_squote`):**
- `'` → consume, DROP, `in_squote = false`.
- any other char (incl. `$` `` ` `` `\` `)` `(` `]` `[` `;` `"`) → consume, push literally
  (no expansion, no delimiter events).

**Inside double-quote (`in_dquote`, not squote):**
- `"` → consume, DROP, `in_dquote = false`.
- `\` → consume; if next ∈ {`"`,`\`,`$`,`` ` ``} consume next and push next (drop the `\`);
  else push `\` (keep) and leave next for the following iteration.
- `$` → the SHARED `$`-classifier block (expansion active; see below).
- `` ` `` → flush pending text; emit `BeginBacktick`.
- `'` → literal push.
- **Paren** `(`/`)` → fire depth/close/bail events (quote-blind). **Bracket** `[`/`]` →
  literal push (protected).
- `;` when `for_header && depth == 0` → fire `ArithSemi` (quote-blind; matches the oracle's
  paren-only `split_top_level_semi`).
- other → push.

**Neither quote:**
- `'` → consume, DROP, `in_squote = true`.
- `"` → consume, DROP, `in_dquote = true`.
- `open_char` / `close_char@depth>0` / `close_char@depth==0` (close/bail) / `;` for-header /
  `` ` `` / `$` — as today (delimiter events; the `$` block below).
- `\` → **Bracket:** push `\`, then consume+push the NEXT char raw (no delimiter/expansion
  processing — protects `\]`/`\[`). **Paren:** push `\` only.
- other → push.

**Shared `$`-classifier block** (used in neither-quote and in-dquote; NEVER reached in
squote): unchanged for `${`→`ParamOpen`, `$(`→`CmdSubOpen`, `$((`→`ArithOpen`,
`$[`→`LegacyArithOpen`, `$name`/special-param/`$digit`→`DollarName` (all flush-then-signal,
`quoted: true`). **CHANGED — bare `$` fallthrough (`_ =>`):** currently `text.push('$')`;
now **flush pending text, then emit a standalone `Lit { text: "$", quoted: true }`**
(matches `arith_string_to_word`'s `flush_lit` + separate-`$`-literal structure). Resolves
the bare-`$`/`$'…'` splits.

**EOF (`None`):** if `in_squote || in_dquote` → `Err(LexError::UnterminatedArith)`
(unterminated quote; the oracle also errors — `UnterminatedArith`/`UnterminatedLegacyArith`
— so both paths error and the input is not byte-comparable, matching the prior-iteration
non-diff pattern for lex-error inputs). Otherwise the existing flush-or-`UnterminatedArith`.

### Why this matches, precisely

- Paren delimiters fire quote-blind → the `$(( ")" ))`/`$(( ')' ))`/`$(( "(" ))` bails are
  preserved; dropping the opening quote up-front and accumulating stripped text converges on
  the same concatenated body as the oracle's retain-then-strip two-pass (verified for the
  probed matrix).
- Single-quote suppresses the `$` block → `'$x'` stays literal.
- Double-quote drops the quotes but keeps the shared `$`/backtick expansion and applies the
  `arith_string_to_word` dquote `\`-escape table → `"$x"`, `"`…`"`, `"${…}"`, `"a\"b"` all
  match.
- Bracket `]`/`[` gated by `!in_squote && !in_dquote`, plus `\c` consumes the next char raw
  → `"]"`, `']'`, `\]` all protect the `]`, and quote-removal then yields the oracle body.

## Differential corpus

**Fixed — new `diff_cmd` (must become byte-identical; all listed in Background):** the CF6
Paren matrix (`"x"`, `x="5"`, `1"2"3`, `'$x'`, `"$x"`, `"a\"b"`, `` "`echo 1`" ``,
`"${x:-]}"`, `"a$(( 1 ))b"`, `""`, `"a"'b'`), the bare-`$` splits (`1 $ 2`, `1 $+ 2`,
`$'x'`), and the CF7 Bracket matrix (`"]"`, `']'`, `\]`, and the already-EQ `${x:-]}`,
`$(echo ])` as regression coverage).

**Pin flipped to `diff_cmd`:** the v258 `$[` quote/backslash pin
`atoms_legacy_arith_quote_backslash_carryforward` (parser.rs:5716) — its three
`assert!`/`assert_eq!` cases (`echo $[ "]" ]` → `UnterminatedQuote`; `echo $[ \] ]` →
two args; `echo $[']']` → `UnterminatedQuote`) become `diff_cmd` (oracle bodies `" ] "`,
`" \] "`, `"]"` respectively). Rewrite the whole test body to `diff_cmd` calls, keeping
the test name.

**Regressions — stay `diff_err`/bail (must NOT change):** `$(( ")" ))`, `$(( ')' ))`,
`$(( "(" ))` (quote-blind bail).

**For-header + quote:** probe `for (( "a;b" ; ; ))` and kin; the `;` is quote-blind so it
should match, but if a residual diverges it is PINNED as the same class as the existing
v256 for-header `;`-in-backtick/`${…}` carry-forward (documented, dormant).

## Testing & gates

- Differential harness in `parser.rs mod tests`: `diff_cmd` for the fixed corpus; `diff_err`
  for both-error inputs; `assert_ne!` for any genuinely-pinned residual.
- The existing ~78 arith tests are the T1 non-regression net — most are quote-free; any that
  asserted a **retained-quote** body are updated in T1 to the stripped form (that assertion
  WAS the CF6 divergence).
- `command.rs` diff-vs-main = EMPTY.
- Both `command_atoms` sites stay `false`.
- `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` green (box is 1 core/1.9GB
  — never `--workspace`, never multi-threaded).
- `cargo build -p huck-syntax` → 0 warnings; `scan_step_arith` matches stay exhaustive.

## Task decomposition (SDD)

- **T1 — the quote sub-mode machinery (lexer.rs).** `Mode::Arith` `in_squote` field (all
  ~14 sites → `false`); `scan_step_arith` signature + dispatch (964); `sync_state!`;
  the squote/dquote/backslash arms + Bracket-delimiter guards + the bare-`$` flush. Gate =
  the existing ~78 arith tests stay green (update any retained-quote assertions).
- **T2 — differential corpus + pin flips + edges (parser.rs).** The full CF6/CF7 matrix as
  `diff_cmd`; flip the v258 `$[` quote pin(s); keep the bail regression pins; probe + pin (if
  needed) the for-header/quote residual.

(T1 and T2 could be one task; splitting keeps the hot-loop scanner change and the parity
corpus separately reviewable — the scanner change is the high-regression-surface part.)

## Live-flip carry-forwards

RESOLVES CF6, CF7, and the bare-`$`/`$'…'` structural split. NEW pin possible only for the
for-header/quote residual (same class as the existing v256 carry-forward). After merge, mark
CF6+CF7 resolved and record v261 in the iteration log. Remaining before the finale: **F2**
(bang-in-compound-body) and **array-lit-subscript-bare-dquote**; CF5/CF8/CF9/CF10 stay
keep-intentional, plus the 2 v260 redirect-ordering pins. No `bash-divergences.md` change
(dormant).
