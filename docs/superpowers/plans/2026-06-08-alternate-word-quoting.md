# v114 — preserve alternate/default word quoting under an unquoted outer expansion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When the outer `${param+word}` / `${param-word}` (`:+`/`:-`, scalar or array) is **unquoted**, expand the substituted *word* preserving its own field structure — so `${a[@]+"${a[@]}"}` keeps empty/spaced elements and `${x+"a b"}` stays one field — fixing the final `mise<TAB>` `_upvars` `invalid option` cascade.

**Architecture:** Gate on the outer `quoted` flag (quoted-outer is already correct → untouched). Add `ExpansionResult::Fields(Vec<Field>)` = pre-split, quoting-final fields = `expand(word)`. The `UseAlternate`/`UseDefault` substituted-word arms (scalar in `param_expansion.rs`, array/assoc in `expand.rs`) return `Fields(...)` when `!quoted`, else the current `Value`/`WordList`. The `expand()` consumer emits `Fields` verbatim (no re-split); `expand_assignment()` joins them. `quoted` is threaded into the scalar modifier path via a new `expand_modifier_quoted` wrapper.

**Tech Stack:** Rust. `src/param_expansion.rs`, `src/expand.rs`. Tests: `cargo test --bin huck`, `cargo test --test alternate_word_quoting_integration`, `bash tests/scripts/alternate_word_quoting_diff_check.sh`.

**Spec:** `docs/superpowers/specs/2026-06-08-alternate-word-quoting-design.md`. Read it first.

