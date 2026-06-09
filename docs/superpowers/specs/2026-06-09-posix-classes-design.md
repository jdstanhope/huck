# huck v119 — POSIX bracket character classes in glob patterns (M-54) Design

**Status:** approved design, ready for implementation plan.
**Implements:** POSIX bracket character classes (`[[:alpha:]]`, `[[:digit:]]`,
`[[:space:]]`, `[[:alnum:]]`, `[[:upper:]]`, `[[:lower:]]`, `[[:blank:]]`,
`[[:punct:]]`, `[[:cntrl:]]`, `[[:graph:]]`, `[[:print:]]`, `[[:xdigit:]]`) inside
glob bracket expressions, in every glob context — `${var/}`/`#`/`%`, `case`,
`[[ == ]]`, completion, and pathname globbing. This is **M-54** (Tier-2/medium).
**Why now:** it's the last `mise<TAB>` residual. `__get_cword_at_cursor_by_ref`
clears a whitespace-only `cur` with `${cur//[[:space:]]/}`; huck doesn't support
the class, so the substitution is a no-op and `cur` keeps a trailing space
(`cur=[ ]` vs bash `[]`). Beyond mise it's a broad correctness gap.
**Branch (impl):** `v119-posix-classes`.

## Background — the bug (verified against bash this session)

The `glob` crate (used at the 5 non-extglob match sites) does not support POSIX
classes, so every class is a no-op / mismatch:

| construct | bash | huck (now) |
|---|---|---|
| `${s//[[:digit:]]/_}` (s="a1 b2") | `a_ b_` | `a1 b2` |
| `${s//[[:alpha:]]/_}` | `_1 _2` | `a1 b2` |
| `case " " in [[:space:]])` | match | no match |
| `[[ "x" == [[:alpha:]] ]]` | `Y` | `N` |
| `${s//[[:digit:]_]/X}` (s="a5_b", mixed) | `aXXb` | unchanged |

huck already has its OWN glob matcher (`src/glob_match.rs`) powering both string
matching (`extglob_match`) and pathname globbing (`extglob_pathname_expand`, the
v91 FS walker) via shared `parse_class`/`Item` machinery — but `parse_class`
(`:198`) doesn't recognize `[:name:]`, so a class is mis-parsed as literal
characters.

**Probed bash semantics (this session, default locale):**
- `[[:space:]]` matches `\v` (vertical tab, 0x0b) — POSIX `space` includes `\v`,
  which Rust's `is_ascii_whitespace` OMITS (needs a custom predicate).
- `[[:print:]]` matches a space; `[[:punct:]]` matches `]` and `!`.
- POSIX classes work with **extglob OFF** (they are standard globs, not extglob).
- Mixed class+literal in one bracket works: `[[:digit:]_]` = digit or `_`.
- An **unknown** class name `[[:bogus:]]` matches **nothing** (not literal, not
  an error): `case ":" in [[:bogus:]])`, `case "[" in [[:bogus:]])` → no match.

## Architecture — extend the own-matcher; route class-bearing patterns to it

