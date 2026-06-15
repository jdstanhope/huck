# v159: Unify the variable-assignment path — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route every variable assignment through one `Shell::assign(dest, op, source)` chokepoint that applies the cross-cutting concerns (readonly → nameref-seam → integer-coerce → case-fold → store) exactly once, with the existing leaf mutators reduced to thin wrappers.

**Architecture:** Pure behavior-preserving refactor. Add four small value types and one `assign()` method to `src/shell_state.rs`. Extract the data-structure manipulation from the current leaf mutators into private `store_*` primitives; move the readonly/attribute logic UP into `assign()`. Rewrite the 8 public leaf mutators as one-line wrappers over `assign()` so their ~115 call sites are untouched. Carve an identity `resolve_assign_target` seam for v160 nameref. The full `cargo test` suite + all 85 `tests/scripts/*_diff_check.sh` harnesses staying **byte-identical green** is the correctness proof — any output diff is a bug.

**Tech Stack:** Rust, the huck shell. Spec: `docs/superpowers/specs/2026-06-15-unify-assignment-path-design.md` (read its "Behavior-preservation invariants" section — it is the correctness contract).

**Note on TDD for a refactor:** the behavior already exists, so the EXISTING suite is the fail-safe. After each task the full suite must stay green (zero diffs). New tests (Task 4) are *characterization* tests — they assert current behavior through the new structure and should pass on first run.

**Build/test commands (huck is a bin crate — NO `--lib`):**
- Build: `cargo build 2>&1 | tail -3`
- Unit/integration tests: `cargo test 2>&1 | tail -5`
- Clippy: `cargo clippy --bins --quiet 2>&1 | tail -3`
- All harnesses: `for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 && echo "ok" || echo "FAIL $s"; done | sort | uniq -c`

---

## Reference: the current code being refactored (all in `src/shell_state.rs`)

- `AssignErr` enum (~line 85): `Readonly`, `BadSubscript` (`#[allow(dead_code)]`), `TypeMismatch`.
- `set(name, value)` (~810): raw scalar writer — `reseed_special_on_assign` then `install_scalar_value`/insert. NO readonly/attributes. ~124 internal call sites; STAYS as the raw path.
- `try_set(name, value) -> Result<(), ()>` (~1189): readonly → reseed → integer-coerce(only existing integer `Scalar`) → case-fold → `install_scalar_value`/insert.
- `replace_array(name, map) -> Result<(), AssignErr>` (~1573): readonly → capture `(exported, integer, case_fold)` → fold each value → insert fresh `Indexed` (readonly:false).
- `set_array_element(name, idx, value) -> Result<(), AssignErr>` (~1608): readonly → fold value → insert into `Indexed` / promote `Scalar`→`Indexed` (element-0 rule) / type-error on `Associative` / create fresh (`case_fold: None`).
- `extend_indexed(name, map) -> Result<(), AssignErr>` (~1667): readonly → promote scalar → create-if-absent → fold+insert each entry at its index.
- `append_array_element(name, idx, value) -> Result<(), AssignErr>` (~1718): `lookup` existing element, `set_array_element(name, idx, existing + value)`.
- `set_associative_element(name, key, value) -> Result<(), AssignErr>` (~1772): readonly → fold value → update-in-place/append pair / type-error if non-associative or unset.
- `append_associative_element(name, key, value)` (~1809): `lookup` + `set_associative_element(existing + value)`.
- `replace_associative(name, pairs) -> Result<(), AssignErr>` (~1846): readonly → capture+fold → insert fresh `Associative`.
- `install_scalar_value(existing, value)` (~2003): the "overwrite element 0 of an existing `Indexed`, else set `Scalar`" rule.
- `eval_integer_coerce(shell, value) -> String` (~1976), `apply_case_fold(fold, value) -> String` (private), `reseed_special_on_assign` (~784), `case_fold_of`/`is_integer`/`is_readonly` readers.

The `unset_array_element`/`unset_associative_element`/`unset` methods are NOT assignments — leave them untouched.

---

