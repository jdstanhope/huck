# v116 — support `[^…]` bracket negation in glob patterns Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `[^…]` (negated bracket class) work as a synonym for `[!…]` in every glob-pattern context (`${var/}`/`#`/`%`, `case`, `[[ == ]]`, completion, pathname globbing), fixing the `mise<TAB>` `: : invalid option` (its `${1//[^$COMP_WORDBREAKS]/}` exclusion-set builder was inverted) and a broad correctness gap.

**Architecture:** The `glob` crate only honors `[!…]`; bash accepts both `[!…]` and `[^…]`. Add a bracket/escape-aware `translate_bracket_negation` helper that rewrites a class-leading `^`→`!`, and apply it before every non-extglob `glob::Pattern::new`/`glob_with` (5 sites). The extglob matcher (`glob_match.rs`) already handles `[^…]` and is untouched.

**Tech Stack:** Rust. `src/glob_match.rs`, `src/param_expansion.rs`, `src/executor.rs`, `src/completion_spec.rs`, `src/expand.rs`. Tests: `cargo test --bin huck`, `cargo test --test bracket_negation_integration`, `bash tests/scripts/bracket_negation_diff_check.sh`.

**Spec:** `docs/superpowers/specs/2026-06-08-bracket-negation-design.md`. Read it first.

