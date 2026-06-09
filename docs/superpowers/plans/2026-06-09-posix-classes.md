# v119 — POSIX bracket character classes in glob patterns (M-54) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Support POSIX bracket character classes (`[[:alpha:]]`, `[[:digit:]]`, `[[:space:]]`, … all 12) inside glob bracket expressions in every glob context (`${var/}`/`#`/`%`, `case`, `[[ == ]]`, completion, pathname globbing), fixing the residual `mise<TAB>` `cur` trailing-space and a broad glob-correctness gap.

**Architecture:** Extend huck's own glob matcher (`src/glob_match.rs`) — `parse_class` recognizes `[:name:]` → a new `ClassAtom::Posix(PosixClass)` (unknown name → `ClassAtom::Never`), matched via ASCII/C-locale char predicates. Add `has_posix_class(pattern)` and, at the 5 match sites, route class-bearing patterns through `extglob_match` / `extglob_pathname_expand` (the own-matcher, which already powers both string and pathname matching), **unconditional on the extglob shopt**.

**Tech Stack:** Rust. `src/glob_match.rs` (matcher), `src/param_expansion.rs`, `src/executor.rs`, `src/completion_spec.rs`, `src/expand.rs` (dispatch). Tests: `cargo test`, a new integration test, a new `tests/scripts/*_diff_check.sh` harness.

**Spec:** `docs/superpowers/specs/2026-06-09-posix-classes-design.md`. Read it first.

