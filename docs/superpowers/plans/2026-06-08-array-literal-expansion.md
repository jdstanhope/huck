# v117 — array-literal element field-expansion (M-112) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make array-literal elements (`arr=(…)` / `arr+=(…)`, including `local`/`declare`/`readonly` forms) undergo full field-expansion — word-splitting, command-substitution splitting, pathname globbing, and the quoted/unquoted `${arr[@]}`/`$@` multi-field rule — so one element yields zero, one, or many values, matching bash and fixing the `mise<TAB>` `: : invalid option`.

**Architecture:** Route each **bare** array-literal element through the existing command-argument expansion path (`glob_expand_word`), advancing the auto-index **per produced field**; keep **subscripted** `[i]=val` elements single-valued (`expand_assignment`). A shared helper `expand_array_elements` does this for both the replace path (`build_array_map`) and the append path (`a+=(…)`). All array-literal forms (`local`/`declare`/bare) already funnel through `apply_one_assignment`, so this single locus fixes them all.

**Tech Stack:** Rust. `src/executor.rs` (`apply_one_assignment`, `build_array_map`), `src/shell_state.rs` (new `extend_indexed`). Tests: `cargo test`, a new integration test, a new `tests/scripts/*_diff_check.sh` harness.

**Spec:** `docs/superpowers/specs/2026-06-08-array-literal-expansion-design.md`. Read it first.

