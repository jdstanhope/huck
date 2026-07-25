# v337 — Flip the `posixpat` bash-suite category to PASS (collating symbols)

Issue: [#302](https://github.com/jdstanhope/huck/issues/302) — POSIX.2 collating
symbols `[[.name.]]` unsupported in bracket expressions.

## Problem

The `posixpat` bash-suite category is a near-miss (diff 21 lines). Its **complete
residual is one coherent feature**: POSIX.2 **collating symbols** `[[.name.]]`
(collating elements inside `[...]`). The POSIX character-*class* half
(`[[:alpha:]]`, `[![:alpha:]]`, …) already passes byte-for-byte; every diverging
line is a collating-symbol case. Fixing it takes the category to **0-diff → PASS**
(Summary PASS 24→25, FAIL 58→57).

```
case a   in [[.a.]])       echo m;; *) echo no; esac   # bash: m   huck: no
case -   in [[.hyphen.]])  echo m;; *) echo no; esac   # bash: m   huck: no
case p   in [[.a.]-[.z.]]) echo m;; *) echo no; esac   # bash: m   huck: no
case ' ' in [[.space.]])   echo m;; *) echo no; esac   # bash: m   huck: no
case 4   in [[.-.]-9])     echo m;; *) echo no; esac   # bash: m   huck: no
```

### Why it fails today (two causes, same feature)

1. **Routing.** `case_item_matches` (`executor.rs:2365`) and `pe_pattern_matches`
   (`param_expansion.rs`) route a pattern to the good `extglob_match` engine only
   when `has_extglob` (extglob on) or `has_posix_class` (`[:`…`:]`) fires. A bare
   `[[.a.]]` triggers neither, so it falls to the `glob` crate, which mishandles
   `[.` and returns no-match.
2. **Matching.** Even routed to `glob_match.rs`, `parse_class` has no `[.name.]`
   branch — it would consume the collating element as literal `[`, `.`, name, `.`
   characters.

### bash's model (from `collsym()`, POSIX.2 table 2.8)

`collsym(name)`:
1. Look the name up in the POSIX.2 collating-symbol table → its character.
2. Else, if the name is a **single character**, return that character
   (`[.a.]`→`a`, `[.-.]`→`-`; letters/digits resolve here — the table omits
   letters, digits are listed but single-char passthrough covers them too).
3. Else → **INVALID** (a multi-char non-name like `zz`, `yyz`).

An INVALID collating symbol is **not an error** — it is a non-matching element.
As a range endpoint it makes that range never match, but other set atoms still
apply (`[[.a.]-[.zz.]p]` still matches `p`). A **reversed** range
(`[.a.]-[.Z.]`, `a`>`Z`) also just does not match.

## Design

All changes in `crates/huck-engine/src/glob_match.rs`, plus one routing-predicate
call at each of the two match sites.

### 1. Collating-symbol table + lookup (new)

Port the POSIX.2 table 2.8 name→char mapping as a Rust `&[(&str, char)]` static
(built from the POSIX standard, NOT copied from bash's GPL `collsyms.h`): the
control names (`NUL`…`US`, `DEL`), whitespace (`space`, `tab`→`\t`,
`newline`→`\n`, `alert`→`\x07`, `backspace`, `vertical-tab`, `form-feed`,
`carriage-return`), and the punctuation names (`exclamation-mark`, `quotation-mark`,
`number-sign`, …, `hyphen`/`hyphen-minus`/`minus`/`dash`→`-`, `period`/`full-stop`,
`grave-accent`, `tilde`, …), including the digit names (`zero`…`nine`). Letters are
omitted (single-char passthrough covers them).

```rust
fn collsym(name: &str) -> Option<char> {
    if let Some(&(_, c)) = POSIX_COLLSYMS.iter().find(|&&(n, _)| n == name) {
        return Some(c);
    }
    let mut it = name.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => Some(c), // single-char collating element
        _ => None,                  // multi-char non-name → invalid
    }
}
```

### 2. `has_collating_symbol` routing predicate (new)

Mirror `has_posix_class`: scan for an unescaped `[.` followed later by `.]`.

```rust
pub fn has_collating_symbol(pattern: &str) -> bool { /* [. … .] like has_posix_class */ }
```

Fold it into the routing condition at both sites so collating-symbol patterns
reach `extglob_match`:

```rust
// executor.rs case_item_matches + param_expansion.rs pe_pattern_matches
if (extglob && has_extglob(&p)) || has_posix_class(&p) || has_collating_symbol(&p) {
    extglob_match(&p, subject, nocase)
} else { /* glob crate */ }
```

### 3. `parse_class` — recognize `[.name.]`, atom-based ranges

Add a `[.` branch parallel to the existing `[:` branch: find the closing `.]`,
resolve via `collsym`. A valid symbol → the resolved char (usable as a range
endpoint); an invalid symbol → a never-match marker.

Refactor the range detection to be **atom-based**. Today `parse_class` peeks
`chars[i+1] == '-'` at the character level, so `[.a.]-[.z.]` can't form a range.
Introduce a small helper that parses ONE bracket atom at the cursor — a
`[.sym.]` collating element (→ `Some(char)` or invalid), a `[:class:]`, a literal
`]`-first, or a plain char — returning its resolved endpoint char (for classes /
invalid symbols, `None` = not a valid range endpoint). Then:

- atom, then `-`, then atom (next is not the closing `]`): if both endpoints are
  valid chars and `lo <= hi` → `ClassAtom::Range(lo, hi)`; if an endpoint is
  invalid or `lo > hi` (reversed) → `ClassAtom::Never` for the range (other set
  atoms unaffected).
- otherwise the single atom → `ClassAtom::Ch(c)` / `ClassAtom::Posix(..)` /
  `ClassAtom::Never` (invalid collating symbol).

`ClassAtom::Never` already exists (used for unknown `[:class:]`). Reuse it for
invalid collating symbols and failed ranges — `class_matches` already treats it
as non-matching, so negation (`[!…]`) composes correctly.

### Verified target behaviors (posixpat collating block)

`[[.a.]]`→a; `[[.hyphen.]-9]` matches `-`; `[[.a.]-[.z.]]` matches `p`;
`[[.-.]]`→`-`; `[[.space.]]`→` `; `[[.grave-accent.]]` no-match→`ok 6`;
`[[.-.]-9]` matches `4`; `[[.yyz.]-[.z.]]` invalid-start→no-match→`ok 8`;
`[[.yyz.][.a.]-z]` matches `c` (invalid atom + valid range)→`ok 9`;
`[[.a.]-[.Z.]]` reversed→no-match→`ok 11`; `[[.a.]-[.zz.]p]` matches `p`→`ok 12`;
`[[.aa.]-[.z.]p]` matches `p`→`ok 13`.

## Testing

Gate = bash 5.2.21 fidelity + `posixpat` at 0 diff + no per-category regressions.

1. **Bash-diff harness** `tests/scripts/collating_symbols_diff_check.sh` (new,
   model on `posix_classes_diff_check.sh`), byte-identical incl. exit: a matrix of
   `case`, `[[ … ]]`, `${x#…}` and glob contexts over single-char `[.a.]`/`[.-.]`,
   named `[.hyphen.]`/`[.space.]`/`[.grave-accent.]`/`[.newline.]`, ranges
   `[.a.]-[.z.]`/`[.hyphen.]-9`, reversed `[.a.]-[.Z.]`, invalid `[.zz.]`/`[.yyz.]`
   (as element and range endpoint), negation `[![.a.]]`, and mixed
   class+collating `[[:digit:][.hyphen.]]`.
2. **`posixpat` category** flips: `HUCK_BASH_TEST_CATEGORY=posixpat` → PASS, 0 diff.
3. **Regression**: huck-engine lib green; the glob/pattern/case `-p huck`
   integration bins green (`glob`, `extglob`, `extglob_pathname`, `posix_classes`,
   `bracket_negation`, `case`, `param_substitution`, `double_bracket`); full
   `run_diff_checks.sh` sweep green; previously-flipped categories stay PASS;
   compare per-category diff-LINE counts against the saved baseline scratch dir —
   the range-parser refactor touches the shared bracket path, so watch `glob-test`,
   `globstar`, `extglob`, `case`, `posixexp`/`posixexp2`, `more-exp` for any
   within-category regression the PASS table would hide.

Per repo constraints: build the binary with `cargo build -p huck`; per-crate tests
single-threaded; NEVER `cargo test --workspace`; guard runner/sweeps with
`ulimit -v` + `timeout`; run the `-p huck` integration bins single-threaded before
push; do NOT copy bash's GPL source — the table is built from the POSIX.2 standard.

## Scope

**In scope.** The collating-symbol table + `collsym` lookup; `has_collating_symbol`
routing predicate + its two call-site wirings; `parse_class` `[.name.]` recognition
and the atom-based range refactor; the harness; the `posixpat` flip; regressions.

**Out of scope.** Multi-character collating elements that map to a >1-char sequence
(none in the C locale / POSIX.2 table — all entries are single chars). Locale-aware
collation ordering (huck is C-locale only, matching the test's `LC_ALL=C`).
Equivalence classes `[=a=]` (not in `posixpat`) — file a follow-up only if a future
category needs them.

## Documentation

- Removes a divergence (no new intentional one). #302 auto-closes via the PR
  (`Closes #302`); `docs/bash-divergences.md` unchanged.
- Update `docs/bash-test-suite-baseline.md` (`posixpat` PASS, Summary PASS 24→25,
  FAIL 58→57); record in `project_huck_iterations.md` + `MEMORY.md`.
