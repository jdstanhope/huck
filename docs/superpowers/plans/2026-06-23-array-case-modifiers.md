# v209 Per-element parameter-expansion modifiers on whole arrays — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Apply per-element parameter-expansion modifiers (Case, RemovePrefix, RemoveSuffix, Substitute, Transform) across whole arrays — `${a[@]^^}` etc. — on both indexed and associative arrays. Resolves M-127 + shrinks the v71/v72 "not supported on array" catchall.

**Architecture:** Add one new match arm to `expand_array_param` (line ~819) and a symmetric arm to `expand_assoc_param` (line ~440) in `crates/huck-engine/src/expand.rs`. The arm iterates the array's values and applies each modifier per-element via the existing `param_expansion::expand_modifier_with_value` scalar machinery (passing `ParamLookup::Element(Some(&value))`). Results collect into `WordList` for quoted `[@]` or joined `Value` for `[*]`/unquoted, matching the existing `[@]`/`[*]` discipline. ~30 LOC of new logic; no new modules; no public API change.

**Tech Stack:** Rust 2021, no new crate deps. All scalar modifier logic already exists in `param_expansion.rs` — v209 is purely a "shape-shift wrapper" that calls the scalar path per-element.

**Branch:** `v209-array-case-modifiers`. Each task ends with a green-suite commit.

**Spec:** `docs/superpowers/specs/2026-06-23-array-case-modifiers-design.md`.

**Key context — `TransformOp` enum** (`crates/huck-syntax/src/lexer.rs:147-154`): today's variants are `PromptExpand, Quote, Upper, Lower, UpperFirst, EscapeExpand` — ALL per-element. The whole-array forms `@A`/`@K`/`@k`/`@a` are M-93 territory and don't exist in the enum yet (lexer rejects them). So a simple `PM::Transform { .. }` match arm without a sub-predicate is sufficient for now; M-93's iteration can refine if it ever adds non-per-element variants.

---

## File structure

**Modify:**
- `crates/huck-engine/src/expand.rs` — add `is_per_element_modifier` predicate + `scalar_apply_per_element` helper; insert per-element match arm in both `expand_array_param` (line ~819) and `expand_assoc_param` (line ~440).
- `docs/bash-divergences.md` — delete the M-127 entry (current-divergences-only policy).

**Create:**
- `tests/scripts/array_modifiers_diff_check.sh` — new bash-diff harness.

No new files in `src/`. No new modules. No public API change.

---

## Task 1: Add the predicates + helper in `expand.rs` (no callers yet)

**Files:**
- Modify: `crates/huck-engine/src/expand.rs` (append the helpers; no call sites yet)

This task adds the new functions but doesn't wire them in. Suite must stay green; new functions are `#[allow(dead_code)]` until Task 2 wires them.

- [ ] **Step 1: Create the branch**

```bash
git checkout -b v209-array-case-modifiers
```

- [ ] **Step 2: Find a good place to add the helpers in expand.rs**

Run:

```bash
grep -n 'fn expand_array_param\|fn expand_assoc_param' crates/huck-engine/src/expand.rs
```

The helpers should go BEFORE `expand_assoc_param` (the first of the two array-expand functions) — somewhere in the file's "helpers" region near `slice_word_list` and `ifs_join_sep`. Pick a location that keeps them near the array expansion paths, not at the end.

- [ ] **Step 3: Add the helpers**

Add this block at a sensible spot before `expand_assoc_param` (e.g., just after `slice_word_list`):