Rather than translate classes to `glob`-crate ranges (impossible to do safely:
`punct`/`cntrl`/`graph`/`print` ranges contain `]`/`-`/`\`/`^`, which corrupt the
crate's bracket parser), parse the ORIGINAL pattern with `[:name:]` intact and
match via ASCII char predicates. This reuses huck's extglob `Item`/`parse_class`
machinery — which already drives BOTH string and pathname matching — so one
change covers all 5 sites with no escaping hazards.

### Component 1 — `PosixClass` + `ClassAtom::Posix` (`src/glob_match.rs`)
- New `enum PosixClass { Alpha, Digit, Alnum, Upper, Lower, Space, Blank, Punct, Cntrl, Graph, Print, Xdigit }`.
- New `ClassAtom::Posix(PosixClass)` variant alongside the existing `Ch`/`Range`.
- `fn posix_class_from_name(name: &str) -> Option<PosixClass>` (the 12 names).
- Membership `fn matches(class, c, case_insensitive) -> bool` via ASCII/C-locale
  predicates (matching bash's effective behavior):
  - `Alpha`→`c.is_ascii_alphabetic()`, `Digit`→`is_ascii_digit`,
    `Alnum`→`is_ascii_alphanumeric`, `Upper`→`is_ascii_uppercase`,
    `Lower`→`is_ascii_lowercase`, `Xdigit`→`is_ascii_hexdigit`,
    `Punct`→`is_ascii_punctuation`, `Cntrl`→`is_ascii_control`,
    `Graph`→`is_ascii_graphic`.
  - `Space`→`matches!(c, ' '|'\t'|'\n'|'\r'|'\x0b'|'\x0c')` (includes `\v`),
    `Blank`→`matches!(c, ' '|'\t')`, `Print`→`c.is_ascii_graphic() || c == ' '`.
  - Under `case_insensitive`, `Upper`/`Lower` both match any ASCII letter
    (bash `nocasematch` makes the whole match case-insensitive). Verify against
    bash during implementation; if bash differs, match bash.

### Component 2 — `parse_class` recognizes `[:name:]`
In `parse_class` (`:198-226`), when scanning the set and the cursor is at `[`
followed by `:`, read to the closing `:]` as a class name: a known name →
`ClassAtom::Posix(class)`; an unknown name → a `ClassAtom` that matches nothing
(an `Posix`-style "empty" atom, e.g. `ClassAtom::Posix` with a `None`/unknown
marker, or a dedicated `ClassAtom::Never`). `[:` NOT forming a valid `[:…:]`
(e.g. a bare `[:` with no closing `:]`) falls back to literal `:` handling
(current behavior). The class-membership matcher (`match_class` / wherever
`ClassAtom` is evaluated) gains the `Posix` arm.

### Component 3 — `has_posix_class(pattern: &str) -> bool`
A scan that returns true iff the pattern contains an unescaped `[:name:]` inside
a bracket expression (skip `\[`; only count `[:…:]` that sits inside an open
`[…]`). Used by the dispatch sites. Mirrors the existing `has_extglob` helper.

### Component 4 — route class-bearing patterns through the own-matcher (5 sites)
At each site, change the dispatch condition from
`if extglob && has_extglob(pat)` to
`if (extglob && has_extglob(pat)) || crate::glob_match::has_posix_class(pat)`
so class-bearing patterns use `extglob_match` (string) /
`extglob_pathname_expand` (pathname). **Unconditional on the extglob shopt.**
Sites: `pe_pattern_matches` (`param_expansion.rs`), `case`
(`executor.rs:1074`), `[[ == ]]` (`executor.rs:1280`), completion `glob_match`
(`completion_spec.rs`), pathname (`expand.rs:1409`). The own-matcher faithfully
handles the plain-glob parts (`*`/`?`/`[abc]`/ranges/`[!…]`) alongside the
classes (verified by v90/v91/v116 that `extglob_match` matches the `glob` crate
for plain patterns).

## Scope & correctness
- All 12 POSIX classes, ASCII/C-locale (bash's effective default for these).
- Negation composes: `[^[:digit:]]` (v116 already maps the leading `^`→`!`; the
  class lives inside a negated set) — the own-matcher's class negation inverts
  membership including the `Posix` atom.
- Routing is unconditional (classes work with extglob off, verified).
- Unknown class name → matches nothing (verified bash behavior).
- Mixed brackets `[[:digit:]a-f_]` — `Posix` atoms coexist with `Ch`/`Range`.

## Must-not-regress
- Plain globs `*`/`?`/`[abc]`/`[a-z]`/`[!abc]`/`[^abc]` (v116) in all contexts —
  routing a class-free pattern is UNCHANGED (the `|| has_posix_class` is false).
- extglob patterns (`!(…)` etc.) — unchanged (already routed).
- A literal `[:` that is NOT a POSIX class (`[:]`, `a[:b]`) — bash treats `[:x:]`
  outside the `[[:name:]]` form per its own rules; verify `[:abc]` (a bracket set
  containing `:`,`a`,`b`,`c`) still matches `:` like bash. The `has_posix_class`
  scan and `parse_class` must only treat `[:name:]` (colon-bracket immediately
  inside a `[`) as a class, leaving an ordinary `:` in a set literal.
- `case`/`[[`/`${}`/completion/pathname plain-glob and extglob suites.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/glob_match.rs` | `PosixClass` enum, `ClassAtom::Posix`, `posix_class_from_name`, membership, `parse_class` `[:name:]` branch, `has_posix_class`; unit tests |
| `src/param_expansion.rs` | `pe_pattern_matches` dispatch `|| has_posix_class` |
| `src/executor.rs` | `case` + `[[ == ]]` dispatch `|| has_posix_class` |
| `src/completion_spec.rs` | `glob_match` dispatch `|| has_posix_class` |
| `src/expand.rs` | pathname dispatch `is_extglob || has_posix_class` |
| `tests/posix_classes_integration.rs` | NEW — `${}`/case/`[[`/pathname × the 12 classes + negation + mixed, vs bash |
| `tests/scripts/posix_classes_diff_check.sh` | NEW — 43rd bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-54 `[fixed v119]`; changelog; README row; Tier counts |

## Testing
1. **Unit** (`src/glob_match.rs`): `posix_class_from_name` (12 names + unknown→
   None); membership for each class incl. the `\v`-in-space and `print`-includes-
   space edges; `parse_class` produces `Posix` atoms for `[[:alpha:]]`,
   `[^[:digit:]]`, mixed `[[:digit:]a-f]`; `has_posix_class` true/false
   (`[[:space:]]` yes, `[abc]`/`[:]`/`a:b` no, `\[[:x:]` escaped-bracket edge).
2. **Integration** (`tests/posix_classes_integration.rs`, binary vs bash,
   file-arg per L-27): `${s//[[:class:]]/_}` for each of the 12 classes;
   `case`/`[[ == ]]` membership (match + non-match); negation `[^[:digit:]]`;
   mixed `[[:digit:]_]`; pathname `[[:upper:]]*` / `[[:digit:]]*` in a fixtured
   temp dir. Each byte-identical to bash.
3. **43rd bash-diff harness** `tests/scripts/posix_classes_diff_check.sh` — a
   representative spread across the 12 classes + negation + mixed + one pathname
   case, byte-identical.
4. **Regression**: full suite (2859+), all 43 harnesses, clippy `--all-targets`.
   Watch `param`/`case`/`dbracket`/`glob`/`extglob`/`completion` suites — a
   regression means routing altered a class-free pattern (it must not).
5. **Payoff (the mise finale)**: drive the real `_init_completion -n :` chain
   with `COMP_WORDS=(mise "")`. Expect huck `cur=[]` (the trailing space cleared
   by `${cur//[[:space:]]/}`) — **fully byte-identical to bash** including
   `cur`. Report before/after. If a further residual surfaces, report honestly
   (the smoke is the gate — v109/v115/v116/v117 lesson).

## Edge cases & notes
- **`\v` in `space`/`print`**: custom predicates (not Rust `is_ascii_whitespace`).
- **Unknown class**: matches nothing — implement as an empty/never-matching atom
  (verify `case ":" in [[:bogus:]])` → no match, not literal).
- **`case_insensitive` (`nocasematch`/`nocaseglob`)**: `Upper`/`Lower` widen to
  any letter; verify against bash; other classes are case-agnostic.
- **Locale**: ASCII/C only. Non-ASCII bytes are out of scope (bash's UTF-8
  locale behavior for `[[:alpha:]]` on multibyte is a separate, deferred gap if
  it ever matters; note as a low `L-` divergence only if a test surfaces it).
- **Pathname leading-dot / separator rules**: inherited from
  `extglob_pathname_expand` (already correct for extglob pathname globbing);
  the integration test must verify a `[[:lower:]]*` pattern doesn't match a
  dotfile and respects `/`.
