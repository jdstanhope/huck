# huck v39 — `$'…'` ANSI-C quoting (M-28)

## Goal

Close bash divergence M-28: implement `$'…'` ANSI-C quoting. The text between the
opening `$'` and the matching `'` is single-quoted (no parameter, command, or
arithmetic expansion) **except** that C-style backslash escape sequences are
decoded. The resulting decoded string behaves as if it had been single-quoted —
no word splitting, no globbing.

Current huck behavior: `$'\n'` lexes as a literal `$` token followed by a
single-quoted Literal containing the two characters `\` and `n`. After v39 it
must produce one quoted Literal whose `text` is a single newline character.

## Scope decisions (locked)

1. **Escape coverage:** full bash set (all 16 forms — listed below).
2. **`\xHH` / `\nnn` interpretation:** treat the numeric value as a Unicode
   codepoint, not a raw byte. Lossless within Rust `String`; diverges from bash
   only for high-bit values, which becomes a new L-* divergence entry.
3. **Unknown escapes:** bash-faithful — preserve both the backslash and the
   following character literally. `$'\q'` → the two-char string `\q`.

## Architecture

Surface area is purely lexical. The change lives entirely in `src/lexer.rs`:

1. **New arm in `read_dollar_expansion`** (currently `src/lexer.rs:683`):
   add `Some('\'') => { … }` between the existing arms. It consumes the
   opening `'`, scans up to the matching unescaped `'`, decoding escapes as it
   goes, and emits a single `WordPart::Literal { text: decoded, quoted: true }`.
   The `quoted: true` flag is what downstream stages already use to suppress
   word splitting and globbing.

2. **New helper `read_ansi_c_quoted`** (private function in `src/lexer.rs`):

   ```rust
   fn read_ansi_c_quoted(
       chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
   ) -> Result<String, LexError>
   ```

   Walks the iterator until an unescaped `'` is consumed, returning the decoded
   text. Encounters EOF → `LexError::UnterminatedQuote`.

3. **Two new `LexError` variants** (in the existing `LexError` enum):
   - `AnsiCInvalidCodepoint(u32)` — `\u`/`\U`/`\xHH`/`\nnn` resolved to a `u32`
     that is not a valid Unicode codepoint (surrogate range or > U+10FFFF).
   - (Existing `UnterminatedQuote` is reused for the unterminated case.)

