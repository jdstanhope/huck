# huck v72 — Associative Arrays Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task is implemented by a fresh subagent, with spec-compliance review and code-quality review between tasks.

**Goal:** Add bash-compatible associative (string-keyed, insertion-ordered) arrays to huck: `declare -A`, element access, all-elements expansion with correct quoting, append-by-key, slicing, `local -A` / `readonly -A`, `declare -p` formatting, and bash-faithful type-mismatch error paths.

**Architecture:** Extend the v71 `VarValue` enum with a third variant `Associative(Vec<(String, String)>)`. No parser/lexer changes needed — v71's `m[key]=v`, `${m[key]}`, `m+=(...)`, `unset m[k]` machinery is reused. The new work is runtime dispatch: every subscripted operation checks the variable's current `VarValue` variant and chooses between v71's arith-based subscript path (Indexed/Scalar/unset) and a new string-based subscript path (Associative).

**Tech Stack:** Rust 1.85+, `Vec<(String, String)>` (no new dep) for insertion-ordered storage.

**Branch:** `v72-assoc-arrays` (create from `main` at the start of Preamble).

**Spec:** `docs/superpowers/specs/2026-06-01-huck-associative-arrays-design.md`.

**Builds on (already in main):** v71 indexed arrays (M-82). Existing helpers reused:
- `VarValue { Scalar, Indexed }` enum in `src/shell_state.rs`
- `Shell::snapshot_var` / `restore_var` (full-Variable clone)
- `Shell::set_array_element`, `append_array_element`, `unset_array_element`, `replace_array`, `lookup_array_element`, `get_array`, `array_max_index`
- `expand::eval_subscript` (arith), `slice_word_list`, `expand_array_param`
- `param_expansion::expand_modifier_with_value`, `expand_word_to_string`
- `executor::apply_one_assignment`, `build_array_map`, `is_array_value_word`
- `command::DeclArg { Plain, Assign }`, `AssignTarget`, `Assignment { target, value, append }`
- `builtins::is_declaration_command`, `format_declare_line`, `parse_subscripted_arg`, `assign_value_is_array`, and the four `_decl` builtin functions

**Commit trailer (every commit):**

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble P.1: Branch setup

- [ ] **Step 1: Verify clean tree on main**

Run: `git status && git rev-parse --abbrev-ref HEAD`
Expected: branch `main`, clean working tree.

- [ ] **Step 2: Create the iteration branch**

Run:
```bash
git checkout -b v72-assoc-arrays
```
Expected: `Switched to a new branch 'v72-assoc-arrays'`.

- [ ] **Step 3: Confirm baseline tests pass**

Run: `cargo test 2>&1 | grep -E "test result|FAILED" | tail -5`
Expected: `test result: ok.` lines, 0 failures.

- [ ] **Step 4: Confirm clippy is clean**

Run: `cargo clippy --all-targets 2>&1 | tail -3`
Expected: `Finished` no warnings.

---

## File-structure map

| File | Responsibility | Tasks |
|------|----------------|-------|
| `src/shell_state.rs` | New `VarValue::Associative` variant; six new associative Shell methods; new `DeclareErr` enum | 1 |
| `src/expand.rs` | `eval_subscript_key` helper; associative dispatch in `expand_array_param` for all 9 forms | 2 |
| `src/executor.rs` | Associative-aware `apply_one_assignment`; `build_associative_map` helper; reject positional-list on assoc | 3 |
| `src/builtins.rs` | `-A` flag in `builtin_declare_decl`/`builtin_local_decl`; `declare -p` assoc format; `-Ai`/`-aA` rejection; assoc dispatch in `builtin_unset` | 4 |
| `tests/associative_arrays_integration.rs` | 10 binary-driven integration tests (new file) | 5 |
| `tests/scripts/arrays_diff_check.sh` | Extend with 5 associative fragments | 5 |
| `docs/bash-divergences.md` | New M-83 entry, cross-refs on M-82/M-79, change-log entry | 5 |
| `README.md` | v72 iteration row | 5 |

---

## Task 1: Data model + Shell mutators

**Files:**
- Modify: `src/shell_state.rs`

**Goal:** Introduce `VarValue::Associative(Vec<(String, String)>)` and add six associative-array Shell methods + a `DeclareErr` enum for the type-mismatch error paths. After this task, the storage exists and can be poked at via methods, but no expansion/assignment/builtin code references the new variant yet.

- [ ] **Step 1: Add the `Associative` variant to `VarValue`**

Edit `src/shell_state.rs`. Find the `VarValue` enum (around line 14):

```rust
pub enum VarValue {
    Scalar(String),
    Indexed(BTreeMap<usize, String>),
    Associative(Vec<(String, String)>),
}
```

Update `VarValue::scalar_view()` to return `""` for `Associative` (associative arrays have no element 0):

```rust
pub fn scalar_view(&self) -> &str {
    match self {
        VarValue::Scalar(s) => s.as_str(),
        VarValue::Indexed(m) => m.get(&0).map(String::as_str).unwrap_or(""),
        VarValue::Associative(_) => "",
    }
}
```

Update `install_scalar_value` (the private helper) to handle the Associative arm:

```rust
fn install_scalar_value(existing: &mut Variable, value: String) {
    match &mut existing.value {
        VarValue::Indexed(m) => { m.insert(0, value); }
        VarValue::Scalar(_) => { existing.value = VarValue::Scalar(value); }
        VarValue::Associative(_) => {
            // Bash: `m=v` on an associative array is an error in modern bash
            // versions ("must use subscript when assigning associative array").
            // The error is emitted by the executor; this helper is reached
            // only on already-validated paths. For safety, log internally and
            // leave the assoc unchanged.
            eprintln!("huck: internal: install_scalar_value on associative array");
        }
    }
}
```

