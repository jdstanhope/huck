# v158: `local`/`declare` case-fold attributes (`-l` / `-u`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the `-l` (lowercase-on-assignment) and `-u` (uppercase-on-assignment) attributes for `declare`/`local`/`typeset`, matching bash byte-for-byte.

**Architecture:** Add a `case_fold: Option<CaseFold>` field to the `Variable` struct (mutually exclusive Lower/Upper by type). Every user assignment already routes through one of five storage mutators on `Shell` (`try_set`, `set_array_element`, `set_associative_element`, `replace_array`, `extend_indexed`); fold the value there, keyed on the target variable's own `case_fold` attribute, *after* any `-i` integer coercion. Flag parsing in the four declaration builtins computes the net `-l`/`-u`/`+l`/`+u` effect per command and stamps the attribute via a new `set_case_fold` mutator. `declare -p` emits `l`/`u` in the flag string.

**Tech Stack:** Rust, the huck shell codebase. Case folding reuses Rust's `str::to_lowercase`/`to_uppercase` — byte-identical to the existing `${v^^}`/`${v,,}` `case_modify` helper for the whole-string no-pattern case (param_expansion.rs:585-590 maps each char through `to_uppercase`/`to_lowercase`, exactly what `str::to_uppercase` does), so it inherits the documented L-04 Unicode behavior with no cross-module dependency.

**Spec:** `docs/superpowers/specs/2026-06-15-declare-local-case-attrs-design.md` — read it for the full bash-behavior table.

**Key reference: the five storage mutators (all in `src/shell_state.rs`)**
- `try_set` (~1176) — scalar assign + scalar `+=` (executor.rs:5485/5489 call it).
- `set_array_element` (~1555) — indexed element assign; `append_array_element` (~1661) calls it, so `a[i]+=v` is covered.
- `set_associative_element` (~1715) — assoc value assign; `append_associative_element` (~1751) calls it.
- `replace_array` (~1525) — whole `a=(…)` array literal.
- `extend_indexed` (~1612) — `a+=(…)` array append.

---

## Task 1: Add the `CaseFold` type, the `Variable.case_fold` field, and the fold helper

**Files:**
- Modify: `src/shell_state.rs` (the `Variable` struct ~58-77, the `Variable::scalar` constructor ~69, and all 23 `Variable { … }` struct literals)

- [ ] **Step 1: Add the `CaseFold` enum and the `case_fold` field**

In `src/shell_state.rs`, just above `pub struct Variable` (~line 58), add:

```rust
/// The case-fold attribute set by `declare -l` / `declare -u`. Mutually
/// exclusive by construction — a variable is Lower, Upper, or neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseFold {
    Lower,
    Upper,
}
```

Then add the field to `Variable`:

```rust
#[derive(Debug, Clone)]
pub struct Variable {
    pub value: VarValue,
    pub exported: bool,
    pub readonly: bool,
    pub integer: bool,
    pub case_fold: Option<CaseFold>,
}
```

And update the `Variable::scalar` constructor (~line 69) to initialize it:

```rust
    pub fn scalar(value: String) -> Self {
        Variable {
            value: VarValue::Scalar(value),
            exported: false,
            readonly: false,
            integer: false,
            case_fold: None,
        }
    }
```

- [ ] **Step 2: Compile to find every struct literal that needs the field**

Run: `cargo build 2>&1 | grep -A2 'missing field' | head -60`
Expected: a list of E0063 "missing field `case_fold`" errors, one per `Variable { … }` literal in `src/shell_state.rs` (there are 23; some are in `#[cfg(test)]` blocks). Note: `CompletionContext::Variable { prefix }` in `completion.rs` is an unrelated enum variant — do NOT touch it.

- [ ] **Step 3: Add `case_fold: None` to every flagged `Variable { … }` literal**

For each E0063 site the compiler reports in `src/shell_state.rs`, add `case_fold: None,` to the struct literal (alongside `integer: false,`). These are at approximately lines 582, 1019, 1041, 1233, 1257, 1291, 1302, 1353, 1409, 1420, 1542, 1594, 1637, 1802, 1827, 1968, and the test literals 2493, 2519, 2544, 2598. All get the same `case_fold: None`. Re-run `cargo build` until there are zero E0063 errors.

- [ ] **Step 4: Add the `apply_case_fold` free function with a unit test**

At the bottom of the non-test code in `src/shell_state.rs` (near `eval_integer_coerce` ~1909, as a free `fn`), add:

