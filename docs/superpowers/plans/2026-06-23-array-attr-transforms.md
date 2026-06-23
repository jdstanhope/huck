# v210 whole-array `${var@OP}` attribute transforms — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the four `${var@OP}` operators (`@A`/`@K`/`@k`/`@a`) on scalars, indexed arrays, and associative arrays, resolving M-93. Also fix L-44 (a)+(b) — bareword subscript keys when safe + trailing space on assoc body — via a shared `declare`-body renderer reused by `declare -p`, bare `declare`, and `@A`.

**Architecture:** 4 new `TransformOp` variants in `huck-syntax`. New module `array_transforms.rs` in `huck-engine` with four functions (`assign_decl` / `kv_string` / `kv_words` / `attr_flags`), each taking `(name, ScopeMode, &Shell)`. Predicate split in `expand.rs` discriminates per-element vs whole-array Transform ops. New whole-array match arms in `expand_array_param` + `expand_assoc_param` route the 4 ops via `array_transforms::*`. Scalar dispatch in `param_expansion.rs` extends the existing `Transform { op }` arm with 4 new sub-cases that also call into `array_transforms::*` with `ScopeMode::ScalarOrElement(value)`.

**Tech Stack:** Rust 2021, no new crate deps. Builds on v209's per-element infrastructure (Tasks 2-3 of v209 added the `is_per_element_modifier` predicate + the per-element arm — v210 refines the predicate and adds a sibling whole-array arm).

**Branch:** `v210-array-attr-transforms`. Each task ends with a green-suite commit.

**Spec:** `docs/superpowers/specs/2026-06-23-array-attr-transforms-design.md`.

