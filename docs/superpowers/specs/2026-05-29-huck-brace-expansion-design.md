# huck v46 — Brace expansion (M-61)

## Goal

Add brace expansion to huck: `{a,b,c}`, `{1..5}`, `{a..e}`,
`{01..10}`, `{1..10..2}`, prefix/suffix (`pre{a,b}post`), Cartesian
product across consecutive braces (`{a,b}{c,d}`), and nested forms
(`{a,{b,c}}`). Brace expansion runs at the lexer stage — one input
Word that contains an unquoted brace produces multiple Word tokens.

This is a new tracked divergence: **M-61: Brace expansion**, added
to `docs/bash-divergences.md` as part of this iteration.

## Scope decisions (locked)

1. **Feature scope**: full bash — comma lists, integer ranges, char
   ranges, zero-pad, step, prefix/suffix, nested, Cartesian.
2. **Cartesian product**: include — `{a,b}{c,d}` → `ac ad bc bd`.
3. **Safety cap**: 65,536 expansions per word. Exceeding → lex error.

## Out of scope (deferred)

- Process substitution (`<(cmd)`, `>(cmd)`) — bash extension unrelated
  to brace expansion.
- Brace expansion inside command substitution (`$(echo {a,b})`) is
  inherited "for free": command substitution captures stdout, which
  is already brace-expanded by the inner shell pass.
- Brace expansion of the second-or-later expansion stage. Brace
  expansion always runs FIRST (before parameter / command / arith /
  pathname). After v46 there is no `${var}` → brace-expand
  interaction; that's a separate divergence.

## Architecture

Two files change:

1. **`src/brace_expand.rs`** — new module containing:
   - `pub fn expand(input: &str) -> Result<Vec<String>, BraceError>`
   - `BraceError` enum with one variant: `TooManyElements`.
   - Internal helpers for body parsing (comma split, integer range,
     char range, zero-pad detection).
   - Unit tests.

2. **`src/lexer.rs`** — at the Word-emission point, detect unquoted
   braces, build a placeholder-bearing concat string, call
   `brace_expand::expand`, then split each result back into
   `WordPart` sequences and emit one Token::Word per result.

3. **`src/lib.rs` or wherever modules are declared** — add
   `pub mod brace_expand;`.

### `expand` algorithm

```
expand(s):
  find first top-level unquoted `{`:
    scan with a depth counter; track sentinel-protected regions
    (\u{0001}..\u{0002} blocks are placeholders and are skipped)
  if no `{` found:
    return [s]
  
  prefix = s[0..lbrace_idx]
  find matching `}` at the same nest level
  if no matching `}`:
    return [s]            // not a brace expression; treat as literal
  body = s[lbrace_idx+1..rbrace_idx]
  suffix = s[rbrace_idx+1..]
  
  items = parse_body(body):
    1. try comma-split at top level (depth-0 commas):
       if >=2 items: return Some(comma list)
    2. try range parse:
       <int>..<int>[..<step>] -> integer range
       <char>..<char>[..<step>] -> char range
       leading zeros on either int endpoint -> zero-padded
    3. if neither: return None (treat as literal)
  
  if items is None:
    // body looked like a brace expr but wasn't valid
    // recurse on what comes AFTER the close brace, so nested
    // unrelated braces still expand
    let rest_expanded = expand(suffix)?
    return rest_expanded.into_iter()
      .map(|r| format!("{prefix}{{{body}}}{r}"))
      .collect()
  
  results = vec![]
  for item in items:
    // item itself may contain braces (nested case)
    let item_expansions = expand(item)?
    for item_expanded in item_expansions:
      let combined = format!("{prefix}{item_expanded}{suffix}")
      // Tail-recurse to handle sequential braces like {a,b}{c,d}
      let nested_expansions = expand(&combined)?
      for n in nested_expansions:
        results.push(n)
        if results.len() > 65_536:
          return Err(BraceError::TooManyElements)
  return Ok(results)
```

Edge: the "fall through to literal" path for invalid brace bodies
matters for `{a` (no close) or `{1..a}` (mixed type) — both must
return the input unchanged.

### `parse_body` (helper)

```rust
fn parse_body(body: &str) -> Option<Vec<String>>
```

1. **Comma split at top level**: walk char by char, tracking brace
   nest depth, collect items between depth-0 commas.
   - If split produces ≥2 non-empty items → return them.
   - If only one item → fall through to step 2.