(The executor's `apply_one_assignment` will reject scalar-RHS-on-associative before reaching this helper. The eprintln is defensive only.)

- [ ] **Step 2: Add `DeclareErr` enum**

Append to `src/shell_state.rs` near the existing `AssignErr` enum (around line 59):

```rust
/// Errors specific to declaration-builtin paths (declare -A on existing
/// indexed/scalar, etc.) that distinguish themselves from assignment errors.
#[derive(Debug)]
pub enum DeclareErr {
    TypeMismatch,
}
```

- [ ] **Step 3: Add the six associative Shell methods**

In `src/shell_state.rs`, append these methods inside the existing `impl Shell` block (after the existing array methods, around line 670):

```rust
/// Returns a reference to the associative array stored under `name`,
/// or `None` if the variable is unset, scalar, or indexed.
pub fn get_associative(&self, name: &str) -> Option<&Vec<(String, String)>> {
    match self.vars.get(name) {
        Some(v) => match &v.value {
            VarValue::Associative(pairs) => Some(pairs),
            _ => None,
        },
        None => None,
    }
}

/// Returns the value at string key `key` for the associative array `name`.
/// `None` if the variable is unset, not associative, or has no such key.
pub fn lookup_associative_element(&self, name: &str, key: &str) -> Option<String> {
    self.get_associative(name).and_then(|pairs| {
        pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    })
}

/// Sets `key` to `value` in the associative array `name`. Preserves
/// insertion order on update (existing key keeps its position; new
/// keys are appended). Honors readonly. Errors with TypeMismatch if
/// the variable exists and is NOT associative.
pub fn set_associative_element(
    &mut self,
    name: &str,
    key: String,
    value: String,
) -> Result<(), AssignErr> {
    if let Some(existing) = self.vars.get(name)
        && existing.readonly
    {
        eprintln!("huck: {name}: readonly variable");
        return Err(AssignErr::Readonly);
    }
    match self.vars.get_mut(name) {
        Some(v) => match &mut v.value {
            VarValue::Associative(pairs) => {
                if let Some(slot) = pairs.iter_mut().find(|(k, _)| k == &key) {
                    slot.1 = value;
                } else {
                    pairs.push((key, value));
                }
            }
            _ => {
                // Caller must check the variant before calling. Return Readonly
                // is wrong; create a more specific error in the future. For
                // v72, treat as a no-op + diagnostic to surface misuse.
                eprintln!("huck: {name}: set_associative_element on non-associative variable");
                return Err(AssignErr::Readonly);
            }
        },
        None => {
            // Caller (executor) must call declare_associative first OR
            // be in a path that creates the associative. Defensive only.
            eprintln!("huck: {name}: set_associative_element on unset variable");
            return Err(AssignErr::Readonly);
        }
    }
    Ok(())
}

/// `m[k]+=v` — concatenate `value` to the existing element at `key`,
/// or set to `value` if no such key. Honors readonly.
pub fn append_associative_element(
    &mut self,
    name: &str,
    key: &str,
    value: &str,
) -> Result<(), AssignErr> {
    let existing = self.lookup_associative_element(name, key).unwrap_or_default();
    self.set_associative_element(name, key.to_string(), existing + value)
}

/// Removes the entry at `key` from the associative array `name`.
/// No-op if the variable is missing, not associative, or has no such key.
/// Honors readonly.
pub fn unset_associative_element(
    &mut self,
    name: &str,
    key: &str,
) -> Result<(), AssignErr> {
    if let Some(existing) = self.vars.get(name)
        && existing.readonly
    {
        eprintln!("huck: {name}: readonly variable");
        return Err(AssignErr::Readonly);
    }
    if let Some(v) = self.vars.get_mut(name)
        && let VarValue::Associative(pairs) = &mut v.value
    {
        pairs.retain(|(k, _)| k != key);
    }
    Ok(())
}

/// Replaces (or creates) `name` as an associative array with the given
/// pairs in insertion order. Honors readonly. Preserves exported flag
/// if the variable exists. For type mismatches (existing indexed/scalar),
/// errors with the bash diagnostic.
pub fn replace_associative(
    &mut self,
    name: &str,
    pairs: Vec<(String, String)>,
) -> Result<(), AssignErr> {
    if let Some(existing) = self.vars.get(name)
        && existing.readonly
    {
        eprintln!("huck: {name}: readonly variable");
        return Err(AssignErr::Readonly);
    }
    let exported = self.vars.get(name).map(|v| v.exported).unwrap_or(false);
    self.vars.insert(name.to_string(), Variable {
        value: VarValue::Associative(pairs),
        exported,
        readonly: false,
        integer: false,
    });
    Ok(())
}

/// Creates an empty associative array under `name`. Enforces bash rules:
/// - Unset → create empty associative.
/// - Already associative → no-op.
/// - Indexed → error: "cannot convert indexed to associative array".
/// - Scalar → error: "cannot convert scalar to associative".
pub fn declare_associative(&mut self, name: &str) -> Result<(), DeclareErr> {
    match self.vars.get(name).map(|v| &v.value) {
        None => {
            self.vars.insert(name.to_string(), Variable {
                value: VarValue::Associative(Vec::new()),
                exported: false,
                readonly: false,
                integer: false,
            });
            Ok(())
        }
        Some(VarValue::Associative(_)) => Ok(()),
        Some(VarValue::Indexed(_)) => {
            eprintln!("huck: declare: {name}: cannot convert indexed to associative array");
            Err(DeclareErr::TypeMismatch)
        }
        Some(VarValue::Scalar(_)) => {
            eprintln!("huck: declare: {name}: cannot convert scalar to associative");
            Err(DeclareErr::TypeMismatch)
        }
    }
}
```

- [ ] **Step 4: Add unit tests in `mod assoc_value_tests`**

Append at the bottom of `src/shell_state.rs`:

```rust
#[cfg(test)]
mod assoc_value_tests {
    use super::*;

    #[test]
    fn scalar_view_returns_empty_for_associative() {
        let v = VarValue::Associative(vec![
            ("k1".to_string(), "v1".to_string()),
            ("k2".to_string(), "v2".to_string()),
        ]);
        assert_eq!(v.scalar_view(), "");
    }

    #[test]
    fn declare_associative_on_unset_creates_empty() {
        let mut shell = Shell::new();
        assert!(shell.declare_associative("m").is_ok());
        assert_eq!(shell.get_associative("m").map(Vec::len), Some(0));
    }

    #[test]
    fn declare_associative_on_existing_associative_is_noop() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.set_associative_element("m", "k".into(), "v".into()).unwrap();
        assert!(shell.declare_associative("m").is_ok());
        assert_eq!(shell.lookup_associative_element("m", "k"), Some("v".into()));
    }

    #[test]
    fn declare_associative_on_indexed_errors() {
        let mut shell = Shell::new();
        let mut m = BTreeMap::new();
        m.insert(0, "x".into());
        shell.vars.insert("a".into(), Variable {
            value: VarValue::Indexed(m),
            exported: false, readonly: false, integer: false,
        });
        assert!(matches!(shell.declare_associative("a"), Err(DeclareErr::TypeMismatch)));
    }

    #[test]
    fn declare_associative_on_scalar_errors() {
        let mut shell = Shell::new();
        shell.set("s", "hello".into());
        assert!(matches!(shell.declare_associative("s"), Err(DeclareErr::TypeMismatch)));
    }

    #[test]
    fn set_associative_element_preserves_insertion_order_on_update() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.set_associative_element("m", "a".into(), "1".into()).unwrap();
        shell.set_associative_element("m", "b".into(), "2".into()).unwrap();
        shell.set_associative_element("m", "c".into(), "3".into()).unwrap();
        // Update existing key "a" — it should stay at position 0.
        shell.set_associative_element("m", "a".into(), "999".into()).unwrap();
        let pairs = shell.get_associative("m").unwrap();
        assert_eq!(pairs[0], ("a".into(), "999".into()));
        assert_eq!(pairs[1], ("b".into(), "2".into()));
        assert_eq!(pairs[2], ("c".into(), "3".into()));
    }

    #[test]
    fn append_associative_element_concatenates() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.set_associative_element("m", "k".into(), "hello".into()).unwrap();
        shell.append_associative_element("m", "k", "_world").unwrap();
        assert_eq!(shell.lookup_associative_element("m", "k"), Some("hello_world".into()));
    }

    #[test]
    fn append_associative_element_creates_when_missing() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.append_associative_element("m", "new", "value").unwrap();
        assert_eq!(shell.lookup_associative_element("m", "new"), Some("value".into()));
    }

    #[test]
    fn unset_associative_element_removes_one_key() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.set_associative_element("m", "a".into(), "1".into()).unwrap();
        shell.set_associative_element("m", "b".into(), "2".into()).unwrap();
        shell.set_associative_element("m", "c".into(), "3".into()).unwrap();
        shell.unset_associative_element("m", "b").unwrap();
        let pairs = shell.get_associative("m").unwrap();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, "a");
        assert_eq!(pairs[1].0, "c");
    }

    #[test]
    fn replace_associative_overwrites() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.set_associative_element("m", "old".into(), "1".into()).unwrap();
        let new_pairs = vec![
            ("x".to_string(), "10".to_string()),
            ("y".to_string(), "20".to_string()),
        ];
        shell.replace_associative("m", new_pairs).unwrap();
        assert!(shell.lookup_associative_element("m", "old").is_none());
        assert_eq!(shell.lookup_associative_element("m", "x"), Some("10".into()));
        assert_eq!(shell.lookup_associative_element("m", "y"), Some("20".into()));
    }

    #[test]
    fn readonly_blocks_set_associative_element() {
        let mut shell = Shell::new();
        shell.declare_associative("m").unwrap();
        shell.set_associative_element("m", "k".into(), "v".into()).unwrap();
        shell.mark_readonly("m");
        assert!(matches!(
            shell.set_associative_element("m", "k2".into(), "v2".into()),
            Err(AssignErr::Readonly)
        ));
        assert!(shell.lookup_associative_element("m", "k2").is_none());
    }
}
```

- [ ] **Step 5: Build and verify**

Run: `cargo build 2>&1 | tail -5`
Expected: clean.

Run: `cargo test --bin huck assoc_value 2>&1 | tail -15`
Expected: 11 tests pass.

Run: `cargo test 2>&1 | grep -E "test result|FAILED" | head -10`
Expected: every line `ok.`; 0 failures. Total ~2036 (2025 existing + 11 new).

Run: `cargo clippy --all-targets 2>&1 | tail -5`
Expected: 0 warnings.

- [ ] **Step 6: Commit**

```bash
git add src/shell_state.rs
git commit -m "$(cat <<'EOF'
foundation: VarValue::Associative + Shell mutators (v72 task 1)

Adds the third VarValue variant (string-keyed, insertion-ordered)
backed by Vec<(String, String)>. Six new associative Shell methods
(get_associative, lookup_associative_element, set_associative_element,
append_associative_element, unset_associative_element,
replace_associative, declare_associative) with readonly + type-
mismatch checks. New DeclareErr enum.

scalar_view() returns "" for Associative (bash: $m on assoc is empty,
not the first value).

No expansion/assignment/builtin paths reference the new variant yet
— Tasks 2-4 wire those.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Expansion semantics

**Files:**
- Modify: `src/expand.rs` (`eval_subscript_key` helper; extend `expand_array_param`)

**Goal:** Reading associative arrays works end-to-end via all 9 expansion forms. Subscript dispatch picks string semantics when the variable is Associative and arith semantics otherwise (matching v71's gotcha for unset variables).

- [ ] **Step 1: Add `eval_subscript_key` helper**

In `src/expand.rs`, near the existing `eval_subscript` (around line 94):

```rust
/// Expands a subscript Word to a string key. Variable expansion and
/// command substitution apply, but no arith. Used for associative
/// array subscripts. The caller decides string vs arith based on the
/// variable's current VarValue variant.
pub(crate) fn eval_subscript_key(
    subscript: &crate::lexer::Word,
    shell: &mut crate::shell_state::Shell,
) -> String {
    crate::param_expansion::expand_word_to_string(subscript, shell)
}
```

- [ ] **Step 2: Extend `expand_array_param` with associative branches**

Find `expand_array_param` in `src/expand.rs` (around line 249). The function currently dispatches on `(modifier, subscript)`. Add a variant-aware prelude at the top that, when the variable is associative, routes to a new sibling `expand_assoc_param`. Implementation sketch:

```rust
fn expand_array_param(
    name: &str,
    modifier: &ParamModifier,
    subscript: &SubscriptKind,
    quoted: bool,
    shell: &mut Shell,
) -> ExpansionResult {
    // Type-aware dispatch: associative arrays get string-key semantics.
    if shell.get_associative(name).is_some() {
        return expand_assoc_param(name, modifier, subscript, quoted, shell);
    }
    // …existing Indexed/Scalar/unset path (v71)…
}
```

Add the new function (mirrors v71's `expand_array_param` structure but uses string-key semantics):

```rust
fn expand_assoc_param(
    name: &str,
    modifier: &ParamModifier,
    subscript: &SubscriptKind,
    quoted: bool,
    shell: &mut Shell,
) -> ExpansionResult {
    use ParamModifier as PM;
    use SubscriptKind as SK;

    // Snapshot helpers: collect_values yields values in insertion order;
    // collect_keys yields string keys in insertion order. Clones once
    // up-front so the rest of the function can borrow `shell` mutably for
    // sub-expansions (e.g., modifier word evaluation).
    let pairs: Vec<(String, String)> = shell
        .get_associative(name)
        .map(|v| v.clone())
        .unwrap_or_default();
    let values: Vec<String> = pairs.iter().map(|(_, v)| v.clone()).collect();
    let keys: Vec<String> = pairs.iter().map(|(k, _)| k.clone()).collect();

    match (modifier, subscript) {
        // ${m[@]} / ${m[*]}
        (PM::None, SK::All) => ExpansionResult::WordList(values),
        (PM::None, SK::Star) => {
            let ifs = shell.lookup_var("IFS").unwrap_or_else(|| " \t\n".to_string());
            let sep = ifs.chars().next().unwrap_or(' ').to_string();
            ExpansionResult::Value(values.join(&sep))
        }
        // ${m[k]} — string-key lookup
        (PM::None, SK::Index(w)) => {
            let key = eval_subscript_key(w, shell);
            let val = shell.lookup_associative_element(name, &key);
            if val.is_none() && shell.shell_options.nounset {
                let msg = format!("{name}[{key}]: unbound variable");
                eprintln!("huck: {msg}");
                shell.pending_fatal_pe_error = Some(1);
                return ExpansionResult::Fatal { status: 1 };
            }
            ExpansionResult::Value(val.unwrap_or_default())
        }
        // ${#m[@]} / ${#m[*]}
        (PM::Length, SK::All) | (PM::Length, SK::Star) => {
            ExpansionResult::Value(pairs.len().to_string())
        }
        // ${#m[k]}
        (PM::Length, SK::Index(w)) => {
            let key = eval_subscript_key(w, shell);
            let val = shell.lookup_associative_element(name, &key).unwrap_or_default();
            ExpansionResult::Value(val.chars().count().to_string())
        }
        // ${!m[@]} / ${!m[*]} — string keys
        (PM::IndirectKeys, SK::All) => {
            if quoted {
                ExpansionResult::WordList(keys)
            } else {
                let ifs = shell.lookup_var("IFS").unwrap_or_else(|| " \t\n".to_string());
                let sep = ifs.chars().next().unwrap_or(' ').to_string();
                ExpansionResult::Value(keys.join(&sep))
            }
        }
        (PM::IndirectKeys, SK::Star) => {
            let ifs = shell.lookup_var("IFS").unwrap_or_else(|| " \t\n".to_string());
            let sep = ifs.chars().next().unwrap_or(' ').to_string();
            ExpansionResult::Value(keys.join(&sep))
        }
        // ${m[@]:o:l} — slicing in insertion order
        (PM::Substring { offset, length }, SK::All) | (PM::Substring { offset, length }, SK::Star) => {
            let sliced = match slice_word_list(&values, offset, length.as_deref(), shell) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("huck: {name}: {e}");
                    shell.pending_fatal_pe_error = Some(1);
                    return ExpansionResult::Fatal { status: 1 };
                }
            };
            if matches!(subscript, SK::All) && quoted {
                ExpansionResult::WordList(sliced)
            } else {
                let ifs = shell.lookup_var("IFS").unwrap_or_else(|| " \t\n".to_string());
                let sep = ifs.chars().next().unwrap_or(' ').to_string();
                ExpansionResult::Value(sliced.join(&sep))
            }
        }
        // Modifier on @/*: v72 doesn't ship per-element modifiers (same
        // deferral as v71). Emit non-fatal diagnostic.
        (other, SK::All | SK::Star) => {
            eprintln!(
                "huck: ${{{name}[…]}}: modifier {:?} not supported on associative array in v72",
                other
            );
            ExpansionResult::Value(String::new())
        }
        // Modifier on a specific key: reuse expand_modifier_with_value
        (modif, SK::Index(w)) => {
            let key = eval_subscript_key(w, shell);
            let val = shell.lookup_associative_element(name, &key);
            crate::param_expansion::expand_modifier_with_value(
                name,
                modif,
                val.as_deref(),
                shell,
            )
        }
    }
}
```

- [ ] **Step 3: Add unit tests in `mod assoc_expansion_tests`**

Append to `src/expand.rs`. Use the existing test harness — find `expand_for_test` and `expand_to_word_list_for_test` in the file (the v71 test harness) and reuse them.

```rust
#[cfg(test)]
mod assoc_expansion_tests {
    use super::*;
    use crate::shell_state::Shell;