```rust
/// Whether this modifier can sensibly apply per-element across a whole
/// array. Used by `expand_array_param` / `expand_assoc_param` to dispatch
/// to the per-element arm rather than the catchall "not supported on array"
/// rejection. The whole-array Transform ops (@A / @K / @k / @a) are NOT
/// currently in TransformOp — when M-93 adds them, this predicate will
/// need a sub-check on the op.
#[allow(dead_code)]
fn is_per_element_modifier(m: &crate::lexer::ParamModifier) -> bool {
    use crate::lexer::ParamModifier as PM;
    matches!(
        m,
        PM::Case { .. }
            | PM::RemovePrefix { .. }
            | PM::RemoveSuffix { .. }
            | PM::Substitute { .. }
            | PM::Transform { .. }
    )
}

/// Apply a scalar modifier to one element's value via the existing
/// `expand_modifier_with_value` scalar path. Wraps the element in
/// `ParamLookup::Element(Some(_))` so default/error modifiers see a present
/// element (every element here has a concrete value — even an empty
/// string).
///
/// Used by the per-element arm in `expand_array_param` / `expand_assoc_param`.
/// Falls through to empty-string output for non-Value results; per-element
/// scalar modifiers should never produce WordList/Fields/Fatal in practice.
#[allow(dead_code)]
fn scalar_apply_per_element(
    name: &str,
    modifier: &crate::lexer::ParamModifier,
    element: &str,
    quoted: bool,
    shell: &mut crate::shell_state::Shell,
) -> String {
    use crate::param_expansion::{expand_modifier_with_value, ExpansionResult, ParamLookup};
    match expand_modifier_with_value(
        name,
        modifier,
        ParamLookup::Element(Some(element)),
        quoted,
        shell,
    ) {
        ExpansionResult::Value(s) => s,
        ExpansionResult::Empty => String::new(),
        _ => String::new(),
    }
}
```

The `#[allow(dead_code)]` is removed in Task 2 when the callers are added.

- [ ] **Step 4: Build + run tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean build; suite green; clippy clean. The new functions are dead-coded; no behavior change.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/expand.rs
git commit -m "$(cat <<'EOF'
v209 task 1: add is_per_element_modifier + scalar_apply_per_element helpers

Pure additions to expand.rs; no call sites yet. is_per_element_modifier is
the predicate the new per-element arm will use to dispatch from the
catchall "not supported on array" rejection. scalar_apply_per_element is
the shape-shift wrapper around expand_modifier_with_value that runs the
existing scalar modifier path on one element's value via
ParamLookup::Element. Both marked #[allow(dead_code)] until Task 2 wires
them into expand_array_param.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Wire the per-element arm into `expand_array_param`

**Files:**
- Modify: `crates/huck-engine/src/expand.rs` — insert the new match arm in `expand_array_param` before the existing catchall.

- [ ] **Step 1: Find the catchall in `expand_array_param`**

```bash
grep -n 'not supported on array in v71' crates/huck-engine/src/expand.rs
```

Should hit one line around 819-824. The arm looks like:

```rust
(other, SK::All | SK::Star) => {
    with_err(|err| e!(err,
        "huck: ${{{name}[…]}}: modifier {:?} not supported on array in v71",
        other
    ));
    ExpansionResult::Value(String::new())
}
```

- [ ] **Step 2: Insert the new arm BEFORE the catchall**

Add this arm immediately above the catchall:

```rust
// Per-element scalar modifier across a whole array. Reuses the existing
// scalar modifier path via `scalar_apply_per_element`; WordList for
// quoted [@]; joined Value for [*] or unquoted [@].
(modif, SK::All | SK::Star) if is_per_element_modifier(modif) => {
    let values = collect_values(shell);
    let transformed: Vec<String> = values
        .iter()
        .map(|v| scalar_apply_per_element(name, modif, v, quoted, shell))
        .collect();
    if matches!(subscript, SK::All) && quoted {
        ExpansionResult::WordList(transformed)
    } else {
        let ifs = shell.ifs();
        let sep = ifs_join_sep(&ifs);
        ExpansionResult::Value(transformed.join(&sep))
    }
}
```

`collect_values` is the existing closure inside `expand_array_param` (line ~643). Same for `ifs_join_sep`.

- [ ] **Step 3: Remove `#[allow(dead_code)]` from `is_per_element_modifier`**