## Task 1: Funnel types, seam, and the scalar path

**Files:** Modify `src/shell_state.rs`.

- [ ] **Step 1: Add the funnel value types**

Near the `AssignErr` enum (~line 85) in `src/shell_state.rs`, add:

```rust
/// Where an assigned value lands. Subscripts are ALREADY resolved by the
/// caller (which holds expansion context); the funnel takes only primitives.
#[derive(Debug, Clone)]
pub enum AssignDest {
    /// Whole variable: `name=…`, `name=(…)`, `read -a name`, `mapfile name`.
    Whole(String),
    /// A single element with an already-resolved subscript.
    Element { name: String, sub: Subscript },
}

/// A subscript resolved by the caller. Index → indexed array (arith-evaluated);
/// Key → associative array (string-evaluated). The caller picks the variant
/// from the target's current shape, as `apply_one_assignment` does today.
#[derive(Debug, Clone)]
pub enum Subscript {
    Index(usize),
    Key(String),
}

/// `=` vs `+=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignKind {
    Set,
    Append,
}

/// The value(s) to store, already fully expanded by the caller.
#[derive(Debug, Clone)]
pub enum AssignSource {
    Scalar(String),
    Indexed(BTreeMap<usize, String>),
    Associative(Vec<(String, String)>),
}

impl AssignDest {
    fn name(&self) -> &str {
        match self {
            AssignDest::Whole(n) => n,
            AssignDest::Element { name, .. } => name,
        }
    }
}
```

- [ ] **Step 2: Add the identity nameref seam**

In the `impl Shell` block (near `try_set`), add:

```rust
    /// Resolves an assignment target through any nameref indirection. Identity
    /// today — huck has no namerefs yet (v160). This is the ONE place a future
    /// `declare -n r=target` will rewrite the destination (name, and for
    /// `declare -n r=arr[i]` a `Whole(r)` into an `Element{arr, Index(i)}`),
    /// with circular-reference detection. Do NOT add nameref behavior here in
    /// v159 — only the seam.
    fn resolve_assign_target(&self, dest: AssignDest) -> AssignDest {
        dest
    }
```

- [ ] **Step 3: Add the shared scalar storage primitive**

Add a private method that performs the raw scalar store (reseed + the
element-0 rule), with NO readonly/attribute logic. Both `assign()` and `set()`
will call it:

```rust
    /// Raw scalar store: RANDOM/SECONDS interception, else overwrite an existing
    /// Indexed array's element 0 (bash's `a=v` rule) or set/create a Scalar.
    /// No readonly check, no attributes — those belong to `assign()`.
    fn store_scalar(&mut self, name: &str, value: String) {
        if self.reseed_special_on_assign(name, &value) {
            return;
        }
        match self.vars.get_mut(name) {
            Some(existing) => install_scalar_value(existing, value),
            None => {
                self.vars.insert(name.to_string(), Variable::scalar(value));
            }
        }
    }
