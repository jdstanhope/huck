# posixpat Collating Symbols Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Flip the `posixpat` bash-suite category to PASS by implementing POSIX.2 collating symbols `[[.name.]]` in the bracket-expression matcher (Summary PASS 24→25, FAIL 58→57).

**Architecture:** All matching logic lives in `crates/huck-engine/src/glob_match.rs`. A pattern reaches the capable `extglob_match` engine only when a routing predicate fires; today collating-symbol patterns fall to the `glob` crate and fail. Fix = (1) a `has_collating_symbol` routing predicate wired at the two match sites, (2) `[.name.]` recognition in `parse_class` backed by a POSIX.2 name→char table.

**Tech Stack:** Rust (crate `huck-engine`); bash-diff harnesses under `tests/scripts/`; the official bash test-suite runner.

Issue: [#302](https://github.com/jdstanhope/huck/issues/302). Spec: `docs/superpowers/specs/2026-07-25-posixpat-collating-symbols-design.md`.

## Global Constraints

- Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- `cargo fmt --all` before every commit (CI enforces `--check`).
- Build the binary with `cargo build -p huck` (the harness uses `target/debug/huck`).
- NEVER `cargo test --workspace` (OOM-kills this 1-core/1.9GB box). Per-crate, single-threaded: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`.
- Guard the bash-suite runner / diff sweeps with `ulimit -v 2000000` + `timeout`.
- Run the `-p huck` integration binaries single-threaded before any push (do NOT restrict `cargo test` with `ulimit -v` — that starves the compile; serial execution is the OOM guard).
- Do NOT copy bash's GPL source (`lib/glob/collsyms.h`) — the table is authored from the POSIX.2 standard (table 2.8). Harness fragments are original.
- Bash source for the runner: `BASH_SOURCE_DIR=/tmp/bash-5.2.21`. Baseline scratch for regression comparison: `/tmp/huck-bash-tests-20260725T194739Z.Se3p2j` (current-main baseline; `posixpat` diff 21 lines).
- Work on branch `v337-posixpat-collating-symbols` off `main`. Do NOT merge to main or push to main.

---

### Task 1: Collating-symbol table, `collsym()` lookup, and `has_collating_symbol()` predicate

**Files:**
- Modify: `crates/huck-engine/src/glob_match.rs` (add the table, `collsym`, `has_collating_symbol`, and unit tests)

**Interfaces:**
- Consumes: nothing new.
- Produces: `fn collsym(name: &str) -> Option<char>` (module-private) and `pub fn has_collating_symbol(pattern: &str) -> bool`. Task 2 uses `collsym`; Task 3 uses `has_collating_symbol`.

**Background:** bash resolves a collating symbol via `collsym()`: look the name up in the POSIX.2 table; else if the name is a single character return it; else INVALID. `has_collating_symbol` mirrors the existing `has_posix_class` (glob_match.rs:189) but scans for `[.` … `.]`.

- [ ] **Step 1: Write failing unit tests**

Add to the `tests` module in `glob_match.rs`:

```rust
#[test]
fn collsym_named_and_single_and_invalid() {
    assert_eq!(collsym("hyphen"), Some('-'));
    assert_eq!(collsym("space"), Some(' '));
    assert_eq!(collsym("grave-accent"), Some('`'));
    assert_eq!(collsym("newline"), Some('\n'));
    assert_eq!(collsym("period"), Some('.'));
    assert_eq!(collsym("a"), Some('a'));   // single-char passthrough (letters omitted from table)
    assert_eq!(collsym("-"), Some('-'));   // single-char passthrough
    assert_eq!(collsym("Z"), Some('Z'));
    assert_eq!(collsym("zz"), None);       // multi-char non-name → invalid
    assert_eq!(collsym("yyz"), None);
}

#[test]
fn has_collating_symbol_detects() {
    assert!(has_collating_symbol("[[.a.]]"));
    assert!(has_collating_symbol("x[[.hyphen.]-9]y"));
    assert!(!has_collating_symbol("[[:alpha:]]"));  // that's a class, not a collating symbol
    assert!(!has_collating_symbol("[abc]"));
    assert!(!has_collating_symbol("plain"));
    assert!(!has_collating_symbol("\\[.a.]"));       // escaped `[` — not a bracket
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p huck-engine --jobs 1 --lib collsym -- --test-threads 1` and `... has_collating_symbol ...`
Expected: FAIL (functions not defined).

- [ ] **Step 3: Add the POSIX.2 table + `collsym` + `has_collating_symbol`**

In `glob_match.rs`:

```rust
/// POSIX.2 table 2.8 collating-symbol names → their C-locale character.
/// Authored from the POSIX standard (NOT copied from bash's GPL collsyms.h).
/// Upper/lower letters are intentionally omitted — single-char passthrough in
/// `collsym` covers them; digits are listed by name here.
static POSIX_COLLSYMS: &[(&str, char)] = &[
    ("NUL", '\0'), ("SOH", '\u{01}'), ("STX", '\u{02}'), ("ETX", '\u{03}'),
    ("EOT", '\u{04}'), ("ENQ", '\u{05}'), ("ACK", '\u{06}'), ("alert", '\u{07}'),
    ("BS", '\u{08}'), ("backspace", '\u{08}'), ("HT", '\t'), ("tab", '\t'),
    ("LF", '\n'), ("newline", '\n'), ("VT", '\u{0b}'), ("vertical-tab", '\u{0b}'),
    ("FF", '\u{0c}'), ("form-feed", '\u{0c}'), ("CR", '\r'), ("carriage-return", '\r'),
    ("SO", '\u{0e}'), ("SI", '\u{0f}'), ("DLE", '\u{10}'), ("DC1", '\u{11}'),
    ("DC2", '\u{12}'), ("DC3", '\u{13}'), ("DC4", '\u{14}'), ("NAK", '\u{15}'),
    ("SYN", '\u{16}'), ("ETB", '\u{17}'), ("CAN", '\u{18}'), ("EM", '\u{19}'),
    ("SUB", '\u{1a}'), ("ESC", '\u{1b}'), ("IS4", '\u{1c}'), ("FS", '\u{1c}'),
    ("IS3", '\u{1d}'), ("GS", '\u{1d}'), ("IS2", '\u{1e}'), ("RS", '\u{1e}'),
    ("IS1", '\u{1f}'), ("US", '\u{1f}'), ("space", ' '), ("exclamation-mark", '!'),
    ("quotation-mark", '"'), ("number-sign", '#'), ("dollar-sign", '$'),
    ("percent-sign", '%'), ("ampersand", '&'), ("apostrophe", '\''),
    ("left-parenthesis", '('), ("right-parenthesis", ')'), ("asterisk", '*'),
    ("plus-sign", '+'), ("comma", ','), ("hyphen", '-'), ("hyphen-minus", '-'),
    ("minus", '-'), ("dash", '-'), ("period", '.'), ("full-stop", '.'),
    ("slash", '/'), ("solidus", '/'), ("zero", '0'), ("one", '1'), ("two", '2'),
    ("three", '3'), ("four", '4'), ("five", '5'), ("six", '6'), ("seven", '7'),
    ("eight", '8'), ("nine", '9'), ("colon", ':'), ("semicolon", ';'),
    ("less-than-sign", '<'), ("equals-sign", '='), ("greater-than-sign", '>'),
    ("question-mark", '?'), ("commercial-at", '@'), ("left-square-bracket", '['),
    ("backslash", '\\'), ("reverse-solidus", '\\'), ("right-square-bracket", ']'),
    ("circumflex", '^'), ("circumflex-accent", '^'), ("underscore", '_'),
    ("grave-accent", '`'), ("left-brace", '{'), ("left-curly-bracket", '{'),
    ("vertical-line", '|'), ("right-brace", '}'), ("right-curly-bracket", '}'),
    ("tilde", '~'), ("DEL", '\u{7f}'),
];

/// Resolve a POSIX.2 collating-symbol name: table lookup, else a single
/// character is a collating element for itself, else invalid (`None`).
fn collsym(name: &str) -> Option<char> {
    if let Some(&(_, c)) = POSIX_COLLSYMS.iter().find(|&&(n, _)| n == name) {
        return Some(c);
    }
    let mut it = name.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// True if `pattern` contains a collating symbol `[.` … `.]` (unescaped `[`).
/// Mirrors `has_posix_class`.
pub fn has_collating_symbol(pattern: &str) -> bool {
    let b: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '\\' {
            i += 2;
            continue;
        }
        if b[i] == '[' && i + 1 < b.len() && b[i + 1] == '.' {
            let mut j = i + 2;
            while j + 1 < b.len() {
                if b[j] == '.' && b[j + 1] == ']' {
                    return true;
                }
                j += 1;
            }
        }
        i += 1;
    }
    false
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo fmt --all && cargo test -p huck-engine --jobs 1 --lib collsym -- --test-threads 1` and `... has_collating_symbol ...`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/glob_match.rs
git commit -m "v337: POSIX.2 collating-symbol table + collsym + has_collating_symbol (#302)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `parse_class` — recognize `[.name.]`, atom-based ranges

**Files:**
- Modify: `crates/huck-engine/src/glob_match.rs` (`parse_class`, ~line 291, + unit tests)

**Interfaces:**
- Consumes: `collsym` (Task 1), `ClassAtom::{Ch,Range,Posix,Never}` (existing, line 91).
- Produces: `extglob_match` now matches `[.name.]` collating symbols and collating ranges. Task 3 relies on this behavior end-to-end.

**Background:** `parse_class` (glob_match.rs:291) currently detects ranges at the character level (`chars[i+1]=='-'`), so `[.a.]-[.z.]` cannot form a range and `[.name.]` is consumed as literal chars. Refactor the loop to parse one **bracket atom** at a time — a `[:class:]`, a `[.sym.]`, or a plain char — each yielding a standalone `ClassAtom` and an optional range-endpoint char; then detect `atom '-' atom` ranges. A range with both endpoints valid → `Range(lo,hi)` (a reversed `lo>hi` naturally matches nothing via `RangeInclusive`); a range whose second endpoint is an invalid collating symbol → `Never`. An invalid collating symbol standalone → `Never`. This reuses the existing `ClassAtom::Never` (class_matches already treats it as non-matching, so `[!…]` negation composes).

- [ ] **Step 1: Write failing unit tests**

Add to the `tests` module (these call the public `extglob_match` — helper `m(p,t)` = `extglob_match(p,t,false)` already exists in the tests module):

```rust
#[test]
fn collating_symbol_matching() {
    // single-char and named collating elements
    assert!(m("[[.a.]]", "a"));
    assert!(!m("[[.a.]]", "b"));
    assert!(m("[[.hyphen.]]", "-"));
    assert!(m("[[.space.]]", " "));
    assert!(!m("[[.grave-accent.]]", " "));   // ` != space  → posixpat ok 6
    // collating symbols as range endpoints
    assert!(m("[[.a.]-[.z.]]", "p"));          // ok 3
    assert!(m("[[.hyphen.]-9]", "-"));         // ok 2
    assert!(m("[[.-.]-9]", "4"));              // ok 7
    assert!(!m("[[.a.]-[.Z.]]", "p"));         // reversed range → no match → ok 11
    // invalid collating symbols (multi-char non-names)
    assert!(!m("[[.yyz.]-[.z.]]", "c"));       // invalid range start → ok 8
    assert!(m("[[.yyz.][.a.]-z]", "c"));       // invalid atom + valid range → ok 9
    assert!(m("[[.a.]-[.zz.]p]", "p"));        // invalid range end, literal p → ok 12
    assert!(m("[[.aa.]-[.z.]p]", "p"));        // invalid range start, literal p → ok 13
    // negation composes
    assert!(!m("[![.a.]]", "a"));
    assert!(m("[![.a.]]", "b"));
    // mixed with a POSIX class
    assert!(m("[[:digit:][.hyphen.]]", "-"));
    assert!(m("[[:digit:][.hyphen.]]", "5"));
    // no regression on plain ranges / literals
    assert!(m("[a-z]", "m"));
    assert!(!m("[a-z]", "M"));
    assert!(m("[a-]", "-"));                   // trailing '-' is literal
    assert!(m("[]a]", "]"));                   // ']' first is literal
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p huck-engine --jobs 1 --lib collating_symbol_matching -- --test-threads 1`
Expected: FAIL (collating symbols mis-parsed as literals).

- [ ] **Step 3: Refactor `parse_class` to atom-based parsing**

Inside `parse_class` (keep the `start`/`i`/`negated`/leading-`]` handling and the unterminated-class fallback unchanged), replace the body of the `while i < chars.len()` loop. Add a nested helper that parses ONE atom at index `k`, returning `(ClassAtom /*standalone*/, Option<char> /*range endpoint*/, usize /*next index*/)`:

```rust
// Parse one bracket atom at `k`: a `[:class:]`, a `[.sym.]`, or a plain char.
// Returns (standalone atom, range-endpoint char if any, index past the atom).
// Assumes chars[k] != ']' (the caller handles the closing bracket).
fn parse_atom(chars: &[char], k: usize) -> (ClassAtom, Option<char>, usize) {
    // [:name:] POSIX class — not a range endpoint.
    if chars[k] == '[' && k + 1 < chars.len() && chars[k + 1] == ':' {
        if let Some(close) = (k + 2..chars.len().saturating_sub(1))
            .find(|&j| chars[j] == ':' && chars[j + 1] == ']')
        {
            let name: String = chars[k + 2..close].iter().collect();
            let atom = match posix_class_from_name(&name) {
                Some(pc) => ClassAtom::Posix(pc),
                None => ClassAtom::Never,
            };
            return (atom, None, close + 2);
        }
    }
    // [.name.] collating symbol — a valid one is a range endpoint.
    if chars[k] == '[' && k + 1 < chars.len() && chars[k + 1] == '.' {
        if let Some(close) = (k + 2..chars.len().saturating_sub(1))
            .find(|&j| chars[j] == '.' && chars[j + 1] == ']')
        {
            let name: String = chars[k + 2..close].iter().collect();
            return match collsym(&name) {
                Some(c) => (ClassAtom::Ch(c), Some(c), close + 2),
                None => (ClassAtom::Never, None, close + 2),
            };
        }
    }
    // Plain char.
    (ClassAtom::Ch(chars[k]), Some(chars[k]), k + 1)
}

// ... in the loop, replacing the old class/range/char block:
while i < chars.len() {
    if chars[i] == ']' {
        closed = true;
        i += 1;
        break;
    }
    let (atom, lo_ep, after) = parse_atom(chars, i);
    // Range: <atom> '-' <atom>, where the '-' is not the trailing set char.
    if lo_ep.is_some() && after < chars.len() && chars[after] == '-'
        && after + 1 < chars.len() && chars[after + 1] != ']'
    {
        let (_batom, hi_ep, after2) = parse_atom(chars, after + 1);
        match (lo_ep, hi_ep) {
            (Some(lo), Some(hi)) => set.push(ClassAtom::Range(lo, hi)),
            _ => set.push(ClassAtom::Never), // invalid collating endpoint
        }
        i = after2;
    } else {
        set.push(atom);
        i = after;
    }
}
```

Note: nested `fn parse_atom` cannot see `parse_class`'s locals (it takes `chars`/`k` explicitly) — define it as a `fn` item inside `parse_class` or at module scope. It calls `collsym` and `posix_class_from_name` (both module-scope).

- [ ] **Step 4: Run the new + existing tests**

Run: `cargo fmt --all && cargo test -p huck-engine --jobs 1 --lib collating_symbol_matching -- --test-threads 1`
Then the whole glob_match test group: `cargo test -p huck-engine --jobs 1 --lib glob_match -- --test-threads 1` (and any `parse_class`/`class`/`bracket`/`extglob` named tests).
Expected: all PASS — new collating tests green AND every pre-existing bracket/class/range/extglob test still green (the atom refactor must preserve `[a-z]`, `[]a]`, `[a-]`, `[[:alpha:]]`, negation, etc.).

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/glob_match.rs
git commit -m "v337: parse_class recognizes [.name.] collating symbols + atom-based ranges (#302)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Wire routing predicate, harness, category flip, regression, docs

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (`case_item_matches`, ~line 2365)
- Modify: `crates/huck-engine/src/param_expansion.rs` (`pe_pattern_matches`, ~line 488)
- Create: `tests/scripts/collating_symbols_diff_check.sh`
- Modify: `docs/bash-test-suite-baseline.md`
- Modify: memory `project_huck_iterations.md` + `MEMORY.md` (outside the repo tree)

**Interfaces:**
- Consumes: `has_collating_symbol` (Task 1), the `[.name.]` matching (Task 2).
- Produces: end-to-end collating-symbol matching in `case`, `${x#…}`, and globbing.

**Background:** Both routing sites currently gate on `(extglob && has_extglob) || has_posix_class`. Add `|| has_collating_symbol(...)` so collating-symbol patterns reach `extglob_match` instead of the `glob` crate.

- [ ] **Step 1: Write the failing harness**

Create `tests/scripts/collating_symbols_diff_check.sh` modeled on `tests/scripts/posix_classes_diff_check.sh` (a `checkf label 'fragment'` runner that diffs bash vs `$HUCK_BIN`, byte-identical incl. stderr + exit). Cover, across `case`, `[[ … ]]`, and `${x#…}` contexts:

```
# single-char + named collating elements
case a in [[.a.]]) echo m;; *) echo no;; esac
case - in [[.hyphen.]]) echo m;; *) echo no;; esac
case ' ' in [[.space.]]) echo m;; *) echo no;; esac
case '`' in [[.grave-accent.]]) echo m;; *) echo no;; esac
# ranges with collating endpoints
case p in [[.a.]-[.z.]]) echo m;; *) echo no;; esac
case - in [[.hyphen.]-9]) echo m;; *) echo no;; esac
case 4 in [[.-.]-9]) echo m;; *) echo no;; esac
# reversed / invalid → no match (not error)
case p in [[.a.]-[.Z.]]) echo bad;; *) echo ok;; esac
case c in [[.yyz.]-[.z.]]) echo bad;; *) echo ok;; esac
case p in [[.a.]-[.zz.]p]) echo m;; *) echo no;; esac
# negation + mixed with a class
case a in [![.a.]]) echo bad;; *) echo ok;; esac
case 5 in [[:digit:][.hyphen.]]) echo m;; *) echo no;; esac
# param-expansion + [[ ]] contexts
x=abc; echo "${x#[[.a.]]}"
[[ - == [[.hyphen.]] ]] && echo yes || echo no
```

Wire it into `tests/scripts/run_diff_checks.sh` the same way `posix_classes_diff_check.sh` is registered (check how that script is listed/discovered and add the new one).

- [ ] **Step 2: Build + run the harness to verify failure**

Run: `cargo build -p huck && HUCK_BIN=$(pwd)/target/debug/huck bash tests/scripts/collating_symbols_diff_check.sh`
Expected: the `case`/`${x#…}`/glob cases FAIL (routing still sends bare `[[.…]]` to the `glob` crate); the `[[ … ]]` cases may already pass (depends on its matcher). At least the `case` cases must FAIL here.

- [ ] **Step 3: Wire `has_collating_symbol` into both routing sites**

`executor.rs` `case_item_matches`:

```rust
let hit = if (extglob && crate::glob_match::has_extglob(&pattern))
    || crate::glob_match::has_posix_class(&pattern)
    || crate::glob_match::has_collating_symbol(&pattern)
{
    crate::glob_match::extglob_match(&pattern, subject, nocase)
} else { /* glob crate — unchanged */ };
```

`param_expansion.rs` `pe_pattern_matches`:

```rust
if (extglob && crate::glob_match::has_extglob(pattern))
    || crate::glob_match::has_posix_class(pattern)
    || crate::glob_match::has_collating_symbol(pattern)
{
    crate::glob_match::extglob_match(pattern, text, !case_sensitive)
} else { /* glob crate — unchanged */ }
```

(Check whether the pathname-glob expansion path — `extglob_pathname_expand` / the glob walker in `expand.rs` — has its own routing gate; if a `ls [[.a.]]`-style glob would bypass `extglob_match`, add `has_collating_symbol` there too. If pathname globbing already always uses `extglob_match`, no change needed. Note the finding in the report.)

- [ ] **Step 4: Rebuild + run the harness to verify pass**

Run: `cargo fmt --all && cargo build -p huck && HUCK_BIN=$(pwd)/target/debug/huck bash tests/scripts/collating_symbols_diff_check.sh`
Expected: all cases PASS (byte-identical bash↔huck).

- [ ] **Step 5: Confirm the `posixpat` category flips**

Run:
```bash
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
ulimit -v 2500000
HUCK_BASH_TEST_CATEGORY=posixpat timeout 120 bash tests/bash-test-suite/runner.sh 2>/dev/null | grep -i posixpat
```
Expected: `| posixpat | PASS |`. If still FAIL, read the new scratch dir's `posixpat.diff` (establish diff direction explicitly — compare `.out` vs the committed `/tmp/bash-5.2.21/tests/posixpat.right`) and reconcile.

- [ ] **Step 6: Full runner + per-category regression check**

Run the full runner to `/tmp/v337-after.md` (background it via a PID-based waiter — never a `pgrep` pattern that could match the waiter itself). Confirm `PASS: 25`, `FAIL: 57`, `TIMEOUT: 0`. Then compare per-category diff-LINE counts against the baseline `/tmp/huck-bash-tests-20260725T194739Z.Se3p2j`:
```bash
OLD=/tmp/huck-bash-tests-20260725T194739Z.Se3p2j
NEW=$(grep -oE '/tmp/huck-bash-tests-[^ )]+' /tmp/v337-after.md | head -1)
for f in "$NEW"/*.diff; do c=$(basename "$f" .diff);
  o=$( [ -f "$OLD/$c.diff" ] && wc -l < "$OLD/$c.diff" || echo 0 ); n=$(wc -l < "$f");
  [ "$o" != "$n" ] && echo "$c: $o -> $n"; done
```
Expected: only `posixpat` changes (21 → 0). No other category's diff-line count increases. Watch `glob-test`, `globstar`, `extglob`, `case`, `posixexp`/`posixexp2`, `more-exp` (the shared bracket-parser path). If any increased, STOP and investigate.

- [ ] **Step 7: Integration bins + full bash-diff sweep**

Run the glob/pattern/case `-p huck` integration bins single-threaded (`glob`, `extglob`, `extglob_pathname`, `posix_classes`, `bracket_negation`, `case`, `param_substitution`, `double_bracket`, plus the rest CI runs) — no `ulimit -v` on cargo. Then `ulimit -v 2000000; timeout 600 bash tests/scripts/run_diff_checks.sh` → green (includes the new harness).

- [ ] **Step 8: Docs + memory + commit**

- Update `docs/bash-test-suite-baseline.md`: add a "Updated by v337" line (`posixpat` PASS, Summary PASS 24→25, FAIL 58→57) and flip the `posixpat` table row.
- Update memory `project_huck_iterations.md` (full v337 entry) + `MEMORY.md` (one-line hook): the collating-symbol feature, the two-part fix, and the durable note (POSIX.2 table authored not copied; atom-based range refactor; establish diff direction explicitly).

```bash
git add crates/huck-engine/src/executor.rs crates/huck-engine/src/param_expansion.rs \
        tests/scripts/collating_symbols_diff_check.sh tests/scripts/run_diff_checks.sh \
        docs/bash-test-suite-baseline.md
git commit -m "v337: route + wire collating symbols; flip posixpat to PASS (#302)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final review + PR (after Task 3)

- Whole-branch diff review (most capable model): confirm the atom refactor preserves all prior bracket behavior, the table matches POSIX.2 (no GPL copy), routing is wired at every context that needs it, and no regressions.
- Push `v337-posixpat-collating-symbols`; open a PR targeting `main` with body `Closes #302`. Wait for CI to finish and pass before handing off (local green ≠ CI green on this 1-core box). Do NOT merge — the user merges.