2. **Integer range**: look for `..` separator. Parse left and right
   as `i64`. If both parse:
   - Step defaults to `1` (or `-1` if right < left).
   - Optional `..step` suffix: parse as positive int.
   - Zero-pad: if either endpoint starts with `0` (and has length ≥2,
     or both endpoints are exactly `0`), compute pad width as
     `max(left_str.len(), right_str.len())`.
   - Generate sequence: start, start+step, ... while in range.
   - Format each as zero-padded if pad width is set.

3. **Char range**: only if both endpoints are exactly one char each
   AND step parsing (if present) gives a positive int. Generate
   sequence using `char` arithmetic (cast to u32, step, cast back).
   - Same direction handling as integer (descending if right < left).
   - No zero-pad for char ranges (always single-char output).

4. **Neither**: return `None`.

### Range edge cases

- `{1..1}` → `[1]` (single-element range).
- `{1..0}` → `[1, 0]` (descending by default step -1).
- `{1..10..3}` → `[1, 4, 7, 10]` (10 is included if it's exactly
  reachable; otherwise the last in-range value).
- `{a..a}` → `[a]`.
- `{a..A}` → `[a, ..., A]` if cast-to-u32 walks downward; this is
  bash-compatible (cross-case ranges include intermediate chars).
- `{1..10..0}` → invalid (zero step) → fall through to literal.
- `{1..-3}` → `[1, 0, -1, -2, -3]` (negative endpoint OK).

### Sentinels and lexer integration

`build_concat_with_sentinels(parts: &[WordPart]) -> (String, Vec<WordPart>)`:

- Result `String` interleaves literal unquoted text with sentinel
  blocks `\u{0001}<idx>\u{0002}` for each non-Literal or quoted part.
- `Vec<WordPart>` is the placeholder list, indexed by the `<idx>` byte.

Then `split_on_sentinels(s: &str, placeholders: &[WordPart])` walks
`s` and rebuilds a `Vec<WordPart>`: literal text becomes
`WordPart::Literal { quoted: false }`; each sentinel block is
replaced by `placeholders[idx].clone()`.

### Lexer code in `tokenize`

Currently emits `tokens.push(Token::Word(Word(parts)))`. Replace
that call with a helper `emit_word_with_braces(tokens, parts)`:

```rust
fn emit_word_with_braces(
    tokens: &mut Vec<Token>,
    parts: Vec<WordPart>,
) -> Result<(), LexError> {
    if !word_contains_unquoted_brace(&parts) {
        tokens.push(Token::Word(Word(parts)));
        return Ok(());
    }
    let (concat, placeholders) = build_concat_with_sentinels(&parts);
    let expansions = crate::brace_expand::expand(&concat)
        .map_err(|_| LexError::BraceExpansionLimit)?;
    for s in expansions {
        let new_parts = split_on_sentinels(&s, &placeholders);
        tokens.push(Token::Word(Word(new_parts)));
    }
    Ok(())
}
```

`word_contains_unquoted_brace`: returns true if any
`WordPart::Literal { text, quoted: false }` contains `{`.

Note: there are multiple `tokens.push(Token::Word(...))` sites in
`tokenize`. ALL of them must be routed through the helper for the
feature to work. The current sites are at the natural word-boundary
points (whitespace, end of input, before operator).

### `LexError` extension

Add one new variant:

```rust
LexError::BraceExpansionLimit
```

Render in `lex_error_message`:

```rust
LexError::BraceExpansionLimit => ": brace expansion: too many elements".to_string(),
```

## Test plan

### Unit tests in `src/brace_expand.rs#[cfg(test)] mod tests`

12 tests:

1. `comma_list_simple` — `expand("{a,b,c}")` → `["a", "b", "c"]`.
2. `comma_list_with_prefix_suffix` — `expand("pre{a,b}post")` →
   `["preapost", "prebpost"]`.
3. `integer_range_ascending` — `expand("{1..5}")` →
   `["1", "2", "3", "4", "5"]`.
4. `integer_range_descending` — `expand("{5..1}")` →
   `["5", "4", "3", "2", "1"]`.
5. `integer_range_with_step` — `expand("{1..10..2}")` →
   `["1", "3", "5", "7", "9"]`.
6. `char_range_ascending` — `expand("{a..e}")` →
   `["a", "b", "c", "d", "e"]`.
7. `zero_padded_range` — `expand("{01..05}")` →
   `["01", "02", "03", "04", "05"]`.
8. `nested_brace` — `expand("{a,{b,c}}")` → `["a", "b", "c"]`.
9. `cartesian_two_braces` — `expand("{a,b}{c,d}")` →
   `["ac", "ad", "bc", "bd"]`.
10. `invalid_brace_is_literal` — `expand("{a")` → `["{a"]`.
11. `invalid_range_falls_through` — `expand("{1..a}")` →
    `["{1..a}"]`.