```

- [ ] **Step 4: Add `assign()` — scalar path new; non-scalar arms delegate to the existing public methods for now**

Add the funnel. In Task 1 ONLY the scalar-`Whole` path is genuinely new (it flows
through `value_with_scalar_attrs` + `store_scalar`). The `Element` and
whole-array/assoc arms call the EXISTING public methods (which still hold their
own readonly + fold logic); Tasks 2-3 hollow those into wrappers + primitives.
This keeps Task 1 small and the suite green. Add:

```rust
    /// The single chokepoint for variable assignment. Applies cross-cutting
    /// concerns in a fixed order — resolve target (nameref seam) → readonly →
    /// integer-coerce → case-fold → store — then dispatches to a storage
    /// primitive. All value-producing paths route through here (directly or via
    /// the thin leaf-mutator wrappers).
    pub fn assign(
        &mut self,
        dest: AssignDest,
        op: AssignKind,
        source: AssignSource,
    ) -> Result<(), AssignErr> {
        let dest = self.resolve_assign_target(dest);
        let name = dest.name().to_string();

        // Readonly check, once, before any store (no partial array writes).
        // NOTE: in Task 1 the Element/whole-array arms below ALSO re-check
        // readonly inside the public methods they call — harmless (this top
        // check returns first). Tasks 2-3 remove the inner checks when those
        // methods become wrappers, leaving this as the single check.
        if self.is_readonly(&name) {
            eprintln!("huck: {name}: readonly variable");
            return Err(AssignErr::Readonly);
        }

        match (&dest, source) {
            // ── Scalar value into a whole variable: `x=v` / `x+=v` (NEW path) ──
            (AssignDest::Whole(_), AssignSource::Scalar(v)) => {
                let v = if op == AssignKind::Append {
                    // Append concatenates onto the scalar VIEW of the current
                    // value (element 0 for an indexed array), matching the
                    // executor's previous `existing + &s` behavior. (No current
                    // caller produces Whole+Scalar+Append — the executor
                    // pre-concatenates and calls Set — but support it for a
                    // complete, uniform funnel.)
                    let existing = self.get(&name).map(str::to_string).unwrap_or_default();
                    existing + &v
                } else {
                    v
                };
                let stored = self.value_with_scalar_attrs(&name, v);
                self.store_scalar(&name, stored);
                Ok(())
            }
            // ── Element + Scalar: delegate to existing public methods (Task 2/3 replaces) ──
            (AssignDest::Element { name: n, sub: Subscript::Index(idx) }, AssignSource::Scalar(v)) => {
                match op {
                    AssignKind::Set => self.set_array_element(n, *idx, v),
                    AssignKind::Append => self.append_array_element(n, *idx, &v),
                }
            }
            (AssignDest::Element { name: n, sub: Subscript::Key(key) }, AssignSource::Scalar(v)) => {
                match op {
                    AssignKind::Set => self.set_associative_element(n, key.clone(), v),
                    AssignKind::Append => self.append_associative_element(n, key.clone(), &v),
                }
            }
            // ── Whole + array/assoc source: delegate (Task 2/3 replaces) ──
            (AssignDest::Whole(n), _, AssignSource::Indexed(m)) => match op {
                AssignKind::Set => self.replace_array(n, m),
                AssignKind::Append => self.extend_indexed(n, m),
            },
            (AssignDest::Whole(n), AssignKind::Set, AssignSource::Associative(p)) => {
                self.replace_associative(n, p)
            }
            (AssignDest::Whole(_), AssignKind::Append, AssignSource::Associative(_)) => {
                unreachable!("associative whole-append is not produced by any caller")
            }
        }
    }

    /// Applies the SCALAR attribute chain (integer-coerce only on an existing
    /// integer-flagged Scalar, then case-fold) to a whole-variable value.
    fn value_with_scalar_attrs(&mut self, name: &str, value: String) -> String {
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
        apply_case_fold(self.case_fold_of(name), coerced)
    }
```

CAUTION (Task 1 only): the public methods called by the non-scalar arms
(`set_array_element` etc.) still do their OWN readonly check. Combined with the
top-of-`assign` readonly check, a readonly target is caught by the TOP check and
returns before reaching them — so there is no double error message. Verify this
holds (the harnesses will catch a regression). Tasks 2-3 remove the inner checks.

- [ ] **Step 5: Rewrite `try_set` and `set()` as delegates**

Replace the body of `try_set` (~1189) with:

```rust
    pub fn try_set(&mut self, name: &str, value: String) -> Result<(), ()> {
        self.assign(AssignDest::Whole(name.to_string()), AssignKind::Set, AssignSource::Scalar(value))
            .map_err(|_| ())
    }
```

Replace the body of `set()` (~810) with a call to the shared primitive (raw, no
attributes — unchanged behavior):

```rust
    pub fn set(&mut self, name: &str, value: String) {
        self.store_scalar(name, value);
    }