**Commit trailer (MANDATORY, canonical — every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

Anchors (verify exact lines — code shifts):
- `pub enum ExpansionResult` (`src/param_expansion.rs:7`).
- `pub fn expand_modifier(name, modifier, shell)` (`:23`) → `expand_modifier_with_value(name, modifier, ParamLookup::Scalar, shell)` (`:28`).
- `pub fn expand_modifier_with_value(name, modifier, source, shell)` (`:51`); scalar arms `UseDefault` (`:101`), `UseAlternate` (`:148`).
- `crate::expand::Field` = `pub struct Field { pub chars: String, pub quoted: Vec<bool> }` (`src/expand.rs:71-75`, derives `Debug, Clone, PartialEq, Eq`).
- Array arms: `expand_array_param` UseAlternate (`src/expand.rs:685`) / UseDefault (`:701`); `expand_assoc_param` UseAlternate (`:392`) / UseDefault (`:407`). Both fns take `quoted: bool`.
- Production scalar `expand_modifier` callers needing real `quoted`: `expand.rs:886` (expand), `:1018` (expand_assignment), `:512` (expand_indirect — has `quoted` param).
- `expand_modifier_with_value` callers needing a `quoted` arg: `expand.rs:382` (assoc element, has `quoted`), `:672` (array element, has `quoted`), `param_expansion.rs:28` (the `expand_modifier` wrapper → `false`).
- `expand()` WordList consumer (`src/expand.rs:908`); `expand_assignment()` WordList consumer (`:1023`).

**Verified bash contract** (`set -- …; echo $#`): `a=(x "" y); ${a[@]+"${a[@]}"}`→3; `a=("a b" c); ${a[@]+"${a[@]}"}`→2; `x=1; ${x+"a b"}`→1; `words=(mise ""); ${words+"${words[@]}"}`→2. Regressions that must stay: `a=(x "" y); "${a[@]+"${a[@]}"}"`→3; `x=1; "${x+a b}"`→1.

---

## Task 1: `ExpansionResult::Fields` + thread `quoted` + producers + consumers + unit-test updates

**Files:** `src/param_expansion.rs`, `src/expand.rs` (one atomic change — must land together to compile + pass).

- [ ] **Step 1: Add the `Fields` variant**

In `src/param_expansion.rs`, add to `pub enum ExpansionResult` (after `WordList`):
```rust
    /// Pre-split, quoting-final fields from expanding a substituted *word*
    /// (the alternate of `${p+word}` / the default of `${p-word}`) when the
    /// OUTER `${…}` is unquoted. The consumer emits these as-is — no further
    /// IFS-splitting or re-joining — so quoted-empty fields survive and
    /// quoted-spaced fields are not re-split. (M-110)
    Fields(Vec<crate::expand::Field>),
```

- [ ] **Step 2: Thread `quoted` into `expand_modifier_with_value` + add `expand_modifier_quoted`**

In `src/param_expansion.rs`, change the signature of `expand_modifier_with_value` to take `quoted: bool` (add it before `shell`):
```rust
pub fn expand_modifier_with_value(
    name: &str,
    modifier: &ParamModifier,
    source: ParamLookup,
    quoted: bool,
    shell: &mut Shell,
) -> ExpansionResult {
```
Keep the 3-arg `expand_modifier` wrapper delegating with `false` (so the ~30 existing unit-test call sites of `expand_modifier` compile unchanged), and add a 4-arg `expand_modifier_quoted` for production scalar callers:
```rust
pub fn expand_modifier(
    name: &str,
    modifier: &ParamModifier,
    shell: &mut Shell,
) -> ExpansionResult {
    expand_modifier_with_value(name, modifier, ParamLookup::Scalar, false, shell)
}

/// Like `expand_modifier`, but the caller supplies the OUTER `${…}` quoting so
/// `${p+word}` / `${p-word}` can field-preserve the substituted word when
/// unquoted (M-110).
pub fn expand_modifier_quoted(
    name: &str,
    modifier: &ParamModifier,
    quoted: bool,
    shell: &mut Shell,
) -> ExpansionResult {
    expand_modifier_with_value(name, modifier, ParamLookup::Scalar, quoted, shell)
}
```

- [ ] **Step 3: Scalar `UseDefault` / `UseAlternate` arms emit `Fields` when `!quoted`**

In `src/param_expansion.rs`, replace the `UseDefault` arm (`:101`):
```rust
        ParamModifier::UseDefault { word, colon } => {
            let raw = get_raw(shell);
            if condition_is_null(raw.as_deref(), *colon) {
                if quoted {
                    ExpansionResult::Value(expand_word_to_string(word, shell))
                } else {
                    ExpansionResult::Fields(crate::expand::expand(word, shell))
                }
            } else {
                ExpansionResult::Value(raw.unwrap_or_default())
            }
        }
```
and the `UseAlternate` arm (`:148`):
```rust
        ParamModifier::UseAlternate { word, colon } => {
            let raw = get_raw(shell);
            if condition_is_null(raw.as_deref(), *colon) {
                ExpansionResult::Empty
            } else if quoted {
                ExpansionResult::Value(expand_word_to_string(word, shell))
            } else {
                ExpansionResult::Fields(crate::expand::expand(word, shell))
            }
        }
```
(Leave `AssignDefault`, `ErrorIfUnset`, and all pattern/case/substring arms unchanged — they keep `expand_word_to_string`.)

- [ ] **Step 4: Update the `expand_modifier_with_value` callers in `expand.rs` to pass `quoted`**

`src/expand.rs:382` (assoc element arm) and `:672` (array element arm) call `expand_modifier_with_value(name, modif, ParamLookup::Element(...), shell)`. Add the `quoted` arg (both functions have a `quoted: bool` param in scope):
```rust
            crate::param_expansion::expand_modifier_with_value(
                name,
                modif,
                crate::param_expansion::ParamLookup::Element(val.as_deref()),
                quoted,
                shell,
            )
```
(Apply to both sites — same shape.)

- [ ] **Step 5: Route the production scalar `expand_modifier` callers through `expand_modifier_quoted`**

In `src/expand.rs`, change the three production callers to pass the real `quoted`:
- `:886` (the `expand()` scalar PE arm): `crate::param_expansion::expand_modifier(name, modifier, shell)` → `crate::param_expansion::expand_modifier_quoted(name, modifier, *quoted, shell)`.
- `:1018` (the `expand_assignment()` scalar PE arm): same → `expand_modifier_quoted(name, modifier, *quoted, shell)`.
- `:512` (the `expand_indirect` final reference): `crate::param_expansion::expand_modifier(n, modifier, shell)` → `expand_modifier_quoted(n, modifier, quoted, shell)` (`expand_indirect` has a `quoted: bool` param).

- [ ] **Step 6: Array + assoc `UseAlternate`/`UseDefault` arms gate on `quoted`**

In `src/expand.rs` `expand_array_param`, replace the `UseAlternate` arm (`:685`):
```rust
        (PM::UseAlternate { word, colon: _ }, SK::All | SK::Star) => {
            if collect_values(shell).is_empty() {
                ExpansionResult::Empty
            } else if quoted {
                // Quoted outer: keep the existing field-preserving WordList /
                // [*]-join path (already correct).
                let words: Vec<String> =
                    expand(word, shell).into_iter().map(|f| f.chars).collect();
                if matches!(subscript, SK::Star) {
                    let ifs = shell.ifs();
                    let sep = ifs_join_sep(&ifs);
                    ExpansionResult::Value(words.join(&sep))
                } else {
                    ExpansionResult::WordList(words)
                }
            } else {
                // Unquoted outer: emit the alternate's own fields verbatim
                // (preserves empties / quoted-spaced fields).
                ExpansionResult::Fields(expand(word, shell))
            }
        }
```
and the `UseDefault` arm (`:701`):
```rust
        (PM::UseDefault { word, colon: _ }, SK::All | SK::Star) => {
            let values = collect_values(shell);
            if !values.is_empty() {
                // Set: behave exactly like ${arr[@]} / ${arr[*]} (unchanged).
                if matches!(subscript, SK::Star) {
                    let ifs = shell.ifs();
                    let sep = ifs_join_sep(&ifs);
                    ExpansionResult::Value(values.join(&sep))
                } else {
                    ExpansionResult::WordList(values)
                }
            } else if quoted {
                // Unset, quoted outer: existing field-preserving path.
                let words: Vec<String> =
                    expand(word, shell).into_iter().map(|f| f.chars).collect();
                if matches!(subscript, SK::Star) {
                    let ifs = shell.ifs();
                    let sep = ifs_join_sep(&ifs);
                    ExpansionResult::Value(words.join(&sep))
                } else {
                    ExpansionResult::WordList(words)
                }
            } else {
                // Unset, unquoted outer: emit the default word's own fields.
                ExpansionResult::Fields(expand(word, shell))
            }
        }
```
Apply the **identical** transformation to `expand_assoc_param`'s `UseAlternate` (`:392`) and `UseDefault` (`:407`) arms (using its local `values` for the set-predicate, exactly mirroring the indexed arms above).

- [ ] **Step 7: Add the `Fields` consumer arms**

In `src/expand.rs` `expand()`, add a `Fields` arm to the `WordPart::ParamExpansion` match (next to the `WordList` arm `:908`):
```rust
                    crate::param_expansion::ExpansionResult::Fields(fields) => {
                        // Already-final fields: concatenate the first onto the
                        // in-progress field, push the rest as new fields. No
                        // IFS-split / re-join — preserves quoted-empty and
                        // quoted-spaced fields verbatim. (M-110)
                        for (i, f) in fields.into_iter().enumerate() {
                            if i > 0 {
                                result.push(std::mem::take(&mut current));
                            }
                            current.chars.push_str(&f.chars);
                            current.quoted.extend(f.quoted);
                            has_emitted = true;
                        }
                    }
```
And in `expand_assignment()` (`:1023` WordList arm), add a `Fields` arm (assignment never splits → join):
```rust
                    crate::param_expansion::ExpansionResult::Fields(fields) => {
                        let ifs = shell.ifs();
                        let sep = ifs_join_sep(&ifs);
                        let joined = fields
                            .iter()
                            .map(|f| f.chars.as_str())
                            .collect::<Vec<_>>()
                            .join(&sep);
                        result.push_str(&joined);
                    }
```

- [ ] **Step 8: Build — fix the unit tests that now observe `Fields`**

Run: `cargo build --bin huck 2>&1 | tail -5`
Expected: compiles (all `ExpansionResult` matches are exhaustive — if a `match` elsewhere on `ExpansionResult` is now non-exhaustive, add a `Fields` arm there; grep `ExpansionResult::WordList` to find them).

Then add a test helper at the top of the `#[cfg(test)] mod tests` in `src/param_expansion.rs`:
```rust
    /// Expected `Fields` result for a single unquoted literal word `s`
    /// (what `${p+s}` / `${p-s}` now returns under an unquoted outer).
    fn fields(s: &str) -> ExpansionResult {
        ExpansionResult::Fields(vec![crate::expand::Field {
            chars: s.to_string(),
            quoted: vec![false; s.chars().count()],
        }])
    }
```
Run: `cargo test --bin huck param 2>&1 | tail -25` — the UseAlternate/UseDefault tests whose **substituted-word branch** ran (they use the 3-arg `expand_modifier`, i.e. `quoted=false`) now return `Fields(...)`. Update each such `assert_eq!(r, ExpansionResult::Value("X".to_string()))` to `assert_eq!(r, fields("X"))`. Leave UNCHANGED: the `Empty` assertions (unset alternate), the UseDefault **set→raw-value** assertions, and all `AssignDefault`/`ErrorIfUnset` assertions. Re-run until `param` tests pass.

- [ ] **Step 9: Add targeted new unit tests for the gate**

Add to the same test module:
```rust
    #[test]
    fn use_alternate_unquoted_returns_fields() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_M110_A", "set".to_string());
        let m = ParamModifier::UseAlternate { word: lit("alt"), colon: false };
        // quoted=false (the 3-arg wrapper) → Fields.
        assert_eq!(expand_modifier("HUCK_M110_A", &m, &mut shell), fields("alt"));
    }

    #[test]
    fn use_alternate_quoted_returns_value() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_M110_B", "set".to_string());
        let m = ParamModifier::UseAlternate { word: lit("alt"), colon: false };
        // quoted=true → the old Value path (no split).
        assert_eq!(
            expand_modifier_quoted("HUCK_M110_B", &m, true, &mut shell),
            ExpansionResult::Value("alt".to_string())
        );
    }
```

- [ ] **Step 10: Verify byte-identical to bash**

```bash
cargo build --bin huck
for f in 'a=(x "" y); set -- ${a[@]+"${a[@]}"}; echo $#; printf "<%s>" "$@"; echo' \
         'a=("a b" c); set -- ${a[@]+"${a[@]}"}; echo $#' \
         'x=1; set -- ${x+"a b"}; echo $#' \
         'words=(mise ""); set -- ${words+"${words[@]}"}; echo $#' \
         'a=(x "" y); set -- "${a[@]+"${a[@]}"}"; echo $#' \
         'x=1; set -- "${x+a b}"; echo $#' \
         'unset u; set -- ${u-"a b"}; echo $#' \
         'declare -A m=([k]="a b"); set -- ${m[@]+"${m[@]}"}; echo $#' \
         'words=(mise ""); set -- -a${#words[@]} words ${words+"${words[@]}"} -v cword 1; echo $#'; do
  b=$(printf '%s\n' "$f" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
  h=$(printf '%s\n' "$f" | ./target/debug/huck 2>&1; echo "EXIT:$?")
  [ "$b" = "$h" ] && echo "MATCH: $f" || { echo "DIFF: $f"; echo " b=[$b]"; echo " h=[$h]"; }
done
```
Expected: nine `MATCH` lines (the last is the `_upvars`/mise shape → both `argc=7`).

- [ ] **Step 11: Full regression + clippy**

Run: `cargo test 2>&1 | grep -E "test result: FAILED" ; cargo test 2>&1 | grep -cE "test result: ok"` (no FAILED). Then `cargo clippy --bin huck 2>&1 | tail -3` (clean). This change is in the central expansion path — if ANY non-`param` test regresses (arrays/associative/bashrc_zero_errors/mise_zero_errors), investigate that case vs bash before proceeding; do NOT mask a real regression.

- [ ] **Step 12: Commit**

```bash
git add src/param_expansion.rs src/expand.rs
git commit -m "$(cat <<'EOF'
fix: preserve alternate/default word quoting under unquoted ${p+word} (M-110)

Under an UNQUOTED outer ${param+word}/${param-word}, the substituted word's own
quoting was lost — empties dropped, quoted-spaced fields re-split — because the
word was returned as a flat Value/WordList and the consumer re-split it. New
ExpansionResult::Fields(Vec<Field>) carries pre-split, quoting-final fields
(= expand(word)); the UseAlternate/UseDefault substituted-word arms emit it when
!quoted (the quoted-outer path, already correct, is gated unchanged). `quoted` is
threaded into the scalar modifier path via expand_modifier_quoted. Fixes
bash_completion's __get_cword_at_cursor_by_ref `${words+"${words[@]}"}` arg-count
desync (mise<TAB>). Closes the converse-M-105 deferred in v110.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1 report
DONE/BLOCKED, commit SHA, the variant + threading + the 6 producer arms + 2 consumer arms, which unit tests were updated to `fields(...)`, the 9 bash MATCH lines, the full-suite green count (no FAILED), clippy status.

---

## Task 2: integration tests + 38th harness + payoff smoke

**Files:**
- Create: `tests/alternate_word_quoting_integration.rs`
- Create: `tests/scripts/alternate_word_quoting_diff_check.sh`

- [ ] **Step 1: Write the integration tests**

Create `tests/alternate_word_quoting_integration.rs` (the `run` helper returns `(stdout, stderr, exit_code)`):
```rust
//! v114: alternate/default word quoting under unquoted outer expansion (M-110).
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
fn array_alt_empty_element_preserved() {
    let (out, _e, _c) = run("a=(x \"\" y)\nset -- ${a[@]+\"${a[@]}\"}\necho $#\nprintf '<%s>' \"$@\"\necho\n");
    assert_eq!(out, "3\n<x><><y>\n", "out: {out}");
}

#[test]
fn array_alt_spaced_element_not_resplit() {
    let (out, _e, _c) = run("a=(\"a b\" c)\nset -- ${a[@]+\"${a[@]}\"}\necho $#\n");
    assert_eq!(out, "2\n", "out: {out}");
}

#[test]
fn scalar_alt_quoted_inner_one_field() {
    let (out, _e, _c) = run("x=1\nset -- ${x+\"a b\"}\necho $#\n");
    assert_eq!(out, "1\n", "out: {out}");
}

#[test]
fn fully_quoted_outer_unchanged_array() {
    let (out, _e, _c) = run("a=(x \"\" y)\nset -- \"${a[@]+\"${a[@]}\"}\"\necho $#\n");
    assert_eq!(out, "3\n", "out: {out}");
}

#[test]
fn fully_quoted_outer_unchanged_scalar() {
    let (out, _e, _c) = run("x=1\nset -- \"${x+a b}\"\necho $#\n");
    assert_eq!(out, "1\n", "out: {out}");
}

#[test]
fn default_word_unset_unquoted_splits_inner_quoting() {
    let (out, _e, _c) = run("unset u\nset -- ${u-\"a b\"}\necho $#\n");
    assert_eq!(out, "1\n", "out: {out}");
}

#[test]
fn assoc_alt_spaced_value_preserved() {
    let (out, _e, _c) = run("declare -A m=([k]=\"a b\")\nset -- ${m[@]+\"${m[@]}\"}\necho $#\n");
    assert_eq!(out, "1\n", "out: {out}");
}

#[test]
fn upvars_mise_shape_arg_count() {
    // The exact bash_completion __get_cword_at_cursor_by_ref shape: the empty
    // trailing element must survive so the -a${#words[@]} count matches.
    let (out, _e, _c) = run("words=(mise \"\")\nset -- -a${#words[@]} words ${words+\"${words[@]}\"} -v cword 1\necho $#\n");
    assert_eq!(out, "7\n", "out: {out}");
}
```
Verify each expectation against the system bash first (some `$#` values are subtle — e.g. `${u-"a b"}` unset, unquoted outer → `"a b"` quoted-inner → 1 field).

- [ ] **Step 2: Run the integration tests**

Run: `cargo build --bin huck && cargo test --test alternate_word_quoting_integration 2>&1 | tail -12`
Expected: all 8 pass.

- [ ] **Step 3: Write the 38th bash-diff harness**

Create `tests/scripts/alternate_word_quoting_diff_check.sh`, modeled on `tests/scripts/printf_v_array_diff_check.sh`:
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v114: alternate/default word quoting
# under an unquoted outer ${param+word}/${param-word} (M-110). Quoted-outer is
# unchanged (regression guard).
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

check "array empty elem"      'a=(x "" y); set -- ${a[@]+"${a[@]}"}; echo $#; printf "<%s>" "$@"; echo'
check "array spaced elem"     'a=("a b" c); set -- ${a[@]+"${a[@]}"}; echo $#'
check "scalar quoted inner"   'x=1; set -- ${x+"a b"}; echo $#'
check "scalar unquoted inner" 'x=1; set -- ${x+a b}; echo $#'
check "fully-quoted array"    'a=(x "" y); set -- "${a[@]+"${a[@]}"}"; echo $#'
check "fully-quoted scalar"   'x=1; set -- "${x+a b}"; echo $#'
check "default unset quoted"  'unset u; set -- ${u-"a b"}; echo $#'
check "default unset unquoted" 'unset u; set -- ${u-a b}; echo $#'
check "assoc spaced value"    'declare -A m=([k]="a b"); set -- ${m[@]+"${m[@]}"}; echo $#'
check "upvars mise shape"     'words=(mise ""); set -- -a${#words[@]} words ${words+"${words[@]}"} -v cword 1; echo $#'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 4: Make it executable + run it + all harnesses**

Run:
```bash
chmod +x tests/scripts/alternate_word_quoting_diff_check.sh && cargo build --bin huck
bash tests/scripts/alternate_word_quoting_diff_check.sh
export HUCK_BIN="$(pwd)/target/debug/huck"
echo "count: $(ls tests/scripts/*_diff_check.sh | wc -l)"
for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done
echo done
```
Expected: `Total: 10, Pass: 10, Fail: 0`; `count: 38`; no `FAIL` lines.

- [ ] **Step 5: Payoff smoke**

Run:
```bash
cargo build --bin huck
printf '%s\n' 'words=(mise ""); set -- ${words+"${words[@]}"}; echo "ALT_OK n=$#"' | ./target/debug/huck 2>&1
printf '%s\n' 'words=(mise ""); set -- -a${#words[@]} words ${words+"${words[@]}"} -v cword 1; echo "UPVARS_OK n=$#"' | ./target/debug/huck 2>&1
```
Expected: `ALT_OK n=2` and `UPVARS_OK n=7` (the arg count now matches `-a${#words[@]}`).

- [ ] **Step 6: Commit**

```bash
git add tests/alternate_word_quoting_integration.rs tests/scripts/alternate_word_quoting_diff_check.sh
git commit -m "$(cat <<'EOF'
test: 38th bash-diff harness + integration for alternate-word quoting (M-110)

10 byte-identical fragments (array empty/spaced elem, scalar quoted/unquoted
inner, fully-quoted-outer regression guards, default unset, assoc, the
_upvars/mise shape) + 8 integration tests.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2 report
DONE/BLOCKED, commit SHA, the 8 integration-test pass line, the `Total: 10, Pass: 10` line, the `count: 38` + no-FAIL line, the payoff-smoke output (`ALT_OK n=2` / `UPVARS_OK n=7`).

---

## Task 3: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Read the structures to update**

```bash
grep -n 'Last updated:\|Bugs (Tier 1) |\|Missing features (Tier 2) |\|^## Change log\|2026-06-08.*v113\|converse-M-105' docs/bash-divergences.md | head
grep -n '| v113 ' README.md
```
Confirm the next free Tier-1 number is **M-110** (this is a correctness bug — Tier 1).

- [ ] **Step 2: Add the M-110 entry (Tier 1)**

In `docs/bash-divergences.md` Tier-1 (Bugs) section, add an **M-110** entry `[fixed v114]` (high): under an UNQUOTED outer `${param+word}` / `${param-word}` (`:+`/`:-`, scalar or array), the substituted word's own quoting was lost — empty fields dropped, quoted-spaced fields re-split — because the word was flattened to `Value`/`WordList` and the consumer re-split it. Fix: new `ExpansionResult::Fields(Vec<Field>)` = `expand(word)`, emitted verbatim; the `UseAlternate`/`UseDefault` substituted-word arms (scalar `param_expansion.rs`, array/assoc `expand.rs`) return it when `!quoted`; the quoted-outer path is gated unchanged (already correct). `quoted` threaded into the scalar path via `expand_modifier_quoted`. Driver: bash_completion's `__get_cword_at_cursor_by_ref` `${words+"${words[@]}"}` with `COMP_WORDS=(mise "")` (reached by `mise<TAB>`) dropped the trailing empty element → the `-a${#words[@]}` count desynced → `_upvars: : invalid option`. Closes the converse-M-105 sub-divergence deferred in v110. 8 integration tests + the 38th harness. Bump the Tier-1 count.

- [ ] **Step 3: Update the v110 converse-M-105 note + Tier counts**

Find the v110 M-105 entry / changelog note that says the converse `${u+"$u"}` set-but-null sub-divergence is deferred, and mark it closed by v114/M-110 (the underlying alternate-word-quoting issue). Update the **Last updated** line to v114 (M-110). Bump the **Bugs (Tier 1)** count by 1 and append `; M-110 alternate/default word quoting under unquoted expansion fixed v114` to its note.

- [ ] **Step 4: Change-log entry + README row**

`docs/bash-divergences.md` change log (after the v113 entry): a `2026-06-08` v114 entry — the `Fields` variant + `!quoted` gate mechanism, the `mise<TAB>` `_upvars` payoff, the bisection (empty/spaced/scalar), the quoted-outer regression guards, the converse-M-105 closure, the 38th harness + test count from Task 2's full-suite run. Add a v114 README row after v113. Use the REAL test count: `cargo test 2>&1 | awk '/test result:/{s+=$4} END{print s}'`.

- [ ] **Step 5: Verify (no placeholders) + commit**

```bash
grep -n 'M-110\|fixed v114\|v114' docs/bash-divergences.md README.md | head
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: v114 — alternate/default word quoting under unquoted expansion (M-110)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 3 report
DONE/BLOCKED, commit SHA, the grep output proving real M-number/version, the test count used.

---

## Final (after all tasks)
- [ ] Whole-branch review: `git log --oneline main..HEAD`, `git diff --stat main..HEAD`.
- [ ] `cargo test 2>&1 | grep -cE 'test result: ok'` (green, no FAILED), `cargo clippy --all-targets 2>&1 | tail -2` (clean).
- [ ] All harnesses: `export HUCK_BIN="$(pwd)/target/debug/huck"; for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done` (silent = pass; 38 files).
- [ ] **Payoff**: the `${words+"${words[@]}"}` / `_upvars` shape preserves the empty element (`UPVARS_OK n=7`) (Task 2 Step 5).
- [ ] AskUserQuestion merge gate, then `git merge --no-ff` + push + delete branch, then update memory files (`project_huck_iterations.md` + `MEMORY.md`).
