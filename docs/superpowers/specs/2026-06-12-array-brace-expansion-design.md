# huck v144 — brace expansion in array-literal elements Design

**Status:** approved design, ready for implementation plan.
**Implements:** brace expansion of BARE array-literal elements — `a=({1..3})` →
`1 2 3`, `a=(f{1,2}.x)` → `f1.x f2.x`, cartesian `a=({a,b}{1,2})` → `a1 a2 b1 b2`.
**Branch (impl):** `v144-array-brace-expansion`.

## Background — what's already done vs. the real gap

The original backlog item **M-102** (IFS word-splitting of an unquoted
command-sub/variable inside an array literal — `a=($(cmd))`, `a=($var)`) is
**already fixed** (v117/M-112 routed bare elements through `glob_expand_word`;
verified byte-identical to bash across ~20 cases incl. empty-IFS no-split,
whitespace trim, `eval`, `local`, `+=`, subscript-then-bare). The M-102 entry is
stale and is DELETED as part of this iteration.

The genuinely-open adjacent gap: **huck does not apply brace expansion to
array-literal elements.** Brace expansion in huck is lexer-level
(`emit_word_with_braces`, `src/lexer.rs`) and runs on top-level command words, but
array-literal elements are parsed separately (`read_array_literal`,
`src/lexer.rs:2415`) and never reach it. So:

| input | bash | huck (before) |
|---|---|---|
| `a=({1..3} z)` | `1 2 3 z` (4) | `{1..3} z` (2) ✗ |
| `a=(x{a,b}y)` | `xay xby` (2) | `x{a,b}y` (1) ✗ |
| `echo {1..3}` (command arg) | `1 2 3` | `1 2 3` ✓ (already works) |

Word-splitting and globbing of array elements already work; only brace expansion
is missing.

## Architecture — reuse the lexer's brace machinery, per bare element

bash's expansion order is: **brace expansion (purely textual) → tilde → param/
command/arith → word-splitting → globbing**. Brace expansion acts only on LITERAL
source braces, never on braces produced by an expansion (`v={1,2}; a=($v)` →
literal `{1,2}`). huck already implements exactly this for command words via
`emit_word_with_braces` (`src/lexer.rs:1200`):

```
emit_word_with_braces(parts):
  if no unquoted brace -> emit one Word
  (concat, placeholders) = build_concat_with_sentinels(parts)   # protect non-literal parts
  for s in brace_expand::expand(concat):
      emit Word(split_on_sentinels(s, placeholders))
```

The sentinel scheme replaces non-literal parts (`$(…)`, `${…}`, quoted runs) with
placeholders so `brace_expand::expand` only sees/expands the literal braces, then
restores them. This is precisely the behavior array elements need.

### Fix: a shared helper + a call in `read_array_literal`

1. **Factor a helper** out of `emit_word_with_braces`:
   ```rust
   /// Brace-expands a word's parts into one-or-more parts-lists. With no
   /// unquoted brace, returns the input unchanged (one list). Non-literal
   /// parts (expansions, quoted runs) are protected via sentinels so only
   /// literal source braces expand.
   fn brace_expand_parts(parts: Vec<WordPart>) -> Result<Vec<Vec<WordPart>>, LexError>
   ```
   `emit_word_with_braces` becomes a thin wrapper: call `brace_expand_parts`, wrap
   each result in `Word` + `Token::Word`, return the count.

2. **Apply it to bare elements** in `read_array_literal` (`src/lexer.rs:2442-2443`).
   Currently:
   ```rust
   let value = read_array_element_word(chars, opts)?;
   elements.push(ArrayLiteralElement { subscript, value });
   ```
   becomes:
   ```rust
   let value = read_array_element_word(chars, opts)?;
   match subscript {
       Some(sub) => elements.push(ArrayLiteralElement { subscript: Some(sub), value }),
       None => {
           for p in brace_expand_parts(value.0)? {
               elements.push(ArrayLiteralElement { subscript: None, value: Word(p) });
           }
       }
   }
   ```

Brace expansion thus happens at lex time (textual, first), producing N bare
elements; the executor's existing `expand_array_elements` → `glob_expand_word`
then runs param/cmdsub expansion + IFS word-splitting + globbing on each — exactly
bash's order. (Verified target: `a=(pre{1,2}$(echo m n))` → `pre1m n pre2m n`, 4
elements — brace first, then per-product cmdsub split.)

## Scope & behavior

### In scope — BARE elements brace-expand (target = bash)