```

- [ ] **Step 6: Build + run the full suite**

Run: `cargo build 2>&1 | tail -3` (clean), then `cargo test 2>&1 | tail -5`
Expected: all pass, unchanged counts.
Run all harnesses: `for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 || echo "FAIL $s"; done; echo "(no FAIL lines above = byte-identical)"`
Expected: no FAIL lines.

- [ ] **Step 7: Commit**

```bash
git add src/shell_state.rs
git commit -m "v159 task 1: assign() funnel + scalar path + nameref seam

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Extract the indexed-array primitives; hollow the indexed mutators into wrappers

**Files:** Modify `src/shell_state.rs`.

Goal: move the STORAGE half of `set_array_element`/`replace_array`/
`extend_indexed` into private `store_*` primitives (no readonly, no fold), make
`assign()` apply the attributes and call them, and rewrite the three public
methods + `append_array_element` as wrappers.

- [ ] **Step 1: Add the indexed storage primitives (storage only — copy the current bodies, STRIP readonly + fold)**

Add these private methods. They are the current bodies of `set_array_element`/
`replace_array`/`extend_indexed` with the readonly check and `apply_case_fold`
lines REMOVED (the funnel now does those). For `replace_array`/`extend_indexed`,
the captured `(exported, integer, case_fold)` stays (attribute PRESERVATION on
the variable is not the same as folding the values):

```rust
    /// Storage only: insert `value` at `idx`, promoting a scalar to indexed
    /// (element-0 rule). Caller has done readonly + fold.
    fn store_indexed_element(&mut self, name: &str, idx: usize, value: String) -> Result<(), AssignErr> {
        match self.vars.get_mut(name) {
            Some(v) => match &mut v.value {
                VarValue::Indexed(m) => { m.insert(idx, value); }
                VarValue::Scalar(s) => {
                    let mut m = BTreeMap::new();
                    if idx == 0 {
                        m.insert(0, value);
                    } else {
                        m.insert(0, std::mem::take(s));
                        m.insert(idx, value);
                    }
                    v.value = VarValue::Indexed(m);
                }
                VarValue::Associative(_) => {
                    eprintln!("huck: {name}: set_array_element on associative variable");
                    return Err(AssignErr::TypeMismatch);
                }
            },
            None => {
                let mut m = BTreeMap::new();
                m.insert(idx, value);
                self.vars.insert(name.to_string(), Variable {
                    value: VarValue::Indexed(m),
                    exported: false, readonly: false, integer: false, case_fold: None,
                });
            }
        }
        Ok(())
    }

    /// Storage only: replace the whole variable with an indexed array of the
    /// given (already-folded) elements, preserving exported/integer/case_fold.
    fn store_indexed_replace(&mut self, name: &str, elements: BTreeMap<usize, String>) -> Result<(), AssignErr> {
        let (exported, integer, case_fold) = match self.vars.get(name) {
            Some(v) => (v.exported, v.integer, v.case_fold),
            None => (false, false, None),
        };
        self.vars.insert(name.to_string(), Variable {
            value: VarValue::Indexed(elements),
            exported, readonly: false, integer, case_fold,
        });
        Ok(())
    }

    /// Storage only: merge (already-folded) entries into the indexed array,
    /// promoting a scalar to element 0 and creating if absent.
    fn store_indexed_extend(&mut self, name: &str, entries: BTreeMap<usize, String>) -> Result<(), AssignErr> {
        if matches!(self.vars.get(name).map(|v| &v.value), Some(VarValue::Scalar(_)))
            && let Some(v) = self.vars.get_mut(name)
            && let VarValue::Scalar(s) = &mut v.value
        {
            let mut m = BTreeMap::new();
            m.insert(0, std::mem::take(s));
            v.value = VarValue::Indexed(m);
        }
        if !self.vars.contains_key(name) {
            self.vars.insert(name.to_string(), Variable {
                value: VarValue::Indexed(BTreeMap::new()),
                exported: false, readonly: false, integer: false, case_fold: None,
            });
        }
        if let Some(v) = self.vars.get_mut(name)
            && let VarValue::Indexed(m) = &mut v.value
        {
            for (idx, val) in entries { m.insert(idx, val); }
            Ok(())
        } else {
            eprintln!("huck: {name}: cannot append array literal to associative array");
            Err(AssignErr::TypeMismatch)
        }
    }
```