    fn shell_with_m() -> Shell {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        s.set_associative_element("m", "first".into(), "x".into()).unwrap();
        s.set_associative_element("m", "second".into(), "y".into()).unwrap();
        s.set_associative_element("m", "third".into(), "z".into()).unwrap();
        s
    }

    #[test]
    fn read_element_by_string_key() {
        let mut s = shell_with_m();
        let out = expand_for_test(&mut s, "${m[second]}");
        assert_eq!(out, "y");
    }

    #[test]
    fn missing_key_is_empty() {
        let mut s = shell_with_m();
        let out = expand_for_test(&mut s, "${m[nope]}");
        assert_eq!(out, "");
    }

    #[test]
    fn quoted_at_yields_values_in_insertion_order() {
        let mut s = shell_with_m();
        let words = expand_to_word_list_for_test(&mut s, r#""${m[@]}""#);
        assert_eq!(words, vec!["x", "y", "z"]);
    }

    #[test]
    fn quoted_star_joins_values_in_insertion_order() {
        let mut s = shell_with_m();
        let out = expand_for_test(&mut s, r#""${m[*]}""#);
        assert_eq!(out, "x y z");
    }

    #[test]
    fn count_returns_pair_count() {
        let mut s = shell_with_m();
        let out = expand_for_test(&mut s, "${#m[@]}");
        assert_eq!(out, "3");
    }

    #[test]
    fn keys_list_returns_string_keys_in_insertion_order() {
        let mut s = shell_with_m();
        let words = expand_to_word_list_for_test(&mut s, r#""${!m[@]}""#);
        assert_eq!(words, vec!["first", "second", "third"]);
    }

    #[test]
    fn element_length_for_associative() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        s.set_associative_element("m", "k".into(), "hello".into()).unwrap();
        let out = expand_for_test(&mut s, "${#m[k]}");
        assert_eq!(out, "5");
    }

    #[test]
    fn slicing_returns_values_in_insertion_order() {
        let mut s = shell_with_m();
        let words = expand_to_word_list_for_test(&mut s, r#""${m[@]:1:1}""#);
        assert_eq!(words, vec!["y"]);
    }

    #[test]
    fn bare_name_returns_empty_for_associative() {
        let mut s = shell_with_m();
        let out = expand_for_test(&mut s, "${m}");
        assert_eq!(out, "");
    }

    #[test]
    fn variable_subscript_expands_as_string() {
        let mut s = shell_with_m();
        s.set("k", "second".into());
        let out = expand_for_test(&mut s, "${m[$k]}");
        assert_eq!(out, "y");
    }

    #[test]
    fn nounset_on_missing_key_fires_pe_error() {
        let mut s = shell_with_m();
        s.shell_options.nounset = true;
        let _ = expand_for_test(&mut s, "${m[nope]}");
        assert!(s.pending_fatal_pe_error.is_some());
    }

    #[test]
    fn modifier_on_missing_key_uses_default() {
        let mut s = shell_with_m();
        let out = expand_for_test(&mut s, "${m[nope]:-fallback}");
        assert_eq!(out, "fallback");
    }
}
```

- [ ] **Step 4: Build and verify**

Run: `cargo build 2>&1 | tail -5`
Expected: clean.

Run: `cargo test --bin huck assoc_expansion 2>&1 | tail -20`
Expected: 12 tests pass.

Run: `cargo test 2>&1 | grep -E "test result|FAILED" | head -10`
Expected: all green. Total ~2048 (2036 from Task 1 + 12 new).

Run: `cargo clippy --all-targets 2>&1 | tail -5`
Expected: 0 warnings.

- [ ] **Step 5: Commit**

```bash
git add src/expand.rs
git commit -m "$(cat <<'EOF'
expand: associative array reads (v72 task 2)

New eval_subscript_key helper (string expansion, no arith). New
expand_assoc_param routed from expand_array_param when the variable
is Associative. Covers all 9 expansion forms — ${m[k]}, ${m[@]} /
${m[*]} quoted+unquoted, ${#m[@]}, ${!m[@]} (returns string keys),
${#m[k]}, ${m[@]:o:l} slicing, bare ${m} (empty), modifier on element.
Nounset fires for missing keys with `huck: NAME[key]: unbound variable`.

No assignment paths yet — Task 3 wires those.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Assignment execution

**Files:**
- Modify: `src/executor.rs` (`apply_one_assignment` 3-way dispatch; `build_associative_map` helper)
- Modify: `src/builtins.rs` (associative-aware `builtin_unset` for `unset m[k]`)

**Goal:** Associative arrays can be UPDATED, APPENDED, partially UNSET. Bash type-mismatch rules enforced (e.g., positional-list `m=(x y z)` rejected on associative). After this task, `declare -A m; m[foo]=bar; echo "${m[foo]}"` works end-to-end (the `declare -A` part still requires Task 4 to be fully wired through the builtin, but `Shell::declare_associative` from Task 1 is callable in tests).

- [ ] **Step 1: Add `build_associative_map` helper in `src/executor.rs`**

Near the existing `build_array_map` (around line 2890):

```rust
/// Builds an associative-array initializer from the compound literal's
/// elements. Each element MUST have an explicit subscript ([key]=value);
/// positional elements (no subscript) are an error.
fn build_associative_map(
    elements: &[crate::lexer::ArrayLiteralElement],
    shell: &mut Shell,
) -> Result<Vec<(String, String)>, ()> {
    let mut out: Vec<(String, String)> = Vec::new();
    for e in elements {
        let key = match &e.subscript {
            Some(sw) => crate::expand::eval_subscript_key(sw, shell),
            None => {
                eprintln!("huck: associative array initializer requires [key]=value form");
                return Err(());
            }
        };
        let val = crate::param_expansion::expand_word_to_string(&e.value, shell);
        if let Some(slot) = out.iter_mut().find(|(k, _)| k == &key) {
            slot.1 = val;
        } else {
            out.push((key, val));
        }
    }
    Ok(out)
}
```

- [ ] **Step 2: Extend `apply_one_assignment` with associative dispatch**

Find `apply_one_assignment` (around line 2813). The current dispatch has 4 cases (target × value-shape). Extend with a variant-aware sub-dispatch:

```rust
pub(crate) fn apply_one_assignment(
    a: &crate::command::Assignment,
    shell: &mut Shell,
) -> Result<(), ()> {
    let trailing_array_literal: Option<&Vec<crate::lexer::ArrayLiteralElement>> =
        a.value.0.last().and_then(|wp| {
            if let crate::lexer::WordPart::ArrayLiteral(els) = wp { Some(els) } else { None }
        });
    let target_name = a.target.name();
    let is_associative = shell.get_associative(target_name).is_some();

    match (&a.target, trailing_array_literal, is_associative) {
        // ───── Associative + compound RHS ─────
        (AssignTarget::Bare(name), Some(elements), true) => {
            // declare -A m=([k]=v ...) OR m=([k]=v ...) on an existing assoc.
            if a.append {
                // m+=([k]=v ...) — merge into existing.
                let new_pairs = build_associative_map(elements, shell)?;
                for (k, v) in new_pairs {
                    shell.set_associative_element(name, k, v).map_err(|_| ())?;
                }
                Ok(())
            } else {
                // m=([k]=v ...) — replace.
                let pairs = build_associative_map(elements, shell)?;
                shell.replace_associative(name, pairs).map_err(|_| ())
            }
        }

        // ───── Associative + scalar RHS (Bare) ─────
        (AssignTarget::Bare(name), None, true) => {
            // m=v or m+=v on an associative — bash error.
            eprintln!(
                "huck: {name}: {} on associative array",
                if a.append { "+=value" } else { "=value" }
            );
            Err(())
        }

        // ───── Associative + subscripted lvalue ─────
        (AssignTarget::Indexed { name, subscript }, None, true) => {
            // m[k]=v or m[k]+=v on associative — STRING subscript.
            let key = crate::expand::eval_subscript_key(subscript, shell);
            let val = crate::param_expansion::expand_word_to_string(&a.value, shell);
            if a.append {
                shell.append_associative_element(name, &key, &val).map_err(|_| ())
            } else {
                shell.set_associative_element(name, key, val).map_err(|_| ())
            }
        }

        // ───── Associative + subscripted compound RHS (m[k]=(...)) ─────
        (AssignTarget::Indexed { name, .. }, Some(_), true) => {
            eprintln!("huck: {name}: cannot assign array literal to associative array element");
            Err(())
        }

        // ───── Indexed/Scalar/unset paths (v71, unchanged) ─────
        (AssignTarget::Bare(name), Some(elements), false) => {
            // …existing v71 path: replace_array / append_array via build_array_map…
            if a.append {
                let values: Vec<String> = elements.iter()
                    .map(|e| crate::param_expansion::expand_word_to_string(&e.value, shell))
                    .collect();
                shell.append_array(name, &values).map_err(|_| ())
            } else {
                let map = build_array_map(elements, name, shell)?;
                shell.replace_array(name, map).map_err(|_| ())
            }
        }
        (AssignTarget::Bare(name), None, false) => {
            // …existing v71 path: try_set or scalar append…
            let s = crate::param_expansion::expand_word_to_string(&a.value, shell);
            if a.append {
                match shell.get_array(name) {
                    Some(_) => shell.append_array_element(name, 0, &s).map_err(|_| ()),
                    None => {
                        let existing = shell.get(name).map(|v| v.to_string()).unwrap_or_default();
                        shell.try_set(name, existing + &s).map_err(|_| ())
                    }
                }
            } else {
                shell.try_set(name, s).map_err(|_| ())
            }
        }
        (AssignTarget::Indexed { name, subscript }, None, false) => {
            // …existing v71 path: numeric subscript…
            let idx = match crate::expand::eval_subscript(subscript, shell, name) {
                Ok(i) => i,
                Err(e) => { eprintln!("huck: {e}"); return Err(()); }
            };
            let v = crate::param_expansion::expand_word_to_string(&a.value, shell);
            if a.append {
                shell.append_array_element(name, idx, &v).map_err(|_| ())
            } else {
                shell.set_array_element(name, idx, v).map_err(|_| ())
            }
        }
        (AssignTarget::Indexed { name, .. }, Some(_), false) => {
            eprintln!("huck: {name}: cannot assign array literal to array element");
            Err(())
        }
    }
}
```

(The non-associative arms are textually identical to the existing v71 implementation — just guarded by the new `is_associative` boolean. Do NOT change their behavior.)

- [ ] **Step 3: Update `builtin_unset` for associative dispatch**

In `src/builtins.rs`, find `builtin_unset` (search for `fn builtin_unset` — should be around line 360-420). The current shape (post-Task 4 fix-up in v71):

```rust
Ok(Some((name, sub_text))) => {
    let sub_word = crate::lexer::Word(vec![
        crate::lexer::WordPart::Literal { text: sub_text.to_string(), quoted: false }
    ]);
    match crate::expand::eval_subscript(&sub_word, shell, name) {
        Ok(idx) => { /* shell.unset_array_element ... */ }
        Err(e) => { /* error path */ }
    }
}
```

Replace with variant-aware dispatch:

```rust
Ok(Some((name, sub_text))) => {
    let sub_word = crate::lexer::Word(vec![
        crate::lexer::WordPart::Literal { text: sub_text.to_string(), quoted: false }
    ]);
    if shell.get_associative(name).is_some() {
        let key = crate::expand::eval_subscript_key(&sub_word, shell);
        if shell.unset_associative_element(name, &key).is_err() {
            return ExecOutcome::Continue(1);
        }
    } else {
        match crate::expand::eval_subscript(&sub_word, shell, name) {
            Ok(idx) => {
                if shell.unset_array_element(name, idx).is_err() {
                    return ExecOutcome::Continue(1);
                }
            }
            Err(e) => {
                eprintln!("huck: unset: {e}");
                return ExecOutcome::Continue(1);
            }
        }
    }
}
```

- [ ] **Step 4: Add unit tests in `mod assoc_assign_tests`**

Append to `src/executor.rs`:

```rust
#[cfg(test)]
mod assoc_assign_tests {
    use super::*;
    use crate::shell_state::Shell;

    fn run(shell: &mut Shell, line: &str) {
        crate::shell::process_line(line, shell, false);
    }

    #[test]
    fn element_assign_on_declared_associative_uses_string_key() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        run(&mut s, "m[foo]=bar");
        assert_eq!(s.lookup_associative_element("m", "foo"), Some("bar".into()));
    }

    #[test]
    fn element_assign_without_declare_creates_indexed() {
        // Bash gotcha: `m[foo]=v` on unset `m` creates indexed (foo→0).
        let mut s = Shell::new();
        run(&mut s, "m[foo]=bar");
        assert!(s.get_array("m").is_some());
        assert!(s.get_associative("m").is_none());
        assert_eq!(s.lookup_array_element("m", 0), Some("bar".into()));
    }

    #[test]
    fn compound_literal_on_associative_uses_keys() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        run(&mut s, "m=([a]=1 [b]=2)");
        assert_eq!(s.lookup_associative_element("m", "a"), Some("1".into()));
        assert_eq!(s.lookup_associative_element("m", "b"), Some("2".into()));
    }

    #[test]
    fn append_compound_on_associative_merges() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        run(&mut s, "m=([a]=1 [b]=2)");
        run(&mut s, "m+=([c]=3 [a]=99)");
        let pairs = s.get_associative("m").unwrap();
        assert_eq!(pairs.len(), 3);
        assert_eq!(s.lookup_associative_element("m", "a"), Some("99".into()));
        assert_eq!(s.lookup_associative_element("m", "c"), Some("3".into()));
    }

    #[test]
    fn append_element_on_associative_concatenates() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        run(&mut s, "m[k]=hello");
        run(&mut s, "m[k]+=_world");
        assert_eq!(s.lookup_associative_element("m", "k"), Some("hello_world".into()));
    }

    #[test]
    fn positional_literal_on_associative_rejects() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        s.set_associative_element("m", "preexisting".into(), "x".into()).unwrap();
        run(&mut s, "m=(a b c)");
        // associative `m` should be unchanged; positional literal is rejected.
        assert_eq!(s.lookup_associative_element("m", "preexisting"), Some("x".into()));
    }

    #[test]
    fn scalar_rhs_on_associative_rejects() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        s.set_associative_element("m", "k".into(), "v".into()).unwrap();
        run(&mut s, "m=newscalar");
        // associative `m` should be unchanged.
        assert_eq!(s.lookup_associative_element("m", "k"), Some("v".into()));
    }

    #[test]
    fn unset_associative_element_removes_one_key() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        run(&mut s, "m[a]=1");
        run(&mut s, "m[b]=2");
        run(&mut s, "m[c]=3");
        run(&mut s, "unset m[b]");
        let pairs = s.get_associative("m").unwrap();
        assert_eq!(pairs.len(), 2);
        assert!(s.lookup_associative_element("m", "b").is_none());
        assert_eq!(s.lookup_associative_element("m", "a"), Some("1".into()));
        assert_eq!(s.lookup_associative_element("m", "c"), Some("3".into()));
    }

    #[test]
    fn unset_whole_associative_removes_variable() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        run(&mut s, "m[a]=1");
        run(&mut s, "unset m");
        assert!(s.get_associative("m").is_none());
        assert!(s.get("m").is_none());
    }

    #[test]
    fn readonly_blocks_element_write_on_associative() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        s.set_associative_element("m", "a".into(), "1".into()).unwrap();
        s.mark_readonly("m");
        run(&mut s, "m[b]=2");
        assert!(s.lookup_associative_element("m", "b").is_none());
    }
}
```

- [ ] **Step 5: Build and verify**

Run: `cargo build 2>&1 | tail -5`
Expected: clean.

Run: `cargo test --bin huck assoc_assign 2>&1 | tail -20`
Expected: 10 tests pass.

Run: `cargo test 2>&1 | grep -E "test result|FAILED" | head -10`
Expected: all green. Total ~2058 (2048 + 10 new).

Run: `cargo clippy --all-targets 2>&1 | tail -5`
Expected: 0 warnings.

- [ ] **Step 6: Commit**

```bash
git add src/executor.rs src/builtins.rs
git commit -m "$(cat <<'EOF'
exec: associative array assignment paths (v72 task 3)

New build_associative_map (rejects positional-list elements). Extended
apply_one_assignment with variant-aware 3-way dispatch — when the
variable is currently Associative, subscripts are string-evaluated and
writes route through set_associative_element / append_associative_element
/ replace_associative. m=(x y z) and m=v on an associative reject with
"on associative array" diagnostic.

builtin_unset now routes `unset m[k]` through the string-key path when
m is associative.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Builtin wiring

**Files:**
- Modify: `src/builtins.rs` (`builtin_declare_decl` and `builtin_local_decl` gain `-A`; `format_declare_line` extended; `-Ai` / `-aA` rejections)

**Goal:** End-to-end `declare -A m`, `declare -A m=([k]=v)`, `declare -p m`, `local -A m`, `readonly m=([k]=v)`, `export m=([k]=v)` rejection — all working with bash-compatible diagnostics. Listing forms (`declare -A` bare) list associatives only.

- [ ] **Step 1: Extend `format_declare_line` for associative**

Find `format_declare_line` in `src/builtins.rs` (around line 668). Current shape handles `Scalar` and `Indexed`. Add the `Associative` arm:

```rust
fn format_declare_line(name: &str, var: &crate::shell_state::Variable) -> String {
    use crate::shell_state::VarValue;
    let mut attrs = String::new();
    // Attribute order matches bash: 'A' (assoc) or 'a' (indexed), then 'i', 'r', 'x'.
    if matches!(var.value, VarValue::Associative(_)) { attrs.push('A'); }
    if matches!(var.value, VarValue::Indexed(_)) { attrs.push('a'); }
    if var.integer { attrs.push('i'); }
    if var.readonly { attrs.push('r'); }
    if var.exported { attrs.push('x'); }
    let attr_part = if attrs.is_empty() { "--".to_string() } else { format!("-{attrs}") };
    let value_part = match &var.value {
        VarValue::Scalar(s) => {
            let escaped = escape_double_quote_value(s);
            format!("=\"{escaped}\"")
        }
        VarValue::Indexed(m) => {
            let mut parts: Vec<String> = Vec::new();
            for (k, v) in m {
                let escaped = escape_double_quote_value(v);
                parts.push(format!("[{k}]=\"{escaped}\""));
            }
            format!("=({})", parts.join(" "))
        }
        VarValue::Associative(pairs) => {
            let mut parts: Vec<String> = Vec::new();
            for (k, v) in pairs {
                let key_escaped = escape_double_quote_value(k);
                let val_escaped = escape_double_quote_value(v);
                parts.push(format!("[\"{key_escaped}\"]=\"{val_escaped}\""));
            }
            format!("=({})", parts.join(" "))
        }
    };
    format!("declare {attr_part} {name}{value_part}")
}
```

Preserve all existing behavior for Scalar and Indexed. The only change is the new Associative arm and the attribute-letter dispatch.

- [ ] **Step 2: Extend `builtin_declare_decl` with `-A` flag**

Find `builtin_declare_decl` (search the file). The function has a flag-parsing section near the top with `flags.array: bool` for `-a`. Add `flags.associative: bool` for `-A`. Find the per-name processing loop and add associative handling.

Sketch (the exact integration depends on the existing structure; preserve everything else):

```rust
// In the flag parser:
'A' => { flags.associative = true; }

// Pre-flight: -aA conflict.
if flags.array && flags.associative {
    eprintln!("huck: declare: cannot specify both -a and -A");
    return ExecOutcome::Continue(1);
}
// -Ai conflict.
if flags.associative && flags.integer {
    eprintln!("huck: declare: integer associative arrays not yet supported");
    return ExecOutcome::Continue(1);
}

// In the per-name loop:
if flags.associative {
    // declare -A NAME or declare -A NAME=(...)
    if let Some(assignment) = …current_assignment_for_this_name… {
        // declare -A NAME=([k]=v ...) — apply through executor's compound path.
        // First, ensure the variable is associative (declare it if unset).
        if shell.get_associative(name).is_none() {
            if shell.declare_associative(name).is_err() {
                return ExecOutcome::Continue(1);
            }
        }
        if crate::executor::apply_one_assignment(&assignment, shell).is_err() {
            return ExecOutcome::Continue(1);
        }
    } else {
        // declare -A NAME with no value: create empty (or no-op if already
        // associative; error on indexed/scalar).
        if shell.declare_associative(name).is_err() {
            return ExecOutcome::Continue(1);
        }
    }
    continue;
}
```

(The exact wiring depends on how the existing `-a` arm is structured. Mirror that pattern for `-A`. If the existing `-a` arm uses a different variable name for "current assignment", reuse the same name for `-A`.)

For bare `declare -A` (no names), list associatives only:

```rust
if flags.associative && names.is_empty() {
    let mut sorted: Vec<(&String, &crate::shell_state::Variable)> = shell.iter_vars()
        .filter(|(_, v)| matches!(v.value, crate::shell_state::VarValue::Associative(_)))
        .collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    for (k, v) in sorted {
        writeln!(out, "{}", format_declare_line(k, v)).ok();
    }
    return ExecOutcome::Continue(0);
}
```

- [ ] **Step 3: Extend `builtin_local_decl` with `-A` flag**

Find `builtin_local_decl` (around line 1096). It already handles `-a`. Mirror that pattern for `-A`. The snapshot machinery already clones the whole Variable (Task 1's VarValue is Clone), so scope restoration falls through automatically.

Sketch:

```rust
// In the flag parser:
if a == "-A" { want_associative = true; i += 1; continue; }

// Pre-flight conflict checks (mirror -a):
if want_array && want_associative {
    eprintln!("huck: local: cannot specify both -a and -A");
    return ExecOutcome::Continue(1);
}

// In the per-name loop:
if want_associative {
    if let Some(assignment) = … {
        if shell.get_associative(name).is_none() {
            if shell.declare_associative(name).is_err() {
                return ExecOutcome::Continue(1);
            }
        }
        if crate::executor::apply_one_assignment(&assignment, shell).is_err() {
            return ExecOutcome::Continue(1);
        }
    } else {
        if shell.declare_associative(name).is_err() {
            return ExecOutcome::Continue(1);
        }
    }
    continue;
}
```

- [ ] **Step 4: Confirm `builtin_readonly_decl` and `builtin_export_decl` work without changes**

`builtin_readonly_decl` routes through `apply_one_assignment` — Task 3 already taught that function to handle associative compound RHS. `readonly m=([k]=v)` should now work:

- If `m` is unset, `apply_one_assignment` hits the v71 Indexed branch (since `is_associative` is false on unset). This is WRONG for associative readonly — we need `readonly` to declare-associative first.

  Update `builtin_readonly_decl`: when the RHS is associative-shaped (has `[key]=value` elements that don't all look numeric? — actually we can't determine that statically), the user is expected to use `declare -A m; readonly m=([k]=v)` OR `readonly -A m=([k]=v)`. Bash supports `readonly -A`. Add the `-A` flag to `builtin_readonly_decl`:

```rust
// Pre-flight scan for -A in DeclArg::Plain entries:
let mut force_associative = false;
let mut leftover_args: Vec<&DeclArg> = Vec::new();
for arg in args {
    if let DeclArg::Plain(s) = arg
        && s == "-A"
    {
        force_associative = true;
    } else {
        leftover_args.push(arg);
    }
}

// For each leftover assignment:
//   if force_associative:
//     ensure shell.declare_associative(name) first, then apply.
//   else:
//     fall through to existing behavior (apply_one_assignment chooses
//     based on existing variant; defaults to Indexed for unset).
```

`builtin_export_decl` already rejects assoc compound RHS via the existing `assign_value_is_array` helper (it checks for trailing `WordPart::ArrayLiteral`, type-agnostic). No change needed — verify by reading the function.

- [ ] **Step 5: Add unit tests in `mod assoc_declare_tests`**

Append to `src/builtins.rs` near the existing `mod array_declare_tests`:

```rust
#[cfg(test)]
mod assoc_declare_tests {
    use super::*;
    use crate::shell_state::Shell;

    fn run(shell: &mut Shell, line: &str) -> ExecOutcome {
        crate::shell::process_line(line, shell, false)
    }

    #[test]
    fn declare_dash_A_creates_empty_associative() {
        let mut s = Shell::new();
        let _ = run(&mut s, "declare -A m");
        assert!(s.get_associative("m").is_some());
        assert_eq!(s.get_associative("m").unwrap().len(), 0);
    }

    #[test]
    fn declare_dash_A_with_value() {
        let mut s = Shell::new();
        let _ = run(&mut s, "declare -A m=([foo]=bar [baz]=qux)");
        assert_eq!(s.lookup_associative_element("m", "foo"), Some("bar".into()));
        assert_eq!(s.lookup_associative_element("m", "baz"), Some("qux".into()));
    }

    #[test]
    fn declare_p_formats_associative() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        s.set_associative_element("m", "k1".into(), "v1".into()).unwrap();
        s.set_associative_element("m", "k2".into(), "v2".into()).unwrap();
        let v = s.iter_vars().find(|(n, _)| n == &"m").unwrap().1;
        let line = format_declare_line("m", v);
        assert_eq!(line, r#"declare -A m=(["k1"]="v1" ["k2"]="v2")"#);
    }

    #[test]
    fn declare_dash_Ai_errors() {
        let mut s = Shell::new();
        let outcome = run(&mut s, "declare -Ai m");
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        assert!(s.get_associative("m").is_none());
    }

    #[test]
    fn declare_dash_aA_errors() {
        let mut s = Shell::new();
        let outcome = run(&mut s, "declare -aA m");
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn declare_dash_A_on_existing_indexed_errors() {
        let mut s = Shell::new();
        let _ = run(&mut s, "a=(x y z)");
        let outcome = run(&mut s, "declare -A a");
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        // Should remain indexed, not converted.
        assert!(s.get_array("a").is_some());
        assert!(s.get_associative("a").is_none());
    }

    #[test]
    fn declare_dash_A_on_existing_scalar_errors() {
        let mut s = Shell::new();
        let _ = run(&mut s, "s=hello");
        let outcome = run(&mut s, "declare -A s");
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn readonly_dash_A_creates_readonly_associative() {
        let mut s = Shell::new();
        let _ = run(&mut s, "readonly -A m=([k]=v)");
        assert!(s.get_associative("m").is_some());
        // Subsequent writes should be blocked.
        let _ = run(&mut s, "m[k2]=v2");
        assert!(s.lookup_associative_element("m", "k2").is_none());
    }

    #[test]
    fn export_associative_rejects() {
        let mut s = Shell::new();
        let outcome = run(&mut s, "export m=([k]=v)");
        assert!(matches!(outcome, ExecOutcome::Continue(1) | ExecOutcome::Exit(1)));
        assert!(s.get_associative("m").is_none());
    }
}
```

- [ ] **Step 6: Build and verify**

Run: `cargo build 2>&1 | tail -5`
Expected: clean.

Run: `cargo test --bin huck assoc_declare 2>&1 | tail -20`
Expected: 9 tests pass.

Run: `cargo test 2>&1 | grep -E "test result|FAILED" | head -10`
Expected: all green. Total ~2067 (2058 + 9 new).

Run: `cargo clippy --all-targets 2>&1 | tail -5`
Expected: 0 warnings.

- [ ] **Step 7: Commit**

```bash
git add src/builtins.rs
git commit -m "$(cat <<'EOF'
builtins: declare -A / local -A / readonly -A (v72 task 4)

- `declare -A NAME` / `declare -A NAME=(...)` create associative arrays
  with bash-compatible type-mismatch errors on existing indexed/scalar
- `declare -p NAME` formats associatives as
  `declare -A NAME=(["k1"]="v1" ["k2"]="v2")` in insertion order
- bare `declare -A` lists associative arrays only
- `declare -Ai` rejected with "integer associative arrays not yet supported"
- `declare -aA` rejected with "cannot specify both -a and -A"
- `local -A NAME` / `local -A NAME=(...)` use existing snapshot machinery
- `readonly -A NAME=(...)` creates readonly associative
- `export NAME=(...)` rejected (existing behavior covers all array shapes)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Integration tests + documentation

**Files:**
- Create: `tests/associative_arrays_integration.rs`
- Modify: `tests/scripts/arrays_diff_check.sh`
- Modify: `docs/bash-divergences.md`
- Modify: `README.md`

**Goal:** End-to-end binary-driven coverage; bash-diff harness extended; user-facing docs reflect M-83 and cross-references.

- [ ] **Step 1: Write integration tests at `tests/associative_arrays_integration.rs`**

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn declare_and_read_roundtrip() {
    let (out, _, _) = run_capture("declare -A m\nm[foo]=bar\necho \"${m[foo]}\"\nexit\n");
    assert!(out.lines().any(|l| l == "bar"), "got: {out:?}");
}

#[test]
fn iteration_order_is_insertion_order() {
    let (out, _, _) = run_capture(
        "declare -A m\nm[a]=1\nm[b]=2\nm[c]=3\nfor k in \"${!m[@]}\"; do echo $k; done\nexit\n"
    );
    let lines: Vec<&str> = out.lines().collect();
    let key_lines: Vec<&str> = lines.iter()
        .filter(|l| ["a","b","c"].contains(l))
        .copied()
        .collect();
    assert_eq!(key_lines, vec!["a", "b", "c"], "expected insertion-order, got: {out:?}");
}

#[test]
fn count_reflects_size_after_add_and_remove() {
    let (out, _, _) = run_capture(
        "declare -A m=([x]=1 [y]=2 [z]=3)\necho \"${#m[@]}\"\nunset m[y]\necho \"${#m[@]}\"\nexit\n"
    );
    let counts: Vec<&str> = out.lines().filter(|l| *l == "3" || *l == "2").collect();
    assert_eq!(counts, vec!["3", "2"], "got: {out:?}");
}

#[test]
fn append_element_concatenates() {
    let (out, _, _) = run_capture(
        "declare -A m\nm[k]=hello\nm[k]+=_world\necho \"${m[k]}\"\nexit\n"
    );
    assert!(out.lines().any(|l| l == "hello_world"), "got: {out:?}");
}

#[test]
fn append_compound_merges_keys() {
    let (out, _, _) = run_capture(
        "declare -A m=([a]=1)\nm+=([b]=2 [c]=3)\necho \"${#m[@]}\"\necho \"${m[b]}\"\nexit\n"
    );
    assert!(out.lines().any(|l| l == "3"), "expected count=3, got: {out:?}");
    assert!(out.lines().any(|l| l == "2"), "expected m[b]=2, got: {out:?}");
}

#[test]
fn unset_element_preserves_order_of_remaining() {
    let (out, _, _) = run_capture(
        "declare -A m=([first]=1 [middle]=2 [last]=3)\nunset m[middle]\nfor k in \"${!m[@]}\"; do echo $k; done\nexit\n"
    );
    let lines: Vec<&str> = out.lines().collect();
    let key_lines: Vec<&str> = lines.iter()
        .filter(|l| ["first","middle","last"].contains(l))
        .copied()
        .collect();
    assert_eq!(key_lines, vec!["first", "last"], "got: {out:?}");
}

#[test]
fn local_associative_scoped_to_function() {
    let (out, _, _) = run_capture(
        "declare -A m=([outer]=1)\n\
         f() { local -A m=([inner]=2); echo \"in: ${!m[@]}\"; }\n\
         f\n\
         echo \"out: ${!m[@]}\"\n\
         exit\n"
    );
    assert!(out.lines().any(|l| l == "in: inner"), "got: {out:?}");
    assert!(out.lines().any(|l| l == "out: outer"), "got: {out:?}");
}

#[test]
fn readonly_associative_blocks_element_write() {
    let (_out, err, _) = run_capture(
        "readonly -A m=([k]=v)\nm[k]=changed\nexit\n"
    );
    assert!(err.contains("readonly variable"), "got stderr: {err:?}");
}

#[test]
fn declare_dash_A_on_indexed_errors() {
    let (_out, err, _) = run_capture(
        "a=(x y z)\ndeclare -A a\necho rc=$?\nexit\n"
    );
    assert!(
        err.contains("cannot convert indexed to associative"),
        "got stderr: {err:?}"
    );
}

#[test]
fn nounset_on_missing_associative_key_is_fatal() {
    let (_out, err, rc) = run_capture(
        "set -u\ndeclare -A m=([k]=v)\necho \"${m[nope]}\"\nexit\n"
    );
    assert!(err.contains("unbound variable"), "got stderr: {err:?}");
    assert_ne!(rc, 0);
}
```

Run: `cargo test --test associative_arrays_integration 2>&1 | tail -15`
Expected: 10 tests pass.

- [ ] **Step 2: Extend bash-diff harness**

Edit `tests/scripts/arrays_diff_check.sh`. Find the `fragments=(` array and add 5 associative fragments at the end (before the closing `)`):

```bash
    'declare -A m=([foo]=bar [baz]=qux); echo "${m[foo]}"; echo "${#m[@]}"'
    'declare -A m; m[a]=1; m[b]=2; for k in "${!m[@]}"; do echo $k; done'
    'declare -A m; m[k]=hi; m[k]+=_bye; echo "${m[k]}"'
    'declare -A m=([x]=1 [y]=2); unset m[x]; for k in "${!m[@]}"; do echo $k; done; echo "${#m[@]}"'
    'declare -A m=([z]=1 [a]=2); m[k]=3; for k in "${!m[@]}"; do echo $k; done'
```

Run: `cargo build && bash tests/scripts/arrays_diff_check.sh`
Expected: "all array fragments produce identical output to bash". If ANY DIFF appears, debug the implementation, not the harness.

- [ ] **Step 3: Add M-83 entry to `docs/bash-divergences.md`**

Find the Tier-2 section. Add **M-83** after **M-82** (numeric order):

```markdown
- **M-83: Associative arrays** — `[fixed v72]` high. String-keyed, insertion-ordered associative arrays via `declare -A NAME` and `declare -A NAME=([k1]=v1 [k2]=v2)`. Element access `${m[key]}` with string-expanded subscripts (no arith — `m[foo]` uses key `"foo"`, not arith-eval-of-`foo`). All-elements `${m[@]}` (WordList in insertion order, no IFS-split when quoted) and `${m[*]}` (IFS-joined). Count `${#m[@]}`. Keys `${!m[@]}` returns string keys in insertion order. Element length `${#m[k]}`. Slicing `${m[@]:offset:length}` slices the value-list in insertion order. Element assign `m[k]=v` preserves existing key's position on update; append `m[k]+=v` concatenates. Append-compound `m+=([k]=v)` merges (existing keys updated in place, new keys appended). `unset m[k]` removes one key, preserving others' order. `local -A NAME[=(...)]` and `readonly -A NAME=(...)` integrate with v52/v54 surfaces. Type-mismatch errors: `declare -A` on existing indexed → "cannot convert indexed to associative array"; on existing scalar → "cannot convert scalar to associative"; positional-list `m=(x y)` on associative → rejected with "must use [key]=value form"; scalar `m=v` on associative → rejected. The bash gotcha is matched: `m[foo]=v` on unset `m` creates an **indexed** array with `m[0]=v` (arith subscript), because string-key semantics require explicit `declare -A` first. Internal storage `VarValue::Associative(Vec<(String, String)>)` for insertion-order preservation; linear `O(n)` ops are fine for shell-typical sizes. **Deferred**: `mapfile -d` (still future); `read -A` (the associative sibling of `read -a`); `BASH_REMATCH` array population (still pending from v71); per-element substitution `${m[@]/pat/repl}` and case-mod `${m[@]^^}` (deferred from v71 for both array types); integer attribute on associatives (`declare -Ai` rejected); exporting associatives (`export m=([k]=v)` rejected).
```

- [ ] **Step 4: Update existing entries with cross-references**

`docs/bash-divergences.md` edits:

- **M-82 (indexed arrays, v71)**: find the line in the Deferred section listing "associative arrays / `declare -A` (v72 candidate)". Change to "associative arrays — shipped v72, see M-83".
- **M-79 (declare)**: the `-A` row currently says "deferred"; change to "fixed v72". The `-Ai` row should be "rejected per scope" (or similar — match the language used for `-ai`).

Use `grep -n "M-82\|M-79" docs/bash-divergences.md` to find the exact entries.

- [ ] **Step 5: Add change-log entry**

At the END of `docs/bash-divergences.md`, after the existing change-log entries:

```markdown
- **2026-06-01**: M-83 (associative arrays) shipped as v72. New `VarValue::Associative(Vec<(String, String)>)` variant with insertion-order semantics (linear `O(n)` ops; matches bash 4.0+ hash-table order). No parser/lexer changes — v71's `m[k]=v`, `${m[k]}`, `m+=(...)`, `unset m[k]` machinery is reused. New `eval_subscript_key` helper (string expansion, no arith); new `expand_assoc_param` mirrors `expand_array_param` with string-key semantics. `apply_one_assignment` gains a 3-way variant dispatch — associative variables use string subscripts, indexed/scalar/unset use v71's arith subscripts. Six new Shell mutators (`get_associative`, `lookup_associative_element`, `set_associative_element`, `append_associative_element`, `unset_associative_element`, `replace_associative`, `declare_associative`) with readonly + type-mismatch checks. New `DeclareErr` enum for the type-mismatch error paths. `builtin_declare_decl` / `builtin_local_decl` / `builtin_readonly_decl` gain `-A` flag; `format_declare_line` extended with associative arm (`declare -A NAME=(["k"]="v" ...)`). Bash gotcha matched: `m[foo]=v` on unset `m` creates **indexed** (arith subscript) — string-key semantics require explicit `declare -A` first. ~42 unit tests across `assoc_value_tests` (Task 1), `assoc_expansion_tests` (Task 2), `assoc_assign_tests` (Task 3), `assoc_declare_tests` (Task 4). 10 binary-driven integration tests in `tests/associative_arrays_integration.rs`. `tests/scripts/arrays_diff_check.sh` extended with 5 associative fragments — all byte-identical to bash. Deferred per M-83: `mapfile -d`, `read -A`, `BASH_REMATCH`, per-element substitution/case-mod, integer associatives, exporting associatives.
```

- [ ] **Step 6: Add v72 row to `README.md`**

Find the version-iteration table (after the v71 row added in the v71 task):

```markdown
| v71       | indexed arrays (M-82)                                          |
| v72       | associative arrays (M-83)                                      |
```

Match the column widths of existing rows.

- [ ] **Step 7: Full verification**

```bash
cargo build 2>&1 | tail -5
cargo test 2>&1 | grep -E "test result|FAILED" | tail -10
cargo clippy --all-targets 2>&1 | tail -5
bash tests/scripts/arrays_diff_check.sh
```

All four should pass. Total test count should be ~2077 (2067 + 10 integration tests).

- [ ] **Step 8: Commit**

```bash
git add tests/associative_arrays_integration.rs tests/scripts/arrays_diff_check.sh docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs+tests: associative arrays shipped v72 (M-83)

10 binary-driven integration tests covering declare/read roundtrip,
insertion-order iteration, count after add+remove, append-element
and append-compound, unset-element order preservation, local -A
scope restoration, readonly enforcement, declare -A on indexed
type-mismatch, and nounset on missing keys.

New M-83 entry in bash-divergences.md plus cross-references on
M-82 (v72 candidate → shipped v72) and M-79 (declare -A fixed v72).
Change-log entry. README v72 row.

bash-diff harness extended with 5 associative fragments; all
byte-identical to bash 5.2.21 output.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final verification & merge prep

- [ ] **Step 1: Full test pass**

Run: `cargo test 2>&1 | grep -E "test result|FAILED" | tail -10`
Expected: 0 failures.

- [ ] **Step 2: Clippy clean**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: no warnings.

- [ ] **Step 3: Bash-diff harness passes**

Run: `bash tests/scripts/arrays_diff_check.sh`
Expected: "all array fragments produce identical output to bash".

- [ ] **Step 4: Verify M-83 entry well-formed**

Run: `grep -n "M-83" docs/bash-divergences.md | head -3`
Expected: at least Tier-2 entry + change-log entry.

- [ ] **Step 5: Confirm v72 row in README**

Run: `grep "v72" README.md`
Expected: one row with `associative arrays (M-83)`.

- [ ] **Step 6: Ask user for merge confirmation via AskUserQuestion (controller, NOT subagent)**

Per the v52-v71 workflow.

- [ ] **Step 7: On approval, merge to main**

```bash
git checkout main
git merge --no-ff v72-assoc-arrays -m "Merge v72: associative arrays (M-83)"
git push origin main
git branch -d v72-assoc-arrays
```

- [ ] **Step 8: Post-merge memory update**

Update `/home/john/.claude/projects/-home-john-projects-shuck/memory/MEMORY.md` and `project_huck_iterations.md` with the v72 entry.

---

## Notes for the implementer

1. **Subagent isolation**: each task is implemented by a fresh subagent. The plan body is their only context (plus the spec, if they want to read it). Don't assume they remember v71's design.

2. **TDD discipline**: write the failing test first when adding new behavior. Run, see fail, make pass.

3. **Reusing v71 paths**: most of v72 is *additive* — extend dispatches, add new variants, leave existing arms untouched. If you find yourself rewriting v71 code, stop and reconsider. The variant-aware dispatch should branch on `is_associative` and fall through to the existing v71 path for indexed/scalar/unset.

4. **The gotcha** (`m[foo]=v` on unset → indexed): this is bash-faithful behavior. Don't add "helpfulness" that diverges. The bash-diff harness in Task 5 will catch divergences.

5. **Code-quality reviewer notes** (anticipate these):
   - "Why is the dispatch `if shell.get_associative(name).is_some()`?" — Because we need the variant check BEFORE deciding subscript semantics. The match could be expanded to take the variant directly, but the `is_some()` check is cleaner for the dispatch boundary.
   - "Why `Vec<(String, String)>` instead of `IndexMap`?" — No new dep; matches bash exactly; small array sizes make O(n) fine; deterministic for tests.
   - "Why does `scalar_view` return `""` for associative?" — Bash: `$m` on associative is empty. Documented in the spec.

6. **Spec-compliance reviewer notes**: verify all 9 expansion forms work; verify 3 assignment variants (compound, element, append) work; verify type-mismatch errors fire for the four cases (declare -A on indexed/scalar, positional-list on assoc, scalar RHS on assoc).
