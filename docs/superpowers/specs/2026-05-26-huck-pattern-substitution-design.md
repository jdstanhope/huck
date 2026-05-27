# huck v32 — `${var/pat/repl}` Pattern Substitution Design

**Goal:** Close M-15 by implementing bash's parameter-substitution forms:
`${var/pat/repl}` (first match), `${var//pat/repl}` (all matches),
`${var/#pat/repl}` (anchored at start), `${var/%pat/repl}` (anchored at end),
plus the no-replacement shortcut `${var/pat}` (treats `repl` as empty).

**Why:** Listed as **high impact** in `docs/bash-divergences.md` Tier 2. Real-world
bash scripts use these extensively for path manipulation, extension swapping,
and one-shot string editing. Currently huck fails with
`LexError::InvalidBraceModifier("/")` at lex time, so the whole script aborts.

## Forms

| Syntax | Meaning |
|---|---|
| `${var/pat/repl}` | Replace the first match of `pat` in `$var` with `repl`. |
| `${var/pat}` | Same as `${var/pat/}` — replace first match with empty. |
| `${var//pat/repl}` | Replace all non-overlapping matches. |
| `${var//pat}` | Replace all matches with empty. |
| `${var/#pat/repl}` | Replace `pat` only if it matches at the start of `$var`. |
| `${var/%pat/repl}` | Replace `pat` only if it matches at the end of `$var`. |
| `${var/#pat}` / `${var/%pat}` | Anchored variants with empty `repl`. |

`pat` is a bash glob, evaluated by the existing `glob::Pattern` crate
(same engine as `${var#pat}` / `${var%pat}` — see
`src/param_expansion.rs::remove_prefix`).

## Semantics

- If `$var` is unset or empty, the result is empty (same as the existing
  remove-prefix/suffix behavior on unset).
- If `pat` fails to compile as a glob, return `$var` unchanged. This matches
  `remove_prefix`'s behavior and bash's silent-no-op on malformed patterns.
- `pat` expansion: `pat` is a `Word`, expanded via `expand_word_to_string`
  before pattern compilation. So `${var/$prefix/X}` works.
- `repl` expansion: `repl` is a `Word`, expanded via `expand_word_to_string`
  before substitution. Standard expansions apply (`$`, `\$`, `${…}`, etc.).
- **Longest match at each starting position** for unanchored substitution —
  this matches bash's behavior for `${var/foo*/X}` against `foobarbaz` (the
  whole tail matches and is replaced, not just `foo`).
- **All-mode advances past the matched span** to avoid overlapping replacements:
  after replacing chars `[s, e)`, the next scan starts at `e`.
- **Empty-match guard:** if the longest match at position `i` is the empty
  string (e.g. pattern `*` against any string can match empty), advance `i` by
  one UTF-8 char to avoid infinite loops. Matches bash behavior.