(These are the EXACT current storage bodies minus the readonly/fold lines — diff
them against the originals at lines 1608/1573/1667 to confirm nothing else changed.)

- [ ] **Step 2: Route `assign()`'s indexed arms through the primitives**

In `assign()`, replace the indexed `Element`/`Whole+Indexed` arms (which in Task 1
called the public methods) so they apply attributes then call the primitives.
The `Element{Index}` + `Scalar` arm (Set & Append) and the `Whole` + `Indexed`
source arm:

```rust
            (AssignDest::Element { name: n, sub: Subscript::Index(idx) }, AssignSource::Scalar(v)) => {
                let idx = *idx;
                let v = if op == AssignKind::Append {
                    self.lookup_array_element(n, idx).unwrap_or_default() + &v
                } else { v };
                let v = apply_case_fold(self.case_fold_of(n), v); // NB: no integer-coerce on elements (preserve)
                self.store_indexed_element(n, idx, v)
            }
            (AssignDest::Whole(n), op2, AssignSource::Indexed(m)) => {
                let fold = self.case_fold_of(n);
                let m: BTreeMap<usize, String> = m.into_iter().map(|(k, v)| (k, apply_case_fold(fold, v))).collect();
                match op2 {
                    AssignKind::Set => self.store_indexed_replace(n, m),
                    AssignKind::Append => self.store_indexed_extend(n, m),
                }
            }
```

(Keep the scalar `Whole` arm from Task 1, and the associative arms still calling
the public methods until Task 3.)

- [ ] **Step 3: Rewrite the public indexed mutators as wrappers**

```rust
    pub fn set_array_element(&mut self, name: &str, idx: usize, value: String) -> Result<(), AssignErr> {
        self.assign(AssignDest::Element { name: name.to_string(), sub: Subscript::Index(idx) }, AssignKind::Set, AssignSource::Scalar(value))
    }
    pub fn append_array_element(&mut self, name: &str, idx: usize, value: &str) -> Result<(), AssignErr> {
        self.assign(AssignDest::Element { name: name.to_string(), sub: Subscript::Index(idx) }, AssignKind::Append, AssignSource::Scalar(value.to_string()))
    }
    pub fn replace_array(&mut self, name: &str, elements: BTreeMap<usize, String>) -> Result<(), AssignErr> {
        self.assign(AssignDest::Whole(name.to_string()), AssignKind::Set, AssignSource::Indexed(elements))
    }
    pub fn extend_indexed(&mut self, name: &str, entries: BTreeMap<usize, String>) -> Result<(), AssignErr> {
        self.assign(AssignDest::Whole(name.to_string()), AssignKind::Append, AssignSource::Indexed(entries))
    }
```

Keep each method's existing doc-comment (move it above the wrapper).

- [ ] **Step 4: Build + full suite green**

Run `cargo build 2>&1 | tail -3`, `cargo test 2>&1 | tail -5`, and the harness
loop. Expected: clean build, all tests pass, no harness FAIL lines. If ANY array
harness diffs, the extraction dropped a behavior — compare the primitive against
the original body. Also run `cargo clippy --bins --quiet 2>&1 | tail -3` (clean).

- [ ] **Step 5: Commit**