Now that `expand_array_param` calls it, the allow is no longer needed.

```rust
fn is_per_element_modifier(m: &crate::lexer::ParamModifier) -> bool {
```

(`scalar_apply_per_element` keeps its `#[allow(dead_code)]` until Task 3 wires the assoc path — OR remove now since Task 2 also calls it. Remove now.)

- [ ] **Step 4: Add unit tests in `crates/huck-engine/src/expand.rs::mod tests`**

Find the existing `#[cfg(test)] mod tests` block (search for `#[cfg(test)]`). Append:

```rust
// ===== v209: per-element modifiers on whole indexed arrays =====

#[test]
fn case_modifier_on_indexed_array_at() {
    use crate::shell_state::Shell;
    use crate::param_expansion::ExpansionResult;
    let mut shell = Shell::new();
    shell.set_indexed_element("a", 0, "foo".to_string()).unwrap();
    shell.set_indexed_element("a", 1, "bar".to_string()).unwrap();
    let result = expand_array_param(
        "a",
        &crate::lexer::ParamModifier::Case {
            direction: crate::lexer::CaseDirection::Upper,
            all: true,
            pattern: None,
        },
        &crate::lexer::SubscriptKind::All,
        true, // quoted
        &mut shell,
    );
    match result {
        ExpansionResult::WordList(words) => assert_eq!(words, vec!["FOO", "BAR"]),
        other => panic!("expected WordList, got {other:?}"),
    }
}

#[test]
fn case_modifier_on_indexed_array_star() {
    use crate::shell_state::Shell;
    use crate::param_expansion::ExpansionResult;
    let mut shell = Shell::new();
    shell.set_indexed_element("a", 0, "foo".to_string()).unwrap();
    shell.set_indexed_element("a", 1, "bar".to_string()).unwrap();
    let result = expand_array_param(
        "a",
        &crate::lexer::ParamModifier::Case {
            direction: crate::lexer::CaseDirection::Upper,
            all: true,
            pattern: None,
        },
        &crate::lexer::SubscriptKind::Star,
        true,
        &mut shell,
    );
    match result {
        ExpansionResult::Value(v) => assert_eq!(v, "FOO BAR"),
        other => panic!("expected Value, got {other:?}"),
    }
}

#[test]
fn remove_suffix_per_element_indexed() {
    use crate::shell_state::Shell;
    use crate::lexer::{ParamModifier as PM, SubscriptKind as SK, Word, WordPart};
    use crate::param_expansion::ExpansionResult;
    let mut shell = Shell::new();
    shell.set_indexed_element("a", 0, "foo.txt".to_string()).unwrap();
    shell.set_indexed_element("a", 1, "bar.md".to_string()).unwrap();
    // pattern Word for `.*`
    let pat = Word(vec![WordPart::Literal { text: ".*".into(), quoted: false }]);
    let result = expand_array_param(
        "a",
        &PM::RemoveSuffix { pattern: pat, longest: false },
        &SK::All,
        true,
        &mut shell,
    );
    match result {
        ExpansionResult::WordList(words) => assert_eq!(words, vec!["foo", "bar"]),
        other => panic!("expected WordList, got {other:?}"),
    }
}

#[test]
fn empty_array_per_element_modifier() {
    use crate::shell_state::Shell;
    use crate::param_expansion::ExpansionResult;
    let mut shell = Shell::new();
    // Don't set anything; array is unset.
    let result = expand_array_param(
        "a",
        &crate::lexer::ParamModifier::Case {
            direction: crate::lexer::CaseDirection::Upper,
            all: true,
            pattern: None,
        },
        &crate::lexer::SubscriptKind::All,
        true,
        &mut shell,
    );
    match result {
        ExpansionResult::WordList(words) => assert!(words.is_empty(), "expected empty WordList, got {words:?}"),
        other => panic!("expected WordList, got {other:?}"),
    }
}
```