**Commit trailer (MANDATORY, canonical — every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

Anchors (verify exact lines — code shifts):
- `src/glob_match.rs` top (add the helper near the top, after the module doc comment); its `parse_class` (~`:135-139`) already accepts `!`/`^` — DO NOT touch.
- `pe_pattern_matches` (`src/param_expansion.rs:~336`): the `else { match glob::Pattern::new(pattern) { … } }` branch.
- `case` (`src/executor.rs:~1077`): `glob::Pattern::new(&pattern)` in the non-extglob branch.
- `[[ == ]]` (`src/executor.rs:~1282`): `let pat = glob::Pattern::new(&pattern_str)…` in the non-extglob branch.
- completion `glob_match` (`src/completion_spec.rs:~373`): `glob::Pattern::new(pattern)`.
- pathname globbing (`src/expand.rs:~1437`): `match glob_with(&pattern, match_opts)` in the non-extglob `else`.

**Verified bash contract:** `${v//[^0-9]/}` (abc123)→`123`; `${v#[^0-9]}`→`bc123`; `case A in [^0-9])`→`letter`; `[[ A == [^0-9] ]]`→`Y`; `echo [^a]file` (afile bfile cfile)→`bfile cfile`; `${v//[^[:digit:]]/}` (ab12cd)→`12`. Regression: `[!0-9]` still negates; `[a^b]` keeps `^` literal; `[0-9]` non-negated.

---

## Task 1: `translate_bracket_negation` helper + unit tests

**Files:**
- Modify: `src/glob_match.rs` (the helper + a unit-test module)

- [ ] **Step 1: Write the failing unit tests**

In `src/glob_match.rs`, add (or extend) a `#[cfg(test)] mod` with:
```rust
#[cfg(test)]
mod bracket_negation_tests {
    use super::translate_bracket_negation;
    use std::borrow::Cow;

    fn t(p: &str) -> String { translate_bracket_negation(p).into_owned() }

    #[test]
    fn leading_caret_becomes_bang() {
        assert_eq!(t("[^abc]"), "[!abc]");
        assert_eq!(t("[^0-9]"), "[!0-9]");
    }
    #[test]
    fn bang_unchanged() { assert_eq!(t("[!abc]"), "[!abc]"); }
    #[test]
    fn plain_class_unchanged() { assert_eq!(t("[abc]"), "[abc]"); }
    #[test]
    fn caret_not_leading_is_literal() {
        assert_eq!(t("[a^b]"), "[a^b]");
        assert_eq!(t("a^b"), "a^b");
        assert_eq!(t("^foo"), "^foo");
    }
    #[test]
    fn literal_first_bracket_after_neg() {
        assert_eq!(t("[^]x]"), "[!]x]");
        assert_eq!(t("[]x]"), "[]x]");
    }
    #[test]
    fn escaped_open_bracket_not_a_class() {
        assert_eq!(t(r"\[^a]"), r"\[^a]");
    }
    #[test]
    fn caret_inside_existing_class_is_literal() {
        // `[a[^b]` is one class containing a,[,^,b — the inner ^ is NOT leading.
        assert_eq!(t("[a[^b]"), "[a[^b]");
    }
    #[test]
    fn multiple_classes_each_converted() {
        assert_eq!(t("x[^0-9]y[^a]z"), "x[!0-9]y[!a]z");
    }
    #[test]
    fn posix_class_inner_brackets() {
        assert_eq!(t("[[:alpha:]]"), "[[:alpha:]]");      // no leading ^
        assert_eq!(t("[^[:digit:]]"), "[![:digit:]]");    // leading ^ converted
    }
    #[test]
    fn no_change_returns_borrowed() {
        assert!(matches!(translate_bracket_negation("[abc]"), Cow::Borrowed(_)));
        assert!(matches!(translate_bracket_negation("plain"), Cow::Borrowed(_)));
    }
}
```

- [ ] **Step 2: Run the unit tests — confirm they fail to compile (helper undefined)**

Run: `cargo test --bin huck bracket_negation 2>&1 | tail -15`
Expected: `cannot find function translate_bracket_negation`.

- [ ] **Step 3: Implement `translate_bracket_negation`**

In `src/glob_match.rs`, add near the top (after the module doc / before `GroupKind`):
```rust
use std::borrow::Cow;

/// Rewrite a class-leading `^` to `!` so the `glob` crate (which only honors
/// `[!…]`) treats `[^…]` as negation, matching bash (which accepts both). Only
/// the FIRST char inside an unescaped class-opening `[` is the negation slot; a
/// `^` anywhere else stays literal. Honors `\[` escapes and the literal-first-`]`
/// rule (`[^]x]` → `[!]x]`, `[]x]` unchanged). Returns the input borrowed when
/// there is nothing to change (zero-copy). (M-113)
pub(crate) fn translate_bracket_negation(pattern: &str) -> Cow<'_, str> {
    if !pattern.contains('[') {
        return Cow::Borrowed(pattern);
    }
    let chars: Vec<char> = pattern.chars().collect();
    let mut out: Option<String> = None; // built lazily on first change
    let mut in_class = false;
    let mut escaped = false;
    let mut pos_in_class = 0usize; // chars seen since '[' (1 = first content char)
    let mut negated = false;       // class opened with `!` or `^`
    for i in 0..chars.len() {
        let c = chars[i];
        let mut emit = c;
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if !in_class {
            if c == '[' {
                in_class = true;
                pos_in_class = 0;
                negated = false;
            }
            // '^' / ']' outside a class are literal — nothing to do.
        } else {
            pos_in_class += 1;
            if pos_in_class == 1 {
                // The negation slot (first char after `[`).
                if c == '^' {
                    emit = '!';
                    negated = true;
                    if out.is_none() {
                        out = Some(chars[..i].iter().collect());
                    }
                } else if c == '!' {
                    negated = true;
                }
                // A `]` here (`[]…`) is a LITERAL `]`; class stays open. Any
                // other char is ordinary class content.
            } else if pos_in_class == 2 && negated && c == ']' {
                // Literal `]` immediately after `[!` / `[^` — class stays open.
            } else if c == ']' {
                in_class = false;
            }
        }
        if let Some(o) = out.as_mut() {
            o.push(emit);
        }
    }
    match out {
        Some(s) => Cow::Owned(s),
        None => Cow::Borrowed(pattern),
    }
}
```

- [ ] **Step 4: Run the unit tests — confirm they pass**

Run: `cargo test --bin huck bracket_negation 2>&1 | tail -15`
Expected: all `bracket_negation_tests` pass.

- [ ] **Step 5: Commit**

```bash
git add src/glob_match.rs
git commit -m "$(cat <<'EOF'
feat: translate_bracket_negation helper ([^...] -> [!...]) (M-113)

Bracket- and escape-aware rewrite of a class-leading `^` to `!`, so the glob
crate (which only honors [!...]) treats [^...] as negation like bash. Only the
first char inside an unescaped class-opening `[` is the negation slot; honors
\[ escapes and the literal-first-] rule. Zero-copy (Cow::Borrowed) when no
change. Not wired in yet.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1 report
DONE/BLOCKED, commit SHA, the helper, the unit-test pass line.

---

## Task 2: apply the helper at the 5 glob-crate sites + integration tests

**Files:**
- Modify: `src/param_expansion.rs`, `src/executor.rs`, `src/completion_spec.rs`, `src/expand.rs`
- Create: `tests/bracket_negation_integration.rs`

- [ ] **Step 1: Write the failing integration tests**

Create `tests/bracket_negation_integration.rs` (the `run` helper returns `(stdout, stderr, exit_code)`):
```rust
//! v116: [^...] bracket negation in glob patterns (M-113).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
fn run(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn subst_negated_class_removes_complement() {
    assert_eq!(run("v=abc123\necho \"${v//[^0-9]/}\"\n").0, "123\n");
}
#[test]
fn remove_prefix_negated_class() {
    assert_eq!(run("v=abc123\necho \"${v#[^0-9]}\"\n").0, "bc123\n");
}
#[test]
fn subst_negated_posix_class() {
    assert_eq!(run("v=ab12cd\necho \"${v//[^[:digit:]]/}\"\n").0, "12\n");
}
#[test]
fn case_negated_class() {
    assert_eq!(run("case A in [^0-9]) echo letter;; *) echo other;; esac\n").0, "letter\n");
}
#[test]
fn dbracket_negated_class() {
    assert_eq!(run("[[ A == [^0-9] ]] && echo Y || echo N\n").0, "Y\n");
}
#[test]
fn bang_negation_still_works() {
    assert_eq!(run("v=abc123\necho \"${v//[!0-9]/}\"\n").0, "123\n");
}
#[test]
fn caret_not_leading_is_literal() {
    // `[a^b]` removes a, ^, b (^ literal in the class) -> "c"
    assert_eq!(run("v=a^bc\necho \"${v//[a^b]/}\"\n").0, "c\n");
}
#[test]
fn pathname_negated_class() {
    // Create files in a temp dir and glob with [^a].
    let (out, _e, _c) = run(
        "d=$(mktemp -d)\ntouch \"$d/afile\" \"$d/bfile\" \"$d/cfile\"\n\
         cd \"$d\"\nfor f in [^a]file; do echo \"$f\"; done\nrm -rf \"$d\"\n");
    assert_eq!(out, "bfile\ncfile\n", "out: {out}");
}
```
Verify each against the system bash first.

- [ ] **Step 2: Run the integration tests — confirm they fail**

Run: `cargo build --bin huck && cargo test --test bracket_negation_integration 2>&1 | tail -20`
Expected: the `[^…]` tests FAIL (inverted/no-match); `bang_negation_still_works`, `caret_not_leading_is_literal` PASS.

- [ ] **Step 3: Apply in `pe_pattern_matches` (param_expansion.rs)**

In `src/param_expansion.rs` (`~:340`), change the `else` branch:
```rust
    } else {
        match glob::Pattern::new(pattern) {
```
to:
```rust
    } else {
        let pattern = crate::glob_match::translate_bracket_negation(pattern);
        match glob::Pattern::new(&pattern) {
```
(The rest of the `match` — `Ok(p) => p.matches_with(text, …)` — is unchanged; `&pattern` is now `&Cow<str>` which derefs to `&str`.)

- [ ] **Step 4: Apply in `case` (executor.rs ~:1077)**

In `src/executor.rs` (the `case` non-extglob branch ~`:1077`), change `glob::Pattern::new(&pattern)` to use the translated pattern:
```rust
            let npat = crate::glob_match::translate_bracket_negation(&pattern);
            glob::Pattern::new(&npat)
```
(Apply to the existing `glob::Pattern::new(&pattern)` call in that branch — introduce `npat` immediately before it and pass `&npat`.)

- [ ] **Step 5: Apply in `[[ == ]]` (executor.rs ~:1282)**

In `src/executor.rs` (the `[[ ==/!= ]]` non-extglob branch ~`:1282`), change:
```rust
                let pat = glob::Pattern::new(&pattern_str)
                    .map_err(|e| format!("bad pattern: {e}"))?;
```
to:
```rust
                let npat = crate::glob_match::translate_bracket_negation(&pattern_str);
                let pat = glob::Pattern::new(&npat)
                    .map_err(|e| format!("bad pattern: {e}"))?;
```

- [ ] **Step 6: Apply in completion (completion_spec.rs ~:373)**

In `src/completion_spec.rs` (`glob_match` fn ~`:373`), change:
```rust
fn glob_match(pattern: &str, candidate: &str) -> bool {
    match glob::Pattern::new(pattern) {
```
to:
```rust
fn glob_match(pattern: &str, candidate: &str) -> bool {
    let pattern = crate::glob_match::translate_bracket_negation(pattern);
    match glob::Pattern::new(&pattern) {
```

- [ ] **Step 7: Apply in pathname globbing (expand.rs ~:1437)**

In `src/expand.rs` (the non-extglob pathname branch ~`:1437`), change:
```rust
            match glob_with(&pattern, match_opts) {
```
to:
```rust
            let npat = crate::glob_match::translate_bracket_negation(&pattern);
            match glob_with(&npat, match_opts) {
```
(Leave the `literal_leading_dot` computation above as-is — it inspects the original `pattern`'s leading char, which is unaffected by a class-internal `^`→`!`.)

- [ ] **Step 8: Run the integration tests — confirm green**

Run: `cargo build --bin huck && cargo test --test bracket_negation_integration 2>&1 | tail -12`
Expected: all 8 tests PASS.

- [ ] **Step 9: Verify byte-identical to bash**

```bash
cargo build --bin huck
for f in 'v=abc123; echo "${v//[^0-9]/}"' \
         'v=abc123; echo "${v#[^0-9]}"' \
         'v=ab12cd; echo "${v//[^[:digit:]]/}"' \
         'case A in [^0-9]) echo letter;; *) echo other;; esac' \
         '[[ A == [^0-9] ]] && echo Y || echo N' \
         'v=abc123; echo "${v//[!0-9]/}"' \
         'v=a^bc; echo "${v//[a^b]/}"' \
         'd=$(mktemp -d); touch "$d"/{afile,bfile,cfile}; cd "$d"; echo [^a]file; rm -rf "$d"'; do
  b=$(printf '%s\n' "$f" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
  h=$(printf '%s\n' "$f" | ./target/debug/huck 2>&1; echo "EXIT:$?")
  [ "$b" = "$h" ] && echo "MATCH: $f" || { echo "DIFF: $f"; echo " b=[$b]"; echo " h=[$h]"; }
done
```
Expected: eight `MATCH` lines.

- [ ] **Step 10: Full regression + clippy**

Run: `cargo test 2>&1 | grep -E "test result: FAILED" ; cargo test 2>&1 | grep -cE "test result: ok"` (no FAILED). Then `cargo clippy --bin huck 2>&1 | tail -3` (clean). Watch the `param`/`case`/`dbracket`/`glob`/`extglob`/`arrays`/completion suites — a regression there means the translation altered a working pattern; investigate vs bash before proceeding.

- [ ] **Step 11: Commit**

```bash
git add src/param_expansion.rs src/executor.rs src/completion_spec.rs src/expand.rs tests/bracket_negation_integration.rs
git commit -m "$(cat <<'EOF'
fix: [^...] bracket negation in ${}, case, [[ ]], completion, globbing (M-113)

Apply translate_bracket_negation before each non-extglob glob::Pattern::new /
glob_with so [^...] negates like bash (it was treated as a literal ^). Fixes
${v//[^0-9]/}, case/[[ == [^...] ]], pathname [^a]*, and bash_completion's
${1//[^$COMP_WORDBREAKS]/} exclusion-set builder (the mise<TAB> cascade root).
[!...] and literal-^ (`[a^b]`) unchanged; extglob matcher already handled [^].

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2 report
DONE/BLOCKED, commit SHA, the 5 site edits, the 8 integration-test pass line, the eight bash MATCH lines, the full-suite green count (no FAILED), clippy status, any regression.

---

## Task 3: 40th bash-diff harness + payoff smoke

**Files:**
- Create: `tests/scripts/bracket_negation_diff_check.sh`

- [ ] **Step 1: Write the harness**

Create `tests/scripts/bracket_negation_diff_check.sh`, modeled on `tests/scripts/bare_local_unset_diff_check.sh`:
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v116: [^...] bracket negation in glob
# patterns (M-113) — ${}/case/[[ ]]/pathname. [!...] + literal-^ regressions.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "subst negated"        'v=abc123; echo "${v//[^0-9]/}"'
check "remove-prefix negated" 'v=abc123; echo "${v#[^0-9]}"'
check "remove-suffix negated" 'v=x9y; echo "${v%[^0-9]}"'
check "subst posix negated"  'v=ab12cd; echo "${v//[^[:digit:]]/}"'
check "case negated"         'case A in [^0-9]) echo letter;; *) echo other;; esac'
check "case negated digit"   'case 5 in [^0-9]) echo letter;; *) echo other;; esac'
check "dbracket negated"     '[[ A == [^0-9] ]] && echo Y || echo N'
check "dbracket negated neg" '[[ 5 == [^0-9] ]] && echo Y || echo N'
check "bang still negates"   'v=abc123; echo "${v//[!0-9]/}"'
check "caret literal"        'v=a^bc; echo "${v//[a^b]/}"'
check "non-negated class"    'v=abc123; echo "${v//[0-9]/}"'
check "pathname negated"     'd=$(mktemp -d); touch "$d"/afile "$d"/bfile "$d"/cfile; cd "$d"; echo [^a]file; rm -rf "$d"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make it executable + run it + all harnesses**

Run:
```bash
chmod +x tests/scripts/bracket_negation_diff_check.sh && cargo build --bin huck
bash tests/scripts/bracket_negation_diff_check.sh
export HUCK_BIN="$(pwd)/target/debug/huck"
echo "count: $(ls tests/scripts/*_diff_check.sh | wc -l)"
for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done
echo done
```
Expected: `Total: 12, Pass: 12, Fail: 0`; `count: 40`; no `FAIL` lines.

- [ ] **Step 3: Payoff smoke (the bash_completion `_init_completion` shape)**

Run:
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
} > /tmp/v116_smoke.sh
echo "--- bash ---"; bash --norc --noprofile /tmp/v116_smoke.sh 2>&1
echo "--- huck (file arg) ---"; ./target/debug/huck /tmp/v116_smoke.sh 2>&1
```
Expected: huck prints `SMOKE cur=[] prev=[mise] cword=1 nwords=2 w0=[mise]` (matching bash) with NO `: : invalid option`. (Report exactly; if `_variables`/`__ltrim_colon_completions` are command-not-found that's a separate already-known gap, but the `: : invalid option` must be gone and cword/nwords/prev correct.)

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/bracket_negation_diff_check.sh
git commit -m "$(cat <<'EOF'
test: 40th bash-diff harness for [^...] bracket negation (M-113)

12 byte-identical fragments across ${}/#/%, case, [[ ]], pathname, plus [!...]
and literal-^ regression guards. Payoff: the _init_completion -n : shape now
yields cword=1 nwords=2 prev=mise with no `: : invalid option`.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 3 report
DONE/BLOCKED, commit SHA, the `Total: 12, Pass: 12` line, the `count: 40` + no-FAIL line, the payoff-smoke output (the `SMOKE …` line; confirm no `: : invalid option`).

---

## Task 4: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Read the structures to update**

```bash
grep -n 'Last updated:\|Bugs (Tier 1) |\|^## Change log\|2026-06-08.*v115\|### M-112:' docs/bash-divergences.md | head
grep -n '| v115 ' README.md
```
Confirm the next free Tier-1 number is **M-113** (correctness bug → Tier 1).

- [ ] **Step 2: Add the M-113 entry (Tier 1)**

In `docs/bash-divergences.md` Tier-1 (Bugs) section (e.g. after `### M-112:`), add a `### M-113:` entry `[fixed v116]` (high): `[^…]` (negated bracket class) was treated as a literal `^` in every `glob`-crate context (`${var/}`/`#`/`%`, `case`, `[[ == ]]`, completion, pathname globbing) because the `glob` crate only honors `[!…]`; bash accepts both. Fix: a `translate_bracket_negation` helper (`src/glob_match.rs`) rewrites a class-leading `^`→`!` (bracket/escape-aware, literal-first-`]` honored), applied before each non-extglob `glob::Pattern::new`/`glob_with` (5 sites); the extglob matcher already accepted `[^…]`. Driver: bash_completion's `${1//[^$COMP_WORDBREAKS]/}` exclusion-set builder was inverted → wrong `words`/`cword` → malformed `_upvars` → the `mise<TAB>` `: : invalid option`; fixing `[^…]` clears it (and likely makes completion functional — closes/subsumes M-112). Note the degenerate `[^]` (nothing after) low edge if it differs. Bump the Tier-1 count.

- [ ] **Step 3: Update M-112 (subsumed) + Tier counts**

The `### M-112:` entry (empty-`words` array) was a SYMPTOM of M-113 (the inverted exclude). Update its status note to reflect that v116/M-113 fixed the underlying cause — verify with the payoff smoke (cword/nwords/prev now correct) and mark M-112 `[fixed v116 (via M-113)]` if the smoke confirms; otherwise leave M-112 deferred with a note that M-113 addressed the exclude-set root and any residual is separate. Bump **Bugs (Tier 1)** count by 1 (M-113) and append `; M-113 [^...] bracket negation in glob patterns fixed v116 (root cause of M-112)` to its note. Update the **Last updated** line to v116 (M-113).

- [ ] **Step 4: Change-log entry + README row**

`docs/bash-divergences.md` change log (after the v115 entry): a `2026-06-08` v116 entry — the `[^…]`→`[!…]` translation mechanism + the 5 sites, the `mise<TAB>` payoff (the exclude-set inversion fixed → `_init_completion` yields correct cword/words/prev → cascade cleared), the broad scope (`${}`/case/`[[`/completion/pathname all confirmed), the M-112 relationship, the 40th harness + test count from Task 3's full-suite run. Add a v116 README row after v115. Use the REAL test count: `cargo test 2>&1 | awk '/test result:/{s+=$4} END{print s}'`.

- [ ] **Step 5: Verify (no placeholders) + commit**

```bash
grep -n 'M-113\|fixed v116\|v116' docs/bash-divergences.md README.md | head
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: v116 — [^...] bracket negation in glob patterns (M-113)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 4 report
DONE/BLOCKED, commit SHA, the grep output proving real M-number/version, whether M-112 was marked fixed-via-M-113 (per the smoke), the test count used.

---

## Final (after all tasks)
- [ ] Whole-branch review: `git log --oneline main..HEAD`, `git diff --stat main..HEAD`.
- [ ] `cargo test 2>&1 | grep -cE 'test result: ok'` (green, no FAILED), `cargo clippy --all-targets 2>&1 | tail -2` (clean).
- [ ] All harnesses: `export HUCK_BIN="$(pwd)/target/debug/huck"; for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done` (silent = pass; 40 files).
- [ ] **Payoff**: the `_init_completion -n :` shape yields `cword=1 nwords=2 prev=mise` with no `: : invalid option` (Task 3 Step 3).
- [ ] AskUserQuestion merge gate, then `git merge --no-ff` + push + delete branch, then update memory files (`project_huck_iterations.md` + `MEMORY.md`). **This may be the iteration that makes `mise<TAB>` actually complete — confirm with the user after merge.**
