# v192: `name=\`⏎`(array)` line continuation before an array literal — Design

**Status:** approved 2026-06-19
**Iteration:** v192
**Origin:** Parse sweep gap on `/usr/bin/byobu-ulevel` (line 87):
`theme_list=\`⏎`(…)` — an array assignment with a `\`-newline line continuation
between `=` and `(`. bash parses it (`bash -n` rc 0; runs fine); huck errors
`syntax error: function definition: expected '()' and a compound-command body`.

## bash contract (verified)

`\`⏎ (backslash-newline) is a line continuation that bash **deletes before
tokenizing** (POSIX 2.2.1), so `arr=\`⏎`(a b c)` is exactly `arr=(a b c)`:

- `arr=\`⏎`(a b c); echo "${arr[1]}"` → `b` (a 3-element indexed array).
- `arr+=\`⏎`(d); echo "${arr[3]}"` → `d` (append form).
- Multiple stacked continuations collapse: `arr=\`⏎`\`⏎`(x)` ≡ `arr=(x)`.
- `arr=\`⏎`foo` ≡ `arr=foo` (scalar — already works in huck).

## Root cause

huck's main tokenizer detects a compound-array RHS by peeking for `(`
IMMEDIATELY after `=`/`+=` (`src/lexer.rs`):

- the `'='` arm (~line 918): `… current.push('='); if chars.peek() == Some(&'(') { … scan_array_literal … }`
- the `'+' if …'='…` arm (~line 952): the `name+=(` form.

For `arr=\`⏎`(`, the char after `=` is the continuation `\`, not `(`. The peek
fails, no array is detected, and the lexer continues: the `\`⏎ is later deleted
by the generic `'\\'` arm, leaving a bare `(a b c)` that the parser reads as a
function definition / subshell → the misleading "function definition" error.
(huck handles `\`⏎ everywhere else fine; it breaks ONLY at this lookahead peek,
which happens before the continuation is consumed.)

## Design

### 1. `skip_line_continuations` helper (`src/lexer.rs`)

```rust
/// Consumes any run of `\`-newline line continuations at the cursor (POSIX
/// 2.2.1: `\<NL>` is deleted before tokenizing). Uses a cloned-cursor 2-char
/// lookahead so a `\` NOT followed by a newline (a real escape like `\x`) is
/// left untouched. No-op when the cursor isn't at a `\<NL>`.
fn skip_line_continuations(chars: &mut CharCursor<'_>) {
    loop {
        let mut probe = chars.clone();
        if probe.next() == Some('\\') && probe.next() == Some('\n') {
            *chars = probe;
        } else {
            return;
        }
    }
}
```

### 2. Call it before the array-`(` peek in the two array-assignment arms

- `'='` arm (~line 924): replace `if chars.peek() == Some(&'(')` with
  `skip_line_continuations(&mut chars); if chars.peek() == Some(&'(')`.
- `'+='` arm (~line 952): the same insertion before its `(` peek.

This is safe and bash-faithful: a `\`⏎ here is deleted regardless (the generic
`'\\'` arm would consume it later for the non-`(` case), so consuming it eagerly
only ADDS the missing array-after-continuation detection without changing the
scalar cases (`arr=\`⏎`foo` still → `arr=foo`; `arr=\x` is untouched — `\x` is
not a continuation).

### 3. NOT the subscripted-lvalue peek (~line 1027)

The `name[i]=(…)` peek is left alone: bash **rejects** `a[2]=(x)` ("cannot assign
list to array member"), so adding the continuation-skip there would make huck
*more* lenient on an already-invalid form. Out of scope.

### 4. Document the `${…}` parse-strictness as intentional

Add an `[intentional]` entry to `docs/bash-divergences.md`: huck rejects
malformed `${…}` (`${}`, `${=1}`, `${ x}`, `${@x}`, `${1abc}`, `${-x}`, `${.}`)
at PARSE time; bash parses them (`bash -n` rc 0) and emits the identical "bad
substitution" only at RUNTIME. huck's early error is by design (the constructs
are invalid in bash either way; catching them at parse is arguably better than
building a deferred-runtime-error path to accept broken syntax). This is the
remaining `${=1}` ×2 sweep entries (perf-completion.sh), now classified, not a
bug to fix.

## Verification

- **New bash-diff harness** `tests/scripts/array_line_continuation_diff_check.sh`
  (byte-identical stdout+exit): `arr=\`⏎`(a b c); printf '%s\n' "${arr[@]}"` and
  `… "${arr[1]}"`; the `+=` append form `arr=(a); arr+=\`⏎`(b c); echo "${arr[2]}"`;
  stacked continuations `arr=\`⏎`\`⏎`(x); echo "${arr[0]}"`; controls
  `arr=\`⏎`foo; echo "$arr"` (scalar) and a normal `arr=(p q); echo "${arr[1]}"`.
- **Lexer unit test** (`src/lexer.rs` `mod tests`): `tokenize("arr=\\\n(a b c)")`
  yields a single assignment Word whose value part is a `WordPart::ArrayLiteral`
  with 3 elements (mirror an existing `name=(…)` array-literal lexer test for the
  exact assertion shape). Plus a negative: `tokenize("arr=\\x")` does NOT produce
  an `ArrayLiteral` (the `\` is a literal escape, not a continuation).
- **Parse-sweep payoff:** re-run `tools/parse_sweep.sh`; `byobu-ulevel` `huck -n`
  rc 0; `HUCK_GAP` 3→2 (the two `perf-completion.sh` `${=1}` entries remain —
  now documented intentional); `HUCK_LENIENT`/`CRASH`/`TIMEOUT` stay 0.
- Full `cargo test` (0 failures); all harnesses + clippy green.

## Docs / close-out

byobu resolves the sweep's last real parse gap (no tracked `M-*`/`L-*` — it was
the deferred v188 candidate). Add the `[intentional]` `${…}`-strictness entry
(adjust the tier count). Record v192 in `project_huck_iterations.md` + `MEMORY.md`;
note the parse sweep is now at **2 intentional gaps, 0 real**.

## Scope boundary

In scope: `skip_line_continuations` + its use before the `name=(` and `name+=(`
array peeks; the harness + lexer tests; the `[intentional]` `${…}` doc entry.
**Not** in scope: the subscripted-lvalue `a[i]=(…)` peek (invalid in bash); the
`${…}` parse-permissive/deferred-bad-substitution feature (intentionally not
built); any other line-continuation site (huck handles `\`⏎ elsewhere already).