**Commit trailer (MANDATORY, canonical — every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

Anchors (verify exact lines — code shifts):
- `enum ClassAtom { Ch(char), Range(char, char) }` (`src/glob_match.rs:93`).
- `parse_class` set-scanning loop (`:213-227`) — the `while i < chars.len()` with the `]`-close, range, and literal arms.
- `class_matches(set, negated, c, ci)` (`:280`) — the per-atom match.
- `has_extglob` (`:100`) — the model for `has_posix_class`.
- Dispatch sites: `pe_pattern_matches` (`param_expansion.rs`, `if extglob && crate::glob_match::has_extglob(pattern)`), `case` (`executor.rs:1074`), `[[ == ]]` (`executor.rs:1280`), completion `glob_match` (`completion_spec.rs`), pathname (`expand.rs:1409`, `let is_extglob = opts.extglob && crate::glob_match::has_extglob(&pattern);`).

**Verified bash contract (probed):** `${s//[[:digit:]]/_}` (a1 b2)→`a_ b_`; `${s//[[:alpha:]]/_}`→`_1 _2`; `case " " in [[:space:]])`→match; `[[ x == [[:alpha:]] ]]`→Y; `[[:space:]]` matches `\v`; `[[:print:]]` matches space; `[[:punct:]]` matches `]`; classes work with extglob OFF; mixed `[[:digit:]_]`→digit-or-`_`; unknown `[[:bogus:]]` matches NOTHING.

---

## Task 1: extend `glob_match.rs` for POSIX classes (own-matcher core)

**Files:**
- Modify: `src/glob_match.rs` (enum, parse, match, `has_posix_class`; unit tests)

- [ ] **Step 1: Write the failing unit tests**

In `src/glob_match.rs`'s `#[cfg(test)] mod`, add:
```rust
#[cfg(test)]
mod posix_class_tests {
    use super::{extglob_match, has_posix_class};

    fn m(p: &str, t: &str) -> bool { extglob_match(p, t, false) }

    #[test]
    fn digit_alpha_space() {
        assert!(m("[[:digit:]]", "5"));
        assert!(!m("[[:digit:]]", "x"));
        assert!(m("[[:alpha:]]", "x"));
        assert!(!m("[[:alpha:]]", "5"));
        assert!(m("[[:space:]]", " "));
        assert!(m("[[:space:]]", "\u{0b}")); // vertical tab — POSIX space includes \v
        assert!(!m("[[:space:]]", "x"));
    }
    #[test]
    fn upper_lower_alnum_xdigit() {
        assert!(m("[[:upper:]]", "A") && !m("[[:upper:]]", "a"));
        assert!(m("[[:lower:]]", "a") && !m("[[:lower:]]", "A"));
        assert!(m("[[:alnum:]]", "Z") && m("[[:alnum:]]", "7") && !m("[[:alnum:]]", "_"));
        assert!(m("[[:xdigit:]]", "f") && m("[[:xdigit:]]", "9") && !m("[[:xdigit:]]", "g"));
    }
    #[test]
    fn punct_cntrl_graph_print_blank() {
        assert!(m("[[:punct:]]", "]") && m("[[:punct:]]", "!") && !m("[[:punct:]]", "a"));
        assert!(m("[[:cntrl:]]", "\u{01}") && !m("[[:cntrl:]]", "a"));
        assert!(m("[[:graph:]]", "!") && !m("[[:graph:]]", " "));
        assert!(m("[[:print:]]", " ") && m("[[:print:]]", "!") && !m("[[:print:]]", "\u{01}"));
        assert!(m("[[:blank:]]", " ") && m("[[:blank:]]", "\t") && !m("[[:blank:]]", "\n"));
    }
    #[test]
    fn negation_and_mixed() {
        assert!(m("[^[:digit:]]", "x") && !m("[^[:digit:]]", "5"));
        assert!(m("[[:digit:]_]", "5") && m("[[:digit:]_]", "_") && !m("[[:digit:]_]", "a"));
        assert!(m("[[:digit:]a-f]", "c") && m("[[:digit:]a-f]", "3") && !m("[[:digit:]a-f]", "z"));
    }
    #[test]
    fn unknown_class_matches_nothing() {
        assert!(!m("[[:bogus:]]", "x"));
        assert!(!m("[[:bogus:]]", ":"));
    }
    #[test]
    fn single_bracket_colon_is_literal_set() {
        // `[:y:]` (single bracket) is a literal set {':','y'}, NOT a class.
        assert!(m("[:y:]", ":") && m("[:y:]", "y") && !m("[:y:]", "z"));
    }
    #[test]
    fn has_posix_class_detection() {
        assert!(has_posix_class("[[:space:]]"));
        assert!(has_posix_class("x[[:digit:]]y"));
        assert!(has_posix_class("[^[:alpha:]]"));
        assert!(!has_posix_class("[abc]"));
        assert!(!has_posix_class("[a-z]"));
        assert!(!has_posix_class("plain*"));
        assert!(!has_posix_class("\\[[:x"));  // escaped, no close
    }
}
```

- [ ] **Step 2: Run — confirm fail (compile errors / mismatches)**

Run: `cargo test --bin huck posix_class 2>&1 | tail -15`
Expected: fails to compile (`has_posix_class` undefined) / assertions fail.

- [ ] **Step 3: Add `PosixClass`, `ClassAtom::Posix`/`Never`, membership**

In `src/glob_match.rs`, after the `ClassAtom` enum (`:93-96`), add:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PosixClass {
    Alpha, Digit, Alnum, Upper, Lower, Space, Blank, Punct, Cntrl, Graph, Print, Xdigit,
}

fn posix_class_from_name(name: &str) -> Option<PosixClass> {
    use PosixClass::*;
    Some(match name {
        "alpha" => Alpha, "digit" => Digit, "alnum" => Alnum,
        "upper" => Upper, "lower" => Lower, "xdigit" => Xdigit,
        "punct" => Punct, "cntrl" => Cntrl, "graph" => Graph,
        "space" => Space, "blank" => Blank, "print" => Print,
        _ => return None,
    })
}

fn posix_matches(pc: PosixClass, c: char, ci: bool) -> bool {
    use PosixClass::*;
    match pc {
        Alpha => c.is_ascii_alphabetic(),
        Digit => c.is_ascii_digit(),
        Alnum => c.is_ascii_alphanumeric(),
        // Under case-insensitive matching, upper/lower widen to any letter.
        Upper => if ci { c.is_ascii_alphabetic() } else { c.is_ascii_uppercase() },
        Lower => if ci { c.is_ascii_alphabetic() } else { c.is_ascii_lowercase() },
        Xdigit => c.is_ascii_hexdigit(),
        Punct => c.is_ascii_punctuation(),
        Cntrl => c.is_ascii_control(),
        Graph => c.is_ascii_graphic(),
        // POSIX `space` includes \v (0x0b), which Rust's is_ascii_whitespace omits.
        Space => matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{0b}' | '\u{0c}'),
        Blank => matches!(c, ' ' | '\t'),
        Print => c.is_ascii_graphic() || c == ' ',
    }
}
```
Extend the `ClassAtom` enum to:
```rust
#[derive(Debug, Clone)]
enum ClassAtom {
    Ch(char),
    Range(char, char),
    Posix(PosixClass),
    Never, // unknown POSIX class name: matches nothing
}
```
In `class_matches` (`:280`), add arms inside the `for atom in set` match (after the `Range` arm):
```rust
            ClassAtom::Posix(pc) => {
                if posix_matches(*pc, c, ci) {
                    hit = true;
                    break;
                }
            }
            ClassAtom::Never => {}
```

- [ ] **Step 4: Recognize `[:name:]` in `parse_class`**

In `parse_class`'s set-scanning loop (`:213`), insert a POSIX-class branch immediately AFTER the `]`-close check and BEFORE the range check:
```rust
    while i < chars.len() {
        if chars[i] == ']' {
            closed = true;
            i += 1;
            break;
        }
        // POSIX class `[:name:]` (the inner `[:` of `[[:name:]]`).
        if chars[i] == '[' && i + 1 < chars.len() && chars[i + 1] == ':' {
            if let Some(close) = (i + 2..chars.len().saturating_sub(1))
                .find(|&k| chars[k] == ':' && chars[k + 1] == ']')
            {
                let name: String = chars[i + 2..close].iter().collect();
                set.push(match posix_class_from_name(&name) {
                    Some(pc) => ClassAtom::Posix(pc),
                    None => ClassAtom::Never,
                });
                i = close + 2; // skip past ":]"
                continue;
            }
            // not a valid `[:...:]` — fall through to literal handling.
        }
        // Range: x-y (where y is not the closing ']').
        if i + 2 < chars.len() && chars[i + 1] == '-' && chars[i + 2] != ']' {
            set.push(ClassAtom::Range(chars[i], chars[i + 2]));
            i += 3;
        } else {
            set.push(ClassAtom::Ch(chars[i]));
            i += 1;
        }
    }