Adjust the `crate::lexer::Word`/`WordPart` import paths if huck-syntax's namespace differs — read the existing `mod tests` block in `expand.rs` for the canonical imports.

- [ ] **Step 5: Build + run tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet case_modifier_on_indexed_array remove_suffix_per_element_indexed empty_array_per_element_modifier
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 4 new tests pass; full suite green; clippy clean. The catchall now only fires for `AssignDefault`, `ErrorIfUnset`, and (if M-93 ever adds them) whole-array Transform ops.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/expand.rs
git commit -m "$(cat <<'EOF'
v209 task 2: per-element modifier arm in expand_array_param

New match arm before the catchall fires for any modifier where
is_per_element_modifier returns true (Case, RemovePrefix, RemoveSuffix,
Substitute, Transform). Iterates collect_values, applies the modifier
per-element via scalar_apply_per_element, collects into WordList for
quoted [@] or joined Value for [*]/unquoted. 4 unit tests cover the
quoted [@], joined [*], RemoveSuffix per-element, and empty-array shapes.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Wire the symmetric arm into `expand_assoc_param`

**Files:**
- Modify: `crates/huck-engine/src/expand.rs` — add the symmetric arm to `expand_assoc_param`.

- [ ] **Step 1: Find the catchall in `expand_assoc_param`**

```bash
grep -n 'not supported on associative array in v72' crates/huck-engine/src/expand.rs
```

Should hit one line around 440-445. The arm looks like:

```rust
(other, SK::All | SK::Star) => {
    with_err(|err| e!(err,
        "huck: ${{{name}[…]}}: modifier {:?} not supported on associative array in v72",
        other
    ));
    ExpansionResult::Value(String::new())
}
```

- [ ] **Step 2: Insert the symmetric arm BEFORE the catchall**

Note the assoc path uses a pre-collected `values: Vec<String>` snapshot (computed at the top of the function, line ~303). Reuse it directly:

```rust
// Per-element scalar modifier across a whole assoc array. Iterates values
// in the pre-collected order (matches existing `${m[@]}` semantics —
// insertion order; pre-existing L-44 divergence from bash's hash order).
(modif, SK::All | SK::Star) if is_per_element_modifier(modif) => {
    let transformed: Vec<String> = values
        .iter()
        .map(|v| scalar_apply_per_element(name, modif, v, quoted, shell))
        .collect();
    if matches!(subscript, SK::All) && quoted {
        ExpansionResult::WordList(transformed)
    } else {
        let ifs = shell.ifs();
        let sep = ifs_join_sep(&ifs);
        ExpansionResult::Value(transformed.join(&sep))
    }
}
```

- [ ] **Step 3: Add unit tests in `mod tests`**

Append to the same `#[cfg(test)] mod tests` block in `expand.rs`:

```rust
#[test]
fn case_modifier_on_associative_array() {
    use crate::shell_state::Shell;
    use crate::param_expansion::ExpansionResult;
    let mut shell = Shell::new();
    shell.declare_associative("m").unwrap();
    shell.set_associative_element("m", "k", "foo".to_string()).unwrap();
    shell.set_associative_element("m", "j", "bar".to_string()).unwrap();
    let result = expand_array_param(
        "m",
        &crate::lexer::ParamModifier::Case {
            direction: crate::lexer::CaseDirection::Upper,
            all: true,
            pattern: None,
        },
        &crate::lexer::SubscriptKind::All,
        true,
        &mut shell,
    );
    match result {
        ExpansionResult::WordList(words) => {
            // Order is insertion order (L-44 intentional divergence).
            // Just assert the set contents.
            let mut sorted = words.clone();
            sorted.sort();
            assert_eq!(sorted, vec!["BAR", "FOO"]);
        }
        other => panic!("expected WordList, got {other:?}"),
    }
}

#[test]
fn substitute_per_element_assoc() {
    use crate::shell_state::Shell;
    use crate::lexer::{ParamModifier as PM, SubscriptKind as SK, SubstAnchor, Word, WordPart};
    use crate::param_expansion::ExpansionResult;
    let mut shell = Shell::new();
    shell.declare_associative("m").unwrap();
    shell.set_associative_element("m", "k", "foo".to_string()).unwrap();
    shell.set_associative_element("m", "j", "boo".to_string()).unwrap();
    let pat = Word(vec![WordPart::Literal { text: "o".into(), quoted: false }]);
    let repl = Word(vec![WordPart::Literal { text: "X".into(), quoted: false }]);
    let result = expand_array_param(
        "m",
        &PM::Substitute {
            pattern: pat,
            replacement: Some(repl),
            anchor: SubstAnchor::Any,
            all: false, // first match only
        },
        &SK::All,
        true,
        &mut shell,
    );
    match result {
        ExpansionResult::WordList(words) => {
            let mut sorted = words.clone();
            sorted.sort();
            // Each value has its first `o` replaced with `X`: foo->fXo, boo->bXo.
            assert_eq!(sorted, vec!["bXo", "fXo"]);
        }
        other => panic!("expected WordList, got {other:?}"),
    }
}
```

If the exact shape of `ParamModifier::Substitute` differs (e.g. `replacement: Option<Word>` vs `Word`, `anchor` variants, `all: bool` naming), match the actual enum at `crates/huck-syntax/src/lexer.rs`. Read the existing `Substitute` arm in `param_expansion.rs::expand_modifier_with_value` for the canonical names.

- [ ] **Step 4: Build + run tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet case_modifier_on_associative_array substitute_per_element_assoc
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 2 new tests pass; full suite green; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/expand.rs
git commit -m "$(cat <<'EOF'
v209 task 3: per-element modifier arm in expand_assoc_param

Symmetric to Task 2's wiring into expand_array_param — same per-element
shape applied to associative arrays. Reuses the pre-collected `values`
snapshot at the top of expand_assoc_param. Iteration order remains
insertion order (L-44 intentional divergence). 2 unit tests cover Case
and Substitute on assoc with order-agnostic assertions.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Bash-diff harness

**Files:**
- Create: `tests/scripts/array_modifiers_diff_check.sh`

- [ ] **Step 1: Create the harness**

