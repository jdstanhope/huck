# huck v90 — extglob string matching Design

**Status:** approved design, ready for implementation plan.
**Implements:** extended glob patterns — `?(…)` `*(…)` `+(…)` `@(…)` `!(…)` (with
`|`-alternation and nesting) — gated on `shopt -s extglob`, in huck's three
**string**-matching pattern contexts: `[[ == / != ]]`, `case`, and parameter
expansion (`${v#…}`/`##`/`%`/`%%`/`/`/`//`/`^`/`,`). huck has no extglob today:
the `extglob` shopt is an inert toggle (v86), the `glob` crate can't match
extglob, and the lexer splits `+(a|b)` into operator tokens so `[[`/`case` can't
even parse it.
**Discovered:** loading a stock Debian `~/.bashrc`'s bash-completion (`shopt -s
extglob` + `+(…)`/`@(…)` patterns in `[[`; the last remaining `[[` load error
after v87).
**Divergence tracker:** new **M-84** (extglob) `[fixed v90 partial]` — string
matching; **M-84a** `[deferred]` — extglob in pathname/filesystem globbing.
**Branch (impl):** `v90-extglob` (created from `main` at plan time).

## Scope

Decided during brainstorming (option "Engine + [[/case/${} string matching"):
build the matcher engine and wire it into the three **string**-match contexts.

**Out of scope** (deferred to v91 = **M-84a**): extglob in **pathname/filesystem
globbing** (`echo +(a|b)*.txt` walking directories). With extglob on, such a word
lexes correctly (one word) but is not filesystem-expanded; this matches no files
/ falls back to the literal exactly as today. Also out of scope: POSIX bracket
classes `[[:alpha:]]` (the separate, still-deferred M-54 — the engine deliberately
does not try to also close it).

## Verified bash 5.2 semantics (the implementation targets these)

The five operators, each over a `|`-separated **pattern-list** (alternatives may
themselves contain `*`/`?`/`[…]`/nested extglob):

| Form | Meaning | Examples (verified) |
|------|---------|---------------------|
| `?(list)` | zero or one of list | `?(abc)` matches `""`, `abc`; not `abcabc` |
| `*(list)` | zero or more | `*(ab)` matches `""`, `ababab` |
| `+(list)` | one or more | `+(ab)` matches `abab`; not `""` |
| `@(list)` | exactly one | `@(ab|cd)` matches `ab`,`cd`; not `abcd` |
| `!(list)` | anything **except** a match of list | `!(bar)` matches `foo`; not `bar` |

Composition (verified): `a@(x|y)b` matches `axb`; `a+(x|y)b` matches `axxb`;
`+([a-z]).txt` matches `file.txt`; `@(a*(b)c)` matches `abc` (nesting).
Parameter expansion (verified): `v=aaab; ${v##+(a)}` → `b`; `v=foobarbar;
${v%%+(bar)}` → `foo`; `v=abcabc; ${v/+(abc)/X}` → `X`. `case`: `case hello in
+([a-z]))` matches.

### Flag-gating policy (and a documented divergence)

bash's extglob flag-sensitivity is **inconsistent across contexts**: `[[ ab ==
+(a|b) ]]` matches even with extglob **off**; `case +(a|b))` is a **syntax error**
with extglob off; `${v##+(a)}` is **literal** (no strip) with extglob off. Rather
than reproduce that per-context quirk, **huck gates extglob uniformly on `shopt
-s extglob`** — parsing *and* matching, in all three contexts. This is clean,
predictable, and **fully faithful for the real usage** (scripts/bash-completion
that `shopt -s extglob` before using extglob).

**Documented divergence (part of M-84):** with extglob **off**, an extglob
operator in `[[`/`case` is treated as ordinary characters (so `[[ x == +(a|b) ]]`
does not parse the parens as a group), whereas bash still honors extglob in `[[`
(and accepts it in `[[` though not `case`). huck requires the flag everywhere.
(`${…}` and `case`-parse-off already agree with bash: literal / error.)

## Section 1 — The matcher engine (`src/glob_match.rs`, NEW)

A pure module, no shell / no filesystem:

```rust
/// Matches `text` against an extglob `pattern` (the WHOLE string must match).
/// `case_insensitive` folds case (for `nocasematch`).
pub fn extglob_match(pattern: &str, text: &str, case_insensitive: bool) -> bool;

/// True if `pattern` contains an extglob operator (`?(` `*(` `+(` `@(` `!(`)
/// at top level or nested — the dispatch predicate for the call sites.
pub fn has_extglob(pattern: &str) -> bool;
```

- Parse the pattern into a sequence of items: `Literal(char)`, `Any`(`*`),
  `One`(`?`), `Class(BracketClass)`, and `Group { kind: ?|*|+|@|!, alts:
  Vec<Pattern> }` (each alt is itself a parsed item-sequence). Parens nest;
  inner `|` separates alts; `\` escapes the next char; `[...]` bracket classes
  parse as today (reuse the existing bracket logic / `glob` semantics for the
  class interior).
- Match by recursive backtracking, anchored to consume the entire `text`:
  - `Literal`/`One`/`Class`/`Any` as standard glob.
  - `?`/`@` groups consume a span matching exactly one (resp. 0-or-1) alt;
    `*`/`+` groups consume a concatenation of zero-or-more (resp. one-or-more)
    alt-matches; backtrack over span boundaries so the rest of the pattern can
    still match.
  - `!(list)` matches a span S iff S does **not** match any alt **and** the
    remainder of the pattern matches the rest of the text (try all split points).
- `case_insensitive` lowercases both sides at the comparison points.

The engine is **only invoked when `has_extglob(pattern)` is true and extglob is
on** — plain `*`/`?`/`[…]` patterns keep using the `glob` crate, so all existing
behavior is byte-unchanged. The engine still implements `*`/`?`/`[…]` because
extglob patterns embed them.

## Section 2 — Lexer: accept `+(a|b)` as one word (`src/lexer.rs`)

`tokenize` has ~399 call sites (mostly tests), so threading a flag through all of
them is untenable. Instead:

- Add `pub struct LexerOptions { pub extglob: bool }` and
  `pub fn tokenize_with_opts(input: &str, opts: LexerOptions) -> Result<Vec<Token>, LexError>`.
- Make the existing `pub fn tokenize(input) -> …` a thin wrapper:
  `tokenize_with_opts(input, LexerOptions { extglob: false })`. All 399 existing
  callers are unchanged (extglob off — current behavior preserved exactly).
- In the word-reader, when `opts.extglob` is true and the next char is one of
  `? * + @ !` immediately followed by `(`, consume the **balanced** parenthesised
  group (track nested `(`/`)` depth; inner `|` and metacharacters are taken
  literally into the word) and append it to the current word as a literal word
  part — mirroring bash's global extglob lexing. (Reuse the depth-tracking
  approach already used for `((…))` arith blocks.)
- Thread the flag from the runtime readers that hold the shell:
  - `process_line` (`src/shell.rs`) reads `shell.shopt_options.get("extglob")` and
    calls `tokenize_with_opts`.
  - `run_sourced_contents` (`src/builtins.rs`) likewise.
  - `continuation::classify` gains an `extglob: bool` parameter (its callers —
    `read_logical_command`, `run_sourced_contents` — pass the shell's flag), so a
    line-broken `[[ +(a|b)` is tokenized consistently.

With extglob **off** the lexer is byte-identical to today. With it **on**,
`+(a|b)` becomes a single Word token, so the **existing `[[`/`case` parsers consume
it as the pattern with no parser change** — only the matchers (Section 3) change.

## Section 3 — Route the three string-match sites to the engine

At each site, dispatch: **if extglob is on AND `has_extglob(pattern)` → call
`extglob_match`; else keep the existing `glob` crate path.**

- **`[[ == / != ]]`** — `eval_binary` (`src/executor.rs`); pass the `nocasematch`
  state as `case_insensitive`.
- **`case`** — `case_item_matches` (`src/executor.rs`); pass `nocasematch`.
- **`${var#…}`/`##`/`%`/`%%`/`/`/`//`/`^`/`,`** — the `glob::Pattern` match sites in
  `src/param_expansion.rs`. These already receive `+(a)` as a literal pattern
  string (v84), so no lexer dependency — purely the matcher dispatch. (`#`/`##`
  anchor at the start and want the shortest/longest leading match; `%`/`%%` at the
  end; `/`/`//` find a match anywhere; the engine must support shortest-vs-longest
  and substring anchoring at these sites — the existing code already iterates
  candidate prefix/suffix lengths and asks the matcher yes/no per candidate, so
  the engine only needs the whole-span yes/no predicate that the loop already
  uses with `glob::Pattern`.)

These sites read `shell.shopt_options.get("extglob")` to gate.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/glob_match.rs` | NEW — `extglob_match`, `has_extglob`, the pattern parser + backtracking matcher; unit tests |
| `src/lexer.rs` | `LexerOptions`, `tokenize_with_opts`, `tokenize` wrapper; extglob word recognition |
| `src/shell.rs` | `process_line` + `read_logical_command` pass extglob to `tokenize_with_opts`/`classify` |
| `src/builtins.rs` | `run_sourced_contents` passes extglob; (module wiring) |
| `src/continuation.rs` | `classify` gains an `extglob: bool` param |
| `src/executor.rs` | `eval_binary` + `case_item_matches` dispatch to `extglob_match` when applicable |
| `src/param_expansion.rs` | the `${}` match sites dispatch to `extglob_match` when applicable |
| `tests/extglob_integration.rs` | NEW — binary-driven integration tests |
| `tests/scripts/extglob_diff_check.sh` | NEW — huck's 17th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | `extglob` flips from inert (M-08d note) to behavioral; new M-84 `[fixed v90 partial]` + M-84a `[deferred]`; changelog; README v90 row |

## Testing

1. **Unit tests** (`src/glob_match.rs`): the full truth table — each of the five
   ops (incl. empty-match and `!()` negation edges), `|`-alternation, nesting
   (`@(a*(b)c)`), mixing with `*`/`?`/`[a-z]`, anchored whole-string matching,
   `case_insensitive`; `has_extglob` true/false detection. Lexer unit tests
   (`src/lexer.rs`): `tokenize_with_opts("+(a|b)", extglob:true)` → one Word;
   `tokenize("+(a|b)")` (default) → the split tokens (unchanged).
2. **Integration tests** (`tests/extglob_integration.rs`): `shopt -s extglob`
   then `[[ aab == +(a|b) ]]`→match, `[[ bar == !(bar) ]]`→no, `case hello in
   +([a-z]))`→match, `v=aaab; ${v##+(a)}`→`b`, `v=abcabc; ${v/+(abc)/X}`→`X`,
   nesting, alternation; `nocasematch`+extglob; and extglob-**off** behavior
   (`+(a|b)` is not special — `${v##+(a)}` doesn't strip, matching bash).
3. **bash-diff harness** `tests/scripts/extglob_diff_check.sh` (huck's 17th),
   byte-identical to bash 5.2: a spread of `[[`/`case`/`${}` extglob fragments,
   each prefixed `shopt -s extglob` (the flag-on path, where huck matches bash
   exactly). The flag-off `[[` divergence is **excluded with a NOTE** (huck
   requires the flag; bash honors extglob in `[[` regardless).

## Edge cases & notes

- **Default-off ⇒ zero behavior change**: with extglob off the lexer, all three
  match sites, and `$-`-unrelated behavior are exactly as today.
- **`has_extglob` dispatch** keeps non-extglob patterns on the `glob` crate, so
  the engine's correctness only affects genuinely-extglob patterns.
- **`!()` negation** is the trickiest matcher case (span-consuming negation) —
  the engine tries all split points for the negated span.
- **Pathname globbing deferred (M-84a)**: `shopt -s extglob; echo +(a|b)` lexes as
  one word but does not filesystem-expand in v90 (no match → literal, as today).
- **Backslash/quoting**: a quoted `'+(a)'` or `\+(a)` is NOT an extglob group
  (quoting suppresses it) — the lexer only forms the group from an *unquoted*
  `X(` sequence, consistent with how quoted globs are already handled.