**Commit trailer (MANDATORY, canonical — every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

Anchors (verify exact lines — code shifts):
- `build_array_map` (`src/executor.rs:4144`) — the per-element loop using `expand_assignment`.
- The `(AssignTarget::Bare(name), Some(elements))` arm of `apply_one_assignment` (`src/executor.rs:4056-4070`) — replace via `build_array_map`/`replace_array`, append via `append_array`.
- `glob_expand_word` (`src/executor.rs:2030`): `fn glob_expand_word(word: &crate::lexer::Word, shell: &mut Shell) -> Result<Vec<String>, ()>` — the donor field+glob path.
- `append_array` (`src/shell_state.rs:886`) — model for the new `extend_indexed` (readonly check + scalar→index0 promotion + create-if-missing).
- `ArrayLiteralElement` (`src/lexer.rs:190`): fields `subscript: Option<Word>`, `value: Word`.
- `eval_subscript(sw, shell, name) -> Result<usize, String>` (used in `build_array_map`).

**Verified bash contract (probed this session):**
`s="a b c"; arr=($s)`→3; `arr=($(echo x y z))`→3; `w=(a b c); arr=("${w[@]}")`→3; `arr=(${w[@]})`→3; `arr=(a $s [9]=z b)` (s="x y")→n=5 idx `0 1 2 9 10`; `arr=(a); s="b c"; arr+=($s)`→3; `e=; arr=(a $e b)`→2 (unquoted-empty drops); `e=; arr=(a "$e" b)`→3 (quoted-empty kept); `w=(a "" c); arr=("${w[@]}")`→3; `arr=([0]=$s)`→1 (subscript value NOT split); `arr=("${w[*]}")`→1 (joined); `arr=(/nonexistent*)`→1 literal.

---

## Task 1: shared `expand_array_elements` helper + replace path (`arr=(…)`)

**Files:**
- Modify: `src/executor.rs` (`build_array_map` ~`:4144`, add `expand_array_elements`)
- Create: `tests/array_literal_expansion_integration.rs`

- [ ] **Step 1: Write the failing integration tests (replace path)**

Create `tests/array_literal_expansion_integration.rs`. The `run` helper executes a script as a FILE ARG (not piped stdin) per L-27 — `[!`/`!`-bearing or history-sensitive fragments must not hit huck's piped-stdin history expansion.
```rust
//! v117: array-literal element field-expansion (M-112).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Run `script` as a file-arg (true non-interactive path). Returns stdout.
fn run(script: &str) -> String {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("huck_v117_{}.sh", std::process::id()));
    {
        let mut f = std::fs::File::create(&path).expect("create temp script");
        f.write_all(script.as_bytes()).unwrap();
    }
    let out = Command::new(huck_bin())
        .arg(&path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn huck");
    let _ = std::fs::remove_file(&path);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn scalar_word_split() {
    assert_eq!(run("s=\"a b c\"\narr=($s)\necho \"n=${#arr[@]}\"\n"), "n=3\n");
}
#[test]
fn cmdsub_split() {
    assert_eq!(run("arr=($(echo x y z))\necho \"n=${#arr[@]}\"\n"), "n=3\n");
}
#[test]
fn quoted_array_at_fans_out() {
    assert_eq!(run("w=(a b c)\narr=(\"${w[@]}\")\necho \"n=${#arr[@]}\"\n"), "n=3\n");
}
#[test]
fn quoted_array_at_keeps_empty_member() {
    assert_eq!(run("w=(a \"\" c)\narr=(\"${w[@]}\")\necho \"n=${#arr[@]}\"\n"), "n=3\n");
}
#[test]
fn unquoted_empty_drops() {
    assert_eq!(run("e=\narr=(a $e b)\necho \"n=${#arr[@]}\"\n"), "n=2\n");
}
#[test]
fn quoted_empty_kept() {
    assert_eq!(run("e=\narr=(a \"$e\" b)\necho \"n=${#arr[@]}\"\n"), "n=3\n");
}
#[test]
fn quoted_star_joins_to_one() {
    assert_eq!(run("w=(a b c)\narr=(\"${w[*]}\")\necho \"n=${#arr[@]}\"\n"), "n=1\n");
}
#[test]
fn subscript_value_not_split() {
    assert_eq!(run("s=\"a b c\"\narr=([0]=$s)\necho \"n=${#arr[@]} z=[${arr[0]}]\"\n"), "n=1 z=[a b c]\n");
}
#[test]
fn mixed_bare_and_subscript_index_continuation() {
    // a→0, x→1, y→2, [9]=z, b→10
    assert_eq!(
        run("s=\"x y\"\narr=(a $s [9]=z b)\necho \"n=${#arr[@]} idx=[${!arr[@]}]\"\n"),
        "n=5 idx=[0 1 2 9 10]\n"
    );
}
#[test]
fn local_array_literal_fans_out() {
    // The mise path shape: a `local` array literal containing "${w[@]}".
    assert_eq!(
        run("w=(a b c)\nf(){ local arr=(p \"${w[@]}\" q); echo \"n=${#arr[@]}\"; }\nf\n"),
        "n=5\n"
    );
}
```

- [ ] **Step 2: Run the tests — confirm they fail**

Run: `cargo build --bin huck && cargo test --test array_literal_expansion_integration 2>&1 | tail -25`
Expected: the splitting/`[@]`/empty tests FAIL (huck collapses to 1); `subscript_value_not_split` and `quoted_star_joins_to_one` PASS (already correct).

- [ ] **Step 3: Add the `expand_array_elements` helper**

In `src/executor.rs`, immediately ABOVE `fn build_array_map`, add:
```rust
/// Field-expands a compound array literal's elements into an explicit
/// `(index → value)` map, starting bare-element auto-indexing at `start`.
///
/// Bare elements (no `[subscript]=`) go through the SAME field+glob path
/// command arguments use (`glob_expand_word`): unquoted word-splitting,
/// command-substitution splitting, pathname globbing, and the
/// quoted/unquoted `${arr[@]}`/`$@` multi-field rule — one element may yield
/// zero, one, or many values, and the implicit index advances per produced
/// FIELD. Subscripted `[i]=value` elements keep single-value semantics (no
/// splitting, via `expand_assignment`) and reset the implicit index to
/// `i + 1`. (M-112)
fn expand_array_elements(
    elements: &[crate::lexer::ArrayLiteralElement],
    name: &str,
    shell: &mut Shell,
    start: usize,
) -> Result<std::collections::BTreeMap<usize, String>, ()> {
    let mut map: std::collections::BTreeMap<usize, String> = std::collections::BTreeMap::new();
    let mut implicit = start;
    for e in elements {
        match &e.subscript {
            Some(sw) => {
                let idx = match crate::expand::eval_subscript(sw, shell, name) {
                    Ok(i) => i,
                    Err(msg) => {
                        eprintln!("huck: {msg}");
                        return Err(());
                    }
                };
                map.insert(idx, expand_assignment(&e.value, shell));
                implicit = idx + 1;
            }
            None => {
                for field in glob_expand_word(&e.value, shell)? {
                    map.insert(implicit, field);
                    implicit += 1;
                }
            }
        }
    }
    Ok(map)
}
```

- [ ] **Step 4: Rewrite `build_array_map` to delegate to the helper**

Replace the entire body of `fn build_array_map` (`src/executor.rs:4144`) so it becomes a thin wrapper (the replace path starts auto-indexing at 0):
```rust
fn build_array_map(
    elements: &[crate::lexer::ArrayLiteralElement],
    name: &str,
    shell: &mut Shell,
) -> Result<std::collections::BTreeMap<usize, String>, ()> {
    expand_array_elements(elements, name, shell, 0)
}
```
(The replace arm at `src/executor.rs:4067-4069` still calls `build_array_map` then `shell.replace_array(name, map)` — unchanged.)

- [ ] **Step 5: Run the tests — confirm green**

Run: `cargo build --bin huck && cargo test --test array_literal_expansion_integration 2>&1 | tail -20`
Expected: all Task-1 tests PASS (note: `arr=(a); s="b c"; arr+=($s)` append is Task 2 — not in this file yet).

- [ ] **Step 6: Quick byte-identical spot check vs bash**

```bash
cargo build --bin huck
for f in 's="a b c"; arr=($s); echo "${#arr[@]}"' \
         'w=(a b c); arr=("${w[@]}"); echo "${#arr[@]}"' \
         'arr=(a $s [9]=z b); s="x y"; arr=(a $s [9]=z b); echo "${!arr[@]}"' \
         'w=(a b c); f(){ local arr=(p "${w[@]}" q); echo "${#arr[@]}"; }; f'; do
  b=$(printf '%s\n' "$f" > /tmp/t.sh; bash --norc --noprofile /tmp/t.sh 2>&1)
  h=$(printf '%s\n' "$f" > /tmp/t.sh; ./target/debug/huck /tmp/t.sh 2>&1)
  [ "$b" = "$h" ] && echo "MATCH: $f" || { echo "DIFF: $f"; echo " b=[$b] h=[$h]"; }
done
```
Expected: all MATCH.

- [ ] **Step 7: Commit**

```bash
git add src/executor.rs tests/array_literal_expansion_integration.rs
git commit -m "$(cat <<'EOF'
fix: field-expand bare array-literal elements (replace path) (M-112)

Route bare arr=(...) elements through glob_expand_word (the command-arg
field+glob path): word-splitting, command-sub splitting, pathname globbing,
and the quoted/unquoted ${arr[@]}/$@ multi-field rule. The implicit index
advances per produced field; subscripted [i]=val stay single-valued. Shared
expand_array_elements helper; build_array_map delegates with start=0. Fixes
local/declare/bare array literals (all funnel through apply_one_assignment).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1 report
DONE/BLOCKED, commit SHA, the helper, the Task-1 test pass line, the MATCH spot-check.

---

## Task 2: append path (`arr+=(…)`) field-expansion

**Files:**
- Modify: `src/shell_state.rs` (add `extend_indexed` near `append_array` ~`:886`)
- Modify: `src/executor.rs` (the `if a.append` branch of the bare-array arm ~`:4059-4065`)
- Modify: `tests/array_literal_expansion_integration.rs` (append tests)

- [ ] **Step 1: Write the failing append tests**

Append to `tests/array_literal_expansion_integration.rs`:
```rust
#[test]
fn append_scalar_split() {
    assert_eq!(run("arr=(a)\ns=\"b c\"\narr+=($s)\necho \"n=${#arr[@]}\"\n"), "n=3\n");
}
#[test]
fn append_array_at_fans_out() {
    assert_eq!(run("arr=(a)\nw=(b c d)\narr+=(\"${w[@]}\")\necho \"n=${#arr[@]}\"\n"), "n=4\n");
}
#[test]
fn append_continues_index() {
    assert_eq!(run("arr=(a b)\narr+=(c d)\necho \"idx=[${!arr[@]}]\"\n"), "idx=[0 1 2 3]\n");
}
#[test]
fn append_to_unset_starts_at_zero() {
    assert_eq!(run("arr+=(x y)\necho \"idx=[${!arr[@]}]\"\n"), "idx=[0 1]\n");
}
```
Verify each against system bash first.

- [ ] **Step 2: Run the append tests — confirm they fail**

Run: `cargo test --test array_literal_expansion_integration append 2>&1 | tail -15`
Expected: `append_scalar_split`/`append_array_at_fans_out` FAIL (collapse); index tests may already pass.

- [ ] **Step 3: Add `extend_indexed` to `src/shell_state.rs`**

Immediately AFTER `pub fn append_array` (`src/shell_state.rs` ~`:924`), add (mirrors `append_array`'s readonly check + scalar→index0 promotion + create-if-missing, but inserts at explicit keys):
```rust
    /// Merges explicit `(index → value)` entries into the named indexed
    /// array, creating it if missing and promoting a scalar to element 0
    /// first. Honors readonly (callers should pre-check to avoid a partial
    /// write; this re-checks defensively). Used by `a+=(elements)` after the
    /// elements are field-expanded with continuation indices already
    /// computed. Appending to an associative array is a type error.
    pub fn extend_indexed(
        &mut self,
        name: &str,
        entries: BTreeMap<usize, String>,
    ) -> Result<(), AssignErr> {
        if let Some(existing) = self.vars.get(name)
            && existing.readonly
        {
            eprintln!("huck: {name}: readonly variable");
            return Err(AssignErr::Readonly);
        }
        // Promote scalar to indexed (scalar becomes element 0).
        if matches!(
            self.vars.get(name).map(|v| &v.value),
            Some(VarValue::Scalar(_))
        ) && let Some(v) = self.vars.get_mut(name)
            && let VarValue::Scalar(s) = &mut v.value
        {
            let mut m = BTreeMap::new();
            m.insert(0, std::mem::take(s));
            v.value = VarValue::Indexed(m);
        }
        if !self.vars.contains_key(name) {
            self.vars.insert(
                name.to_string(),
                Variable {
                    value: VarValue::Indexed(BTreeMap::new()),
                    exported: false,
                    readonly: false,
                    integer: false,
                },
            );
        }
        if let Some(v) = self.vars.get_mut(name)
            && let VarValue::Indexed(m) = &mut v.value
        {
            for (idx, val) in entries {
                m.insert(idx, val);
            }
            Ok(())
        } else {
            eprintln!("huck: {name}: cannot append array literal to associative array");
            Err(AssignErr::TypeMismatch)
        }
    }
```

- [ ] **Step 4: Rewrite the append branch in `apply_one_assignment`**

In `src/executor.rs`, replace the `if a.append { … }` block of the `(AssignTarget::Bare(name), Some(elements))` arm (`:4059-4065`):
```rust
            if a.append {
                // a+=(elements): append new keys after max_index.
                let values: Vec<String> = elements
                    .iter()
                    .map(|e| expand_assignment(&e.value, shell))
                    .collect();
                shell.append_array(name, &values).map_err(|_| ())
            } else {
```
with:
```rust
            if a.append {
                // a+=(elements): field-expand each bare element (split/glob/[@])
                // and append after the current max index, honoring explicit
                // [i]=v elements. Readonly pre-check avoids a partial write.
                if shell.is_readonly(name) {
                    eprintln!("huck: {name}: readonly variable");
                    return Err(());
                }
                // Starting auto-index: max+1 for an existing array; 1 for a
                // scalar (which promotes to element 0); 0 when unset — matching
                // append_array / extend_indexed promotion.
                let start = if shell.get_array(name).is_some() {
                    shell.array_max_index(name).map_or(0, |m| m + 1)
                } else if shell.get(name).is_some() {
                    1
                } else {
                    0
                };
                let map = expand_array_elements(elements, name, shell, start)?;
                shell.extend_indexed(name, map).map_err(|_| ())
            } else {
```
(Leave the `else { … build_array_map … replace_array … }` block untouched.)

- [ ] **Step 5: Run the append tests — confirm green**

Run: `cargo test --test array_literal_expansion_integration 2>&1 | tail -20`
Expected: all integration tests (Task 1 + Task 2) PASS.

- [ ] **Step 6: Verify append vs bash + readonly/scalar edges**

```bash
cargo build --bin huck
for f in 'arr=(a); s="b c"; arr+=($s); echo "${#arr[@]}"' \
         'arr=(a b); arr+=(c d); echo "${!arr[@]}"' \
         'arr+=(x y); echo "${!arr[@]}"' \
         's=scal; s+=(a b); echo "${!s[@]} [${s[*]}]"'; do
  b=$(printf '%s\n' "$f" > /tmp/t.sh; bash --norc --noprofile /tmp/t.sh 2>&1)
  h=$(printf '%s\n' "$f" > /tmp/t.sh; ./target/debug/huck /tmp/t.sh 2>&1)
  [ "$b" = "$h" ] && echo "MATCH: $f" || { echo "DIFF: $f"; echo " b=[$b] h=[$h]"; }
done
```
Expected: all MATCH (the scalar `s+=(a b)` promotes `s` to `[scal]` then appends → idx `0 1 2`, `[scal a b]`).

- [ ] **Step 7: Full regression + clippy**

Run: `cargo test 2>&1 | grep -E "test result: FAILED"` (empty) ; then `cargo clippy --all-targets 2>&1 | tail -3` (clean). Watch `arrays`/`assoc`/`declare`/`param`/`completion` suites — a regression means the field path altered a working case; investigate vs bash before proceeding.

- [ ] **Step 8: Commit**

```bash
git add src/executor.rs src/shell_state.rs tests/array_literal_expansion_integration.rs
git commit -m "$(cat <<'EOF'
fix: field-expand bare array-literal elements (append path) (M-112)

a+=(elements) now field-expands bare elements (split/glob/${arr[@]}) via the
shared expand_array_elements helper, appending after max_index and honoring
explicit [i]=v elements. New shell_state extend_indexed merges explicit
index→value entries (readonly check + scalar->[0] promotion + create).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2 report
DONE/BLOCKED, commit SHA, the append edits, the full integration-test pass line, the MATCH spot-check, full-suite green (no FAILED), clippy status.

---

## Task 3: 41st bash-diff harness + payoff smoke

**Files:**
- Create: `tests/scripts/array_literal_expansion_diff_check.sh`

- [ ] **Step 1: Write the harness**

Create `tests/scripts/array_literal_expansion_diff_check.sh` (run fragments as FILE-ARG scripts per L-27, since some contain history-sensitive characters):
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v117: array-literal element
# field-expansion (M-112) — split/cmdsub/glob/${arr[@]}/empties/mixed-index/
# append. Fragments run as file-arg scripts (L-27: huck history-expands piped
# stdin; the true non-interactive path is a file arg).
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

check "scalar split"        's="a b c"; arr=($s); echo "${#arr[@]}"'
check "cmdsub split"        'arr=($(echo x y z)); echo "${#arr[@]}"'
check "quoted [@] fan-out"  'w=(a b c); arr=("${w[@]}"); echo "${#arr[@]}"'
check "unquoted [@] fan"    'w=(a b c); arr=(${w[@]}); echo "${#arr[@]}"'
check "[@] keeps empty"     'w=(a "" c); arr=("${w[@]}"); echo "${#arr[@]}"'
check "quoted [*] joins"    'w=(a b c); arr=("${w[*]}"); echo "${#arr[@]}"'
check "unquoted-empty drop" 'e=; arr=(a $e b); echo "${#arr[@]}"'
check "quoted-empty kept"   'e=; arr=(a "$e" b); echo "${#arr[@]}"'
check "subscript no-split"  's="a b c"; arr=([0]=$s); echo "${#arr[@]} [${arr[0]}]"'
check "mixed index cont"    's="x y"; arr=(a $s [9]=z b); echo "${!arr[@]}"'
check "append split"        'arr=(a); s="b c"; arr+=($s); echo "${#arr[@]}"'
check "append continues"    'arr=(a b); arr+=(c d); echo "${!arr[@]}"'
check "glob match"          'd=$(mktemp -d); touch "$d"/f1.txt "$d"/f2.txt; cd "$d"; arr=(*.txt); echo "${#arr[@]}"; rm -rf "$d"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make executable, run it, run ALL harnesses**

```bash
chmod +x tests/scripts/array_literal_expansion_diff_check.sh && cargo build --bin huck
bash tests/scripts/array_literal_expansion_diff_check.sh
export HUCK_BIN="$(pwd)/target/debug/huck"
echo "count: $(ls tests/scripts/*_diff_check.sh | wc -l)"
for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done
echo done
```
Expected: `Total: 13, Pass: 13, Fail: 0`; `count: 41`; no `FAIL` lines.

- [ ] **Step 3: Payoff smoke (the real bash_completion chain)**

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
} > /tmp/v117_smoke.sh
echo "--- bash ---"; bash --norc --noprofile /tmp/v117_smoke.sh 2>&1
echo "--- huck ---"; ./target/debug/huck /tmp/v117_smoke.sh 2>&1
```
Expected: huck prints `SMOKE cur=[] prev=[mise] cword=1 nwords=2 w0=[mise]` (matching bash) with NO `: : invalid option`. Report the EXACT bash and huck lines. If a further gap surfaces (e.g. `_variables`/`__ltrim_colon_completions` command-not-found is a separate known gap), the `: : invalid option` MUST be gone and cword/nwords/prev/w0 correct; report honestly either way — the smoke is the gate, do NOT over-claim (v109/v115/v116 lesson).

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/array_literal_expansion_diff_check.sh
git commit -m "$(cat <<'EOF'
test: 41st bash-diff harness for array-literal field-expansion (M-112)

13 byte-identical fragments (split/cmdsub/glob/${arr[@]}/empties/mixed-index/
append). Payoff: the _init_completion -n : chain now yields cword=1 nwords=2
prev=mise with no `: : invalid option`.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 3 report
DONE/BLOCKED, commit SHA, the `Total: 13, Pass: 13` line, the `count: 41` + no-FAIL line, the EXACT payoff-smoke output (confirm no `: : invalid option`; report whether `mise<TAB>` is now functional or what residual remains).

---

## Task 4: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Read the structures to update**

```bash
grep -n 'Last updated:\|Bugs (Tier 1) |\|Low-impact (Tier 4) |\|^## Change log\|### M-112:\|### M-113:' docs/bash-divergences.md | head
grep -n '| v116 ' README.md
```

- [ ] **Step 2: Replace the M-112 entry (corrected root cause + fixed v117)**

In `docs/bash-divergences.md`, replace the ENTIRE `### M-112:` block (currently titled "bash_completion `_upvars` dynamic-scope upvar idiom does not propagate") with:
```
### M-112: array-literal elements (`arr=(…)` / `arr+=(…)`) were not field-expanded
- **Status**: `[fixed v117]`
- **Severity**: high (no word-split / glob / `${arr[@]}` fan-out in any array literal; the remaining `mise<TAB>` blocker)
- **huck (was)**: the array-literal evaluator expanded each syntactic element to exactly ONE string (`expand_assignment`), so `arr=($s)`, `arr=($(cmd))`, `arr=("${w[@]}")`, `arr=(${w[@]})`, and `arr+=($s)` all collapsed to a single element (bash: split into N). Subscripted `[i]=val` and quoted `"${w[*]}"` were already correct.
- **bash**: each BARE element undergoes word-splitting, command-substitution splitting, pathname globbing, and the quoted/unquoted `${arr[@]}`/`$@` multi-field rule (one element → 0..n values); subscripted `[i]=value` values are single words.
- **Root cause**: `build_array_map` / the `a+=(…)` arm used the single-string `expand_assignment` per element. Command arguments already field-expand correctly via `glob_expand_word`.
- **Fix (v117)**: a shared `expand_array_elements` helper (`src/executor.rs`) routes BARE elements through `glob_expand_word` (the command-arg field+glob path), advancing the implicit index per produced FIELD; subscripted `[i]=val` stay single-valued. `build_array_map` delegates (replace, start=0); the append arm starts at max+1 and merges via the new `extend_indexed` (`src/shell_state.rs`). All `local`/`declare`/`readonly`/bare array literals funnel through `apply_one_assignment`, so one locus fixes them all. 41st harness + integration tests.
- **Driver / payoff**: bash_completion's `_upvars` callers build `upargs=(-aN "$2" "${words[@]}" -v …)`; the collapse desynced the count from `-aN` → `_upvars` mis-parsed → `bash_completion: : : invalid option` and `words`/`cword` failed to propagate to `_get_comp_words_by_ref`. v117 makes the chain yield `cword=1 nwords=2 prev=mise`.
- **Corrected diagnosis**: v116 wrongly attributed M-112 to a "`_upvars` dynamic-scope unset-reveal idiom" (a theory from a hand-written repro). Per-step instrumentation of the real `_upvars` showed it receives 6 args instead of 8 — the array-literal collapse in the caller. `unset`/dynamic scope work correctly.
```

- [ ] **Step 3: Add a new deferred entry for the `eval x=(…)` command-argument panic**

In the Tier-1 (Bugs) section, after the M-113 entry, add:
```
### M-114: array literal as a command argument (`eval x=(…)` unescaped) panics
- **Status**: `[deferred]` (found v117)
- **Severity**: medium (a panic/abort, but off the common path)
- **huck**: an array literal `name=(…)` appearing as a command ARGUMENT (not a leading assignment) — e.g. `eval x=(a b)` with UNESCAPED parens — reaches `expand()` as a parser-internal `WordPart::ArrayLiteral` and panics (`internal error: … must not reach expand(); try_split_assignment is supposed to consume it`, `src/expand.rs:982`). `try_split_assignment` only consumes the literal when it's the command's leading assignment.
- **bash**: treats `x=(…)` specially even as an argument and does not error.
- **Workaround / why low-urgency**: the real `_upvars` (and most code) ESCAPE the parens (`eval $2=\(…\)`), which lexes as a plain word and works; quoted `eval "x=(a b)"` also works. Only literal unescaped `cmd name=(…)` triggers it.
- **Next**: make a command-argument `ArrayLiteral` expand to its reconstructed `name=(…)` text (or otherwise not reach `expand()` via `unreachable!`). Own iteration.
```

- [ ] **Step 4: Update summary counts + Last-updated**

In `docs/bash-divergences.md`:
- Line 24 `| Bugs (Tier 1) | 21 |`: bump to `22` (M-114 added, deferred). Keep "all fixed EXCEPT …" wording: M-112 is now fixed, so the EXCEPT set becomes `M-114` (and any other still-deferred Tier-1; verify none others). Append to the notes: `; M-112 array-literal field-expansion fixed v117 (corrected root cause); M-114 array-literal-as-command-argument panic deferred (found v117)`.
- Line 3 "Last updated": replace with
```
**Last updated:** 2026-06-08 (after v117: array-literal elements (`arr=(…)`/`arr+=(…)`) are field-expanded — split/glob/`${arr[@]}` fan-out (M-112, the real `mise<TAB>` blocker, corrected from v116's wrong dynamic-scope diagnosis); the `eval x=(…)` command-argument array-literal panic logged deferred as M-114).
```

- [ ] **Step 5: Change-log entry + README row**

Append to the END of the `## Change log` section:
```
- **2026-06-08**: M-112 (array-literal element field-expansion) shipped as v117. Bare `arr=(…)`/`arr+=(…)` elements now route through `glob_expand_word` (the command-arg field+glob path): word-splitting, command-sub splitting, pathname globbing, and the quoted/unquoted `${arr[@]}`/`$@` multi-field rule (one element → 0..n values, implicit index per field); subscripted `[i]=val` stay single. Shared `expand_array_elements` helper; `build_array_map` delegates (start 0); the append arm starts at max+1 and merges via the new `extend_indexed`. All `local`/`declare`/`readonly`/bare literals funnel through `apply_one_assignment`. **mise<TAB> payoff: the `_upvars` callers' `upargs=(… "${words[@]}" …)` no longer collapse → `_get_comp_words_by_ref` yields `cword=1 nwords=2 prev=mise`, the `: : invalid option` is gone.** Corrected v116's wrong "`_upvars` dynamic-scope" M-112 diagnosis (the bug was the caller's array-literal collapse, shown by per-step `_upvars` arg-count instrumentation). 41st harness `array_literal_expansion_diff_check.sh` + integration tests; full suite <N> tests pass, clippy clean. Logged M-114 (`eval x=(…)` command-argument array-literal panic, deferred).
```
Replace `<N>` with the real count from `cargo test 2>&1 | awk '/test result:/{s+=$4} END{print s}'`.

Add a README row after the v116 row (match column spacing):
```
| v117      | **array-literal element field-expansion (M-112)** — `arr=(…)`/`arr+=(…)` elements weren't field-expanded: `arr=($s)`, `arr=($(cmd))`, `arr=("${w[@]}")`, `arr+=($s)` all collapsed to ONE element (bash splits into N). Fix: shared `expand_array_elements` routes BARE elements through `glob_expand_word` (the command-arg field+glob path — split/cmdsub/glob/`${arr[@]}`/`$@`, implicit index per produced field); subscripted `[i]=val` stay single. `build_array_map` delegates (replace, start 0); the `a+=(…)` arm starts at max+1 and merges via the new `extend_indexed`. All `local`/`declare`/bare literals funnel through `apply_one_assignment`. **PAYOFF: bash_completion's `_upvars` callers (`upargs=(… "${words[@]}" …)`) no longer desync → `_get_comp_words_by_ref` yields `cword=1 nwords=2 prev=mise`, the `mise<TAB>` `: : invalid option` is gone.** Corrected v116's wrong dynamic-scope M-112 diagnosis. Byte-identical to bash across split/cmdsub/glob/`[@]`/empties/mixed-index/append. 41st harness `array_literal_expansion_diff_check.sh` (13 fragments) + integration tests; full suite <N> tests pass, clippy clean. Logged M-114 (`eval x=(…)` argument array-literal panic, deferred) |
```

- [ ] **Step 6: Verify (no placeholders) + commit**

```bash
grep -n 'M-112\|M-114\|fixed v117\|v117' docs/bash-divergences.md README.md | head
# Confirm no literal "<N>" placeholder remains:
grep -n '<N>' docs/bash-divergences.md README.md && echo "PLACEHOLDER LEFT — fix" || echo "no placeholders"
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: v117 — array-literal element field-expansion (M-112); M-114 logged

M-112 fixed v117 (corrected root cause: array-literal element collapse, not
the v116 dynamic-scope theory). M-114 (eval x=(...) command-argument
array-literal panic) logged deferred.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 4 report
DONE/BLOCKED, commit SHA, the grep output proving M-112 `[fixed v117]` + M-114 added, confirmation no `<N>` placeholder remains, the test count used.

---

## Final (after all tasks)
- [ ] Whole-branch review: `git log --oneline main..HEAD`, `git diff --stat main..HEAD`.
- [ ] `cargo test 2>&1 | grep -cE 'test result: ok'` (green, no FAILED), `cargo clippy --all-targets 2>&1 | tail -2` (clean).
- [ ] All harnesses: `export HUCK_BIN="$(pwd)/target/debug/huck"; for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done` (silent = pass; 41 files).
- [ ] **Payoff**: the `_init_completion -n :` chain yields `cword=1 nwords=2 prev=mise` with no `: : invalid option` (Task 3 Step 3).
- [ ] AskUserQuestion merge gate, then `git merge --no-ff` + push + delete branch, then update memory files (`project_huck_iterations.md` + `MEMORY.md`). **This should be the iteration that makes `mise<TAB>` functional — confirm with the user after merge; if a residual remains, report it honestly and scope the next iteration.**