```bash
cat > tests/scripts/array_modifiers_diff_check.sh <<'HARNESS_EOF'
#!/usr/bin/env bash
# v209: Bash-diff harness for per-element parameter-expansion modifiers
# applied across whole arrays. Runs each fragment through bash and huck;
# stdout must match byte-for-byte.
set -u

cd "$(dirname "$0")/../.." || exit 1
cargo build --quiet --workspace --bin huck >/dev/null 2>&1
HUCK=target/debug/huck
if [ ! -x "$HUCK" ]; then
    echo "FAIL: huck binary not found at $HUCK" >&2
    exit 1
fi

FAIL=0
check() {
    local label=$1 frag=$2
    local b h
    b=$(bash -c "$frag" 2>&1)
    h=$("$HUCK" -c "$frag" 2>&1)
    if [ "$b" != "$h" ]; then
        echo "FAIL [$label]"
        echo "  bash: $b"
        echo "  huck: $h"
        FAIL=1
    else
        echo "PASS [$label]"
    fi
}

# === Case modification (M-127 literal scope) ===
check 'case-upper-all'        'a=(foo bar baz); echo "${a[@]^^}"'
check 'case-upper-first'      'a=(foo bar baz); echo "${a[@]^}"'
check 'case-lower-all'        'a=(FOO BAR); echo "${a[@],,}"'
check 'case-lower-first'      'a=(FOO BAR); echo "${a[@],}"'
check 'case-pattern-arg'      'a=(hello world); echo "${a[@]^^[hl]}"'
check 'case-star-join'        'a=(foo bar); echo "${a[*]^^}"'
check 'case-empty-array'      'a=(); echo "[${a[@]^^}]"'
check 'case-assoc-array'      'declare -A m=([k]=foo [j]=bar); for v in "${m[@]^^}"; do echo "<$v>"; done | sort'

# === Per-element prefix / suffix / substitute ===
check 'suffix-shortest'       'a=(foo.txt bar.md baz.txt); echo "${a[@]%.*}"'
check 'prefix-longest'        'a=(foo.txt bar.md); echo "${a[@]##*.}"'
check 'substitute-first'      'a=(foo bar baz); echo "${a[@]/a/X}"'
check 'substitute-all'        'a=(foo bar baz); echo "${a[@]//[ao]/X}"'
check 'substitute-star'       'a=(foo bar); echo "${a[*]/o/_}"'
check 'suffix-assoc'          'declare -A m=([k]=foo.txt [j]=bar.md); for v in "${m[@]%.*}"; do echo "<$v>"; done | sort'

# === Per-element Transform (the per-element @OP subset) ===
check 'transform-upper'       'a=(foo BAR baz); echo "${a[@]@U}"'
check 'transform-lower'       'a=(foo BAR baz); echo "${a[@]@L}"'
check 'transform-upper-first' 'a=(foo BAR baz); echo "${a[@]@u}"'
check 'transform-quote'       'a=(foo "bar baz"); printf "%s\n" "${a[@]@Q}"'
check 'transform-lower-assoc' 'declare -A m=([k]=Foo [j]=Bar); for v in "${m[@]@L}"; do echo "<$v>"; done | sort'

# === Edge cases ===
check 'sparse-indexed'        'a=([0]=foo [5]=bar [10]=baz); echo "${a[@]^^}"'
check 'empty-element'         'a=(foo "" bar); printf "[%s]\n" "${a[@]^^}"'
check 'single-element'        'a=(foo); echo "${a[@]^^}"'
check 'field-discipline'      'a=(foo bar); for x in "${a[@]^^}"; do echo "<$x>"; done'

if [ $FAIL -ne 0 ]; then
    echo "array_modifiers_diff_check FAILED" >&2
    exit 1
fi
echo "array_modifiers_diff_check OK"
HARNESS_EOF
chmod +x tests/scripts/array_modifiers_diff_check.sh
```

- [ ] **Step 2: Run the harness**

```bash
bash tests/scripts/array_modifiers_diff_check.sh
```

Expected: all ~23 checks PASS.

If any check fails because of a real divergence:
- Investigate the specific fragment.
- If it's a scope-cut item (`@P` machine-dependence, `@Q` non-ASCII byte handling, assoc iteration order), adjust the fragment to avoid the divergence (the assoc fragments already use `| sort` to dodge L-44).
- If it's a real correctness bug in v209's wiring, fix in Task 2 or 3.

The assoc tests use `| sort` to be order-agnostic (L-44 intentional divergence). If any other test produces hash-dependent output, add the same dodge.

- [ ] **Step 3: Run full suite + clippy**

```bash
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: green; clippy clean.

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/array_modifiers_diff_check.sh
git commit -m "$(cat <<'EOF'
v209 task 4: bash-diff harness for array per-element modifiers

23 fragments across Case (8), prefix/suffix/substitute (6), per-element
Transform (5), and edge cases (4). Asserts byte-identical stdout between
bash -c and huck -c. Assoc fragments pipe through sort to dodge the L-44
insertion-vs-hash-order divergence. @P excluded (machine-dependent prompt
expansion); ANSI-C @E byte-faithful cases excluded (L-11 char-vs-byte).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Explicit rejection regression tests

**Files:**
- Modify: `crates/huck-engine/src/expand.rs::mod tests` — pin the remaining catchall behavior so a future refactor doesn't accidentally over-generalize.

- [ ] **Step 1: Add rejection tests to `mod tests`**

Append to the same `#[cfg(test)] mod tests` block in `expand.rs`:

```rust
// ===== v209: pin remaining "not supported on array" catchall =====

#[test]
fn assign_default_on_array_still_errors() {
    use crate::shell_state::Shell;
    use crate::lexer::{ParamModifier as PM, SubscriptKind as SK, Word, WordPart};
    use crate::param_expansion::ExpansionResult;
    let mut shell = Shell::new();
    shell.set_indexed_element("a", 0, "foo".to_string()).unwrap();
    let word = Word(vec![WordPart::Literal { text: "default".into(), quoted: false }]);
    let result = expand_array_param(
        "a",
        &PM::AssignDefault { word, colon: true },
        &SK::All,
        true,
        &mut shell,
    );
    // Catchall returns Value(""). Bash also errors on `${a[@]:=word}`.
    match result {
        ExpansionResult::Value(v) => assert_eq!(v, ""),
        other => panic!("expected empty Value (catchall rejection), got {other:?}"),
    }
}

#[test]
fn error_if_unset_on_array_still_errors() {
    use crate::shell_state::Shell;
    use crate::lexer::{ParamModifier as PM, SubscriptKind as SK, Word, WordPart};
    use crate::param_expansion::ExpansionResult;
    let mut shell = Shell::new();
    let word = Word(vec![WordPart::Literal { text: "msg".into(), quoted: false }]);
    let result = expand_array_param(
        "a",
        &PM::ErrorIfUnset { word, colon: true },
        &SK::All,
        true,
        &mut shell,
    );
    // Catchall returns Value(""); deferred follow-on if real friction.
    match result {
        ExpansionResult::Value(v) => assert_eq!(v, ""),
        other => panic!("expected empty Value (catchall rejection), got {other:?}"),
    }
}
```