| input | result |
|---|---|
| `a=({1..3})` | `1 2 3` (3) |
| `a=(x{a,b}y)` | `xay xby` (2) |
| `a=({a,b}{1,2})` | `a1 a2 b1 b2` (4, cartesian) |
| `a=(f{1,2}.x)` | `f1.x f2.x` (2) |
| `a=({1} z)` | `{1} z` (2 — single-element brace stays literal) |
| `a=("{1,2}" x)` | `{1,2} x` (2 — quoted brace literal) |
| `v={1,2}; a=($v)` | `{1,2}` (1 — brace from expansion NOT re-expanded) |
| `a=(pre{1,2}$(echo m n))` | `pre1m n pre2m n` (4 — brace then cmdsub split) |
| `a=([0]=p q{1,2} r)` | `[0]=p [1]=q1 [2]=q2 [3]=r` (subscripted single + bare brace) |

The implicit auto-index advances per produced element (existing
`expand_array_elements` behavior — the lexer just hands it more bare elements).

### Out of scope — subscripted element value with a brace (deferred, low-impact)

A SUBSCRIPTED element keeps single-value semantics (brace stays literal):
- `declare -A m=([k]=x{a,b})` → `[k]="x{a,b}"` — **MATCHES bash** (bash keeps the
  brace literal for associative subscripts).
- `a=([2]=x{a,b})` (indexed) → huck: `a[2]="x{a,b}"`; bash: quirkily drops the
  subscript and emits two BARE literals `[0]="[2]=xa" [1]="[2]=xb"`. This is the
  ONE divergence. Pathological (no real script writes `[i]=val{brace}`); bash's
  own behavior here is surprising and indexed-vs-associative-inconsistent.
  Documented as a new low-impact `[deferred]` entry rather than replicated.

## Documented divergences
- **DELETE M-102** (array-literal element word-splitting) — verified already fixed
  by v117. Remove the Tier-2 entry; Tier-2 count 20 → 19.
- **ADD a new low-impact entry** (Tier-4): an INDEXED subscripted array element
  whose value contains a literal brace (`a=([2]=x{a,b})`) keeps the value literal,
  whereas bash brace-expands the whole `[i]=…` word into bare literals (dropping
  the subscript). Associative subscripts (`[k]=x{a,b}`) match bash (literal).
  Low/pathological. Tier-4 count 31 → 32.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/lexer.rs` | Factor `brace_expand_parts(parts) -> Result<Vec<Vec<WordPart>>, LexError>` out of `emit_word_with_braces` (which becomes a thin wrapper); call it for BARE elements in `read_array_literal`. |
| `tests/scripts/array_brace_expansion_diff_check.sh` (NEW, 64th) | Bash-diff over the in-scope matrix (incl. `declare -p`, `local`, `+=`, assoc). |
| `docs/bash-divergences.md` | DELETE M-102 (Tier-2 20→19); ADD the indexed-subscript-brace low-impact entry (Tier-4 31→32). |

## Testing

1. **Lexer unit tests** (`src/lexer.rs` mod tests): `brace_expand_parts` on literal
   `{1,2}` parts → 2 lists; on no-brace parts → 1 (unchanged); on a parts list with
   a non-literal part (sentinel-protected) → braces expand, the non-literal part is
   preserved in each. `read_array_literal` for `({1..3})` → 3 bare elements; for
   `([2]=x z)` → 1 subscripted + 1 bare (subscript untouched); for `("{1,2}")` →
   1 element (quoted brace literal).
2. **Bash-diff harness** `tests/scripts/array_brace_expansion_diff_check.sh` (64th)
   — the full in-scope matrix above, each asserted byte-identical to bash via
   `declare -p a` (captures indices + values exactly). Include `declare -a a=(…)`,
   `local a=(…)` inside a function, `a+=(…)` append, and `declare -A` (assoc, where
   `[k]=x{a,b}` must stay literal — confirms the out-of-scope path matches bash).
3. **Full regression:** entire suite + ALL harnesses green — ESPECIALLY the existing
   brace-expansion tests (`emit_word_with_braces` must be unchanged behaviorally via
   the wrapper) and the v117 array-literal field/glob tests. `clippy` clean.

## Edge cases & notes
- The `emit_word_with_braces` wrapper must preserve its existing return-count
  contract (callers push the word's start offset that many times to keep the
  offset sidecar in lockstep) — the refactor is behavior-preserving.
- Brace-expansion error (e.g. exceeding the limit) propagates as
  `LexError::BraceExpansionLimit`, same as command words.
- Subscripted elements are untouched — no behavior change for `[i]=value` (matches
  bash except the documented indexed-brace edge).
- **Git safety:** implementer subagents must NOT `git checkout <sha>`; the
  controller verifies the branch tip before merging. Commit trailer:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