12. `too_many_elements_errors` — `expand("{1..70000}")` →
    `Err(BraceError::TooManyElements)`.

### Lexer-level unit tests in `src/lexer.rs#[cfg(test)] mod tests`

5 tests, drive `tokenize` and assert on the token stream:

13. `tokenize_brace_emits_multiple_words` — input `"echo {a,b,c}"`
    yields 4 word tokens (`echo`, `a`, `b`, `c`).
14. `tokenize_brace_preserves_var` — input `"echo $x{a,b}"` yields 3
    word tokens (`echo`, then two words each containing a Var part
    followed by a Literal part).
15. `tokenize_quoted_brace_not_expanded` — input `"echo \"{a,b}\""`
    yields 2 word tokens (`echo` and a single quoted Literal
    `{a,b}`).
16. `tokenize_single_quoted_brace_not_expanded` — input
    `"echo '{a,b}'"` yields 2 word tokens.
17. `tokenize_backslash_brace_not_expanded` — input
    `"echo \\{a,b\\}"` yields 2 word tokens (the `\{...\}`
    literal). NOTE: backslash escapes are handled by huck's existing
    backslash logic; the brace expander never sees the `{` because
    it's not in the Literal text as `{` — it's `{` already (the
    lexer's escape processing turns `\{` into a `{` literal that's
    treated as quoted-for-brace-purposes). Confirm the lexer's
    backslash arm strips the `\` and produces a Literal with
    `quoted: true` for that char; otherwise we need to track
    escaped braces separately.

    If the lexer's backslash handling does NOT mark the resulting
    Literal as quoted, this test must change to assert the current
    behavior (brace expansion happens despite the escape — a known
    divergence to document as L-*).

### Integration tests in `tests/brace_expansion_integration.rs`

3 tests:

1. `brace_list_in_echo` — script `echo {a,b,c}\nexit\n`; stdout
   contains a line `a b c`.
2. `brace_range_in_for_loop` — script `for i in {1..3}; do echo
   "i=$i"; done\nexit\n`; stdout contains lines `i=1`, `i=2`,
   `i=3`.
3. `brace_cartesian` — script `for d in /tmp/{a,b}/{x,y}; do echo
   $d; done\nexit\n`; stdout contains 4 lines (`/tmp/a/x`,
   `/tmp/a/y`, `/tmp/b/x`, `/tmp/b/y`).

### Smoke

`cargo test --all-targets` must pass. PTY flake tolerated.

## Implementation tasks

1. **Module + lexer integration + 17 unit tests**:
   - Create `src/brace_expand.rs` with `expand`, `parse_body`,
     `BraceError`, and 12 unit tests.
   - Wire `pub mod brace_expand;` into the crate root
     (`src/main.rs` or wherever modules are declared — likely
     `src/main.rs:1+`).
   - Add `LexError::BraceExpansionLimit` and its
     `lex_error_message` arm in `src/shell.rs::lex_error_message`.
   - Implement `word_contains_unquoted_brace`,
     `build_concat_with_sentinels`, `split_on_sentinels`,
     `emit_word_with_braces` in `src/lexer.rs`.
   - Replace every `tokens.push(Token::Word(Word(parts)))` site in
     `tokenize` with `emit_word_with_braces(&mut tokens, parts)?`.
   - Append 5 lexer unit tests.
2. **Integration tests**: create
   `tests/brace_expansion_integration.rs` with 3 tests.
3. **Docs**:
   - Add **M-61: Brace expansion** entry to
     `docs/bash-divergences.md` as `[fixed v46]`. Insert in the
     appropriate section (probably between the Tier 2 / "Word
     expansion" subsection — find the right home).
   - Add change-log entry.
   - Add README v46 row.
   - Remove `brace expansion (\`{a,b,c}\`)` from README's "Not yet
     implemented" stanza.

Three tasks. TDD within each.

## Acceptance criteria

- All 12 brace_expand unit tests pass.
- All 5 lexer integration unit tests pass.
- All 3 integration tests pass.
- `cargo test --all-targets` passes (modulo known PTY flake).
- `cargo clippy --all-targets -- -D warnings` passes.
- `docs/bash-divergences.md` has the new M-61 entry as
  `[fixed v46]`.
- README v46 row added; "Not yet implemented" stanza no longer
  lists brace expansion.
- `echo {a,b,c}` prints `a b c`.
- `for i in {1..5}; do echo $i; done` prints `1 2 3 4 5` on
  separate lines.
- Quoted braces are NOT expanded (`echo "{a,b}"` prints
  `{a,b}`).