**Key context — current code shapes** (verified pre-plan):
- `crates/huck-syntax/src/lexer.rs:147` — `TransformOp` enum (6 variants today).
- `crates/huck-syntax/src/lexer.rs:3459-3475` — `@`-arm in parse, returns `InvalidBraceModifier` for `A`/`K`/`k`/`a`.
- `crates/huck-syntax/src/generate.rs:694-700` — `TransformOp` round-trip table.
- `crates/huck-engine/src/shell_state.rs:38-58` — `VarValue::{Scalar, Indexed, Associative}` with BTreeMap (sorted by usize) for Indexed and `Vec<(String, String)>` (insertion order) for Associative.
- `crates/huck-engine/src/shell_state.rs:69-90` — `Variable { value, exported, readonly, integer, case_fold, nameref }`. No `traced` field.
- `crates/huck-engine/src/builtins.rs:797-810` — `declare_scalar_quote(v)` (scalar quoter: empty→bare, control→ANSI-C, metas→single-quoted with `'\''` rewrite, plain→bare).
- `crates/huck-engine/src/builtins.rs:815-853` — `format_declare_line(name, var)` ("declare -X name=…" form for `declare -p` / `export -p`).
- `crates/huck-engine/src/builtins.rs:858-890` — `render_declare_value_part(var)` (the `=<value>` body suffix; currently quotes subscript keys ALWAYS via `escape_double_quote_value` + omits trailing space — this is L-44's home).
- `crates/huck-engine/src/builtins.rs:896-910` — `format_declare_bare_line(name, var)` (bare `declare` no-args output).
- `crates/huck-engine/src/expand.rs` (v209) — `is_per_element_modifier` predicate (line ~281) currently matches `Transform { .. }` for ALL variants; will refine to discriminate.
- `crates/huck-engine/src/param_expansion.rs:240-269` — scalar `Transform { op }` arm; reads value via `lookup_v` then applies the per-op transform.

---

## File structure

**Modify:**
- `crates/huck-syntax/src/lexer.rs` — extend `TransformOp` with 4 variants, parse them.
- `crates/huck-syntax/src/generate.rs` — round-trip the 4 new variants.
- `crates/huck-engine/src/shell_state.rs` — add `pub(crate) fn get_var(&self, name: &str) -> Option<&Variable>`.
- `crates/huck-engine/src/builtins.rs` — refactor `render_declare_value_part` to use bareword keys + trailing space on assoc (L-44 a+b fix).
- `crates/huck-engine/src/expand.rs` — predicate split + whole-array match arm in `expand_array_param` and `expand_assoc_param`.
- `crates/huck-engine/src/param_expansion.rs` — extend `Transform { op }` arm with 4 new sub-cases.
- `docs/bash-divergences.md` — delete M-93; shrink L-44 to ordering residual.
- `docs/architecture.md` — one-sentence pointer to `array_transforms.rs`.

**Create:**
- `crates/huck-engine/src/array_transforms.rs` — new module, ~150 LOC.
- `tests/scripts/array_transforms_diff_check.sh` — bash-diff harness, ~25 fragments.

No public API changes; no new crates; no architecture impact.

---

## Task 1: Lexer — add 4 new `TransformOp` variants + parse + round-trip

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — extend `TransformOp` enum + parse match.
- Modify: `crates/huck-syntax/src/generate.rs` — extend the TransformOp letter table.

- [ ] **Step 1: Create the branch**

```bash
git checkout main
git pull --ff-only
git checkout -b v210-array-attr-transforms
```

- [ ] **Step 2: Extend `TransformOp` enum**

Find the enum at `crates/huck-syntax/src/lexer.rs:147`:

```bash
grep -n "pub enum TransformOp" crates/huck-syntax/src/lexer.rs
```

Replace the enum with:

```rust
/// Scalar and whole-array `${var@OP}` transform operators (bash 5.x).
/// Per-element across arrays via v209's per-element arm; whole-array
/// via v210's whole-array arm; scalar via the param_expansion path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformOp {
    PromptExpand, // @P — prompt-string expansion of the value
    Quote,        // @Q — shell-quote so the result re-reads as the same value
    Upper,        // @U — uppercase all
    Lower,        // @L — lowercase all
    UpperFirst,   // @u — uppercase first char
    EscapeExpand, // @E — expand backslash escapes ($'...' style)
    AssignDecl,   // @A — declare-style assignment string
    KvString,     // @K — k/v pairs as one quoted-internally string
    KvWords,      // @k — k/v pairs as word list
    AttrFlags,    // @a — attribute flag letters
}
```

(Keep the `#[derive(...)]` line that was on the original enum.)

- [ ] **Step 3: Extend the parse match**

Find the `@`-arm at `crates/huck-syntax/src/lexer.rs:3459`:

```bash
grep -n "Some('P') => TransformOp::PromptExpand" crates/huck-syntax/src/lexer.rs
```

Add 4 new arms BEFORE the `other =>` catchall:

```rust
                Some('A') => TransformOp::AssignDecl,
                Some('K') => TransformOp::KvString,
                Some('k') => TransformOp::KvWords,
                Some('a') => TransformOp::AttrFlags,
```

Update the catchall comment to drop the "@A/@K/@k/@a (deferred)" reference:

```rust
                other => {
                    // Unknown letter — bad substitution.
                    return Err(LexError::InvalidBraceModifier(format!(
```

- [ ] **Step 4: Extend `generate.rs` round-trip**

Find the TransformOp table at `crates/huck-syntax/src/generate.rs:694`:

```bash
grep -n "TransformOp::PromptExpand => 'P'" crates/huck-syntax/src/generate.rs
```

Add 4 new entries:

```rust
                TransformOp::AssignDecl => 'A',
                TransformOp::KvString => 'K',
                TransformOp::KvWords => 'k',
                TransformOp::AttrFlags => 'a',
```

- [ ] **Step 5: Extend lexer unit tests**

Find the existing TransformOp parse-roundtrip test at `crates/huck-syntax/src/lexer.rs:8059`:

```bash
grep -n '"\${v@P}", TransformOp::PromptExpand' crates/huck-syntax/src/lexer.rs
```

The test is a table of `(source, expected_variant)` pairs. Add 4 new entries:

```rust
            ("${v@A}", TransformOp::AssignDecl),
            ("${v@K}", TransformOp::KvString),
            ("${v@k}", TransformOp::KvWords),
            ("${v@a}", TransformOp::AttrFlags),
```

- [ ] **Step 6: Build + run tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: full suite green; clippy clean; new lexer test entries pass.

- [ ] **Step 7: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/generate.rs
git commit -m "$(cat <<'EOF'
v210 task 1: lexer + generate support for @A/@K/@k/@a transforms

Extends TransformOp with 4 new variants (AssignDecl/KvString/KvWords/AttrFlags),
parses them at the @-arm, and round-trips them via generate.rs. No expansion
wiring yet — those calls hit param_expansion's existing Transform arm which
panics on the unmatched variants (Tasks 5-8 wire them up). The dead-coding is
fine for the implementation order: Tasks 2-7 build the wiring bottom-up; only
Task 7 makes the new ops reachable via expansion.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

Note: between Task 1 and Task 8, the param_expansion `Transform { op }` arm's `match op` is non-exhaustive — Rust will refuse to compile. Add a temporary `_ => String::new(),` catchall to the match in Task 1's edit, OR mark the match as having a catchall added in Task 8 with a TODO comment. Use the catchall approach: in `crates/huck-engine/src/param_expansion.rs` find the match starting at line 242 and add at the end of the match (before the closing brace):

```rust
                _ => String::new(),
```

Include this edit in the Task 1 commit so the suite stays green.

---

## Task 2: `Shell::get_var` accessor

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` — add `pub(crate) fn get_var`.

- [ ] **Step 1: Locate the existing `lookup_var` and `iter_vars` accessors**

```bash
grep -n "pub fn lookup_var\|pub fn iter_vars" crates/huck-engine/src/shell_state.rs
```

`lookup_var` returns `Option<String>` (scalar view). `iter_vars` returns `(&String, &Variable)`. v210 needs a single-name `&Variable` accessor — for `array_transforms` to inspect attribute flags and the value shape.

- [ ] **Step 2: Add the accessor**

Insert near `lookup_var` (just below it works):

```rust
    /// Return the raw `Variable` (value + attribute flags) for `name`,
    /// following nameref chains. Used by v210's `array_transforms` to
    /// read the var kind (`Scalar`/`Indexed`/`Associative`) and the
    /// per-var attribute flags (`exported`/`readonly`/`integer`/etc.)
    /// when rendering `${var@A}` / `${var@K}` / `${var@k}` / `${var@a}`.
    /// Returns `None` for an unset variable.
    pub(crate) fn get_var(&self, name: &str) -> Option<&Variable> {
        let resolved = if self.is_nameref(name) {
            // Follow the nameref chain; preserve final-target lookup
            // semantics matching lookup_var.
            match self.resolve_nameref(name) {
                ResolvedName::Name(n) => n,
                ResolvedName::Element { .. } => name.to_string(),
            }
        } else {
            name.to_string()
        };
        self.vars.get(&resolved)
    }
```

If `resolve_nameref` signature differs from what's sketched (return type or argument shape), adapt — the goal is "follow the nameref to the final var name, then `self.vars.get(...)`". Read `shell_state.rs::lookup_var` for the canonical nameref-follow shape and mirror it.

- [ ] **Step 3: Build + run tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean. `#[allow(dead_code)]` is NOT needed because the function will be called in Task 3+; if the suite complains about unused, add `#[allow(dead_code)]` and remove it in Task 3.

- [ ] **Step 4: Commit**

```bash
git add crates/huck-engine/src/shell_state.rs
git commit -m "$(cat <<'EOF'
v210 task 2: Shell::get_var accessor for full Variable lookup

Adds pub(crate) fn get_var(name) -> Option<&Variable> that follows
namerefs and returns the underlying Variable. Used by Tasks 3-6's
array_transforms module to inspect both value shape (Scalar/Indexed/
Associative) and attribute flags (exported/readonly/integer/case_fold/
nameref) when rendering @A/@K/@k/@a output.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `array_transforms.rs` module scaffold

**Files:**
- Create: `crates/huck-engine/src/array_transforms.rs`
- Modify: `crates/huck-engine/src/lib.rs` (or wherever `mod` declarations live) — register the new module.

- [ ] **Step 1: Find where modules are declared**

```bash
grep -n "^mod \|^pub mod \|^pub(crate) mod " crates/huck-engine/src/lib.rs
```

- [ ] **Step 2: Create `array_transforms.rs` with the scaffold**

```rust
// crates/huck-engine/src/array_transforms.rs

//! Whole-array and scalar `${var@OP}` transform implementations for
//! `@A` (declare-style assignment), `@K` (k/v pairs string), `@k` (k/v
//! pairs word list), and `@a` (attribute flag letters). Called from
//! `expand_array_param` / `expand_assoc_param` for the whole-array
//! forms and from `param_expansion::expand_modifier_with_value`'s
//! `Transform { op }` arm for the scalar / single-element forms.

use crate::shell_state::Shell;

/// Where the modifier was applied: a whole array (`[@]` / `[*]`
/// subscript) or a single value (scalar variable, no subscript, or
/// specific `[i]` subscript).
pub(crate) enum ScopeMode {
    /// `[@]` or `[*]` — operate on the whole array's key/value pairs.
    Whole,
    /// Scalar variable, no subscript, or specific `[i]` — operate on
    /// a single resolved value. Carries the value so callers don't
    /// re-look-it-up. For `${arr[0]@A}` this carries the value of
    /// `arr[0]`; for `${arr@A}` it carries the scalar view (which is
    /// `arr[0]` for indexed, empty for associative).
    ScalarOrElement(String),
}

/// `${var@A}` — declare-style assignment string.
pub(crate) fn assign_decl(_name: &str, _scope: ScopeMode, _shell: &Shell) -> String {
    // Implementation in Task 5.
    String::new()
}

/// `${var@K}` — k/v pairs as a single quoted-internally string.
pub(crate) fn kv_string(_name: &str, _scope: ScopeMode, _shell: &Shell) -> String {
    // Implementation in Task 6.
    String::new()
}

/// `${var@k}` — k/v pairs as a word list (each k and v a separate
/// field when used under quoted `[@]`).
pub(crate) fn kv_words(_name: &str, _scope: ScopeMode, _shell: &Shell) -> Vec<String> {
    // Implementation in Task 6.
    Vec::new()
}

/// `${var@a}` — attribute flag letters in canonical order, or empty.
pub(crate) fn attr_flags(_name: &str, _shell: &Shell) -> String {
    // Implementation in Task 6.
    String::new()
}
```

- [ ] **Step 3: Register the module**

In `crates/huck-engine/src/lib.rs` (or wherever, per Step 1's grep), add:

```rust
mod array_transforms;
```

(Match the surrounding `mod` declaration style — `pub(crate) mod` if that's what the file uses.)

- [ ] **Step 4: Build + run tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean build; suite green; clippy clean. `_name` / `_scope` / `_shell` prefixes suppress unused warnings; the underscore prefix style is preferred over `#[allow(unused)]`. If clippy still complains about an unused function, add `#[allow(dead_code)]` to it temporarily (Task 5-6 remove).

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/array_transforms.rs crates/huck-engine/src/lib.rs
git commit -m "$(cat <<'EOF'
v210 task 3: array_transforms.rs module scaffold

New module with four pub(crate) entry points (assign_decl/kv_string/
kv_words/attr_flags) and ScopeMode { Whole, ScalarOrElement(String) }.
All functions stub-return empty; Tasks 5-6 implement the bodies.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: L-44 renderer cleanup — bareword keys + trailing space

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` — refactor `render_declare_value_part`.

This task is INDEPENDENT of the new ops; it's the L-44 (a)+(b) fix. It's intentionally done BEFORE Task 5 so `assign_decl` can use the corrected renderer directly.

- [ ] **Step 1: Find `render_declare_value_part`**

```bash
grep -n "fn render_declare_value_part" crates/huck-engine/src/builtins.rs
```

Should hit line ~858. The current function (lines 858-890) renders:
- Scalars → `="<value>"` (double-quoted with `escape_double_quote_value`).
- Indexed arrays → `=([<k>]="<v>" [<k2>]="<v2>")` (no trailing space).
- Assoc arrays → `=(["<k>"]="<v>" ["<k2>"]="<v2>")` (double-quoted KEYS, no trailing space).

After v210:
- Scalars → unchanged.
- Indexed → unchanged (no trailing space).
- Assoc → keys BAREWORD when `^[A-Za-z0-9_-]+$`, else double-quoted; body has TRAILING SPACE before `)`.

- [ ] **Step 2: Add a helper `quote_subscript_key`**

Add immediately above `render_declare_value_part` (so it's local context-private):

```rust
/// Renders an associative-array subscript key for `declare`-style
/// output. Bash uses bareword when the key matches `[A-Za-z0-9_-]+`
/// (covers identifiers, integers including negative, dashed words);
/// otherwise double-quoted with `\$`/`\\`/`\"`/`` \` `` escapes
/// (same policy as values inside `(…)`). Resolves L-44(a).
fn quote_subscript_key(k: &str) -> String {
    if !k.is_empty() && k.bytes().all(|b| {
        matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-')
    }) {
        k.to_string()
    } else {
        format!("\"{}\"", crate::escape_double_quote_value(k))
    }
}
```

- [ ] **Step 3: Update the `Associative` arm of `render_declare_value_part`**

Replace the `VarValue::Associative(pairs)` arm with:

```rust
        VarValue::Associative(pairs) => {
            let parts: Vec<String> = pairs
                .iter()
                .map(|(k, v)| {
                    format!(
                        "[{}]=\"{}\"",
                        quote_subscript_key(k),
                        crate::escape_double_quote_value(v)
                    )
                })
                .collect();
            if parts.is_empty() {
                "=()".to_string()
            } else {
                // Bash assoc body has a trailing space before `)`.
                // Indexed body does NOT (L-44 (b) quirk).
                format!("=({} )", parts.join(" "))
            }
        }