- **`#`/`%` anchors:** `/#` only tries the match starting at index 0; `/%` only
  tries the match aligned to the end. Both implicitly behave as `all=false`
  (you can't have multiple matches at a single anchor).
- **UTF-8 boundaries:** all index advancement uses `char_indices`, same as
  `remove_prefix`/`remove_suffix`.

## Scope (in)

- All six forms in the table above.
- Pattern + replacement undergo full Word expansion before substitution.
- `\/` inside the lex-time operand scan escapes a literal slash so it doesn't
  end the pattern. Other escape semantics inside the operand match existing
  `${var#pat}` operand handling.
- Quoted contexts: `"${var/pat/repl}"` produces a single field, same as
  `"${var}"` (parser already routes through `ParamExpansion { quoted, .. }`).

## Scope (out — deferred to later iterations)

- M-16 `${var:off:len}` substring — separate ticket.
- M-17 `${var^^}` / `${var,,}` case modification — separate ticket.
- `${@/pat/repl}` / `${*/pat/repl}` array-style substitution — huck has no
  arrays. The existing positional-parameters path uses `${N}` only and won't
  route here.
- Regex patterns — bash's `${var/pat/repl}` is glob-only too, so this is not
  a divergence.

## Files Touched

| File | What changes |
|---|---|
| `src/lexer.rs` | Add `SubstAnchor` enum + `ParamModifier::Substitute { pattern, replacement, anchor, all }` variant. New `Some('/')` arm in `parse_braced_param` (around line 1098 alongside `'#'` / `'%'`). New helper `scan_substitution_operand` that walks the chars iterator and splits pattern from replacement on the first unescaped `/`. |
| `src/param_expansion.rs` | New `ParamModifier::Substitute` arm in `expand_modifier`. New helper `substitute(value, pattern, replacement, anchor, all)` paralleling `remove_prefix`/`remove_suffix`. Unit tests for the helper. |
| `tests/param_expansion_integration.rs` | New integration tests covering all six forms via the binary entry point: first-match, all-match, anchored-prefix, anchored-suffix, empty-repl, UTF-8 boundary, escaped-slash, unset-var, glob with `*`, mid-script use in a pipeline. |
| `docs/bash-divergences.md` | Mark M-15 fixed (Status, changelog line). |
| `README.md` | Bump status table row for parameter expansion if it lists per-form support; bump current-version blurb to v32. |

## AST

```rust
// src/lexer.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubstAnchor {
    None,    // ${var/pat/repl} and ${var//pat/repl}
    Prefix,  // ${var/#pat/repl}
    Suffix,  // ${var/%pat/repl}
}

pub enum ParamModifier {
    // ... existing variants ...
    Substitute {
        pattern: Word,
        replacement: Word,
        anchor: SubstAnchor,
        all: bool,  // true for `//`, false for `/`, `/#`, `/%`
    },
}
```

`all` and `anchor` are independent fields rather than a single enum so the
lexer can set them in parallel without ceremony, mirroring the `longest: bool`
field on `RemovePrefix`/`RemoveSuffix`. Per the semantics above, anchored
variants implicitly behave as `all=false` — we set `all=false` at lex time
for `/#` and `/%` and rely on the evaluator to honor that.

## Lexer Algorithm

Inside `parse_braced_param`, after the existing `Some('%')` arm:

```rust
Some('/') => {
    let all = chars.peek() == Some(&'/');
    if all { chars.next(); }
    let anchor = match chars.peek() {
        Some('#') if !all => { chars.next(); SubstAnchor::Prefix }
        Some('%') if !all => { chars.next(); SubstAnchor::Suffix }
        _ => SubstAnchor::None,
    };
    let (pattern, replacement) = scan_substitution_operand(chars)?;
    parts.push(WordPart::ParamExpansion {
        name,
        modifier: ParamModifier::Substitute { pattern, replacement, anchor, all },
        quoted,
    });
    Ok(())
}
```

