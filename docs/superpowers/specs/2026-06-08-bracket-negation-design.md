# huck v116 — support `[^…]` bracket negation in glob patterns (M-113) Design

**Status:** approved design, ready for implementation plan.
**Implements:** `[^…]` (negated bracket class) as a synonym for `[!…]` in every
glob-pattern context — new **M-113** (Tier-1 bug). It's the actual root cause of
the `mise<TAB>` `bash_completion: : : invalid option`, and a broad correctness
gap.
**Why now:** bash_completion's `_get_comp_words_by_ref`/`_init_completion` compute
the word-break exclusion set with `${1//[^$COMP_WORDBREAKS]/}`. huck treats `[^…]`
as a literal `^` (the `glob` crate only honors `[!…]`), so the substitution is
INVERTED → wrong `exclude` → `__get_cword_at_cursor_by_ref` builds a broken
`words`/`cword` → a malformed `_upvars` call → `: : invalid option` on `mise<TAB>`.
**Branch (impl):** `v116-bracket-negation`.

## Background — the bug (verified against bash)

`[^…]` is broken in EVERY non-extglob glob context; `[!…]` already works:

| construct | bash | huck (now) |
|---|---|---|
| `${v//[^0-9]/}` (v=abc123) | `123` | `abc` (inverted) |
| `${v#[^0-9]}` (v=abc123) | `bc123` | `abc123` (no match) |
| `case A in [^0-9])` | `letter` | `other` |
| `[[ A == [^0-9] ]]` | `Y` | `N` |
| `echo [^a]file` (afile bfile cfile) | `bfile cfile` | `afile` |
| `${v//[!0-9]/}` (the other negation) | `123` | `123` ✓ |
| `${v//[a^b]/}` (`^` not class-leading) | (removes a/^/b) | same ✓ (literal `^`) |

**Root cause:** the `glob` crate uses `[!…]` for class negation and treats `[^…]`
as a literal-`^` set. bash accepts BOTH `[!…]` and `[^…]` as negation. huck's
extglob matcher (`src/glob_match.rs::parse_class`, ~`:139`) ALREADY accepts both
`!` and `^` — so only the `glob`-crate code paths are wrong.

## Architecture — a shared `^`→`!` translation, applied at the glob-crate sites

Add a small, bracket- and escape-aware helper that rewrites a **class-leading**
`^` to `!`, then feed the normalized pattern to the `glob` crate. The extglob
matcher is untouched (already correct), so all pattern contexts become
consistent. This is surgical and keeps the battle-tested `glob` crate for plain
globs (preferred over re-routing everything through `glob_match.rs`).