```

Indexed arm stays unchanged (no trailing space).

- [ ] **Step 4: Update existing unit tests that captured the OLD assoc format**

Run the test suite once to discover any failing tests:

```bash
cargo test --workspace --quiet 2>&1 | grep -E '^(test|---- )' | head -50
```

For each failing test that compared assoc output to the OLD form (`["k"]=` always quoted, no trailing space), update the expected output:
- `["foo"]="bar"` → `[foo]="bar"` for plain identifier keys.
- `[<single-item-body>])` → `[<single-item-body>] )` for assoc.

If multiple tests need this, do them in one pass. Don't add new tests yet — Task 5's unit tests cover the new policy directly.

- [ ] **Step 5: Add 3 unit tests for the renderer policy**

Find the existing `mod tests` block in `builtins.rs`:

```bash
grep -n "#\[cfg(test)\]" crates/huck-engine/src/builtins.rs | head -3
```

Append (adjust imports based on the surrounding tests in the file):

```rust
    #[test]
    fn assoc_key_bareword_for_identifier() {
        use crate::shell_state::{Variable, VarValue};
        let var = Variable {
            value: VarValue::Associative(vec![("foo".into(), "v".into())]),
            exported: false,
            readonly: false,
            integer: false,
            case_fold: None,
            nameref: false,
        };
        let out = render_declare_value_part(&var);
        // Bareword key, trailing space before `)`.
        assert_eq!(out, r#"=([foo]="v" )"#);
    }

    #[test]
    fn assoc_key_quoted_for_metachar() {
        use crate::shell_state::{Variable, VarValue};
        let var = Variable {
            value: VarValue::Associative(vec![("a b".into(), "v".into())]),
            exported: false,
            readonly: false,
            integer: false,
            case_fold: None,
            nameref: false,
        };
        let out = render_declare_value_part(&var);
        // Quoted key (has space).
        assert_eq!(out, r#"=(["a b"]="v" )"#);
    }

    #[test]
    fn indexed_has_no_trailing_space() {
        use std::collections::BTreeMap;
        use crate::shell_state::{Variable, VarValue};
        let mut m = BTreeMap::new();
        m.insert(0usize, "x".to_string());
        m.insert(1usize, "y".to_string());
        let var = Variable {
            value: VarValue::Indexed(m),
            exported: false,
            readonly: false,
            integer: false,
            case_fold: None,
            nameref: false,
        };
        let out = render_declare_value_part(&var);
        // No trailing space before `)`.
        assert_eq!(out, r#"=([0]="x" [1]="y")"#);
    }
```

- [ ] **Step 6: Build + run tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean; 3 new tests pass; any updated existing tests pass.

- [ ] **Step 7: Run existing bash-diff harnesses that exercise `declare -p` / bare `declare`**

```bash
ls tests/scripts/ | grep -E 'declare|export' | xargs -I{} bash tests/scripts/{} || true
```

These were previously passing-by-coincidence on huck's old assoc output. After v210 they should pass-by-correctness against bash. If any FAIL because the harness had captured huck's old output verbatim, update the harness — flag the change in the commit.

If any FAIL because the harness asserts hash-order-sensitive output, dodge with `| sort` (L-44 (c) is still open).

- [ ] **Step 8: Commit**

```bash
git add crates/huck-engine/src/builtins.rs tests/scripts/
git commit -m "$(cat <<'EOF'
v210 task 4: L-44 (a)+(b) — bareword assoc keys + trailing space

render_declare_value_part now uses bareword subscript keys when they
match ^[A-Za-z0-9_-]+$ (identifiers, integers including negative,
dashed identifiers) — matches bash. Assoc body grows a trailing space
before `)` to match bash's well-known inconsistency vs indexed body.
Three new unit tests pin the policy. Existing harnesses + tests that
captured the OLD assoc form updated.

L-44 (a) and (b) are now closed; L-44 (c) — insertion vs hash order —
remains as the only residual.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: `assign_decl` implementation

**Files:**
- Modify: `crates/huck-engine/src/array_transforms.rs` — fill in `assign_decl`.

The dispatch table from the spec:

| Subject                                  | Output                                                |
|------------------------------------------|-------------------------------------------------------|
| unset                                    | empty                                                 |
| scalar no attrs `s=hello`                | `s='hello'`                                           |
| attributed scalar `declare -x ev=42`     | `declare -x ev='42'`                                  |
| scalar control chars                     | `s=$'…'` (ANSI-C form)                                |
| scalar with `'` quote                    | `q='it'\''s'`                                         |
| `${arr}` / `${arr[i]}` indexed           | `declare -a arr='value-of-[0]-or-[i]'`                |
| `${arr[@]}` indexed                      | `declare -a arr=([0]="x" [1]="y" [2]="z")`            |
| `${m}` / `${m[k]}` assoc                 | `declare -A m` (no body)                              |
| `${m[@]}` assoc                          | `declare -A m=([k]="v1" [j]="v2" )` (trailing space)  |
| empty indexed / assoc                    | `declare -[aA] name=()`                               |

- [ ] **Step 1: Update the `use` block in `array_transforms.rs`**

Replace the `use` line with:

```rust
use crate::shell_state::{Shell, Variable, VarValue};
```

- [ ] **Step 2: Implement `assign_decl`**

Replace the stub `assign_decl` with:

```rust
pub(crate) fn assign_decl(name: &str, scope: ScopeMode, shell: &Shell) -> String {
    let Some(var) = shell.get_var(name) else {
        return String::new();
    };
    match scope {
        ScopeMode::Whole => assign_decl_whole(name, var),
        ScopeMode::ScalarOrElement(val) => assign_decl_scalar_or_element(name, var, &val),
    }
}

/// Whole-array form: full `declare -[aA] name=(…)` reusing the
/// shared `format_declare_line` renderer (post-Task 4: with bareword
/// keys + trailing space on assoc).
fn assign_decl_whole(name: &str, var: &Variable) -> String {
    // `format_declare_line` already emits the full line including
    // attribute flags + value body. That's exactly the `@A` output.
    crate::builtins::format_declare_line_for_render(name, var)
}

/// Scalar / single-element form:
///   - plain scalar no attrs           → `name='value'`
///   - attributed scalar               → `declare -X name='value'`
///   - indexed array (no sub or [i])   → `declare -a name='value'`
///   - associative (no sub or [k])     → `declare -A name` (no body)
///   - unset                           → empty (handled by caller)
fn assign_decl_scalar_or_element(name: &str, var: &Variable, val: &str) -> String {
    use crate::shell_state::VarValue::*;
    let quoted_val = crate::builtins::declare_scalar_quote_for_render(val);
    let has_attrs = var.exported || var.readonly || var.integer
        || var.case_fold.is_some() || var.nameref;
    match &var.value {
        Scalar(_) => {
            if has_attrs {
                let attrs = render_attr_prefix(var, /*include_kind=*/false);
                format!("declare {attrs} {name}='{}'", crate::builtins::escape_alias_value(val))
            } else {
                format!("{name}={quoted_val}")
            }
        }
        Indexed(_) => {
            let attrs = render_attr_prefix(var, /*include_kind=*/true);
            format!("declare {attrs} {name}='{}'", crate::builtins::escape_alias_value(val))
        }
        Associative(_) => {
            // `${m}` on assoc → scalar_view is empty → `declare -A m`
            // with no body. Attribute flags still appear.
            let attrs = render_attr_prefix(var, /*include_kind=*/true);
            format!("declare {attrs} {name}")
        }
    }
}

/// Builds the `-[aAirxlu]+` prefix (without the leading `declare `
/// keyword). `include_kind=true` adds `a`/`A` for array/assoc; for
/// scalars use `include_kind=false`. Matches the order in
/// `format_declare_line`: `n`, `a`/`A`, `i`, `r`, `x`, `l`/`u`.
fn render_attr_prefix(var: &Variable, include_kind: bool) -> String {
    let mut flags = String::new();
    if var.nameref { flags.push('n'); }
    if include_kind {
        match &var.value {
            VarValue::Indexed(_) => flags.push('a'),
            VarValue::Associative(_) => flags.push('A'),
            _ => {}
        }
    }
    if var.integer { flags.push('i'); }
    if var.readonly { flags.push('r'); }
    if var.exported { flags.push('x'); }
    match var.case_fold {
        Some(crate::shell_state::CaseFold::Lower) => flags.push('l'),
        Some(crate::shell_state::CaseFold::Upper) => flags.push('u'),
        None => {}
    }
    if flags.is_empty() {
        "--".to_string()
    } else {
        format!("-{flags}")
    }
}
```

- [ ] **Step 3: Expose `format_declare_line` and `declare_scalar_quote` as pub(crate)**

In `crates/huck-engine/src/builtins.rs`, rename or alias the two private helpers to be reachable from `array_transforms`:

```bash
grep -n "fn format_declare_line\|fn declare_scalar_quote" crates/huck-engine/src/builtins.rs
```

Either:
- (A) Change `fn format_declare_line(...)` to `pub(crate) fn format_declare_line(...)`, then in `array_transforms` call `crate::builtins::format_declare_line(name, var)` directly. Drop the `_for_render` alias from Step 2's code.
- (B) Add thin `pub(crate)` wrappers `format_declare_line_for_render` and `declare_scalar_quote_for_render` next to the originals that just delegate.

Prefer (A) — fewer indirections. Update Step 2's calls to:

```rust
    crate::builtins::format_declare_line(name, var)
```

and:

```rust
    let quoted_val = crate::builtins::declare_scalar_quote(val);
```

Also expose `escape_alias_value` if not already (it's used in the literal single-quote rewrite path). Verify:

```bash
grep -n "fn escape_alias_value\|pub(crate) fn escape_alias_value" crates/huck-engine/src/builtins.rs
```

If it's private, mark it `pub(crate)`.

- [ ] **Step 4: Add `assign_decl` unit tests**

In `array_transforms.rs`, append a `mod tests` block at the end:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell_state::{Shell, VarValue};

    #[test]
    fn assign_decl_scalar_no_attrs() {
        let mut shell = Shell::new();
        shell.set("s", "hello".to_string()).unwrap();
        let out = assign_decl("s", ScopeMode::ScalarOrElement("hello".into()), &shell);
        assert_eq!(out, "s='hello'");
    }

    #[test]
    fn assign_decl_exported_scalar() {
        let mut shell = Shell::new();
        shell.set("ev", "42".to_string()).unwrap();
        shell.export("ev");
        let out = assign_decl("ev", ScopeMode::ScalarOrElement("42".into()), &shell);
        assert_eq!(out, "declare -x ev='42'");
    }

    #[test]
    fn assign_decl_unset_is_empty() {
        let shell = Shell::new();
        let out = assign_decl("nope", ScopeMode::ScalarOrElement(String::new()), &shell);
        assert_eq!(out, "");
    }

    #[test]
    fn assign_decl_indexed_whole_uses_renderer() {
        let mut shell = Shell::new();
        shell.set_indexed_element("a", 0, "x".to_string()).unwrap();
        shell.set_indexed_element("a", 1, "y".to_string()).unwrap();
        let out = assign_decl("a", ScopeMode::Whole, &shell);
        assert_eq!(out, r#"declare -a a=([0]="x" [1]="y")"#);
    }

    #[test]
    fn assign_decl_assoc_whole_has_trailing_space() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.set_associative_element("m", "k".to_string(), "v1".to_string()).unwrap();
        let out = assign_decl("m", ScopeMode::Whole, &shell);
        // Single entry; trailing space before `)`.
        assert_eq!(out, r#"declare -A m=([k]="v1" )"#);
    }

    #[test]
    fn assign_decl_assoc_no_subscript_no_body() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.set_associative_element("m", "k".to_string(), "v".to_string()).unwrap();
        // ${m@A} (no subscript) → scalar_view is empty → no body.
        let out = assign_decl("m", ScopeMode::ScalarOrElement(String::new()), &shell);
        assert_eq!(out, "declare -A m");
    }
}
```

If method names (`shell.set`, `shell.mark_exported`, `shell.set_indexed_element`, `shell.declare_associative`, `shell.set_associative_element`) differ from the actual API, adjust — read `shell_state.rs` for the canonical names. Earlier v209 work confirmed `set_indexed_element(name, idx: usize, val: String)`, `declare_associative(name)`, and `set_associative_element(name, key: String, val: String)`. The setter for a plain scalar is likely `set` or `assign`.

- [ ] **Step 5: Build + run tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: clean; 6 new tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/array_transforms.rs crates/huck-engine/src/builtins.rs
git commit -m "$(cat <<'EOF'
v210 task 5: assign_decl + helper plumbing

Implements assign_decl for both Whole (full body via the shared
format_declare_line) and ScalarOrElement (scalar/element form with
correct `declare -X` prefix per var kind+attrs) scopes. Marks the
3 builtins.rs helpers it depends on (format_declare_line,
declare_scalar_quote, escape_alias_value) pub(crate). 6 unit tests
cover scalar no-attrs, exported scalar, unset, indexed whole, assoc
whole with trailing space, and assoc no-subscript with no body.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `kv_string` + `kv_words` + `attr_flags` implementations

**Files:**
- Modify: `crates/huck-engine/src/array_transforms.rs` — fill in the remaining 3 functions.

- [ ] **Step 1: Implement `kv_string`**

The dispatch (from spec):
- `Whole` on indexed → `0 "x" 1 "y" 2 "z"` (bareword keys, double-quoted values, single-space separator; **no trailing space** based on bash probe — but verify against the harness, see Task 9).
- `Whole` on assoc → `k "v1" j "v2" ` (bareword keys when safe, double-quoted values, **trailing space** before final value-end).
- `ScalarOrElement(val)` → `'val'` (single-quoted; use existing `declare_scalar_quote`).
- Unset → empty.

Replace the stub:

```rust
pub(crate) fn kv_string(name: &str, scope: ScopeMode, shell: &Shell) -> String {
    let Some(var) = shell.get_var(name) else {
        return String::new();
    };
    match scope {
        ScopeMode::Whole => kv_string_whole(var),
        ScopeMode::ScalarOrElement(val) => {
            if val.is_empty() {
                String::new()
            } else {
                crate::builtins::declare_scalar_quote(&val)
            }
        }
    }
}

fn kv_string_whole(var: &Variable) -> String {
    use crate::shell_state::VarValue::*;
    match &var.value {
        Indexed(m) => {
            let parts: Vec<String> = m
                .iter()
                .map(|(k, v)| format!("{k} \"{}\"", crate::escape_double_quote_value(v)))
                .collect();
            parts.join(" ")
        }
        Associative(pairs) => {
            let parts: Vec<String> = pairs
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{} \"{}\"",
                        quote_subscript_key_local(k),
                        crate::escape_double_quote_value(v)
                    )
                })
                .collect();
            if parts.is_empty() {
                String::new()
            } else {
                // Bash adds a trailing space after the final value for
                // assoc @K (mirrors the @A assoc body inconsistency).
                format!("{} ", parts.join(" "))
            }
        }
        Scalar(_) => String::new(),
    }
}

