# v333 — Flip the `nquote` bash-suite category to PASS

Issue: [#289](https://github.com/jdstanhope/huck/issues/289) — `$'\c\\'` off-by-one
and `$'…'` wrongly expanded in heredoc value-word `${…}` operands.

## Problem

The `nquote` bash-suite category is a near-miss (diff 12 lines) with two roots in
`$'…'` (ANSI-C) quoting. Fixing both takes the category to **0-diff → PASS**
(Summary PASS 21→22, FAIL 61→60). Both are verified byte-identical to bash 5.2.21.

### Root A — `\c\\` control-char escape off-by-one

In `$'…'`, `\c\` is Ctrl-\ (0x1C). The **escaped-backslash form `\c\\`** must
consume BOTH backslashes; huck's `\c` arm read a single following char, so `\c\\`
consumed only `\c\` (3 chars), leaving a stray `\` that corrupted the next escape.

```
bash  $'\c\\'              -> 1c
bash  $'\c\a'              -> 1c 61
bash  $'\c\\\c]'           -> 1c 1d
bash  $'\c[\c\\\c]\c^\c_\c?' -> 1b 1c 1d 1e 1f 7f
huck (before) $'\c\\\c]'   -> 1c 5c 63 5d   (fs, then literal `\c]`)
```

### Root B — `$'…'` in a heredoc VALUE-WORD `${…}` operand

bash disables `$'…'` ANSI-C quoting entirely inside a here-document body — INCLUDING
inside a `${x-word}` **value-word** operand — but a **pattern** operand
(`${x%…}`/`${x#…}`/`${x/…}`/case) still expands it (patterns are always
glob-processed). huck expanded `$'…'` in both.

```
# in a heredoc body:
${none-a$'\01'b}   bash: a$'\01'b (literal)   huck(before): a<0x01>b   # value word
${V%a$'\01'b}      bash: pattern matches (expands)                     # pattern
```
Verified per operator in a heredoc: `-`/`=`/`?`/`+` (value) → **literal**;
`%`/`#`/`/`-replacement/`^`,`,`-case (pattern/replacement) → **expand**. Outside a
heredoc (dquote / unquoted) BOTH expand — only the heredoc value-word case is wrong.

The value-word-vs-pattern distinction is known ONLY to the parser: `${x-word}` and
`${x%pat}` both push the same `Mode::ParamWordOperand` (only `${x/…}` replacement
uses a distinct pattern mode), so the lexer cannot tell them apart from the atoms
alone — the `-` vs `%` operator lives in the parser.

## Design

Two independent fixes. Root A is a localized decoder arm. Root B threads a
parser-set `is_pattern` flag into the mode so the lexer's `$'` arm can suppress
ANSI-C only for a heredoc value word — **parser-driven, no lexer lookahead, no
lexer→parser dependency** (the lexer only reads a mode field the parser set).

### Root A — `crates/huck-syntax/src/lexer.rs`, `decode_ansi_c_escape`

Add an explicit `Some('\\')` arm to the `\c` match (before the generic `Some(c)`),
consuming the escaped-backslash pair:

```rust
Some('\\') => {
    // `\c\` is Ctrl-\ (0x1C). The escaped-backslash form `\c\\` consumes
    // the SECOND backslash as part of the control target; `\c\X` for any
    // other X yields Ctrl-\ and leaves X. Matches bash: `\c\\` -> 1c,
    // `\c\a` -> 1c 61, `\c\\\c]` -> 1c 1d.
    if chars.peek() == Some(&'\\') {
        chars.next();
    }
    out.push('\x1C');
}
```

### Root B — `is_pattern` on `Mode::ParamWordOperand`

1. **`crates/huck-syntax/src/lexer.rs`**:
   - Add `is_pattern: bool` to `Mode::ParamWordOperand`.
   - The dispatch destructures it and passes it to `scan_step_param_operand`; the
     other operand modes (`ParamSubstPatternOperand`/`ParamSubstringOffsetOperand`/
     `ParamSubscriptOperand`) pass `is_pattern = true` (patterns are glob-processed;
     offset/subscript are arithmetic — all keep the always-expand behavior).
   - `scan_step_param_operand` gains an `is_pattern: bool` parameter; its `$'…'`
     arm becomes `Some('\'') if !(self.emitting_heredoc.is_some() && !is_pattern)`
     — i.e. suppress ANSI-C only when emitting a heredoc body AND this is a value
     word. On suppression it falls through to the lone-`$` literal arm; the `'`
     then scans as an ordinary run char (as a bare `'…'` already does in a heredoc
     operand). `emitting_heredoc` stays `Some` while nested operand atoms are
     scanned mid-body, so this is the correct signal — read-only, no lookahead.

2. **`crates/huck-syntax/src/parser.rs`** — set `is_pattern` at every
   `Mode::ParamWordOperand` construction (the parser knows the operator):
   - `false` (VALUE word): `UseDefault` (`-`), `AssignDefault` (`=`),
     `ErrorIfUnset` (`?`), `UseAlternate` (`+`).
   - `true` (PATTERN / expand): `RemovePrefix` (`#`), `RemoveSuffix` (`%`), the
     `/`-replacement word, `Case` (`^`/`,`) pattern, the substring length operand,
     and the two bad-substitution cleanup operands (word discarded — value
     irrelevant, use `true`).

   (`bash`-verified in a heredoc: replacement `${V/X/$'\t'}` and case `${v^$'\t'}`
   EXPAND; `${none=X$'\t'Y}` and `${set+X$'\t'Y}` stay LITERAL.)

3. Fix the `Mode::ParamWordOperand` constructions in the lexer/parser test modules
   (they gain `is_pattern: false`).

## Testing

Gate = bash 5.2.21 fidelity + `nquote` at 0 diff.

1. **Bash-diff harness** `tests/scripts/nquote_ansi_c_diff_check.sh` (model on an
   existing `-c`/heredoc harness), byte-identical incl. stderr + exit:
   - Root A: `printf '%s' $'\c\\'` / `$'\c\a'` / `$'\c\\\c]'` /
     `$'\c[\c\\\c]\c^\c_\c?'` — od-compared to bash.
   - Root B value word (heredoc, LITERAL): `${none-X$'\t'Y}`, `${none=…}`,
     `${set+…}`.
   - Root B pattern (heredoc, EXPAND): `${V%$'\t'}`, `${V#…}`, `${V/X/$'\t'}`,
     `${v^$'\t'}`.
   - Regression: dquote `"${none-X$'\t'Y}"` and unquoted `${none-X$'\t'Y}` still
     EXPAND; a bare `$'\t'` in a heredoc body already stays literal (unchanged).
2. **`nquote` category** flips: `HUCK_BASH_TEST_CATEGORY=nquote` → PASS, 0 diff
   (was 12).
3. **Regression**: huck-syntax + huck-engine lib green; nquote1/nquote5/iquote/
   cprint/rhs-exp stay PASS (nquote2/3/4 are a separate Ctrl-A/hex-escape class,
   unaffected — #52/#63); the param-substitution / ansi-c / heredoc / here-string /
   array / braced-special / indirect `-p huck` integration bins green; full
   `run_diff_checks.sh` sweep green.

Per repo constraints: build with `cargo build -p huck`; per-crate tests
single-threaded; NEVER `cargo test --workspace`; guard sweeps with
`ulimit -v 1500000` + `timeout`; run the `-p huck` integration bins
single-threaded before push; NO GPL bash text.

## Scope

**In scope.** Root A (`\c\\`); Root B (`is_pattern` on `ParamWordOperand` + the
parser setters + the lexer gate); the harness; the category flip; regressions.

**Out of scope.** The nquote2/3/4 divergences (Ctrl-A IFS word-splitting;
`$'\xHH'`/`$'\nnn'` codepoint-vs-byte and C1 controls — #52/#63); `$"…"` locale
quoting in a heredoc value word (a separate, pre-existing divergence not in the
`nquote` diff). No refactor of the param-expansion subsystem beyond adding the one
`is_pattern` flag.

## Documentation

- Removes a divergence (no new intentional one). #289 auto-closes via the PR
  (`Closes #289`). `docs/bash-divergences.md` unchanged.
- Update `docs/bash-test-suite-baseline.md` ("Updated by v333": `nquote` PASS,
  Summary PASS 21→22, FAIL 61→60); record in `project_huck_iterations.md` +
  `MEMORY.md`.