(No test for `${a[@]@A}` because the lexer rejects `@A` BEFORE construction reaches `expand_array_param` — that's M-93's lexer-level work, not v209's.)

- [ ] **Step 2: Run + commit**

```bash
cargo test --workspace --quiet assign_default_on_array error_if_unset_on_array
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 2 new tests pass; full suite green; clippy clean.

```bash
git add crates/huck-engine/src/expand.rs
git commit -m "$(cat <<'EOF'
v209 task 5: pin remaining catchall behavior with rejection tests

Two regression tests assert that ${a[@]:=word} (AssignDefault — bash errors
too) and ${a[@]:?word} (ErrorIfUnset — deferred follow-on) continue to
return empty Value through the catchall, so a future is_per_element_modifier
over-generalization is caught. No test for @A — the lexer rejects it before
expand reaches; that's M-93 territory.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Update bash-divergences.md + final verify

**Files:**
- Modify: `docs/bash-divergences.md` — delete the M-127 entry.

- [ ] **Step 1: Delete the M-127 entry**

Find the entry (around line 57 in the "Parameter expansion modifiers" section):

```bash
grep -n 'M-127' docs/bash-divergences.md
```

Delete the entire `- **M-127: ...` paragraph (one bullet item). Use Edit to remove the lines.

The pre-edit line reads (approximately):
```
- **M-127: case modification on a whole array (`${a[@]^^}` / `${a[@],,}` / `${a[@]^}` / `${a[@],}`)** — `[deferred]` low (found in the v157 runtime sweep, batch 4). huck errors `${a[…]}: modifier Case { … } not supported on array in v71` when a case-modification modifier (`^^`/`,,`/`^`/`,`, with or without a pattern) is applied to the `[@]`/`[*]` form. bash applies the case fold to EVERY element (`a=(foo bar); echo "${a[@]^^}"` → `FOO BAR`). The per-element form `${a[1]^^}` and the scalar form `${v^^}` both already work (v37), so the case-fold machinery exists — the gap is that the array-iteration path in the modifier dispatch doesn't map the `Case` modifier over each element the way it does for substring/replace. Fix: in the `[@]`/`[*]` modifier branch, apply `case_modify` per element (mirroring how the other per-element modifiers are handled). The same v71 "not supported on array" guard also rejects a few other modifiers on `[@]`; only `Case` was observed in the sweep.
```

After this iteration, the broader sweep covers Case, RemovePrefix, RemoveSuffix, Substitute, and per-element Transform on both indexed and associative arrays — all per-element forms are now bash-correct.

Per docs/bash-divergences.md's "current-divergences-only" policy: DELETE the M-127 entry entirely. Do NOT flip it to `[fixed v209]`.

Per the spec's Out-of-scope list, also UPDATE the doc summary table (if the M-127 entry was counted in the Tier 2 count). Find the summary table at the top:

```bash
grep -n 'Tier 2' docs/bash-divergences.md | head -3
```

The Tier 2 row was 13 entries (after v206 removed L-25 from Tier 4). After v209 deletes M-127, Tier 2 is 12. Update the count cell.

- [ ] **Step 2: Final full-suite + harness sweep**

```bash
cargo test --workspace --quiet
cargo test --workspace --doc --quiet
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --workspace --quiet

bash tests/scripts/array_modifiers_diff_check.sh

# All existing harnesses:
for h in tests/scripts/*_diff_check.sh; do
    bash "$h" > /tmp/h.out 2>&1
    if [ $? -ne 0 ]; then
        echo "FAIL: $h"
        tail -10 /tmp/h.out
    fi
done

# Headless CLI smoke:
./target/release/huck -c 'echo hello'
echo "exit=$?"
```

Expected: all green; release binary builds; 129 harnesses (128 existing + new array_modifiers) pass; smoke test prints `hello` and `exit=0`.

- [ ] **Step 3: Commit**

```bash
git add docs/bash-divergences.md
git commit -m "$(cat <<'EOF'
v209 task 6: remove M-127 from bash-divergences.md

M-127 (case modification on whole arrays) is fully resolved by v209's
per-element arm in expand_array_param + expand_assoc_param. The broader
sweep also covers RemovePrefix, RemoveSuffix, Substitute, and per-element
Transform on whole arrays. Per current-divergences-only policy, the M-127
entry is removed entirely (history lives in git + iteration memory).
Tier 2 count updated from 13 to 12.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 4: Stop — do NOT merge**

The final whole-branch code review is the controller's call after Task 6. Stop after this commit.

---

## Self-review

**Spec coverage:**
- `is_per_element_modifier` predicate: Task 1 (defined) + Task 2 (allow-removed).
- `scalar_apply_per_element` helper: Task 1 + Task 2.
- Per-element arm in `expand_array_param`: Task 2.
- Per-element arm in `expand_assoc_param`: Task 3.
- Bash-diff harness with ~24 fragments: Task 4.
- Rejection regression tests for `${a[@]:=word}` / `${a[@]:?word}`: Task 5.
- Delete M-127 entry: Task 6.
- Final verify + smoke: Task 6.

**Placeholder scan:** No "TBD"/"implement later". Each code block is complete enough to type-check. The `crate::lexer::Word`/`WordPart`/`SubstAnchor` paths in tests rely on huck-syntax — the implementer is told to read existing test imports for the canonical paths.

**Type consistency:**
- `is_per_element_modifier(&ParamModifier) -> bool` consistent in Tasks 1, 2, 3.
- `scalar_apply_per_element(name, modifier, element, quoted, shell) -> String` consistent in Tasks 1, 2, 3.
- New match arm shape `(modif, SK::All | SK::Star) if is_per_element_modifier(modif)` consistent in Tasks 2 and 3.
- Quoted-`[@]` → WordList; everything else → joined Value: consistent.

**6 tasks. ~30 LOC of production logic + ~150 LOC of tests + ~80 LOC of harness.** Comparable to v208 in scope. The work is concentrated in `expand.rs` with no new modules.