/// Local copy of the bareword-subscript-key policy. Could be shared
/// with builtins::quote_subscript_key once that's pub(crate); for now
/// inline to avoid making a 1-call-site helper public.
fn quote_subscript_key_local(k: &str) -> String {
    if !k.is_empty()
        && k.bytes()
            .all(|b| matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
    {
        k.to_string()
    } else {
        format!("\"{}\"", crate::escape_double_quote_value(k))
    }
}
```

If `escape_double_quote_value` is at `crate::` root path (it's referenced that way in builtins.rs), the call works as-is. If it's a `pub(crate)` in a different module, adjust the path.

Note: the bash trailing-space behavior for indexed `@K` is not 100% certain from echo output (which strips trailing whitespace before newline). The bash-diff harness in Task 9 is authoritative — if a fragment fails because of trailing-space mismatch, adjust the `Indexed` arm to add a trailing space too.

- [ ] **Step 2: Implement `kv_words`**

The dispatch:
- `Whole` on indexed `[@]` → `["0", "x", "1", "y", "2", "z"]` (each key and value a separate word; **no quoting** — they're alphabet-style words).
- `Whole` on assoc `[@]` → `["k", "v1", "j", "v2"]` (no quoting; insertion order).
- `ScalarOrElement(val)` → `[declare_scalar_quote(val)]` (same as `kv_string` scalar form, wrapped in a single-element Vec for the WordList shape).
- Unset → `[]`.

Replace the stub:

```rust
pub(crate) fn kv_words(name: &str, scope: ScopeMode, shell: &Shell) -> Vec<String> {
    let Some(var) = shell.get_var(name) else {
        return Vec::new();
    };
    match scope {
        ScopeMode::Whole => kv_words_whole(var),
        ScopeMode::ScalarOrElement(val) => {
            if val.is_empty() {
                Vec::new()
            } else {
                vec![crate::builtins::declare_scalar_quote(&val)]
            }
        }
    }
}

fn kv_words_whole(var: &Variable) -> Vec<String> {
    use crate::shell_state::VarValue::*;
    match &var.value {
        Indexed(m) => {
            let mut out = Vec::with_capacity(m.len() * 2);
            for (k, v) in m {
                out.push(k.to_string());
                out.push(v.clone());
            }
            out
        }
        Associative(pairs) => {
            let mut out = Vec::with_capacity(pairs.len() * 2);
            for (k, v) in pairs {
                out.push(k.clone());
                out.push(v.clone());
            }
            out
        }
        Scalar(_) => Vec::new(),
    }
}
```

- [ ] **Step 3: Implement `attr_flags`**

Letter table (matches `format_declare_line`'s order): `n`, `a`/`A`, `i`, `r`, `x`, `l`/`u`. For an unset variable, empty.

Replace the stub:

```rust
pub(crate) fn attr_flags(name: &str, shell: &Shell) -> String {
    let Some(var) = shell.get_var(name) else {
        return String::new();
    };
    let mut flags = String::new();
    if var.nameref { flags.push('n'); }
    match &var.value {
        VarValue::Indexed(_) => flags.push('a'),
        VarValue::Associative(_) => flags.push('A'),
        VarValue::Scalar(_) => {}
    }
    if var.integer { flags.push('i'); }
    if var.readonly { flags.push('r'); }
    if var.exported { flags.push('x'); }
    match var.case_fold {
        Some(crate::shell_state::CaseFold::Lower) => flags.push('l'),
        Some(crate::shell_state::CaseFold::Upper) => flags.push('u'),
        None => {}
    }
    flags
}
```

- [ ] **Step 4: Add unit tests for the 3 new functions**

Append to the existing `mod tests` block in `array_transforms.rs`:

```rust
    #[test]
    fn kv_string_indexed_whole() {
        let mut shell = Shell::new();
        shell.set_indexed_element("a", 0, "x".to_string()).unwrap();
        shell.set_indexed_element("a", 1, "y".to_string()).unwrap();
        let out = kv_string("a", ScopeMode::Whole, &shell);
        assert_eq!(out, r#"0 "x" 1 "y""#);
    }

    #[test]
    fn kv_string_assoc_whole_has_trailing_space() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.set_associative_element("m", "k".to_string(), "v1".to_string()).unwrap();
        let out = kv_string("m", ScopeMode::Whole, &shell);
        assert_eq!(out, r#"k "v1" "#);
    }

    #[test]
    fn kv_words_indexed_whole_yields_alternating() {
        let mut shell = Shell::new();
        shell.set_indexed_element("a", 0, "x".to_string()).unwrap();
        shell.set_indexed_element("a", 1, "y".to_string()).unwrap();
        let out = kv_words("a", ScopeMode::Whole, &shell);
        assert_eq!(out, vec!["0", "x", "1", "y"]);
    }

    #[test]
    fn kv_words_unset_is_empty() {
        let shell = Shell::new();
        let out = kv_words("nope", ScopeMode::ScalarOrElement(String::new()), &shell);
        assert!(out.is_empty());
    }

    #[test]
    fn attr_flags_indexed_is_a() {
        let mut shell = Shell::new();
        shell.set_indexed_element("a", 0, "x".to_string()).unwrap();
        let out = attr_flags("a", &shell);
        assert_eq!(out, "a");
    }

    #[test]
    fn attr_flags_assoc_is_A() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        let out = attr_flags("m", &shell);
        assert_eq!(out, "A");
    }

    #[test]
    fn attr_flags_scalar_no_attrs_is_empty() {
        let mut shell = Shell::new();
        shell.set("s", "x".to_string()).unwrap();
        let out = attr_flags("s", &shell);
        assert_eq!(out, "");
    }

    #[test]
    fn attr_flags_unset_is_empty() {
        let shell = Shell::new();
        let out = attr_flags("nope", &shell);
        assert_eq!(out, "");
    }

    #[test]
    fn attr_flags_multi() {
        let mut shell = Shell::new();
        shell.set("n", "5".to_string()).unwrap();
        shell.mark_integer("n");
        shell.mark_readonly("n");
        shell.export("n");
        let out = attr_flags("n", &shell);
        // Letter order: n, a/A, i, r, x, l/u → "irx".
        assert_eq!(out, "irx");
    }
```

Verified Shell API: `mark_integer(name)`, `mark_readonly(name)`, `export(name)` (the exported flag setter is named `export`, NOT `mark_exported`). `set(name, value: String)` for plain scalars. If anything else differs, grep `shell_state.rs` for the canonical name.

- [ ] **Step 5: Build + run tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 9 new tests pass; full suite green; clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/array_transforms.rs
git commit -m "$(cat <<'EOF'
v210 task 6: kv_string + kv_words + attr_flags

Three remaining array_transforms functions. kv_string emits bareword
keys + double-quoted values; assoc form has trailing space (matches
@A's inconsistency). kv_words emits alternating key/value words for
WordList output under quoted [@]. attr_flags concatenates flag letters
in canonical order (n,a/A,i,r,x,l/u). 9 unit tests cover each.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Predicate split + whole-array dispatch arm

**Files:**
- Modify: `crates/huck-engine/src/expand.rs` — refine `is_per_element_modifier`, add `is_whole_array_transform_op`, insert whole-array arm in `expand_array_param` and `expand_assoc_param`.

- [ ] **Step 1: Refine `is_per_element_modifier`**

Find it near line ~281:

```bash
grep -n "fn is_per_element_modifier" crates/huck-engine/src/expand.rs
```

Replace the existing function with:

```rust
/// Whether this modifier dispatches to v209's per-element arm. Case /
/// RemovePrefix / RemoveSuffix / Substitute always do; Transform
/// dispatches per-element ONLY for the 6 scalar-style ops (P/Q/U/L/u/E).
/// The 4 whole-array ops (A/K/k/a) route through the v210 whole-array
/// arm; see `is_whole_array_transform_op`.
fn is_per_element_modifier(m: &crate::lexer::ParamModifier) -> bool {
    use crate::lexer::ParamModifier as PM;
    match m {
        PM::Case { .. }
        | PM::RemovePrefix { .. }
        | PM::RemoveSuffix { .. }
        | PM::Substitute { .. } => true,
        PM::Transform { op } => is_per_element_transform_op(*op),
        _ => false,
    }
}

/// `${var@OP}` ops that operate on a single value (per-element when
/// applied across an array): P (prompt-expand), Q (shell-quote),
/// U (upper), L (lower), u (upper-first), E (escape-expand).
fn is_per_element_transform_op(op: crate::lexer::TransformOp) -> bool {
    use crate::lexer::TransformOp::*;
    matches!(op, PromptExpand | Quote | Upper | Lower | UpperFirst | EscapeExpand)
}

/// `${var@OP}` ops that operate on the whole array (KEYS+VALUES or
/// type info): A (declare-style), K (k/v string), k (k/v word list),
/// a (attribute flags).
fn is_whole_array_transform_op(op: crate::lexer::TransformOp) -> bool {
    use crate::lexer::TransformOp::*;
    matches!(op, AssignDecl | KvString | KvWords | AttrFlags)
}
```

- [ ] **Step 2: Add the whole-array arm to `expand_array_param`**

Find the existing per-element arm (added by v209 Task 2) and insert the new whole-array arm IMMEDIATELY AFTER it (still BEFORE the v71 catchall):

```bash
grep -n "is_per_element_modifier(modif)\|not supported on array in v71" crates/huck-engine/src/expand.rs
```

Add this arm after the per-element arm:

```rust
            (crate::lexer::ParamModifier::Transform { op }, sub)
                if is_whole_array_transform_op(*op) =>
            {
                use crate::array_transforms::{self as at, ScopeMode};
                use crate::lexer::SubscriptKind as SK;
                use crate::lexer::TransformOp::*;
                let scope = if matches!(sub, SK::All | SK::Star) {
                    ScopeMode::Whole
                } else {
                    // Specific subscript or no subscript → scalar-or-
                    // element form. For [i], the value is the element
                    // at that subscript; for no subscript, the scalar
                    // view (already resolved by collect_values to
                    // values[0] or empty).
                    let val = match sub {
                        SK::Index(_) => {
                            // collect_values resolved [i] already;
                            // it's the first (only) element of the
                            // pre-collected list. Fall back to ""
                            // on miss.
                            let vs = collect_values(shell);
                            vs.into_iter().next().unwrap_or_default()
                        }
                        _ => {
                            // No subscript → scalar view via Shell.
                            shell.lookup_var(name).unwrap_or_default()
                        }
                    };
                    ScopeMode::ScalarOrElement(val)
                };
                match op {
                    AssignDecl => ExpansionResult::Value(at::assign_decl(name, scope, shell)),
                    KvString => ExpansionResult::Value(at::kv_string(name, scope, shell)),
                    KvWords => {
                        let words = at::kv_words(name, scope, shell);
                        if matches!(sub, SK::All) && quoted {
                            ExpansionResult::WordList(words)
                        } else {
                            let ifs = shell.ifs();
                            let sep = ifs_join_sep(&ifs);
                            ExpansionResult::Value(words.join(&sep))
                        }
                    }
                    AttrFlags => ExpansionResult::Value(at::attr_flags(name, shell)),
                    _ => unreachable!("guarded by is_whole_array_transform_op"),
                }
            }
```

Note: the use of `collect_values(shell)` is the v209 closure already in scope. If the closure is consumed before the new arm (i.e. moved already), reorder: bring the new arm BEFORE the per-element arm OR refactor `collect_values` to take `&shell` borrow only. Read v209's arm to confirm.

- [ ] **Step 3: Add the symmetric arm to `expand_assoc_param`**

Find the assoc catchall:

```bash
grep -n "not supported on associative array in v72" crates/huck-engine/src/expand.rs
```

Insert the same arm shape immediately after the per-element arm in `expand_assoc_param`. The assoc path uses a pre-collected `values: Vec<String>` snapshot at the top of the function (line ~351 per v209's task 3). For `ScopeMode::ScalarOrElement`, the value for `Index(k)` is the value at key `k` (look it up via `shell.get_var` → `Associative(pairs)` → find by key), for no-subscript it's empty string (assoc scalar view is empty):

```rust
            (crate::lexer::ParamModifier::Transform { op }, sub)
                if is_whole_array_transform_op(*op) =>
            {
                use crate::array_transforms::{self as at, ScopeMode};
                use crate::lexer::SubscriptKind as SK;
                use crate::lexer::TransformOp::*;
                let scope = if matches!(sub, SK::All | SK::Star) {
                    ScopeMode::Whole
                } else {
                    let val = match sub {
                        SK::Index(_) => {
                            // The pre-collected `values` snapshot at
                            // the top of expand_assoc_param has only
                            // the resolved element when a specific
                            // [k] subscript is in play; first entry.
                            values.first().cloned().unwrap_or_default()
                        }
                        _ => String::new(), // assoc scalar view is empty
                    };
                    ScopeMode::ScalarOrElement(val)
                };
                match op {
                    AssignDecl => ExpansionResult::Value(at::assign_decl(name, scope, shell)),
                    KvString => ExpansionResult::Value(at::kv_string(name, scope, shell)),
                    KvWords => {
                        let words = at::kv_words(name, scope, shell);
                        if matches!(sub, SK::All) && quoted {
                            ExpansionResult::WordList(words)
                        } else {
                            let ifs = shell.ifs();
                            let sep = ifs_join_sep(&ifs);
                            ExpansionResult::Value(words.join(&sep))
                        }
                    }
                    AttrFlags => ExpansionResult::Value(at::attr_flags(name, shell)),
                    _ => unreachable!("guarded by is_whole_array_transform_op"),
                }
            }
```

If the `values` snapshot name differs in the actual function, adapt. Read the function head for the canonical name.

- [ ] **Step 4: Add unit tests for the whole-array arm**

Append to the `mod tests` block in `expand.rs`:

```rust
#[test]
fn transform_assign_decl_on_indexed_at() {
    use crate::shell_state::Shell;
    use crate::param_expansion::ExpansionResult;
    use crate::lexer::{ParamModifier as PM, TransformOp, SubscriptKind as SK};
    let mut shell = Shell::new();
    shell.set_indexed_element("a", 0, "x".to_string()).unwrap();
    shell.set_indexed_element("a", 1, "y".to_string()).unwrap();
    let result = expand_array_param(
        "a",
        &PM::Transform { op: TransformOp::AssignDecl },
        &SK::All,
        true,
        &mut shell,
    );
    match result {
        ExpansionResult::Value(v) => assert_eq!(v, r#"declare -a a=([0]="x" [1]="y")"#),
        other => panic!("expected Value, got {other:?}"),
    }
}

#[test]
fn transform_kv_words_on_indexed_yields_wordlist() {
    use crate::shell_state::Shell;
    use crate::param_expansion::ExpansionResult;
    use crate::lexer::{ParamModifier as PM, TransformOp, SubscriptKind as SK};
    let mut shell = Shell::new();
    shell.set_indexed_element("a", 0, "x".to_string()).unwrap();
    shell.set_indexed_element("a", 1, "y".to_string()).unwrap();
    let result = expand_array_param(
        "a",
        &PM::Transform { op: TransformOp::KvWords },
        &SK::All,
        true,
        &mut shell,
    );
    match result {
        ExpansionResult::WordList(words) => assert_eq!(words, vec!["0", "x", "1", "y"]),
        other => panic!("expected WordList, got {other:?}"),
    }
}

#[test]
fn transform_attr_flags_indexed_yields_a() {
    use crate::shell_state::Shell;
    use crate::param_expansion::ExpansionResult;
    use crate::lexer::{ParamModifier as PM, TransformOp, SubscriptKind as SK};
    let mut shell = Shell::new();
    shell.set_indexed_element("a", 0, "x".to_string()).unwrap();
    let result = expand_array_param(
        "a",
        &PM::Transform { op: TransformOp::AttrFlags },
        &SK::All,
        true,
        &mut shell,
    );
    match result {
        ExpansionResult::Value(v) => assert_eq!(v, "a"),
        other => panic!("expected Value, got {other:?}"),
    }
}

#[test]
fn transform_assign_decl_on_assoc_at() {
    use crate::shell_state::Shell;
    use crate::param_expansion::ExpansionResult;
    use crate::lexer::{ParamModifier as PM, TransformOp, SubscriptKind as SK};
    let mut shell = Shell::new();
    shell.declare_associative("m").unwrap();
    shell.set_associative_element("m", "k".to_string(), "v1".to_string()).unwrap();
    let result = expand_array_param(
        "m",
        &PM::Transform { op: TransformOp::AssignDecl },
        &SK::All,
        true,
        &mut shell,
    );
    match result {
        ExpansionResult::Value(v) => assert_eq!(v, r#"declare -A m=([k]="v1" )"#),
        other => panic!("expected Value, got {other:?}"),
    }
}
```

- [ ] **Step 5: Build + run tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet transform_assign_decl_on_indexed_at transform_kv_words_on_indexed_yields_wordlist transform_attr_flags_indexed_yields_a transform_assign_decl_on_assoc_at
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 4 new tests pass; full suite green; clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/expand.rs
git commit -m "$(cat <<'EOF'
v210 task 7: predicate split + whole-array Transform arm

Refines is_per_element_modifier to discriminate Transform { op } by op,
adds is_per_element_transform_op (P/Q/U/L/u/E) and is_whole_array_transform_op
(A/K/k/a). New match arm in expand_array_param + expand_assoc_param,
inserted between v209's per-element arm and the v71/v72 catchall,
routes the 4 whole-array ops through array_transforms::*. WordList vs
Value discrimination per spec: KvWords + quoted [@] → WordList; all
others → Value. 4 unit tests cover the major shapes.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Scalar dispatch in `param_expansion.rs`

**Files:**
- Modify: `crates/huck-engine/src/param_expansion.rs` — fill in the 4 new `TransformOp` cases.

- [ ] **Step 1: Find the `Transform { op }` arm**

```bash
grep -n "ParamModifier::Transform { op }" crates/huck-engine/src/param_expansion.rs
```

Should hit line ~240. The arm uses `let v = lookup_v(shell);` then matches on `op`. Today it has 6 cases (P/Q/U/L/u/E) + the temporary `_ => String::new()` catchall added in Task 1.

- [ ] **Step 2: Replace the catchall with the 4 new cases**

Add these BEFORE the temporary catchall (and remove the catchall once they're in):

```rust
                crate::lexer::TransformOp::AssignDecl => {
                    crate::array_transforms::assign_decl(
                        name,
                        crate::array_transforms::ScopeMode::ScalarOrElement(v.clone()),
                        shell,
                    )
                }
                crate::lexer::TransformOp::KvString => {
                    crate::array_transforms::kv_string(
                        name,
                        crate::array_transforms::ScopeMode::ScalarOrElement(v.clone()),
                        shell,
                    )
                }
                crate::lexer::TransformOp::KvWords => {
                    // Scalar/element form returns a single-word Vec
                    // (since there's no [@] under scalar dispatch).
                    // Join with IFS sep (effectively just the one word).
                    let words = crate::array_transforms::kv_words(
                        name,
                        crate::array_transforms::ScopeMode::ScalarOrElement(v.clone()),
                        shell,
                    );
                    let sep = crate::expand::ifs_join_sep(&shell.ifs());
                    words.join(&sep)
                }
                crate::lexer::TransformOp::AttrFlags => {
                    crate::array_transforms::attr_flags(name, shell)
                }
```

REMOVE the temporary `_ => String::new(),` catchall added in Task 1 — the match is now exhaustive over the 10 TransformOp variants.

- [ ] **Step 3: Add 4 scalar-path unit tests**

Find the `mod tests` block in `param_expansion.rs`:

```bash
grep -n "#\[cfg(test)\]" crates/huck-engine/src/param_expansion.rs | head -3
```

Append:

```rust
    #[test]
    fn transform_assign_decl_on_scalar() {
        use crate::shell_state::Shell;
        let mut shell = Shell::new();
        shell.set("s", "hello".to_string()).unwrap();
        let m = crate::lexer::ParamModifier::Transform {
            op: crate::lexer::TransformOp::AssignDecl,
        };
        let result = expand_modifier_with_value(
            "s", &m, crate::param_expansion::ParamLookup::Element(Some("hello")),
            false, &mut shell,
        );
        match result {
            crate::param_expansion::ExpansionResult::Value(v) => assert_eq!(v, "s='hello'"),
            other => panic!("expected Value, got {other:?}"),
        }
    }

    #[test]
    fn transform_assign_decl_on_attributed_scalar() {
        use crate::shell_state::Shell;
        let mut shell = Shell::new();
        shell.set("ev", "42".to_string()).unwrap();
        shell.export("ev");
        let m = crate::lexer::ParamModifier::Transform {
            op: crate::lexer::TransformOp::AssignDecl,
        };
        let result = expand_modifier_with_value(
            "ev", &m, crate::param_expansion::ParamLookup::Element(Some("42")),
            false, &mut shell,
        );
        match result {
            crate::param_expansion::ExpansionResult::Value(v) => assert_eq!(v, "declare -x ev='42'"),
            other => panic!("expected Value, got {other:?}"),
        }
    }

    #[test]
    fn transform_assign_decl_on_unset_is_empty() {
        use crate::shell_state::Shell;
        let mut shell = Shell::new();
        let m = crate::lexer::ParamModifier::Transform {
            op: crate::lexer::TransformOp::AssignDecl,
        };
        let result = expand_modifier_with_value(
            "nope", &m, crate::param_expansion::ParamLookup::Element(None),
            false, &mut shell,
        );
        match result {
            crate::param_expansion::ExpansionResult::Empty => {}
            other => panic!("expected Empty, got {other:?}"),
        }
    }

    #[test]
    fn transform_attr_flags_on_exported() {
        use crate::shell_state::Shell;
        let mut shell = Shell::new();
        shell.set("ev", "42".to_string()).unwrap();
        shell.export("ev");
        let m = crate::lexer::ParamModifier::Transform {
            op: crate::lexer::TransformOp::AttrFlags,
        };
        let result = expand_modifier_with_value(
            "ev", &m, crate::param_expansion::ParamLookup::Element(Some("42")),
            false, &mut shell,
        );
        match result {
            crate::param_expansion::ExpansionResult::Value(v) => assert_eq!(v, "x"),
            other => panic!("expected Value, got {other:?}"),
        }
    }
```

If `expand_modifier_with_value` / `ParamLookup` signatures differ, adapt by reading the existing tests in `param_expansion.rs::mod tests`.

For the unset test: if `ParamLookup::Element(None)` doesn't naturally produce `Empty` for AssignDecl (because the new `_v` is empty string), the function may instead produce `Value("")`. In that case, the assertion becomes `Value("")` not `Empty`. Adapt to match observed behavior — both `Empty` and `Value("")` are bash-faithful for unset (bash output is just empty bytes).

- [ ] **Step 4: Build + run tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet transform_assign_decl_on_scalar transform_assign_decl_on_attributed_scalar transform_assign_decl_on_unset_is_empty transform_attr_flags_on_exported
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 4 new tests pass; full suite green; clippy clean. The match is now exhaustive; the temporary catchall removed.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/param_expansion.rs
git commit -m "$(cat <<'EOF'
v210 task 8: scalar @A/@K/@k/@a dispatch in param_expansion

Extends the Transform { op } arm with 4 new sub-cases that delegate to
array_transforms::* with ScopeMode::ScalarOrElement(value). The match
is now exhaustive over the 10 TransformOp variants; removes the
temporary catchall from Task 1. 4 unit tests cover scalar @A (no
attrs), attributed scalar @A, unset @A, and @a on an exported scalar.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Bash-diff harness

**Files:**
- Create: `tests/scripts/array_transforms_diff_check.sh`

- [ ] **Step 1: Create the harness**

```bash
cat > tests/scripts/array_transforms_diff_check.sh <<'HARNESS_EOF'
#!/usr/bin/env bash
# v210: bash-diff harness for ${var@A}/@K/@k/@a transforms.
# Asserts byte-identical stdout between bash -c and huck -c.
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

# === @A on scalars ===
check 'A-scalar-plain'        's=hello; echo "${s@A}"'
check 'A-scalar-quote'        'q="it'\''s"; echo "${q@A}"'
check 'A-scalar-empty'        's=; echo "[${s@A}]"'
check 'A-scalar-unset'        'echo "[${u@A}]"'
check 'A-scalar-exported'     'declare -x ev=42; echo "${ev@A}"'
check 'A-scalar-readonly'     'declare -r r=42; echo "${r@A}"'
check 'A-scalar-integer'      'declare -i n=5; echo "${n@A}"'
check 'A-scalar-multi'        'declare -irx mix=7; echo "${mix@A}"'

# === @A on indexed arrays ===
check 'A-indexed-at'          'a=(x y z); echo "${a[@]@A}"'
check 'A-indexed-no-sub'      'a=(x y z); echo "${a@A}"'
check 'A-indexed-i-sub'       'a=(x y z); echo "${a[1]@A}"'
check 'A-indexed-empty'       'declare -a e=(); echo "${e[@]@A}"'

# === @A on assoc arrays — pipe through sort for L-44 (c) order ===
check 'A-assoc-at-sorted'     'declare -A m=([k]=v1 [j]=v2); echo "${m[@]@A}" | tr "[" "\n" | sort'
check 'A-assoc-no-sub'        'declare -A m=([k]=v1 [j]=v2); echo "${m@A}"'
check 'A-assoc-empty'         'declare -A em=(); echo "${em[@]@A}"'

# === @K and @k ===
check 'K-indexed-at'          'a=(x y); echo "${a[@]@K}"'
check 'k-indexed-at'          'a=(x y); echo "${a[@]@k}"'
check 'k-indexed-for-loop'    'a=(x y); for w in "${a[@]@k}"; do echo "<$w>"; done'
check 'K-assoc-sorted'        'declare -A m=([k]=v); echo "${m[@]@K}"'

# === @a attribute flags ===
check 'a-scalar-no-attrs'     's=hello; echo "[${s@a}]"'
check 'a-scalar-integer'      'declare -i n=5; echo "${n@a}"'
check 'a-scalar-exported'     'declare -x e=1; echo "${e@a}"'
check 'a-scalar-multi'        'declare -irx mix=7; echo "${mix@a}"'
check 'a-indexed'             'a=(x); echo "${a@a}"'
check 'a-assoc'               'declare -A m=([k]=v); echo "${m@a}"'
check 'a-unset'               'echo "[${u@a}]"'

# === Combined / round-trip via eval ===
check 'A-round-trip'          'a=(x y); s="${a[@]@A}"; unset a; eval "$s"; echo "${a[@]}"'

if [ $FAIL -ne 0 ]; then
    echo "array_transforms_diff_check FAILED" >&2
    exit 1
fi
echo "array_transforms_diff_check OK"
HARNESS_EOF
chmod +x tests/scripts/array_transforms_diff_check.sh
```

- [ ] **Step 2: Run the harness**

```bash
bash tests/scripts/array_transforms_diff_check.sh
```

Expected: all ~26 checks PASS.

If any check fails because of a real correctness bug in v210's wiring, fix in Task 5/6/7/8 — investigate with a focused reproducer first, don't tweak the harness fragment to hide the bug.

If any check fails because of:
- **L-44 (c) hash-vs-insertion order**: dodge with `| sort` or `| tr "[" "\n" | sort` as needed. The harness already does this for `A-assoc-at-sorted` and `K-assoc-sorted` (if applicable).
- **Trailing whitespace stripping by echo**: `echo` may strip trailing whitespace before newline, so a trailing-space mismatch may not surface. Use `printf '[%s]\n'` to surface trailing whitespace explicitly.
- **Bash version-specific behavior**: confirm the bash version (`bash --version`); v210 targets bash 5.x.

- [ ] **Step 3: Run full suite + clippy**

```bash
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: green; clippy clean.

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/array_transforms_diff_check.sh
# Include any expand.rs / array_transforms.rs fixes if bugs surfaced.
git commit -m "$(cat <<'EOF'
v210 task 9: bash-diff harness for @A/@K/@k/@a transforms

~26 fragments across @A on scalars (8), @A on indexed (4), @A on
assoc (3, with sort to dodge L-44(c)), @K/@k (4), @a (7), and one
round-trip via eval. Asserts byte-identical stdout between bash -c
and huck -c.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Final sweep — docs, harness regression, verify, stop before merge

**Files:**
- Modify: `docs/bash-divergences.md` — delete M-93, shrink L-44.
- Modify: `docs/architecture.md` — one-sentence pointer.

- [ ] **Step 1: Delete M-93 from `bash-divergences.md`**

```bash
grep -n 'M-93' docs/bash-divergences.md
```

Delete the entire M-93 bullet item. The current entry reads approximately:

```
- **M-93: `${var@OP}` array/attribute transforms (`@A`/`@K`/`@k`/`@a`)** — `[deferred]` low. huck: the assignment-statement form `@A`, the key/value array forms `@K`/`@k`, and the attribute-flags form `@a` error (unsupported `@`-operator). bash: `@A` reproduces a `declare`-style assignment string, `@a` lists attribute flags, `@K`/`@k` expand associative-array key/value pairs. M-86 follow-on; the scalar transforms (`@P`/`@Q`/`@U`/`@L`/`@u`/`@E`) shipped in v96.
```

Use the Edit tool to remove the entire paragraph (one bullet).

- [ ] **Step 2: Update Tier 2 count in the summary table**

```bash
grep -n 'Tier 2' docs/bash-divergences.md | head -3
```

After v209 the Tier 2 count is 12. After v210 it's 11. Update the count cell.

- [ ] **Step 3: Shrink L-44 entry**

```bash
grep -n 'L-44' docs/bash-divergences.md
```

The current L-44 entry covers three facts: (a) quoted vs bareword subscript keys, (b) trailing space, (c) hash-vs-insertion order. v210 resolves (a) and (b); (c) remains.

Replace the L-44 bullet with a shortened version that documents only the ordering residual. Approximately:

```
- **L-44: associative-array iteration order in `declare -p` / bare-`declare` / `${var@A}` / `${var@K}` etc.** — `[deferred]`, low. huck lists associative-array elements in insertion order, bash in internal hash order. Both round-trip correctly through `eval`/`source` (the value is the point), so this is cosmetic. Impractical to match bash's hash order; v210 closed the related quoting+trailing-space facts (originally tracked as (a)+(b) here).
```

Tier 4 count unchanged.

- [ ] **Step 4: Add an architecture.md pointer**

```bash
grep -n 'where to add\|parameter expansion' docs/architecture.md | head -10
```

Find the parameter-expansion section of the where-to-add cheatsheet. Add (or extend an existing) sentence:

```markdown
- **Whole-array `${var@OP}` transforms (`@A`/`@K`/`@k`/`@a`)** — implemented in `crates/huck-engine/src/array_transforms.rs`. New op variants land in `huck-syntax`'s `TransformOp` enum, then route through `is_whole_array_transform_op` in `expand.rs` for `[@]`/`[*]` subscripts or through `param_expansion.rs`'s `Transform { op }` arm for the scalar / single-element form.
```

If the architecture doc doesn't have a where-to-add cheatsheet, add the sentence to the most natural parameter-expansion section.

- [ ] **Step 5: Final full-suite + harness sweep**

```bash
cargo test --workspace --quiet
cargo test --workspace --doc --quiet
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --workspace --quiet

bash tests/scripts/array_transforms_diff_check.sh

# All existing harnesses:
for h in tests/scripts/*_diff_check.sh; do
    bash "$h" > /tmp/h.out 2>&1
    rc=$?
    if [ $rc -ne 0 ]; then
        echo "FAIL: $h (exit $rc)"
        tail -10 /tmp/h.out
    fi
done

# Headless CLI smoke:
./target/release/huck -c 'echo hello'
echo "exit=$?"
```

Expected: all green; release binary builds; all harnesses pass; smoke prints `hello` + `exit=0`.

If any pre-existing harness FAILS at this point, investigate. Most likely cause: a harness captured huck's old `declare -p`/bare-`declare` output verbatim and now sees the new bareword-keys / trailing-space output. Update those harnesses to the new (correct) form.

- [ ] **Step 6: Commit**

```bash
git add docs/bash-divergences.md docs/architecture.md
git commit -m "$(cat <<'EOF'
v210 task 10: remove M-93, shrink L-44, architecture pointer

M-93 (${var@OP} array/attribute transforms) is fully resolved by
v210's array_transforms module + the whole-array arm in
expand_array_param/expand_assoc_param + scalar dispatch in
param_expansion. Tier 2 count: 12 → 11.

L-44 was a 3-fact bullet covering (a) subscript key quoting,
(b) trailing space, (c) insertion vs hash order. v210 closed (a)
and (b) via the shared declare-body renderer; only (c) remains.
Shrunk the entry to the ordering residual; Tier 4 count unchanged.

architecture.md gains a where-to-add pointer for the new module.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 7: Stop — do NOT merge**

The final whole-branch code review is the controller's call after Task 10. Stop after this commit.

---

## Self-review

**Spec coverage:**
- Lexer additions (Section 1 of spec): Task 1.
- Semantic dispatch table (Section 2): Tasks 5, 6 (array_transforms implementations).
- Architecture refactor (Section 3): Tasks 3 (scaffold), 7 (predicate split + array arm), 8 (scalar dispatch).
- L-44 shared renderer (Section 4): Task 4.
- Testing strategy (Section 5): Tasks 1, 4, 5, 6, 7, 8 (unit tests), 9 (bash-diff harness), 10 (final harness sweep).
- Doc updates (Section 5 + Out-of-scope): Task 10.
- Risk-1 (bash quoting edge cases): harness in Task 9 + Task 10 sweep catches.
- Risk-2 (hash order surfacing): each assoc-order-sensitive fragment in the harness pipes through sort.
- Risk-3 (L-46 empty-array-body distinction): noted in spec, NOT fixed in v210 (inheriting L-46).
- Risk-4 (future attribute letter): canonical table is fixed in `attr_flags`; no compile-time exhaustiveness guarantee, but easy to extend.

**Placeholder scan:**
- No "TBD" / "implement later" / "fill in details" — every step has concrete code.
- The "implementation in Task 5/6" stub comments in Task 3's scaffold are intentional (the bodies arrive in those tasks).
- Task 8 step 3 notes "if Empty doesn't naturally produce, adapt" — that's a conditional, not a placeholder. Acceptable because the test verifies whichever shape comes out and bash output is byte-empty either way.

**Type consistency:**
- `ScopeMode { Whole, ScalarOrElement(String) }` — same definition in Tasks 3, 5, 6, 7, 8.
- `array_transforms::{assign_decl, kv_string, kv_words, attr_flags}` — same signatures in Tasks 5, 6, 7, 8.
- `is_whole_array_transform_op(op: TransformOp) -> bool` — same in Task 7 declaration and reference.
- `Shell::get_var(name) -> Option<&Variable>` — same in Tasks 2, 5, 6.
- Test method names (`shell.set`, `shell.set_indexed_element`, `shell.declare_associative`, `shell.set_associative_element`, `shell.mark_exported`, `shell.mark_integer`, `shell.mark_readonly`) — consistent across tasks; each task notes "adapt if API differs."

**10 tasks. ~250 LOC of production logic (~150 in array_transforms.rs, ~80 modified across builtins/expand/param_expansion, ~20 in lexer/generate) + ~250 LOC of tests + ~70 LOC of harness.** Comparable to v209 in scope; slightly larger because v210 also covers L-44 cleanup and touches more files (lexer, two crates).