```rust
/// Applies a variable's case-fold attribute to a value string. `None`
/// returns the value unchanged. Lower/Upper use Rust's whole-string
/// `to_lowercase`/`to_uppercase`, which is byte-identical to the
/// `${v,,}`/`${v^^}` `case_modify` helper for the no-pattern case (and
/// therefore inherits the same documented L-04 Unicode behavior).
fn apply_case_fold(fold: Option<CaseFold>, value: &str) -> String {
    match fold {
        None => value.to_string(),
        Some(CaseFold::Lower) => value.to_lowercase(),
        Some(CaseFold::Upper) => value.to_uppercase(),
    }
}
```

In the `#[cfg(test)] mod tests` block of `src/shell_state.rs`, add:

```rust
    #[test]
    fn apply_case_fold_lower_upper_and_none() {
        assert_eq!(apply_case_fold(None, "AbC"), "AbC");
        assert_eq!(apply_case_fold(Some(CaseFold::Lower), "AbC"), "abc");
        assert_eq!(apply_case_fold(Some(CaseFold::Upper), "AbC"), "ABC");
        // idempotent
        assert_eq!(apply_case_fold(Some(CaseFold::Lower), "abc"), "abc");
    }
```

- [ ] **Step 5: Run the test**

Run: `cargo test --lib apply_case_fold_lower_upper_and_none`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/shell_state.rs
git commit -m "v158 task 1: add CaseFold attribute field + apply_case_fold helper

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Add the `case_fold_of` reader and `set_case_fold` mutator

**Files:**
- Modify: `src/shell_state.rs` (alongside `mark_integer`/`unmark_integer` ~1245-1273)

- [ ] **Step 1: Write the failing test**

In the `#[cfg(test)] mod tests` block of `src/shell_state.rs`, add:

```rust
    #[test]
    fn set_case_fold_creates_and_clears() {
        let mut shell = Shell::new_for_test();
        // create-if-absent, like mark_integer
        shell.set_case_fold("x", Some(CaseFold::Lower));
        assert_eq!(shell.case_fold_of("x"), Some(CaseFold::Lower));
        // overwrite (later-wins mutual exclusivity is handled by the caller)
        shell.set_case_fold("x", Some(CaseFold::Upper));
        assert_eq!(shell.case_fold_of("x"), Some(CaseFold::Upper));
        // clear
        shell.set_case_fold("x", None);
        assert_eq!(shell.case_fold_of("x"), None);
        // unknown var reads None
        assert_eq!(shell.case_fold_of("nope"), None);
    }
```

Note: use whatever test-shell constructor the surrounding tests use (search the test module for `Shell::new` / a `new_for_test` helper and match it).

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib set_case_fold_creates_and_clears`
Expected: FAIL — `case_fold_of`/`set_case_fold` not found.

- [ ] **Step 3: Implement the reader and mutator**

After `unmark_integer` (~line 1273) in `src/shell_state.rs`, add:

```rust
    /// The case-fold attribute on `name`, or `None` if unset / no attribute.
    pub fn case_fold_of(&self, name: &str) -> Option<CaseFold> {
        self.vars.get(name).and_then(|v| v.case_fold)
    }

    /// Sets (or clears, with `None`) the case-fold attribute on `name`.
    /// Creates an empty scalar if the variable is unset, mirroring
    /// `mark_integer` (so `declare -l NAME` with no value declares it).
    pub fn set_case_fold(&mut self, name: &str, fold: Option<CaseFold>) {
        if let Some(v) = self.vars.get_mut(name) {
            v.case_fold = fold;
        } else {
            self.vars.insert(
                name.to_string(),
                Variable {
                    value: VarValue::Scalar(String::new()),
                    exported: false,
                    readonly: false,
                    integer: false,
                    case_fold: fold,
                },
            );
        }
    }
```

- [ ] **Step 4: Run the test**

Run: `cargo test --lib set_case_fold_creates_and_clears`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/shell_state.rs
git commit -m "v158 task 2: add case_fold_of reader + set_case_fold mutator

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Fold values in the five storage mutators

**Files:**
- Modify: `src/shell_state.rs` — `try_set` (~1176), `set_array_element` (~1555), `set_associative_element` (~1715), `replace_array` (~1525), `extend_indexed` (~1612)

The fold must read the *target's* `case_fold` attribute and apply *after* integer coercion. Because each mutator first needs an immutable read (`case_fold_of`) and then a mutable borrow, compute the fold value into a local before taking `&mut`, exactly as `try_set` already does for integer coercion.

- [ ] **Step 1: Write the failing tests**

In the `#[cfg(test)] mod tests` block of `src/shell_state.rs`, add:

```rust
    #[test]
    fn storage_mutators_apply_case_fold() {
        let mut shell = Shell::new_for_test();

        // scalar via try_set
        shell.set_case_fold("s", Some(CaseFold::Lower));
        shell.try_set("s", "ABCdef".to_string()).unwrap();
        assert_eq!(shell.get("s"), Some("abcdef"));

        // scalar += (try_set with concatenated value) folds the whole result
        shell.try_set("s", "abcdef".to_string() + "GHI").unwrap();
        assert_eq!(shell.get("s"), Some("abcdefghi"));

        // indexed element
        shell.set_case_fold("arr", Some(CaseFold::Lower));
        shell.set_array_element("arr", 1, "XYZ".to_string()).unwrap();
        assert_eq!(shell.lookup_array_element("arr", 1).as_deref(), Some("xyz"));

        // associative value folded, key NOT folded
        shell.set_case_fold("m", Some(CaseFold::Lower));
        shell.set_associative_element("m", "Key".to_string(), "VALUE".to_string()).unwrap();
        assert_eq!(shell.get_associative("m").unwrap().iter()
            .find(|(k, _)| k == "Key").map(|(_, v)| v.as_str()), Some("value"));

        // whole-array literal via replace_array, attribute preserved
        shell.set_case_fold("lit", Some(CaseFold::Lower));
        let mut map = std::collections::BTreeMap::new();
        map.insert(0usize, "ABC".to_string());
        map.insert(1usize, "DeF".to_string());
        shell.replace_array("lit", map).unwrap();
        assert_eq!(shell.lookup_array_element("lit", 0).as_deref(), Some("abc"));
        assert_eq!(shell.lookup_array_element("lit", 1).as_deref(), Some("def"));
        assert_eq!(shell.case_fold_of("lit"), Some(CaseFold::Lower)); // preserved

        // upper attribute through array append (extend_indexed)
        shell.set_case_fold("app", Some(CaseFold::Upper));
        let mut em = std::collections::BTreeMap::new();
        em.insert(0usize, "abc".to_string());
        shell.extend_indexed("app", em).unwrap();
        assert_eq!(shell.lookup_array_element("app", 0).as_deref(), Some("ABC"));
    }
```

Adjust accessor names (`get`, `lookup_array_element`, `get_associative`) to the real ones if they differ — grep the test module for existing usages and match them.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib storage_mutators_apply_case_fold`
Expected: FAIL — values are stored un-folded.

- [ ] **Step 3: Fold in `try_set`**

Rewrite the body of `try_set` (~1176) so the fold applies after integer coercion, in both the integer and non-integer branches. Replace the existing body (from the `do_integer_coerce` block through the final `else { insert }`) with:

```rust
        // Compute the final stored string: integer-coerce first (only on an
        // existing integer-flagged Scalar), then apply the case-fold attribute.
        let do_integer_coerce = self.is_integer(name)
            && self
                .vars
                .get(name)
                .is_some_and(|v| matches!(v.value, VarValue::Scalar(_)));
        let coerced = if do_integer_coerce {
            eval_integer_coerce(self, &value)
        } else {
            value
        };
        let folded = apply_case_fold(self.case_fold_of(name), &coerced);

        if do_integer_coerce {
            if let Some(existing) = self.vars.get_mut(name) {
                existing.value = VarValue::Scalar(folded);
            }
            return Ok(());
        }
        if let Some(existing) = self.vars.get_mut(name) {
            install_scalar_value(existing, folded);
            Ok(())
        } else {
            self.vars.insert(name.to_string(), Variable::scalar(folded));
            Ok(())
        }
```

(Keep the readonly check and the `reseed_special_on_assign` early-return that precede this block unchanged.)

- [ ] **Step 4: Fold in `set_array_element`**

In `set_array_element` (~1555), after the readonly check and before the `match self.vars.get_mut(name)`, compute the folded value:

```rust
        let value = apply_case_fold(self.case_fold_of(name), &value);
```

Then use this `value` in the existing match arms (they already move `value` into the map / scalar-promote). This covers `a[i]=v` and `a[i]+=v` (the latter calls `set_array_element` with the concatenated string).

- [ ] **Step 5: Fold in `set_associative_element` (value only, not key)**

In `set_associative_element` (~1715), after the readonly check, fold ONLY the value parameter (leave the key/subscript untouched):

```rust
        let value = apply_case_fold(self.case_fold_of(name), &value);