```
(Keep the rest of `parse_class` — the `!closed` unterminated fallback and `Item::Class { negated, set }` — unchanged.)

- [ ] **Step 5: Add `has_posix_class`**

After `has_extglob` (`:114`), add:
```rust
/// True if `pattern` contains a POSIX bracket class `[:name:]` (the
/// `[[:name:]]` form) — an unescaped `[:` followed later by `:]`. Liberal: a
/// false positive only routes a class-free pattern through the (faithful)
/// own-matcher, which is harmless.
pub fn has_posix_class(pattern: &str) -> bool {
    let b: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '\\' {
            i += 2;
            continue;
        }
        if b[i] == '[' && i + 1 < b.len() && b[i + 1] == ':' {
            let mut j = i + 2;
            while j + 1 < b.len() {
                if b[j] == ':' && b[j + 1] == ']' {
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

- [ ] **Step 6: Run unit tests — confirm green**

Run: `cargo test --bin huck posix_class 2>&1 | tail -15`
Expected: all `posix_class_tests` PASS. If `class_matches`/`parse_class` are referenced by other glob tests, run `cargo test --bin huck glob 2>&1 | tail -8` (no regressions).

- [ ] **Step 7: Clippy + commit**

Run: `cargo clippy --bin huck 2>&1 | tail -3` (clean; the new `Copy` enum + `matches!` should be idiomatic).
```bash
git add src/glob_match.rs
git commit -m "$(cat <<'EOF'
feat: POSIX bracket character classes in the glob matcher (M-54)

parse_class recognizes [:name:] -> ClassAtom::Posix(PosixClass) (unknown name
-> ClassAtom::Never, matches nothing), matched via ASCII/C-locale char
predicates (all 12 classes; space includes \v, print = graphic-or-space).
New has_posix_class scanner. Not wired into the dispatch sites yet.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1 report
DONE/BLOCKED, commit SHA, the `posix_matches` + `parse_class` branch as written, the unit-test pass line, clippy status.

---

## Task 2: route class-bearing patterns at the 5 sites + integration tests

**Files:**
- Modify: `src/param_expansion.rs`, `src/executor.rs`, `src/completion_spec.rs`, `src/expand.rs`
- Create: `tests/posix_classes_integration.rs`

- [ ] **Step 1: Write the failing integration tests**

Create `tests/posix_classes_integration.rs` (file-arg `run` helper with pid+atomic-counter temp path, per L-27):
```rust
//! v119: POSIX bracket character classes in glob patterns (M-54).
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
fn run(script: &str) -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v119_{}_{}.sh", std::process::id(), n));
    { let mut f = std::fs::File::create(&path).unwrap(); f.write_all(script.as_bytes()).unwrap(); }
    let out = Command::new(huck_bin()).arg(&path).stdin(Stdio::null()).output().unwrap();
    let _ = std::fs::remove_file(&path);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn subst_digit_alpha_space() {
    assert_eq!(run("s=\"a1 b2\"\necho \"[${s//[[:digit:]]/_}]\"\n"), "[a_ b_]\n");
    assert_eq!(run("s=\"a1 b2\"\necho \"[${s//[[:alpha:]]/_}]\"\n"), "[_1 _2]\n");
    assert_eq!(run("s=\"a b\tc\"\necho \"[${s//[[:space:]]/_}]\"\n"), "[a_b_c]\n");
}
#[test]
fn subst_upper_lower_alnum_punct() {
    assert_eq!(run("s=\"aXbY\"\necho \"[${s//[[:upper:]]/_}]\"\n"), "[a_b_]\n");
    assert_eq!(run("s=\"a.b!c\"\necho \"[${s//[[:punct:]]/_}]\"\n"), "[a_b_c]\n");
    // a,1,b are alnum (→X); the literal `_` is NOT alnum (kept) → "XX_X".
    assert_eq!(run("s=\"a1_b\"\necho \"[${s//[[:alnum:]]/X}]\"\n"), "[XX_X]\n");
}
#[test]
fn case_and_dbracket_membership() {
    assert_eq!(run("case \" \" in [[:space:]]) echo SP;; *) echo no;; esac\n"), "SP\n");
    assert_eq!(run("case \"5\" in [[:space:]]) echo SP;; *) echo no;; esac\n"), "no\n");
    assert_eq!(run("[[ \"x\" == [[:alpha:]] ]] && echo Y || echo N\n"), "Y\n");
    assert_eq!(run("[[ \"5\" == [[:alpha:]] ]] && echo Y || echo N\n"), "N\n");
}
#[test]
fn negation_and_mixed() {
    assert_eq!(run("[[ \"x\" == [^[:digit:]] ]] && echo Y || echo N\n"), "Y\n");
    assert_eq!(run("s=\"a5_b\"\necho \"[${s//[[:digit:]_]/X}]\"\n"), "[aXXb]\n");
}
#[test]
fn extglob_off_classes_still_work() {
    assert_eq!(run("shopt -u extglob\ncase \"5\" in [[:digit:]]) echo D;; *) echo no;; esac\n"), "D\n");
}
#[test]
fn pathname_upper_class() {
    let s = "d=$(mktemp -d); touch \"$d\"/Afile \"$d\"/bfile \"$d\"/Cfile\n\
             cd \"$d\"; for f in [[:upper:]]*; do echo \"$f\"; done; rm -rf \"$d\"\n";
    assert_eq!(run(s), "Afile\nCfile\n");
}
```
**NOTE for the implementer:** verify EVERY fragment's expected value against real bash FIRST (`printf '%s\n' "$frag" | bash --norc --noprofile`) and hard-code bash's exact output before relying on the assertion — the values above were derived by hand; bash is the source of truth (the v115/v117 lesson).

- [ ] **Step 2: Run — confirm fail**

Run: `cargo build --bin huck && cargo test --test posix_classes_integration 2>&1 | tail -20`
Expected: most class tests FAIL (huck no-ops the class); fix in the next steps.

- [ ] **Step 3: Route in `pe_pattern_matches` (param_expansion.rs)**

Change the condition:
```rust
    if extglob && crate::glob_match::has_extglob(pattern) {
```
to:
```rust
    if (extglob && crate::glob_match::has_extglob(pattern))
        || crate::glob_match::has_posix_class(pattern)
    {
```
(The `extglob_match(pattern, text, !case_sensitive)` body is unchanged; the `else` glob-crate branch stays for class-free patterns.)

- [ ] **Step 4: Route in `case` (executor.rs:1074)**

Change `if extglob && crate::glob_match::has_extglob(&pattern) {` to:
```rust
        let hit = if (extglob && crate::glob_match::has_extglob(&pattern))
            || crate::glob_match::has_posix_class(&pattern)
        {
```
(Match the existing `let hit = if … {` shape; only the condition changes.)

- [ ] **Step 5: Route in `[[ == ]]` (executor.rs:1280)**

Change `if extglob && crate::glob_match::has_extglob(&pattern_str) {` to:
```rust
            let matched = if (extglob && crate::glob_match::has_extglob(&pattern_str))
                || crate::glob_match::has_posix_class(&pattern_str)
            {
```

- [ ] **Step 6: Route in completion `glob_match` (completion_spec.rs)**

Find the `if extglob && crate::glob_match::has_extglob(pattern)` (or equivalent) in `glob_match` and OR in `|| crate::glob_match::has_posix_class(pattern)`. If `glob_match` does NOT currently have an extglob branch (it may go straight to `glob::Pattern`), add one: route through `crate::glob_match::extglob_match(pattern, candidate, /*ci*/ false)` when `has_posix_class(pattern)` (use the same case sensitivity the surrounding code uses for `glob::Pattern`). Verify the exact current shape and mirror it.

- [ ] **Step 7: Route in pathname (expand.rs:1409)**

Change:
```rust
        let is_extglob = opts.extglob && crate::glob_match::has_extglob(&pattern);
```
to:
```rust
        let is_extglob = (opts.extglob && crate::glob_match::has_extglob(&pattern))
            || crate::glob_match::has_posix_class(&pattern);
```
(So a POSIX-class pattern takes the `extglob_pathname_expand` branch. The variable name `is_extglob` is now slightly misnamed but keeping it minimizes churn; optionally add a one-line comment `// also routes POSIX-class patterns`.)

- [ ] **Step 8: Run integration tests — confirm green**

Run: `cargo build --bin huck && cargo test --test posix_classes_integration 2>&1 | tail -15`
Expected: all PASS.

- [ ] **Step 9: Byte-identical spot check + full regression + clippy**

```bash
cargo build --bin huck
for f in 's="a1 b2"; echo "${s//[[:digit:]]/_}"' \
         'case " " in [[:space:]]) echo SP;; *) echo no;; esac' \
         '[[ "x" == [^[:digit:]] ]] && echo Y || echo N' \
         's="a5_b"; echo "${s//[[:digit:]_]/X}"' \
         'd=$(mktemp -d); touch "$d"/Af "$d"/bf; cd "$d"; echo [[:upper:]]*; rm -rf "$d"'; do
  printf '%s\n' "$f" > /tmp/t.sh
  b=$(bash --norc --noprofile /tmp/t.sh 2>&1); h=$(./target/debug/huck /tmp/t.sh 2>&1)
  [ "$b" = "$h" ] && echo "MATCH: $f" || { echo "DIFF: $f"; echo " b=[$b] h=[$h]"; }
done
cargo test 2>&1 | grep -E "test result: FAILED" || echo "no failures"
cargo clippy --all-targets 2>&1 | tail -3
```
Expected: all MATCH; no FAILED; clippy clean. Watch `param`/`case`/`dbracket`/`glob`/`extglob`/`completion` suites — a regression means routing altered a class-free pattern.

- [ ] **Step 10: Commit**

```bash
git add src/param_expansion.rs src/executor.rs src/completion_spec.rs src/expand.rs tests/posix_classes_integration.rs
git commit -m "$(cat <<'EOF'
fix: route POSIX-class glob patterns through the own-matcher at 5 sites (M-54)

pe_pattern_matches, case, [[ == ]], completion glob_match, and pathname
globbing now route a pattern containing a [:name:] POSIX class through
extglob_match / extglob_pathname_expand (unconditional on the extglob shopt),
so the 12 classes work in ${}/case/[[/completion/pathname like bash.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2 report
DONE/BLOCKED, commit SHA, the 5 dispatch edits, the integration-test pass line, the MATCH spot-check, full-suite green (no FAILED), clippy status. Note the real expected value you used for the `[[:alnum:]]` fragment.

---

## Task 3: 43rd bash-diff harness + mise payoff smoke

**Files:**
- Create: `tests/scripts/posix_classes_diff_check.sh`

- [ ] **Step 1: Write the harness**

Create `tests/scripts/posix_classes_diff_check.sh` (file-arg execution per L-27):
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v119: POSIX bracket character classes
# in glob patterns (M-54). Spread across the 12 classes + negation + mixed +
# pathname. File-arg execution (L-27: huck history-expands piped stdin).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h tf
    tf=$(mktemp)
    printf '%s\n' "$frag" > "$tf"
    b=$(bash --norc --noprofile "$tf" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tf" 2>&1; echo "EXIT:$?")
    rm -f "$tf"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "subst digit"   's="a1 b2"; echo "${s//[[:digit:]]/_}"'
check "subst alpha"   's="a1 b2"; echo "${s//[[:alpha:]]/_}"'
check "subst alnum"   's="a1_b2"; echo "${s//[[:alnum:]]/_}"'
check "subst space"   's=$'"'"'a b\tc'"'"'; echo "${s//[[:space:]]/_}"'
check "subst upper"   's="aXbY"; echo "${s//[[:upper:]]/_}"'
check "subst lower"   's="aXbY"; echo "${s//[[:lower:]]/_}"'
check "subst punct"   's="a.b!c]"; echo "${s//[[:punct:]]/_}"'
check "subst xdigit"  's="9fg"; echo "${s//[[:xdigit:]]/_}"'
check "subst blank"   's=$'"'"'a b\tc\nd'"'"'; echo "${s//[[:blank:]]/_}"'
check "case space"    'case " " in [[:space:]]) echo SP;; *) echo no;; esac'
check "case digit no" 'case "x" in [[:digit:]]) echo D;; *) echo no;; esac'
check "dbracket alpha" '[[ "x" == [[:alpha:]] ]] && echo Y || echo N'
check "dbracket neg"   '[[ "x" == [^[:digit:]] ]] && echo Y || echo N'
check "mixed class"    's="a5_b"; echo "${s//[[:digit:]_]/X}"'
check "extglob off"    'shopt -u extglob; case "5" in [[:digit:]]) echo D;; *) echo no;; esac'
check "pathname upper" 'd=$(mktemp -d); touch "$d"/Af "$d"/bf "$d"/Cf; cd "$d"; echo [[:upper:]]*; rm -rf "$d"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```
(If any fragment's `$'…'` quoting is awkward through the harness, the implementer may simplify the literal — but keep the 12 classes + negation + mixed + extglob-off + pathname coverage and verify each is byte-identical.)

- [ ] **Step 2: Make executable, run it, run ALL harnesses**

```bash
chmod +x tests/scripts/posix_classes_diff_check.sh && cargo build --bin huck
bash tests/scripts/posix_classes_diff_check.sh
export HUCK_BIN="$(pwd)/target/debug/huck"
echo "count: $(ls tests/scripts/*_diff_check.sh | wc -l)"
for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done
echo all-harnesses-done
```
Expected: `Total: 16, Pass: 16, Fail: 0`; `count: 43`; no `FAIL` lines. If a fragment FAILs, report the diff (do NOT change source).

- [ ] **Step 3: Payoff smoke (the mise finale — `cur` byte-identical)**

```bash
cargo build --bin huck
BC=/usr/share/bash-completion/bash_completion
{
  echo 'shopt -s extglob'
  sed -n '/^_upvars()/,/^}/p' "$BC"
  sed -n '/^__reassemble_comp_words_by_ref()/,/^}/p' "$BC"
  sed -n '/^__get_cword_at_cursor_by_ref()/,/^}/p' "$BC"
  sed -n '/^_get_comp_words_by_ref()/,/^}/p' "$BC"
  sed -n '/^_variables()/,/^}/p' "$BC"
  sed -n '/^_init_completion()/,/^}/p' "$BC"
  cat <<'DRIVE'
COMP_WORDBREAKS=$' \t\n"'"'"'><=;|&(:'
COMP_LINE='mise '; COMP_POINT=5; COMP_WORDS=(mise ""); COMP_CWORD=1
f() { local cur prev words cword split; _init_completion -n : || { echo "init rc=$?"; return; }
      echo "SMOKE cur=[$cur] prev=[$prev] cword=$cword nwords=${#words[@]} w0=[${words[0]}]"; }
f
DRIVE
} > /tmp/v119_smoke.sh
echo "--- bash ---"; bash --norc --noprofile /tmp/v119_smoke.sh 2>&1
echo "--- huck ---"; ./target/debug/huck /tmp/v119_smoke.sh 2>&1
```
Expected: huck prints `SMOKE cur=[] prev=[mise] cword=1 nwords=2 w0=[mise]` — **`cur=[]` (NO trailing space), fully byte-identical to bash**. Report the EXACT bash and huck blocks. State whether `cur` now matches (the mise residual is closed) or what remains. Honest gate: the smoke decides; do not over-claim.

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/posix_classes_diff_check.sh
git commit -m "$(cat <<'EOF'
test: 43rd bash-diff harness for POSIX classes in globs (M-54)

16 byte-identical fragments across the 12 classes + negation + mixed +
extglob-off + pathname. Payoff: the mise _init_completion smoke's cur is now
[] (trailing space cleared by ${cur//[[:space:]]/}), byte-identical to bash.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 3 report
DONE/BLOCKED, commit SHA, the `Total: 16, Pass: 16` line, the `count: 43` + no-FAIL line, and the EXACT payoff-smoke output. State whether `cur=[]` now (mise residual closed).

---

## Task 4: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Read the structures**

```bash
grep -n 'Last updated:\|Missing features (Tier 2) |\|^## Change log\|M-54:' docs/bash-divergences.md | head
grep -n '| v118 ' README.md
cargo test 2>&1 | awk '/test result:/{s+=$4} END{print "TESTCOUNT="s}'
```
Use the real TESTCOUNT for `<N>`.

- [ ] **Step 2: Flip M-54 to fixed**

In `docs/bash-divergences.md`, the `- **M-54: POSIX bracket character classes …** — \`[deferred]\` medium. …` line: change `[deferred]` to `[fixed v119]` and append a fix description:
```
 Fixed v119: huck's own glob matcher (`src/glob_match.rs`) gained `ClassAtom::Posix` — `parse_class` recognizes `[:name:]` (12 classes, unknown→matches-nothing), matched via ASCII/C-locale char predicates (`space` includes `\v`, `print`=graphic-or-space); class-bearing patterns route through `extglob_match`/`extglob_pathname_expand` at all 5 sites (`pe_pattern_matches`, `case`, `[[ == ]]`, completion, pathname), unconditional on the extglob shopt. Closed the residual `mise<TAB>` `cur` trailing-space (`${cur//[[:space:]]/}` now clears it). 43rd harness + integration tests.
```

- [ ] **Step 3: Tier-2 count + Last-updated**

- The `| Missing features (Tier 2) | <count> |` summary cell lists fixed items inline; append `; M-54 POSIX bracket character classes fixed v119`. (The count is "outstanding"; if M-54 was counted as outstanding, decrement it — check how the cell is maintained and mirror the v90/v91/v98 style entries already there.)
- "Last updated" line → replace with:
```
**Last updated:** 2026-06-09 (after v119: POSIX bracket character classes (`[[:alpha:]]` etc.) work in all glob contexts (M-54) — fixes the residual `mise<TAB>` `cur` trailing-space; the `_init_completion` chain is now fully byte-identical to bash).
```

- [ ] **Step 4: Change-log entry + README row**

Append to the END of `## Change log`:
```
- **2026-06-09**: M-54 (POSIX bracket character classes in glob patterns) shipped as v119. huck's own glob matcher gained `ClassAtom::Posix(PosixClass)` — `parse_class` recognizes `[:name:]` (all 12 classes; unknown name → matches nothing), matched via ASCII/C-locale char predicates (`space` includes `\v`, `print`=graphic-or-space); a new `has_posix_class` routes class-bearing patterns through `extglob_match`/`extglob_pathname_expand` at all 5 sites (`pe_pattern_matches`, `case`, `[[ == ]]`, completion, pathname globbing), unconditional on the extglob shopt (POSIX classes are standard globs). **PAYOFF: closes the LAST `mise<TAB>` residual — `__get_cword_at_cursor_by_ref`'s `${cur//[[:space:]]/}` now clears a whitespace-only `cur`, so the `_init_completion -n :` smoke is fully byte-identical to bash (`cur=[]`).** 43rd harness `posix_classes_diff_check.sh` (16 fragments) + integration tests; full suite <N> tests pass, clippy clean.
```
Add a README row after the v118 row:
```
| v119      | **POSIX bracket character classes in globs (M-54)** — `[[:alpha:]]`/`[[:digit:]]`/`[[:space:]]`/… (all 12) didn't work in any glob context (the `glob` crate lacks them), so `${s//[[:digit:]]/_}`, `case`/`[[ == [[:class:]] ]]`, and pathname globbing all no-op'd. Fix: huck's own matcher (`glob_match.rs`) gained `ClassAtom::Posix` — `parse_class` recognizes `[:name:]` (unknown→matches-nothing) via ASCII/C-locale char predicates (`space` incl. `\v`, `print`=graphic-or-space); new `has_posix_class` routes class-bearing patterns through `extglob_match`/`extglob_pathname_expand` at all 5 sites, unconditional on the extglob shopt. **PAYOFF: closes the LAST `mise<TAB>` residual — `${cur//[[:space:]]/}` now clears the whitespace-only `cur`, so `_init_completion` is fully byte-identical to bash (`cur=[]`).** Byte-identical across the 12 classes + negation + mixed + pathname. 43rd harness `posix_classes_diff_check.sh` (16 fragments) + integration tests; full suite <N> tests pass, clippy clean |
```

- [ ] **Step 5: Verify + commit**

```bash
grep -n 'M-54\|fixed v119\|v119' docs/bash-divergences.md README.md | head
grep -n '<N>' docs/bash-divergences.md README.md && echo "PLACEHOLDER LEFT" || echo "no placeholders"
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: v119 — POSIX bracket character classes in globs (M-54 fixed)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 4 report
DONE/BLOCKED, commit SHA, the grep proving M-54 `[fixed v119]`, no `<N>` placeholder, the test count used.

---

## Final (after all tasks)
- [ ] Whole-branch review: `git log --oneline main..HEAD`, `git diff --stat main..HEAD`.
- [ ] `cargo test 2>&1 | grep -cE 'test result: ok'` (green, no FAILED), `cargo clippy --all-targets 2>&1 | tail -2` (clean).
- [ ] All harnesses: `export HUCK_BIN="$(pwd)/target/debug/huck"; for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done` (silent = pass; 43 files).
- [ ] **Payoff**: the `_init_completion -n :` smoke is fully byte-identical incl. `cur=[]` (Task 3 Step 3).
- [ ] AskUserQuestion merge gate, then `git merge --no-ff` + push + delete branch, then update memory files (`project_huck_iterations.md` + `MEMORY.md`; MEMORY.md is near its size cap — compress older entries while updating). **This closes the LAST known `mise<TAB>` blocker — confirm with the user after merge that `mise<TAB>` now works end-to-end in their real shell.**