```bash
git add src/shell_state.rs
git commit -m "v159 task 2: extract indexed-array storage primitives; mutators -> wrappers

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Extract the associative primitives; hollow the associative mutators into wrappers

**Files:** Modify `src/shell_state.rs`.

- [ ] **Step 1: Add the associative storage primitives (storage only — STRIP readonly + fold)**

```rust
    /// Storage only: set `key`=`value` (already folded) in the associative
    /// array, preserving insertion order; type-error if non-associative/unset.
    fn store_assoc_element(&mut self, name: &str, key: String, value: String) -> Result<(), AssignErr> {
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
                    eprintln!("huck: {name}: set_associative_element on non-associative variable");
                    return Err(AssignErr::TypeMismatch);
                }
            },
            None => {
                eprintln!("huck: {name}: set_associative_element on unset variable");
                return Err(AssignErr::TypeMismatch);
            }
        }
        Ok(())
    }

    /// Storage only: replace the whole variable with an associative array of the
    /// given (already-folded) pairs, preserving exported/integer/case_fold.
    fn store_assoc_replace(&mut self, name: &str, pairs: Vec<(String, String)>) -> Result<(), AssignErr> {
        let (exported, integer, case_fold) = match self.vars.get(name) {
            Some(v) => (v.exported, v.integer, v.case_fold),
            None => (false, false, None),
        };
        self.vars.insert(name.to_string(), Variable {
            value: VarValue::Associative(pairs),
            exported, readonly: false, integer, case_fold,
        });
        Ok(())
    }
```

NOTE: confirm `replace_associative`'s current body (~1846) to copy its exact
attribute-capture (it preserves `exported`/`integer`/`case_fold` like
`replace_array`); match it precisely so the extraction is behavior-identical.

- [ ] **Step 2: Route `assign()`'s associative arms through the primitives**

Replace the associative arms in `assign()` (Task 1 had them calling public
methods):

```rust
            (AssignDest::Element { name: n, sub: Subscript::Key(key) }, AssignSource::Scalar(v)) => {
                let key = key.clone();
                let v = if op == AssignKind::Append {
                    self.lookup_associative_element(n, &key).unwrap_or_default() + &v
                } else { v };
                let v = apply_case_fold(self.case_fold_of(n), v);
                self.store_assoc_element(n, key, v)
            }
            (AssignDest::Whole(n), AssignKind::Set, AssignSource::Associative(p)) => {
                let fold = self.case_fold_of(n);
                let p: Vec<(String, String)> = p.into_iter().map(|(k, v)| (k, apply_case_fold(fold, v))).collect();
                self.store_assoc_replace(n, p)
            }
            (AssignDest::Whole(_), AssignKind::Append, AssignSource::Associative(_)) => {
                unreachable!("associative whole-append is not produced by any caller")
            }
```

- [ ] **Step 3: Rewrite the public associative mutators as wrappers**

```rust
    pub fn set_associative_element(&mut self, name: &str, key: String, value: String) -> Result<(), AssignErr> {
        self.assign(AssignDest::Element { name: name.to_string(), sub: Subscript::Key(key) }, AssignKind::Set, AssignSource::Scalar(value))
    }
    pub fn append_associative_element(&mut self, name: &str, key: &str, value: &str) -> Result<(), AssignErr> {
        self.assign(AssignDest::Element { name: name.to_string(), sub: Subscript::Key(key.to_string()) }, AssignKind::Append, AssignSource::Scalar(value.to_string()))
    }
    pub fn replace_associative(&mut self, name: &str, pairs: Vec<(String, String)>) -> Result<(), AssignErr> {
        self.assign(AssignDest::Whole(name.to_string()), AssignKind::Set, AssignSource::Associative(pairs))
    }
```

- [ ] **Step 4: Confirm `assign()` has no leftover stub arms**

The `(_, source) => …` catch-all / `assign_compound` / `*_legacy` scaffolding
from Task 1 must be GONE — `assign()` should now have explicit arms for every
real `(dest, op, source)` combination plus the two `unreachable!` guards
(indexed/assoc whole-append-with-... — keep only those that are genuinely
unreachable). Verify there is no `todo!()`/dead bridge left.

- [ ] **Step 5: Build + full suite + clippy green**

`cargo build`, `cargo test`, harness loop, `cargo clippy --bins --quiet` — all
clean, no harness FAIL lines.

- [ ] **Step 6: Commit**

```bash
git add src/shell_state.rs
git commit -m "v159 task 3: extract associative storage primitives; mutators -> wrappers

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Verification — funnel-uniformity tests, builtin-routing guard, raw-`set()` audit

**Files:** Modify `src/shell_state.rs` (unit tests), create `tests/scripts/assign_funnel_diff_check.sh`.

