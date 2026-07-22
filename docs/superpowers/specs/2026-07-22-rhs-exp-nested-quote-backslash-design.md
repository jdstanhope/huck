# v321 — RHS value-family expansion: strip backslash in a nested `"…"` when the `${…}` is double-quoted

Issue: [#253](https://github.com/jdstanhope/huck/issues/253)

## Problem

Inside a **double-quoted** parameter expansion of the *value family*
(`${v:-word}` / `${v:=word}` / `${v:?word}` / `${v:+word}`, and the
non-colon `${v-word}` / `${v+word}` forms), a backslash that sits **inside a
nested `"…"` span within the word** is handled by the wrong quoting rule.

bash 5.2.21 strips the backslash before *any* following character in that
position — a *double de-quoting* — but **only** when the enclosing `${…}` is
itself inside double quotes. huck keeps the backslash: it applies plain
double-quote rules (backslash escapes only `$` `` ` `` `"` `\`; before any
other char the backslash is retained) in every case.

### The exact rule (verified against bash 5.2.21)

Let the word of a value-family expansion contain a nested `"…"` span, and let
`\X` be a backslash followed by character `X` inside that span.

| enclosing `${…}` context | `\p` (X ∉ `$` `` ` `` `"` `\`) | `\$`, `\\`, `\"`, `` \` `` (X special) |
|---|---|---|
| **double-quoted** (`"…${v:+…"\p"…}…"`) | `p` — backslash **dropped** | `$`, `\`, `"`, `` ` `` — dropped (same either way) |
| **unquoted** (`…${v:+…"\p"…}…`) | `\p` — backslash **kept** (standard dq) | `$`, `\`, `"`, `` ` `` — dropped |

Two facts bound the change precisely:

1. The divergence appears **only** when BOTH hold: (a) the outer `${…}` is
   inside double quotes, and (b) the backslash is inside a *nested* `"…"`
   span in the word. With an unquoted outer `${…}`, or for a backslash
   *outside* the nested span, huck already matches bash.
2. For a backslash before a double-quote-special char (`$` `` ` `` `"` `\`),
   huck already matches bash in *all* cases — both rules drop the backslash
   and keep the char. So only the **non-special** arm changes, and only when
   the enclosing context is double-quoted.

Controls confirming the scope (all bash 5.2.21):
- `recho "A\pB"` (plain double quote, no `${…}`) → `A\pB` (backslash kept —
  the aggressive strip is unique to the param-operand-nested-quote case).
- `recho "A${v:+'\p'}"` (nested **single** quotes) → `A'\p'` (single quotes
  are literal characters inside a double-quoted `${…}`; unchanged path).

This is the sole remaining divergence in the bash test-suite `rhs-exp`
category; fixing it flips that category FAIL → PASS (bash-suite PASS 17 → 18).

## Design

### Single site

The value-family word operand is scanned by
`Mode::ParamWordOperand { in_dquote, enclosing_dquote }` in
`crates/huck-syntax/src/lexer.rs`, via `scan_step_param_operand(sep, end,
in_dquote, enclosing_dquote)`. The `enclosing_dquote` flag is already the
OUTER `"…"` context the `${…}` sits in (distinct from `in_dquote`, the
scanner's internal `"…"`-span state). The function already receives both.

The change is confined to the `in_dquote == true` branch's backslash arm
(the `Some('\\') => { … }` block). Today that arm is:

```rust
Some('\\') => {
    self.cursor.next(); // consume `\`
    match self.cursor.peek().copied() {
        Some(e @ ('$' | '`' | '"' | '\\')) => {
            self.cursor.next();
            // emit `e` (the escaped char), quoted:true
        }
        _ => {
            // emit "\" + next char, quoted:true   ← keeps the backslash
        }
    }
    return Ok(Step::Produced);
}
```

The fix gates **only** the `_ =>` (non-special) arm on `enclosing_dquote`:

- `enclosing_dquote == true` → consume the next char and emit **just that
  char** (`quoted: true`) — the backslash is dropped (`\p` → `p`).
- `enclosing_dquote == false` → unchanged: emit `"\" + next char`
  (`quoted: true`) so `\p` stays `\p`.

The special arm (`$` `` ` `` `"` `\`) is left exactly as-is: it already emits
the escaped char under both rules, so its bytes are identical. Keeping it
untouched holds the diff to one arm and preserves the escaped-quote behavior
(`\"` inside the nested span stays a literal `"`, not a span terminator).

### Why nothing else moves

- **`enclosing_dquote == false`** path is untouched, so unquoted-outer
  expansions (which already match bash) do not change.
- The **`in_dquote == false`** branch (outside the nested span) is untouched,
  so a backslash in the un-nested part of the word keeps standard behavior.
- **Pattern operands** (`#` / `%` / `/`) and **substring offsets** use
  different modes (`Mode::ParamSubstPatternOperand`,
  `Mode::ParamSubstringOffsetOperand`) and different scan steps; they are not
  reached by this change and are explicitly out of scope.
- **Single-quote spans** inside a double-quoted `${…}` are a different code
  path (literal characters); untouched.

### The value family shares this mode

`${v:-…}` / `${v:=…}` / `${v:?…}` / `${v:+…}` and the non-colon `${v-…}` /
`${v+…}` all parse their word operand under `Mode::ParamWordOperand`, so the
one arm fixes the whole family. Verified against bash: all six strip `\p` →
`p` under a double-quoted outer context.

## Testing

The gate is **bash 5.2.21 fidelity**, not internal-scanner shape.

1. **Bash-diff harness — the gold standard.** Add
   `tests/scripts/rhs_exp_nested_quote_diff_check.sh` following the existing
   `*_diff_check.sh` pattern: run synthetic fragments through both bash and
   huck and assert byte-identical output. Fragments (author them fresh — do
   NOT copy bash's GPL `rhs-exp.tests`), each printed via `printf '<%s>\n'`:
   - double-quoted outer, nested `"\p"` → `<a=pb>`-shaped (backslash dropped)
   - double-quoted outer, nested `"\'"` → backslash dropped
   - double-quoted outer, nested `"\$"` / `"\\"` → special chars (unchanged;
     regression guard)
   - double-quoted outer, **bare** `\p` (not nested) → `\p` kept (guard)
   - **unquoted** outer, nested `"\p"` → `\p` kept (guard)
   - one case for each of `:-` / `:=` / `:+` to cover the family
   - plain `"A\pB"` (no `${…}`) → `\p` kept (guard the scope boundary)
   No wiring needed: `run_diff_checks.sh` auto-discovers every
   `tests/scripts/*_diff_check.sh`, so the filename suffix is sufficient.

2. **Lexer unit tests** (`crates/huck-syntax/src/lexer.rs` test module) — the
   three shapes at the token level: a double-quoted `${v:+"\p"}` operand
   lexes the `p` as a `quoted:true` Literal with no backslash; the
   unquoted-outer counterpart keeps `\p`; a `\$` in the nested span stays a
   single `$` under both. Assert on the emitted `TokenKind::Lit` text.

3. **Bash test-suite category flip** — run the `rhs-exp` category through the
   runner and confirm it flips to PASS:
   `HUCK_BASH_TEST_CATEGORY=rhs-exp BASH_SOURCE_DIR=<bash-src> bash
   tests/bash-test-suite/runner.sh` → `rhs-exp | PASS`, empty diff. Update
   `docs/bash-test-suite-baseline.md` PASS count 17 → 18 and move `rhs-exp`
   from the FAIL near-miss list to PASS.

4. **Full sweep unchanged.** `tests/scripts/run_diff_checks.sh` must stay
   green (the fix is lexer-local; no `-c` behavior changes elsewhere).

Per repo constraints: build the binary with `cargo build -p huck`; run
per-crate tests single-threaded (`cargo test -p huck-syntax --lib --jobs 1 --
--test-threads 1`); guard the bash-diff sweep with `ulimit -v 1500000` +
`timeout`.

## Scope

**In scope.** The single backslash-arm gate in `scan_step_param_operand`
(huck-syntax); the `rhs_exp_nested_quote_diff_check.sh` harness; lexer unit
tests; the baseline-doc PASS-count update.

**Out of scope.** Pattern operands (`#`/`%`/`/`) and substring offsets;
single-quote span handling; any other `rhs-exp` behavior (the category's only
remaining divergence is this one root — the diff is exactly the three
nested-quote hunks).

## Documentation

- `docs/bash-test-suite-baseline.md`: PASS 17 → 18; move `rhs-exp` to PASS.
- `docs/architecture.md`: if the param-operand quoting rules are described
  there, note the `enclosing_dquote`-gated nested-quote backslash strip.
- No new intentional divergence (this removes a divergence); #253 auto-closes
  via the PR body (`Closes #253`).