`scan_substitution_operand` walks the chars iterator collecting pattern bytes
until an *unescaped* `/` or the closing `}`. On `\/` it consumes the backslash
and pushes a literal `/` to the pattern bytes. On `\\` it consumes both and
pushes one `\`. Other `\x` sequences pass through unchanged so the existing
operand parser sees them. On unescaped `/`, switch to replacement mode and
keep scanning until `}`. Both halves go through `parse_braced_operand` to
produce `Word`s. Missing replacement (closing `}` reached before any `/`)
yields an empty `Word`.

The function returns `Err(LexError::UnterminatedBrace)` if the closing `}`
isn't found, matching existing behavior.

## Evaluator Algorithm

```rust
ParamModifier::Substitute { pattern, replacement, anchor, all } => {
    let v = shell.get(name).unwrap_or("").to_string();
    let pat = expand_word_to_string(pattern, shell);
    let rep = expand_word_to_string(replacement, shell);
    ExpansionResult::Value(substitute(&v, &pat, &rep, *anchor, *all))
}
```

`substitute(value, pattern, replacement, anchor, all)`:

- Compile `pattern` with `glob::Pattern::new`. On error, return `value.to_string()`.
- Build the same `boundaries: Vec<usize>` as `remove_prefix` (char start
  indices + value.len()).
- **Anchor = Prefix:** look for the longest match starting at index 0. If
  found, return `replacement + &value[end..]`. Otherwise return `value`.
- **Anchor = Suffix:** look for the longest match ending at index value.len()
  (i.e. iterate `boundaries` ascending: smallest `start` whose
  `value[start..]` matches is the longest tail match). Return
  `&value[..start] + replacement`.
- **Anchor = None, all = false:** scan `boundaries` ascending; at each `start`,
  find the longest `end > start` (from `boundaries` descending) such that
  `value[start..end]` matches. First start with a non-empty match: return
  `value[..start] + replacement + value[end..]`. If no match found, return
  value unchanged.
- **Anchor = None, all = true:** the same scan as above but in a loop; after
  each replacement, continue scanning from `end`. Empty-match guard: if the
  longest match at `start` is empty (`end == start`), advance `start` by one
  char boundary instead of replacing. Append remaining tail at the end.

The all-mode loop returns the new built string and never modifies the
mid-iteration boundaries (we rebuild for each new scan position from a single
slice). Replacement insertion isn't recursive — the replacement string is
inserted verbatim and not re-scanned.

## Edge cases & invariants

| Input | Expected output | Why |
|---|---|---|
| `var=foobar; echo ${var/o/X}` | `fXobar` | First match only. |
| `var=foobar; echo ${var//o/X}` | `fXXbar` | All matches. |
| `var=hello; echo ${var/#he/HI}` | `HIllo` | Anchored prefix matches. |
| `var=hello; echo ${var/#xo/HI}` | `hello` | Anchored prefix misses. |
| `var=hello; echo ${var/%lo/LO}` | `helLO` | Anchored suffix matches. |
| `var=foo; echo ${var/o}` | `fo` | Empty repl = removal. |
| `var=aaa; echo ${var//a}` | `` | All matches, empty repl. |
| `var=path; echo ${var//\//-}` | `path` (no `/` in value) | Escaped `/` in pattern. |
| `var=a/b/c; echo ${var//\//-}` | `a-b-c` | Escaped `/` in pattern, hits. |
| `var=xyz; echo ${var//*/Q}` | `Q` (longest match, single replacement) | Glob `*` matches the whole string at i=0; advance past tail. |
| `var=; echo ${var/x/y}` | `` | Empty value short-circuits. |
| `var=café; echo ${var/é/E}` | `cafE` | UTF-8 boundary safe. |
| `var=foo; echo ${var/[/X}` | `foo` | Invalid glob → unchanged. |
| `${var/pat` (no `}`) | LexError::UnterminatedBrace | Same as other modifiers. |

## Testing

- **Lexer unit tests** in `src/lexer.rs::tests`: one per form (`/`, `//`,
  `/#`, `/%`), plus operand-scanner edge cases (`\/`, missing repl, empty
  pattern, unterminated `}`). Mirrors existing `RemovePrefix`/`RemoveSuffix`
  test density.
- **Evaluator unit tests** in `src/param_expansion.rs::tests`: covers
  `substitute()` directly with every edge-case row above plus the
  glob-invalid case.
- **Integration tests** in a new file `tests/param_substitution_integration.rs`
  (follows `param_expansion_integration.rs` pattern): pipes scripts through
  the binary and asserts stdout. Cover: every form, mid-script pipeline use,
  inside double quotes, escaped slash, empty result.

## Risks / notes

- The `/` token is currently rejected at lex time; introducing the new arm is
  additive. No existing valid input changes meaning.
- `glob::Pattern` doesn't natively support "longest match starting at i" —
  we work around this with the same `boundaries` scan as
  `remove_prefix`/`remove_suffix`. O(n²) on string length, acceptable for
  shell-script parameter substitution; same complexity bash uses for its
  glob path.
- Replacement string is **not** re-scanned for further substitutions
  (bash-compat — bash also doesn't recurse). So `${var//foo/foofoo}` against
  `foo` yields `foofoo`, not infinite loop.
- The escape rule `\/` only fires *inside the substitution operand scan*. The
  existing operand parser (`parse_braced_operand`) keeps its own quoting
  semantics for `\$`, `\` etc. unchanged.