### Component 1 — `translate_bracket_negation` (`src/glob_match.rs`)
```rust
/// Rewrite a class-leading `^` to `!` so the `glob` crate (which only honors
/// `[!…]`) treats `[^…]` as negation, matching bash (which accepts both). Only
/// the FIRST char inside an unescaped class-opening `[` (or after `[!`) is the
/// negation position; a `^` anywhere else stays literal. Honors `\[` escapes
/// and the literal-first-`]` rule (`[^]x]` → `[!]x]`, `[]x]` unchanged).
/// Returns the input borrowed when there is no class-leading `^` (zero-copy).
pub(crate) fn translate_bracket_negation(pattern: &str) -> std::borrow::Cow<'_, str>
```
Algorithm (char walk, tracking `in_class` and a one-char escape):
- A backslash escapes the next char (it can't open/close a class).
- Outside a class, an unescaped `[` opens a class; the **immediately following**
  char is the candidate negation position:
  - if it is `^` → emit `!` (the fix);
  - if it is `!` → leave (already negation);
  - in either case, a `]` that comes next (after `[`, `[^`→`[!`, or `[!`) is a
    LITERAL `]` (does not close the class).
- Inside a class, `]` (not in the literal-first position) closes it; `[` is
  literal; `^` is literal (only the leading one is negation, already handled).
- POSIX sub-classes `[:alpha:]` etc. live inside `[...]`; their inner `[`/`]`
  are handled by the in_class state (a `[` inside a class is literal, and
  `[:…:]` is consumed as literal content — the helper does not need to special-
  case it, only to not treat the inner `[` as a new class-opener).
- Build into a `String` only when a change is made; otherwise return
  `Cow::Borrowed(pattern)`.

### Component 2 — apply it at the five `glob`-crate sites
At each site, the existing shape is `if extglob && has_extglob(pat) { extglob_match… } else { glob::Pattern::new(pat)/glob_with(pat) }`. Normalize the pattern in the **else** (glob-crate) branch only:
1. **`pe_pattern_matches`** (`src/param_expansion.rs:~340`) — covers `${var/pat}`,
   `${var#pat}`, `${var%pat}` (they all match through this primitive). Normalize
   before `glob::Pattern::new`.
2. **`case`** (`src/executor.rs:~1077`) — normalize before `glob::Pattern::new`.
3. **`[[ == ]]`** (`src/executor.rs:~1282`) — normalize before `glob::Pattern::new`.
4. **completion** (`src/completion_spec.rs:~373`) — normalize before
   `glob::Pattern::new`.
5. **pathname globbing** (`src/expand.rs:~1432`) — normalize before
   `glob::glob_with(&pattern, …)`.

The `is_err()` guards (in `remove_prefix`/`remove_suffix`/`substitute`,
param_expansion `:357/:381/:416/:526`) test for unclosed-bracket errors; `[^…]`
is not an error there, so they need no change — but normalizing them too is
harmless and keeps behavior consistent if a future pattern relies on it (apply
the helper there only if trivial; not required).

## Scope & correctness
- Only the **class-leading** `^` is converted. `[a^b]`, `a^b`, `^foo`, `${v//^/}`
  keep `^` literal — verified bash-equivalent.
- `[!…]` is unchanged (already works).
- Escaped `\[` does not open a class (so `\[^` stays literal `[^`).
- Literal-first-`]`: `[^]x]`→`[!]x]` (still a valid negated class containing `]`
  and `x`); `[]x]` (no `^`) unchanged.
- Applies uniformly to `${}` (`/`,`#`,`%`), `case`, `[[ == ]]`, completion, and
  pathname globbing — all confirmed broken today.

## Must-not-regress
- `[!…]` negation in all contexts (unchanged).
- Non-negated classes `[abc]`, ranges `[a-z]`, POSIX `[[:alpha:]]`.
- Plain globs `*`/`?`; literal `^` outside a class and `^` not class-leading.
- Extglob patterns (`!(…)` etc.) — routed to `glob_match` (already handles `[^]`).
- `glob::Pattern::escape` (expand.rs:1133) — unaffected (escaping, not matching).
- Quoted metacharacters (a quoted `[^a]` is literal — the quoting/`has_metachar`
  path decides whether to glob at all; the helper only runs on
  treated-as-pattern strings).

## Files & responsibilities

| File | Change |
|------|--------|
| `src/glob_match.rs` | `translate_bracket_negation` helper + unit tests |
| `src/param_expansion.rs` | normalize in `pe_pattern_matches` glob-crate branch |
| `src/executor.rs` | normalize in `case` + `[[ == ]]` glob-crate branches |
| `src/completion_spec.rs` | normalize before `glob::Pattern::new` |
| `src/expand.rs` | normalize before `glob::glob_with` (pathname globbing) |
| `tests/bracket_negation_integration.rs` | NEW — `${}`/case/`[[`/glob across `[^]`/`[!]`/literal |
| `tests/scripts/bracket_negation_diff_check.sh` | NEW — 40th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-113 `[fixed v116]`; changelog; README row; Tier counts |

## Testing

1. **Unit** (`src/glob_match.rs`): `translate_bracket_negation`:
   - `[^abc]` → `[!abc]`; `[!abc]` → unchanged; `[abc]` → unchanged.
   - `[a^b]` → unchanged (`^` not leading); `^x` / `a^b` → unchanged.
   - `[^]x]` → `[!]x]`; `[]x]` → unchanged.
   - `\[^a]` → unchanged (escaped `[`); `x[^0-9]y[^a]z` → both converted.
   - `[[:alpha:]]` → unchanged; `[^[:digit:]]` → `[![:digit:]]`.
   - returns `Cow::Borrowed` when no change (assert via `matches!(.., Cow::Borrowed)`).
2. **Integration** (`tests/bracket_negation_integration.rs`, binary-driven, vs bash):
   - `v=abc123; echo "${v//[^0-9]/}"` → `123`; `echo "${v#[^0-9]}"` → `bc123`.
   - `case A in [^0-9]) echo letter;; *) echo other;; esac` → `letter`.
   - `[[ A == [^0-9] ]] && echo Y || echo N` → `Y`.
   - pathname: in a temp dir with `afile bfile cfile`, `echo [^a]file` → `bfile cfile`.
   - regression: `[!0-9]` still negates; `[a^b]` treats `^` literal; `[0-9]` non-negated.
3. **40th bash-diff harness** `tests/scripts/bracket_negation_diff_check.sh` —
   byte-identical fragments for all the above (pathname one in a fixtured dir).
4. **Regression**: full suite (2814+), all 40 harnesses, clippy clean. Watch
   `param`/`case`/`dbracket`/`glob`/`extglob`/`arrays`/completion suites.
5. **Payoff**: the extracted `_init_completion -n :` / `_get_comp_words_by_ref`
   shape (`COMP_WORDS=(mise ""); COMP_CWORD=1`) now yields `cword=1 nwords=2
   w0=mise prev=mise` (matching bash) with NO `: : invalid option`. Report
   before/after.

## Edge cases & notes
- **`^` as the only class char** (`[^]`, nothing after): bash treats this
  degenerate/unterminated class as a LITERAL string `[^]` (verified: `[[ x ==
  [^] ]]` and `[[ ] == [^] ]]` are both false). The helper would convert it to
  `[!]`, whose treatment by the `glob` crate may differ from bash's literal —
  this is a pathological, non-load-bearing edge (no real pattern is just `[^]`);
  accept whatever the harness shows and note it as a low `L-` divergence if it
  differs, rather than special-casing.
- **`nocaseglob`/`nocasematch`** flow unchanged (the helper runs before, on the
  pattern string; case-insensitivity is a `MatchOptions`/flag concern).
- This does NOT touch the extglob matcher; extglob `[^]` already worked.
- The separate **piped-stdin history-expansion** bug found during triage is NOT
  part of v116 (out of scope; a future note).