No parser, AST, executor, parameter-expansion, arithmetic, or trap changes.
Caller stages already handle a `WordPart::Literal { quoted: true }` correctly
(it's the same shape that `'…'` produces today).

## Escape table

All 16 bash escapes are supported:

| Escape | Result codepoint |
|---|---|
| `\a` | U+0007 (BEL) |
| `\b` | U+0008 (BS) |
| `\e`, `\E` | U+001B (ESC) |
| `\f` | U+000C (FF) |
| `\n` | U+000A (LF) |
| `\r` | U+000D (CR) |
| `\t` | U+0009 (HT) |
| `\v` | U+000B (VT) |
| `\\` | U+005C (`\`) |
| `\'` | U+0027 (`'`) — the only way to embed `'` |
| `\"` | U+0022 (`"`) |
| `\?` | U+003F (`?`) |
| `\nnn` | 1–3 octal digits (0–7), value taken as Unicode codepoint |
| `\xHH` | 1–2 hex digits, value taken as Unicode codepoint |
| `\uXXXX` | 1–4 hex digits, Unicode codepoint |
| `\UXXXXXXXX` | 1–8 hex digits, Unicode codepoint |
| `\cX` | control char: `X & 0x1F` for letters; `\c?` → U+007F (DEL) |

### Greedy digit consumption rules

- `\nnn`: at most 3 octal digits (`0`–`7`). Stop at first non-octal digit. At
  least one digit must follow `\` for this rule to fire; otherwise it's a
  different escape entirely (the `\` arm is selected by the first char).
  Example: `\18` → U+0001 then literal `8`. Example: `\012` → U+000A.
- `\xHH`: at most 2 hex digits. `\x` followed by no hex digit → literal `\x`
  (unknown-escape rule applies).
- `\uXXXX`: at most 4 hex digits. `\u` with no hex digit → literal `\u`.
- `\UXXXXXXXX`: at most 8 hex digits. `\U` with no hex digit → literal `\U`.

### Control-char rule (`\cX`)

- `\cA`..`\cZ` and `\ca`..`\cz` → U+0001..U+001A (bit 6 cleared, i.e.
  `(c.to_ascii_uppercase() as u32) & 0x1F`).
- `\c?` → U+007F.
- `\c@` → U+0000.
- Other `\cX` characters: take low 5 bits of the ASCII value (matches bash).
- `\c` with no follow-char (EOF) → literal `\c` (unknown-escape rule).

### Unknown escape

Any backslash sequence not matched above — including `\1xx` where `xx` are not
octal, `\xZZ`, or `\q` — falls through as the literal two-character sequence
`\X`. This is the bash-faithful behavior selected in scope-decision Q3.

## Edge cases

- **Empty body**: `$''` emits one `WordPart::Literal { text: "", quoted: true }`,
  matching the existing `''` empty-quote contract.
- **EOF inside body**: any EOF before the closing `'` → `LexError::UnterminatedQuote`.
- **`\` at EOF inside body**: same — `UnterminatedQuote`.
- **Embedding `'`**: only via `\'`. The closing `'` is matched on the first
  non-escaped quote, so `$'a\'b'` → the three-char string `a'b`.
- **Codepoint validation**: `char::from_u32(v)` is called for every numeric
  escape. Returns `None` for surrogates (U+D800–U+DFFF) and values > U+10FFFF;
  those produce `LexError::AnsiCInvalidCodepoint(v)`.
- **Concatenation**: `$'\n'foo"$x"` produces three `WordPart`s in the same
  `Word`: quoted Literal `"\n"`, unquoted Literal `"foo"`, quoted Var `x`.
  Existing word concatenation already handles this — no parser change needed.
- **Inside `"…"`**: bash 5.1+ does NOT process `$'…'` inside double quotes; the
  `$` is treated literally. huck's double-quoted scanner does not call
  `read_dollar_expansion` when followed by `'`, and we do not change that.
  v39 only activates outside double quotes.
- **`[[ ]]` and `case`**: these consume `Word` tokens, so they naturally get the
  decoded quoted Literal — no special handling needed.

## Divergences introduced

A new entry in `docs/bash-divergences.md` (next available number is L-11; the
v33 entry already takes L-10):

> **L-11: `$'\xHH'` and `$'\nnn'` with values > 0x7F produce UTF-8 codepoints,
> not raw bytes** — `[divergence v39]` low. bash: `$'\xFF'` inserts byte 0xFF.
> huck: inserts U+00FF (UTF-8 bytes 0xC3 0xBF). Consequence of huck's
> Unicode-by-default convention (aligns with L-04). Scripts that depend on raw
> high bytes are affected; ASCII-range escapes (`< 0x80`) are bit-identical.

M-28 itself moves from `[deferred]` to `[fixed v39]`.

## Test plan

### Lexer unit tests (in `src/lexer.rs`)

~10 tests, each driving the full pipeline (`tokenize`) and asserting on the
emitted `Token::Word` parts:

1. `$'\n'` → one quoted Literal containing `"\n"`.
2. `$'a\tb'` → one quoted Literal `"a\tb"`.
3. `$'\\\''` → one quoted Literal `"\\'"`.
4. `$'\x48\x69'` → quoted Literal `"Hi"`.
5. `$'é'` → quoted Literal `"é"`.
6. `$'\U0001F600'` → quoted Literal containing the grinning-face emoji.
7. `$'\cA\cZ'` → quoted Literal `"\x01\x1a"`.
8. `$'\q'` → quoted Literal `"\\q"` (two chars: backslash + q).
9. `$''` → one empty quoted Literal.
10. `$'unterminated` (no closing quote) → `Err(LexError::UnterminatedQuote)`.
11. `$'\uD800'` → `Err(LexError::AnsiCInvalidCodepoint(0xD800))`.
12. `$'a\nb'foo` (concatenation) → single Word with two Literal parts.

### Integration tests (in `tests/ansi_c_quoting_integration.rs`)

~6 binary-driven tests using the existing integration harness:

1. `echo $'a\tb'` → stdout `a\tb` followed by newline (verify tab is real).
2. `echo $'é'` → stdout `é\n`.
3. `printf '%s' $'\x48\x69'` → stdout `Hi` (no newline).
4. `x=$'\n'; echo "[$x]"` → stdout `[\n]`.
5. `for c in $'\cA' $'\cZ'; do printf '%d ' "'$c"; done; echo` → stdout `1 26 `.
6. `case $'\t' in $'\t') echo yes ;; *) echo no ;; esac` → stdout `yes`.

### Smoke

Full suite (`cargo test --all-targets`) must pass after the change. PTY flake
`pty_compound_stage_pipeline_stops_and_resumes` is tolerated under load per
prior iterations.

## Implementation tasks

1. **Lexer recognizer + decoder** — add the new `Some('\'')` arm in
   `read_dollar_expansion`, add `read_ansi_c_quoted` helper, add
   `AnsiCInvalidCodepoint(u32)` variant to `LexError` with a `Display` arm,
   plus the ~10 unit tests above. Test-first.
2. **Integration tests** — create `tests/ansi_c_quoting_integration.rs`
   covering the six scenarios above.
3. **Docs** — flip M-28 to `[fixed v39]`, add the new L-NN entry for
   byte-vs-codepoint, add a README v39 row, add a CHANGELOG entry, and verify
   the full suite passes.

Three tasks, one commit per task. TDD within each task.

## Acceptance criteria

- All new lexer unit tests pass.
- All new integration tests pass.
- `cargo test --all-targets` passes (modulo the known PTY flake).
- `clippy --all-targets -- -D warnings` passes.
- `docs/bash-divergences.md` shows M-28 as `[fixed v39]` with the new L-NN
  divergence row.
- A `$'…'` expression anywhere a `Word` is expected behaves as if the decoded
  string had been single-quoted, with no further expansion.
