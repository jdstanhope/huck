# v360 — One model for which delimiter an unexpected EOF names

Issue: [#635](https://github.com/jdstanhope/huck/issues/635)

## Problem

When input ends in the middle of a construct, bash prints one of two shapes:

```
unexpected EOF while looking for matching `X'     (Shape 3)
syntax error: unexpected end of file              (Shape 2)
```

Shape 3 names a delimiter and a line. bash picks both from the innermost still-open
**matched pair** — its `parse_matched_pair` is a character scanner that recurses on
nested openers and, at end of input, reports whichever pair it is still inside.

huck picks them from something else. The delimiter comes from *which scanner
happened to raise*: `lex_is_shape3` maps a `LexError` variant to a `Delim`. The line
comes from the innermost **mode frame** (`err_open_off`), with `err_open_hint` and
`Mode::Arith::quote_open_off` added as one-off patches when a frame turned out to be
the wrong thing to ask.

huck's frames are not bash's pairs, and every divergence in this area is that
mismatch. Fixing them one at a time does not converge: #627 and #631 demonstrably
move each other's answers, because they are two readings of the same missing rule.

## Scope

**In:** which delimiter an unexpected EOF names, and which line it is reported at.

**Out, and deliberately so:**

- The semantic half of [#624](https://github.com/jdstanhope/huck/issues/624). huck
  keeps the `\` of an escaped quote in an arith body, so `$((1+\"2\"))` evaluates
  different text than bash's. That is arith-scanner behaviour, not delimiter naming.
  Only #624's diagnostic half is in scope.
- [#628](https://github.com/jdstanhope/huck/issues/628). A `for (( … )` header closed
  by a single `)` is a *near-token* error in bash, a different message shape.
- `[[ … ]]` conditional-expression wording, and `echo (` being read as a function
  definition. Both surfaced in the same sweep; neither is Shape 2 or a matched pair.
- The Shape 2 constructs themselves (`if`, `while`, `case`, `{ }`, `( )`, function
  bodies). They keep their parser errors. They are the model's boundary, and the
  acceptance criteria require showing they did not move.

## How the model was derived

Measured against bash 5.2.21 — the compat target — not inferred from bash's source,
which is not available on the build host. Each fragment is placed on **line 3 of a
4-line script**, so a line number that is right for the wrong reason (first line,
last line, one past EOF) still shows up.

| sweep | shape | cells | divergent |
| --- | --- | --- | --- |
| depth 1 | 15 contexts × 11 openers | 165 | 16 |
| depth 2 | 8 outers × 9 middles × 9 inners | 648 | 62 |

Plus targeted probes for the cells the sweeps could not reach: the subscript context,
escapes inside `${…}` and backticks, and nested-arithmetic observability.

The 62 depth-2 divergences are four families — #627 (37 rows), #631 (6), #633 (7),
#634 (12) — and two shapes no cell reaches at all, because they are not
"EOF inside an open pair": #629, and #624's diagnostic half.

## The model

### Pair inventory

| pair | opened by | names | line reported |
| --- | --- | --- | --- |
| double quote | `"` | `"` | where it opened |
| single quote | `'` | `'` | where it opened |
| backtick | `` ` `` | `` ` `` | where it opened |
| command substitution | `$(` | `)` | **where input ran out** |
| parameter | `${` | `}` | where it opened |
| arithmetic | `$((` | `)` | where it opened |
| legacy arithmetic | `$[` | `]` | where it opened |
| array literal | `(` after `name=` | `)` | where it opened |
| lvalue subscript | `[` in `a[…]=` | `]` | where it opened |

A subscript inside `${a[…]}` is **not** a pair: `echo ${a[` names `}`, the enclosing
parameter expansion, in both shells today. Only the assignment-lvalue `[` is one.
huck diverges there for an unrelated reason — it never recognises the assignment at
all (`v[` reports `v[: command not found`) — which is
[#75](https://github.com/jdstanhope/huck/issues/75)'s family and out of scope here.
The lvalue row is in the inventory because the model must not *introduce* a
divergence there, not because v360 fixes one.

`$(` is the only pair with the "where input ran out" rule. huck's renderer already
has that rule (`emit_matching`'s `Delim::DollarParen => eof_line`); what is wrong
today is never the line rule itself, only which pair gets picked.

### Suppression rules

Which openers do **not** create a pair, by enclosing context:

1. Inside `'…'` — nothing at all. The pair is opaque.
2. Inside `"…"` — a `'` is literal, so it opens no pair.
3. Inside an arithmetic body (`$((`, `$[`, `((`, `for ((`) — `${` and `$[` open no
   pair, but `$((` does. This asymmetry is measured, not assumed: `$[1+$((2+` names
   `)` (the inner arithmetic) while `$((1+$[2+` names `)` (the *outer* one).
4. Inside `${…}` — a `'` **does** open a pair, even when the whole expansion sits in
   a double-quoted context. This is the one place `'` stops being literal for
   reporting purposes, and it is #631.
5. A backslash-escaped `\"` or `\'` never opens a pair, in any context. This is
   #624's diagnostic half, which passes today only because of a guard bolted onto
   one scanner; here it becomes the general rule.
6. At EOF, the pair report **wins over construct validation**. `echo ${${x` is
   ``matching `}'`` in bash, because bash is still collecting the pair and never
   reaches the judgement that the expansion is malformed.

### Two consequences worth stating

- **`v=(` changes message shape**, not just its delimiter: today huck emits its own
  `syntax error: unterminated array literal '('` at the input's last line; bash emits
  Shape 3 at the `(`'s own line. Its **exit status changes too** — bash exits 1 where
  huck exits 2 (#633).
- **#629 stops being a special case.** A `$((` re-read as a command substitution keeps
  its arithmetic pair, so it reports `)` at the arithmetic's opening line rather than
  inheriting `$(`'s "where input ran out" rule.

## Architecture

### The stack

```rust
struct Pair { delim: Delim, open_off: usize, line: LineRule }
// LineRule = Open | Eof
```

A `Vec<Pair>` on the `Lexer`.

- **Pushed** from `push_mode` for the mode-backed pairs, gated by the suppression
  table against the current top. The gate is where rules 1–3 live.
- **Pushed explicitly** by the two pairs with no mode of their own: a quote span
  inside an arithmetic body, and the `'` span inside a `${…}` operand (rule 4).
- **Popped** symmetrically, with a `debug_assert` that the popped delimiter is the
  expected one.
- **Snapshotted** by `mark` and restored by `rewind`, alongside `modes` — the same
  treatment `mode_open_offs` gets today, and required by the `$((` → `$( (` rewind.

It **replaces** four existing mechanisms rather than joining them:
`mode_open_offs`, `err_open_off`, `err_open_hint`, and `Mode::Arith::quote_open_off`.

**The stack is read only for diagnostics.** A push/pop bug degrades an error message;
it cannot change what the shell executes. This is what makes the change safe to make
across ten scanners at once.

### The reporting path

- `scan_step_guarded` snapshots the stack **top** on error, instead of computing an
  offset from the mode stack.
- `lex_is_shape3` stops mapping *variant → `Delim`*. The renderer takes the reported
  pair, and `emit_matching`'s `$(` special case becomes the pair's own `LineRule`.
- The `LexError` variants keep their control-flow role. `is_unterminated_lex` and
  continuation classification are untouched, which is what keeps the REPL out of this
  change.
- The parser's two hardcoded Shape 3 sites — `unterminated_cmdsub` and
  `unterminated_backtick`, which name `Delim::DollarParen` / `Delim::Backtick`
  themselves — read the reported pair instead, so the stack is the single authority.

### EOF beats validation

This is the only part that touches the parser's error path.

`parse_param_expansion` today is 612 lines with **eleven exits** — ten `return Err`
and one `return Ok` — and **21 `pop_mode` calls** scattered among them, each manually
re-paired with `restore_dq!()`, some popping two modes. That shape cannot host a
drain hook, and threading pair push/pop correctness through eleven returns is exactly
the hazard the model is meant to remove.

So the work is **two-phase**:

- **Phase 0 — single-exit conversion.** Restructure `parse_param_expansion` into the
  closure + single `pop_mode` shape `parse_arith_expansion` already uses. Behaviour
  preserving, and **proved inert before anything else lands**: zero expected-value
  edits, every bash-suite category diffed against `origin/main`, full sweep green.
- **Phase 1 onward — the model**, on top of that single exit. On a *validation*
  failure the wrapper drains to the pair's close using a character skip driven by the
  same suppression table; if the drain runs out of input, the lex error wins and the
  pair reports. One wrapper covers both #634 causes: `${${x` (validation deferred)
  and `${$(` (the drain scans into the `$(`, opens a comsub pair, and reports `)` at
  the EOF line).

## Verification

- **A generated matrix harness**, `eof_delimiter_matrix_diff_check.sh`, building the
  813 cells programmatically rather than listing them. This is a new harness style
  for the repo — every existing one is a hand-written list with commentary — and it
  is justified because the model's claim *is* "every combination agrees". Measured
  cost: ~70 s (13 s + 56 s), about 9% on a ~13-minute sweep.
- **Hand-written rows, in the existing commentary style, for what the matrix cannot
  reach**: #629's `$((1+2)` shape; multi-line cases where a pair opens on an earlier
  line than the EOF, since the line rule is invisible in single-line cells; the piped
  stdin driver, which re-lexes the buffer through a different top-level path; and the
  escaped-quote rows pinning #624's diagnostic half.
- **Unit tests on the stack itself** — top-of-stack delimiter, offset and line rule
  for representative inputs, asserted without rendering an error. This is the payoff
  for putting the model in one place: the model is checkable independently of message
  formatting.
- **Shape 2 control rows** for `if`, `while`, `case`, `{ }`, `( )` and function
  bodies. The whole design rests on that boundary holding, and the same `(` proves
  the boundary is real: a subshell is Shape 2, an array literal is Shape 3.
- Every harness must be **run red first**, against a binary built at the parent
  commit, before it is believed.
- Existing harnesses stay green, in particular `unterminated_eof_diff_check.sh`,
  `arith_eof_quote_diff_check.sh` and `arith_for_header_eof_diff_check.sh`, which are
  the regression net for the line rules.

## Acceptance criteria

1. Phase 0 lands with zero expected-value edits and a green sweep, proved by a
   per-category diff against `origin/main`.
2. The 813-cell matrix is fully green, both drivers.
3. #627, #629, #631, #633 and #634 are closed by the PR, and #635 with them.
4. #624 stays open, with a comment recording that its diagnostic half is now covered
   by the model and only the semantic half remains — the handling #606 got.
5. Shape 2 constructs are byte-identical to before the change.
6. Full `tests/scripts/run_diff_checks.sh` green; every `-p huck` integration binary,
   both `--lib` suites, and pinned clippy clean.

## Risks

- **Push/pop asymmetry** across ten scanners. Mitigated by pushing from `push_mode`
  where possible (one site, already paired with `pop_mode`), a `debug_assert` on the
  popped delimiter, and the property that the stack is diagnostics-only.
- **Phase 0 is a refactor of a central function.** Mitigated by landing it alone, and
  by the zero-expected-value-edits proof; if that proof does not come out clean, the
  refactor is wrong and the model work does not start on top of it.
- **The generated harness could mask a regression by regenerating its own
  expectations.** It does not: every cell is a live bash-vs-huck comparison, with no
  stored expected output.