```

placed before the value is stored. Confirm the key variable is never passed through `apply_case_fold`.

- [ ] **Step 6: Fold + preserve the attribute in `replace_array`**

In `replace_array` (~1525), change the attribute-capture line and fold each element. Replace:

```rust
        let (exported, integer) = match self.vars.get(name) {
            Some(v) => (v.exported, v.integer),
            None => (false, false),
        };
        self.vars.insert(
            name.to_string(),
            Variable {
                value: VarValue::Indexed(elements),
                exported,
                readonly: false,
                integer,
            },
        );
```

with:

```rust
        let (exported, integer, case_fold) = match self.vars.get(name) {
            Some(v) => (v.exported, v.integer, v.case_fold),
            None => (false, false, None),
        };
        let elements = elements
            .into_iter()
            .map(|(k, v)| (k, apply_case_fold(case_fold, &v)))
            .collect();
        self.vars.insert(
            name.to_string(),
            Variable {
                value: VarValue::Indexed(elements),
                exported,
                readonly: false,
                integer,
                case_fold,
            },
        );
```

- [ ] **Step 7: Fold in `extend_indexed`**

In `extend_indexed` (~1612), the final loop inserts `entries` into the map. Fold each value as it is inserted. Capture the attribute before the `&mut` borrow and apply it in the loop. Change:

```rust
        if let Some(v) = self.vars.get_mut(name)
            && let VarValue::Indexed(m) = &mut v.value
        {
            for (idx, val) in entries {
                m.insert(idx, val);
            }
            Ok(())
        } else {
```

to:

```rust
        let fold = self.case_fold_of(name);
        if let Some(v) = self.vars.get_mut(name)
            && let VarValue::Indexed(m) = &mut v.value
        {
            for (idx, val) in entries {
                m.insert(idx, apply_case_fold(fold, &val));
            }
            Ok(())
        } else {
```

- [ ] **Step 8: Run the test**

Run: `cargo test --lib storage_mutators_apply_case_fold`
Expected: PASS. Then run `cargo test --lib` to confirm no regression in existing variable/array tests.
Expected: all pass.

- [ ] **Step 9: Commit**

```bash
git add src/shell_state.rs
git commit -m "v158 task 3: fold values in the five storage mutators

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Parse `-l`/`-u`/`+l`/`+u` flags in the declaration builtins

**Files:**
- Modify: `src/builtins.rs` — `builtin_declare_decl` (~1680, the not-yet-impl site ~1750), `builtin_local_decl` (~1369, site ~1397), `builtin_declare` (~954, site ~1001), `builtin_local` (~623)

The four declaration paths each scan attribute flags. Today `-l`/`-u`/`-n` hit a `not yet implemented` arm. Replace the `-l`/`-u` handling with attribute tracking. **`-n` keeps erroring** (deferred to v159) — leave `b'n'` in the not-yet-implemented arm.

bash semantics to reproduce (verified, see spec): within ONE command, if both `-l` and `-u` appear → set the attribute to `None` (clears any prior). If only one minus flag → that one (clearing the other, which `set_case_fold` does by overwrite). `+l` clears Lower, `+u` clears Upper.

- [ ] **Step 1: Determine the per-command net effect**

In each builtin's flag-scan loop, track four booleans across the whole command's flag args (do NOT apply per-flag; compute the net after scanning):
- `saw_minus_l`, `saw_minus_u` (from `-l` / `-u`, including grouped forms like `-lu`, `-il`)
- `saw_plus_l`, `saw_plus_u` (from `+l` / `+u`)

Remove the `l` and `u` bytes from the `not yet implemented` match arm (keep `n`). For the minus path, where `-i`/`-r` set their `want_integer`/`want_readonly` booleans, add `b'l' => saw_minus_l = true,` and `b'u' => saw_minus_u = true,`. For the plus path (where `+i`/`+r` are handled), add `b'l' => saw_plus_l = true,` and `b'u' => saw_plus_u = true,`. (Match the exact local-variable / control-flow style each builtin already uses for `-i`/`-r`.)

- [ ] **Step 2: Compute and apply the attribute per named variable**

After the flag scan, compute the net minus attribute:

```rust
let minus_case_fold: Option<Option<crate::shell_state::CaseFold>> = if saw_minus_l && saw_minus_u {
    Some(None) // both in one command cancel → clear
} else if saw_minus_l {
    Some(Some(crate::shell_state::CaseFold::Lower))
} else if saw_minus_u {
    Some(Some(crate::shell_state::CaseFold::Upper))
} else {
    None // no minus case flag in this command
};
```

For each NAME the command declares, apply the case attribute alongside where `-i`/`-r` are applied (the per-name apply loop that calls `mark_integer`/`mark_readonly`):

```rust
// minus form: set/clear the case attribute (do this BEFORE assigning the
// value so an inline `declare -u x=hello` folds the value on assignment).
if let Some(fold) = minus_case_fold {
    shell.set_case_fold(name, fold);
}
// plus form: clear only the matching attribute.
if saw_plus_l && shell.case_fold_of(name) == Some(crate::shell_state::CaseFold::Lower) {
    shell.set_case_fold(name, None);
}
if saw_plus_u && shell.case_fold_of(name) == Some(crate::shell_state::CaseFold::Upper) {
    shell.set_case_fold(name, None);
}
```

CRITICAL ORDERING: the `set_case_fold` call must happen BEFORE the value assignment for that name (so `declare -u x=hello` stamps Upper, then the `x=hello` assignment routes through `try_set` and folds to `HELLO`). Inspect how each builtin orders "apply attributes" vs "assign value" — `-i` already has this exact ordering requirement (it marks integer before assigning so the RHS arith-coerces), so place the `set_case_fold` call at the same point as `mark_integer`.

- [ ] **Step 3: Apply the same change to all four builtins**

Repeat Steps 1-2 in `builtin_declare_decl`, `builtin_local_decl`, `builtin_declare`, and `builtin_local`. They share the pattern but have separate flag-scan loops. `typeset` is an alias that routes through `builtin_declare`/`builtin_declare_decl` — verify it inherits the behavior (no separate change needed). If a builtin has no inline-value assignment path (e.g. a pure attribute-only form), the ordering note is moot there but harmless.

- [ ] **Step 4: Build and smoke-test against bash**

Run: `cargo build 2>&1 | tail -3 && echo BUILD_OK`
Expected: BUILD_OK.

Run these and compare to bash:
```bash
./target/debug/huck -c 'declare -l x; x=ABCdef; echo "$x"'        # abcdef
./target/debug/huck -c 'declare -u x=hello; echo "$x"'            # HELLO
./target/debug/huck -c 'declare -lu x; x=AbC; echo "$x"'          # AbC
./target/debug/huck -c 'declare -l x; declare -u x; x=AbC; echo "$x"'  # ABC
./target/debug/huck -c 'declare -l x; x=ABC; declare +l x; x=DEF; echo "$x"'  # DEF
./target/debug/huck -c 'f(){ local -u v=hi; echo "$v"; }; f'      # HI
./target/debug/huck -c 'declare -l arr; arr=(ABC DeF); echo "${arr[@]}"'  # abc def
```
Expected: each matches the bash comment.

- [ ] **Step 5: Commit**

```bash
git add src/builtins.rs
git commit -m "v158 task 4: parse -l/-u/+l/+u in declare/local/typeset

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Emit `-l`/`-u` in `declare -p`

**Files:**
- Modify: `src/builtins.rs` — `format_declare_line` (~778)

bash's flag order is `a/A`, `i`, `r`, `x`, then `l`/`u` LAST (verified: `declare -irxl a`, `declare -xl d`, `declare -ail c`).

- [ ] **Step 1: Add the `l`/`u` emission**

In `format_declare_line` (~781), after the `if var.exported { attrs.push('x'); }` block (~797), add:

```rust
    match var.case_fold {
        Some(crate::shell_state::CaseFold::Lower) => attrs.push('l'),
        Some(crate::shell_state::CaseFold::Upper) => attrs.push('u'),
        None => {}
    }
```

- [ ] **Step 2: Verify against bash**

Run: `cargo build 2>&1 | tail -1`
```bash
./target/debug/huck -c 'declare -l x; x=ab; declare -p x'    # declare -l x="ab"
./target/debug/huck -c 'declare -irxl a=1; declare -p a'     # declare -irxl a="1"
./target/debug/huck -c 'declare -lu x; declare -p x'         # declare -- x
```
Expected: each matches bash byte-for-byte.

- [ ] **Step 3: Commit**

```bash
git add src/builtins.rs
git commit -m "v158 task 5: emit -l/-u in declare -p output

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Bash-diff harness

**Files:**
- Create: `tests/scripts/local_case_attrs_diff_check.sh`

- [ ] **Step 1: Write the harness**

Model it on an existing harness's `check` helper (e.g. `tests/scripts/exec_diff_check.sh` — copy its header, `HUCK_BIN` resolution, and the stdout+exit `check` function). Create `tests/scripts/local_case_attrs_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for declare/local -l / -u case-fold attrs.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

chk() { local l="$1" f="$2" b h
    b=$(bash --norc --noprofile -c "$f" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$f" 2>/dev/null; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

chk "lower scalar"        'declare -l x; x=ABCdef; echo "$x"'
chk "upper scalar"        'declare -u x; x=ABCdef; echo "$x"'
chk "inline upper"        'declare -u x=hello; echo "$x"'
chk "not retroactive"     'x=ABC; declare -l x; echo "$x"'
chk "same-cmd cancel"     'declare -lu x; x=AbC; echo "$x"'
chk "same-cmd cancel -p"  'declare -lu x; declare -p x'
chk "last wins l->u"      'declare -l x; declare -u x; x=AbC; echo "$x"'
chk "last wins u->l"      'declare -u x; declare -l x; x=AbC; echo "$x"'
chk "remove +l"           'declare -l x; x=ABC; declare +l x; x=DEF; echo "$x"'
chk "array each elem"     'declare -l arr; arr=(ABC DeF GHI); echo "${arr[@]}"'
chk "array elem assign"   'declare -l arr; arr=(ABC DeF GHI); arr[1]=XYZ; echo "${arr[1]}"'
chk "array append"        'declare -u arr; arr=(ab); arr+=(cd ef); echo "${arr[@]}"'
chk "assoc value not key" 'declare -lA m; m[Key]=VALUE; echo "${m[Key]}"; echo "${!m[@]}"'  # value folded, key (Key) preserved; avoid declare -p (L-44 assoc-format divergence)
chk "integer then upper"  'declare -iu x; x=3+4; echo "$x"'
chk "scalar append fold"  'declare -l x; x=ABC; x+=DEF; echo "$x"'
chk "local lower scope"   'f(){ local -l v=HELLO; echo "$v"; }; f; echo "${v:-unset}"'
chk "local upper scope"   'f(){ local -u v=lo; echo "$v"; }; f; echo "${v:-unset}"'
chk "declare -p lower"    'declare -l x; x=ab; declare -p x'
chk "declare -p flags"    'declare -irxl a=1; declare -p a'
chk "typeset -u"          'typeset -u x=abc; echo "$x"'
chk "plus on cancelled"   'declare -lu x; declare +u x; x=AbC; echo "$x"'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
```

- [ ] **Step 2: Make it executable and run it**

Run: `chmod +x tests/scripts/local_case_attrs_diff_check.sh && cargo build 2>&1 | tail -1 && bash tests/scripts/local_case_attrs_diff_check.sh`
Expected: every line `PASS`, final `21 passed, 0 failed`. If any case FAILs, fix the implementation (Task 3/4/5) — the harness is the source of truth for bash parity. Do NOT weaken a case to make it pass; if bash genuinely does something the design didn't anticipate, note it and adjust.

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/local_case_attrs_diff_check.sh
git commit -m "v158 task 6: bash-diff harness for -l/-u case-fold attrs

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final verification (after all tasks)

- [ ] `cargo build` clean, `cargo clippy --quiet` clean.
- [ ] `cargo test` FULLY green.
- [ ] Every `tests/scripts/*_diff_check.sh` byte-identical (run them all; the suite is currently 84 + this new one = 85). No regressions — especially the existing `declare`/`local`/array harnesses.
- [ ] Spot-check that `-n` still errors with the not-yet-implemented message (deferred to v159): `./target/debug/huck -c 'declare -n r=x' 2>&1` should still print the not-yet-implemented diagnostic.

## Notes for the implementer

- **Why centralize in the five mutators:** every user assignment in `apply_inline_assignments` (executor.rs:5474-5506) routes through exactly these five functions, so folding there covers scalar/`+=`/array-element/array-literal/array-append/assoc uniformly without touching the executor. Do not scatter folding into the executor.
- **Not-retroactive falls out for free:** declaring the attribute calls `set_case_fold`, which does NOT write the value, so the stored value is untouched until the next assignment routes through a folding mutator.
- **Idempotence:** `apply_case_fold` is idempotent for case, so even if a value passes through two folding mutators it stays correct.
- **`Shell::new_for_test`:** if no such constructor exists, use whatever the existing `#[cfg(test)]` tests in `shell_state.rs` use to build a `Shell`; match the surrounding test style exactly.
```