These are CHARACTERIZATION tests — they assert existing behavior through the new
structure and should PASS on first run. If one fails, the refactor changed
behavior — fix the refactor, not the test.

- [ ] **Step 1: Funnel-uniformity unit tests (same attribute, every path)**

In the `#[cfg(test)] mod tests` block of `src/shell_state.rs`, add (use the real
accessors — `get`, `lookup_array_element`, `get_associative` — matching existing
tests; `Shell::new()` constructor):

```rust
    #[test]
    fn assign_funnel_applies_case_fold_on_every_path() {
        let mut shell = Shell::new();
        // scalar
        shell.set_case_fold("s", Some(CaseFold::Upper));
        shell.assign(AssignDest::Whole("s".into()), AssignKind::Set, AssignSource::Scalar("abc".into())).unwrap();
        assert_eq!(shell.get("s"), Some("ABC"));
        // indexed element
        shell.set_case_fold("a", Some(CaseFold::Upper));
        shell.assign(AssignDest::Element { name: "a".into(), sub: Subscript::Index(2) }, AssignKind::Set, AssignSource::Scalar("xy".into())).unwrap();
        assert_eq!(shell.lookup_array_element("a", 2).as_deref(), Some("XY"));
        // whole indexed
        let mut m = std::collections::BTreeMap::new();
        m.insert(0usize, "lo".to_string());
        shell.set_case_fold("b", Some(CaseFold::Upper));
        shell.assign(AssignDest::Whole("b".into()), AssignKind::Set, AssignSource::Indexed(m)).unwrap();
        assert_eq!(shell.lookup_array_element("b", 0).as_deref(), Some("LO"));
    }

    #[test]
    fn assign_funnel_readonly_blocks_all_paths() {
        let mut shell = Shell::new();
        shell.try_set("r", "init".into()).unwrap();
        shell.mark_readonly("r");
        assert!(shell.assign(AssignDest::Whole("r".into()), AssignKind::Set, AssignSource::Scalar("x".into())).is_err());
        assert_eq!(shell.get("r"), Some("init")); // unchanged
    }
```

- [ ] **Step 2: Builtin-routing guard harness (proves no raw-`set()` bypass)**

Create `tests/scripts/assign_funnel_diff_check.sh` (model the `chk` helper on
`tests/scripts/local_case_attrs_diff_check.sh`). It drives each value-producing
builtin against a case-folded variable and byte-compares with bash — if any
builtin bypassed the funnel via raw `set()`, the fold would be missing and the
case would diff:

```bash
#!/usr/bin/env bash
# Proves every value-producing builtin routes assignment through the funnel:
# each writes a -u/-l variable and the stored value must be folded (matches bash).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
chk() { local l="$1" f="$2" b h
    b=$(bash --norc --noprofile -c "$f" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$f" 2>/dev/null; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

chk "read folds"       'declare -u v; printf "abc\n" | { read v; echo "$v"; }'
chk "read -a folds"    'declare -u arr; read -a arr <<< "pp qq"; echo "${arr[@]}"'
chk "printf -v folds"  'declare -u v; printf -v v "%s" hello; echo "$v"'
chk "mapfile folds"    'declare -u arr; mapfile -t arr <<< $'"'"'aa\nbb'"'"'; echo "${arr[@]}"'
chk "getopts folds"    'declare -u o; set -- -a -b val; while getopts "ab:" o; do echo "$o"; done'
chk "default-assign"   'declare -u v; : "${v:=def}"; echo "$v"'
chk "scalar set"       'declare -l x; x=ABC; echo "$x"'
chk "array literal"    'declare -l a; a=(AA Bb); echo "${a[@]}"'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
```

`chmod +x` it and run: `bash tests/scripts/assign_funnel_diff_check.sh` →
all PASS. (Determine bash's actual output for each first; the `${v:=def}` case
folds in bash only if assignment-expansion routes through the attribute — verify
and keep only cases where bash and huck genuinely agree. If a case reveals a REAL
remaining bypass in huck, that's a finding: report it, do not delete the case.)

- [ ] **Step 3: Raw-`set()` audit (documented)**

Run `grep -n '\.set(' src/builtins.rs src/executor.rs src/param_expansion.rs | grep -v '\.set(true\|\.set(false\|shopt'` and inspect each remaining `shell.set(NAME, …)` on a user-reachable variable. Confirm each is a shell-internal / special-var write (env import, `OPTIND`, `RANDOM`/`SECONDS`, static builtin vars, `BASH_REMATCH`, `PWD`/`OLDPWD`, etc.) and NOT a user-assignment path that should honor attributes. In the harness file header comment, record the audit conclusion (the list of legitimate raw-`set()` sites). If any user-assignment path is found using raw `set()`, switch it to `try_set`/`assign()` and note it in the commit.

- [ ] **Step 4: Build + full suite + new harness + clippy**

`cargo build`, `cargo test` (new unit tests pass), `bash tests/scripts/assign_funnel_diff_check.sh` (all pass), full harness loop (now 86 files, no FAIL), `cargo clippy --bins --quiet` (clean).

- [ ] **Step 5: Commit**

```bash
git add src/shell_state.rs tests/scripts/assign_funnel_diff_check.sh
git commit -m "v159 task 4: funnel-uniformity tests + builtin-routing guard + raw-set audit

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final verification (after all tasks)

- [ ] `cargo build` clean, `cargo clippy --bins --quiet` clean.
- [ ] `cargo test` FULLY green, unchanged counts (except the new Task 4 unit tests; any test that referenced a now-private method adapted, not weakened).
- [ ] All `tests/scripts/*_diff_check.sh` byte-identical (86 incl. the new one) — **this is the proof the refactor preserved behavior**.
- [ ] `assign()` has explicit arms for every real `(dest, op, source)` combination; the only `unreachable!`s are genuinely-unreachable whole-append-associative cases; no `todo!()`/legacy bridges remain.
- [ ] `resolve_assign_target` exists as the identity seam with its nameref doc comment.
- [ ] The 8 public leaf mutators are one-line wrappers; `set()` calls `store_scalar`; private `store_*` primitives hold the storage logic with NO readonly/attribute lines.

## Notes for the implementer

- **The suite is the spec.** This refactor adds no behavior. If any existing
  harness or test changes output, you broke an invariant — read the spec's
  "Behavior-preservation invariants" and the original mutator body, don't adjust
  the test.
- **Integer/case-fold asymmetry is intentional** — array elements are case-folded
  but NOT integer-coerced today. Preserve it; do not add integer coercion to the
  element/array arms.
- **Borrow checker:** compute folded/coerced values into locals before taking
  `&mut self` (the v158 pattern). `case_fold_of`/`is_integer`/`is_readonly`/
  `lookup_*` are `&self`; do them first.
- **Idempotent fold** means the TEMP bridges in Task 1 (if you used the
  non-recommended form) are safe, but prefer the RECOMMENDED Task-1 Step-5 form
  (element/compound arms call existing public methods until Tasks 2-3 hollow them).
- **Do the extraction by diffing**, not retyping: copy the current mutator body
  into the `store_*` primitive, then delete exactly the readonly-check block and
  the `apply_case_fold` line. Anything else changing is a mistake.
- **The executor needs NO changes.** `apply_one_assignment` (executor.rs) already
  calls the public leaf mutators (`try_set`, `set_array_element`, `replace_array`,
  `set_associative_element`, `replace_associative`, `extend_indexed`,
  `append_*_element`). Once those become wrappers over `assign()`, the executor is
  routed through the funnel transitively — do not rewrite it. Its existing
  readonly PRE-checks before array-append loops stay (they short-circuit before
  the funnel; harmless and behavior-preserving). The same is true of the
  value-producing builtins (`read`/`printf -v`/`getopts`/`mapfile`/`${x:=}`): they
  already call `try_set`/`replace_array`/`set_array_element`, so they route through
  the funnel for free. Task 4 only PROVES this (guard harness) and audits raw
  `set()`; it changes no production code unless the audit finds a real bypass.
