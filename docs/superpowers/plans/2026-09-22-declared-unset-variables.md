# v365 — Declared-but-Unset Variables Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give huck bash's "declared but unset" variable state, so a valueless declaration records attributes without materialising a value, and the export set answers bash's two different questions.

**Architecture:** `VarValue` gains an `Unset(Shape)` variant. The six storage primitives in `shell_state.rs` materialise the shape on first write; `lookup_var`/`get` return `None` for an unset value, which makes every ordinary reader correct for free; a named handful of sites (`-v`, nounset, `declare -p`, `@A`, `set`/`compgen -v`, `exported_env`, the declaration paths) learn the distinction explicitly.

**Tech Stack:** Rust 2024 edition, workspace crates `huck-syntax` / `huck-engine` / `huck-cli`; bash-diff harnesses under `tests/scripts/`.

**Spec:** `docs/superpowers/specs/2026-09-22-declared-unset-variables-design.md`

**Issues:** #600 (primary), #225, #33, #691, #692, #777. Hub 2A of #765.

## Out of Scope

Named here so a task cannot drift into them:

- The array/associative conversion table (#347, #697, #698, #734) — hub 2B.
  This iteration only makes `Unset` a shape that table can already see via
  `value.shape()`.
- Byte-transparent values (#738) — hub 8, the second `VarValue` pass.
- `${v@a}` attribute queries — they read attributes, not values, and need
  nothing.

## Global Constraints

- Every commit ends with `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`.
- `cargo fmt --all` before every commit; CI enforces `--check`.
- Lint with the CI-pinned toolchain: `cargo +1.97.1 clippy --workspace --all-targets --locked -- -D warnings`. A newer local stable MISSES warnings CI raises.
- Tests run PER CRATE, single-threaded: `cargo test -p <crate> --locked --jobs 1 --lib -- --test-threads 1`. Never `cargo test --workspace` (OOM-kills this 1-core/1.9 GB box).
- Build the binary with `cargo build -p huck` (debug) and `cargo build --release --locked --bin huck` — the sweep needs both.
- Harness fragments run from `/tmp`, never the repo (a `> $(…)` probe writes files into the cwd).
- Every behavioural claim is measured against `bash --norc --noprofile` 5.2.21 before it is asserted.
- Branch: `v365-declared-unset`, off `main`. Do not merge; the PR is handed to the user.

---

### Task 1: The `Unset(Shape)` variant and the write side

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (`VarValue` at :38, `scalar_view` at :52, `Variable` at :72, `store_scalar` at :2361, `store_indexed_element` at :2383, `store_indexed_replace` at :2434, `store_indexed_extend` at :2459, `store_assoc_element` at :2506, `store_assoc_replace` at :2540, `install_scalar_value` at :3871)
- Test: `crates/huck-engine/src/shell_state/unset_value_tests.rs` (new), registered in `shell_state.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub enum Shape { Scalar, Indexed, Associative }`; `VarValue::Unset(Shape)`; `VarValue::shape(&self) -> Shape`; `VarValue::is_unset(&self) -> bool`; `Variable::unset(shape: Shape) -> Variable`. Later tasks match on `VarValue::Unset(_)` and call `Variable::unset`.

- [ ] **Step 1: Write the failing test**

Create `crates/huck-engine/src/shell_state/unset_value_tests.rs`:

```rust
use super::*;

/// Every value mutator must MATERIALISE an unset variable — the structural
/// risk of this iteration is a write path that stores a value while leaving
/// the variable unset, which would silently report a live variable as unset.
#[test]
fn every_mutator_materialises_an_unset_variable() {
    // (label, shape, apply) — `apply` performs one write through the public
    // mutator surface. After it, the variable must be set.
    let cases: Vec<(&str, Shape, Box<dyn Fn(&mut Shell)>)> = vec![
        (
            "scalar set",
            Shape::Scalar,
            Box::new(|sh: &mut Shell| sh.set("v", "x".to_string())),
        ),
        (
            "indexed element",
            Shape::Indexed,
            Box::new(|sh: &mut Shell| {
                sh.set_indexed_element("v", 2, "x".to_string()).unwrap();
            }),
        ),
        (
            "indexed extend",
            Shape::Indexed,
            Box::new(|sh: &mut Shell| {
                sh.extend_indexed("v", vec!["a".to_string()]).unwrap();
            }),
        ),
        (
            "indexed replace",
            Shape::Indexed,
            Box::new(|sh: &mut Shell| {
                sh.replace_indexed("v", vec!["a".to_string()]).unwrap();
            }),
        ),
        (
            "assoc element",
            Shape::Associative,
            Box::new(|sh: &mut Shell| {
                sh.set_associative_element("v", "k", "x".to_string()).unwrap();
            }),
        ),
    ];
    for (label, shape, apply) in cases {
        let mut sh = Shell::new();
        sh.vars.insert("v".to_string(), Variable::unset(shape));
        assert!(
            sh.vars["v"].value.is_unset(),
            "{label}: fixture should start unset"
        );
        apply(&mut sh);
        assert!(
            !sh.vars["v"].value.is_unset(),
            "{label}: the mutator left the variable UNSET — a silent write path"
        );
    }
}

/// A scalar read of an unset value is the empty string (the 12 `scalar_view`
/// callers keep working); the shape survives for the conversion table.
#[test]
fn unset_value_reads_empty_and_keeps_its_shape() {
    assert_eq!(VarValue::Unset(Shape::Indexed).scalar_view(), "");
    assert_eq!(VarValue::Unset(Shape::Indexed).shape(), Shape::Indexed);
    assert_eq!(VarValue::Scalar("x".to_string()).shape(), Shape::Scalar);
    assert_eq!(
        VarValue::Associative(crate::assoc_map::AssocMap::new()).shape(),
        Shape::Associative
    );
    assert!(!VarValue::Scalar(String::new()).is_unset());
}

/// An unset scalar that is written through `install_scalar_value` becomes a
/// Scalar; an unset INDEXED written the same way becomes element 0 (bash's
/// `y=v` on an array is `y[0]=v`).
#[test]
fn install_scalar_value_materialises_by_shape() {
    let mut v = Variable::unset(Shape::Scalar);
    assert!(!install_scalar_value(&mut v, "x".to_string()));
    assert!(matches!(&v.value, VarValue::Scalar(s) if s == "x"));

    let mut v = Variable::unset(Shape::Indexed);
    assert!(!install_scalar_value(&mut v, "x".to_string()));
    match &v.value {
        VarValue::Indexed(m) => assert_eq!(m.get(&0).map(String::as_str), Some("x")),
        other => panic!("expected Indexed, got {other:?}"),
    }

    // An unset ASSOCIATIVE rejects a scalar install, exactly as a set one does.
    let mut v = Variable::unset(Shape::Associative);
    assert!(install_scalar_value(&mut v, "x".to_string()));
}
```

Register the module next to the existing `shell_state` test modules in `crates/huck-engine/src/shell_state.rs`:

```rust
#[cfg(test)]
mod unset_value_tests;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p huck-engine --locked --jobs 1 --lib -- --test-threads 1 unset_value`
Expected: FAIL — `Shape` and `VarValue::Unset` do not exist (compile error).

- [ ] **Step 3: Write minimal implementation**

In `crates/huck-engine/src/shell_state.rs`, extend the value type:

```rust
/// The kind of value a variable holds — and, when it holds none, the kind it
/// WILL hold. bash keeps the array/assoc flag in the variable's attributes,
/// so a valueless `declare -a y` is still an indexed array for conversion
/// purposes (`declare -a y; declare -A y` still refuses).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Scalar,
    Indexed,
    Associative,
}

#[derive(Debug, Clone)]
pub enum VarValue {
    Scalar(String),
    Indexed(BTreeMap<usize, String>),
    Associative(crate::assoc_map::AssocMap),
    /// Declared, with NO value (bash's `SHELL_VAR.value == NULL`): created by
    /// `declare -a/-A/-i/…`, bare `declare`/`local`, and by `readonly`/`export`
    /// naming a variable that does not exist yet. Any assignment materialises
    /// it; `unset` removes the entry outright. See #600.
    Unset(Shape),
}
```

Add to `impl VarValue`, beside `scalar_view`:

```rust
    /// The shape this value has, or (when unset) will have on first write.
    pub fn shape(&self) -> Shape {
        match self {
            VarValue::Scalar(_) => Shape::Scalar,
            VarValue::Indexed(_) => Shape::Indexed,
            VarValue::Associative(_) => Shape::Associative,
            VarValue::Unset(s) => *s,
        }
    }

    /// True when the variable is declared but holds no value.
    pub fn is_unset(&self) -> bool {
        matches!(self, VarValue::Unset(_))
    }
```

Extend `scalar_view`'s match with the new arm:

```rust
            VarValue::Unset(_) => "",
```

Add the constructor beside `Variable::scalar`:

```rust
    /// A declared variable with no value, of the given shape.
    pub fn unset(shape: Shape) -> Self {
        Variable {
            value: VarValue::Unset(shape),
            exported: false,
            readonly: false,
            integer: false,
            case_fold: None,
            nameref: false,
        }
    }
```

Materialise in `install_scalar_value`:

```rust
fn install_scalar_value(existing: &mut Variable, value: String) -> bool {
    match &mut existing.value {
        VarValue::Indexed(m) => {
            m.insert(0, value);
            false
        }
        VarValue::Scalar(_) => {
            existing.value = VarValue::Scalar(value);
            false
        }
        VarValue::Associative(_) => true,
        // #600: the first write materialises the declared shape. An unset
        // INDEXED takes the element-0 rule, exactly as a set one does; an
        // unset ASSOCIATIVE rejects a scalar install like its set twin.
        VarValue::Unset(Shape::Scalar) => {
            existing.value = VarValue::Scalar(value);
            false
        }
        VarValue::Unset(Shape::Indexed) => {
            let mut m = BTreeMap::new();
            m.insert(0, value);
            existing.value = VarValue::Indexed(m);
            false
        }
        VarValue::Unset(Shape::Associative) => true,
    }
}
```

In each of `store_indexed_element`, `store_indexed_replace`, `store_indexed_extend`, convert an unset variable to an empty `Indexed` map before the existing logic runs, e.g. at the top of the `Some(v) => match &mut v.value` arm:

```rust
            Some(v) => {
                // #600: materialise before the shape match below sees it.
                if matches!(v.value, VarValue::Unset(Shape::Indexed | Shape::Scalar)) {
                    v.value = VarValue::Indexed(BTreeMap::new());
                }
                match &mut v.value {
                    // … existing arms unchanged …
                }
            }
```

and in `store_assoc_element` / `store_assoc_replace`:

```rust
            Some(v) => {
                if matches!(v.value, VarValue::Unset(_)) {
                    v.value = VarValue::Associative(crate::assoc_map::AssocMap::new());
                }
                match &mut v.value {
                    // … existing arms unchanged …
                }
            }
```

The compiler will flag every other `match` on `VarValue` in the crate. For this task, give each a `VarValue::Unset(_)` arm that behaves as the EMPTY value of its shape — that is the pre-#600 behaviour, so nothing changes yet. Tasks 2–4 replace the arms that must differ.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p huck-engine --locked --jobs 1 --lib -- --test-threads 1 unset_value`
Expected: PASS, 3 tests.

Then the whole crate, to prove the placeholder arms changed nothing:

Run: `cargo test -p huck-engine --locked --jobs 1 --lib -- --test-threads 1`
Expected: PASS, no new failures.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/shell_state.rs crates/huck-engine/src/shell_state/unset_value_tests.rs
git commit -m "feat(#600): add VarValue::Unset(Shape) and materialise it on first write

The variant and the write side only: every storage primitive turns an unset
variable into its shape's empty value before storing, and a table-driven test
asserts that invariant across all five mutators. Every other match on
VarValue gets an arm that behaves as the empty value, so behaviour is
unchanged until the reader tasks.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: The read side

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (`lookup_var` at :1556, `get` at :1384, `is_set` at :1808)
- Modify: `crates/huck-engine/src/builtins.rs` (`format_declare_line` at :1272, `format_declare_bare_line` at :1386)
- Modify: `crates/huck-engine/src/expand.rs` (the nounset raise at :881)
- Test: `crates/huck-engine/src/shell_state/unset_value_tests.rs` (extend)

**Interfaces:**
- Consumes: `VarValue::Unset(Shape)`, `Variable::unset`, `VarValue::is_unset` (Task 1).
- Produces: `lookup_var`/`get` answer `None` for an unset variable; `is_set` answers false; `format_declare_line` emits no `=…`. Task 3 relies on all four.

- [ ] **Step 1: Write the failing test**

Append to `crates/huck-engine/src/shell_state/unset_value_tests.rs`:

```rust
/// An unset variable reads exactly like a name that does not exist — which
/// is what keeps all 63 `lookup_var` callers correct without edits.
#[test]
fn unset_reads_as_absent() {
    let mut sh = Shell::new();
    sh.vars.insert("v".to_string(), Variable::unset(Shape::Scalar));
    assert_eq!(sh.lookup_var("v"), None);
    assert_eq!(sh.get("v"), None);
    assert!(!sh.is_set("v"), "[[ -v v ]] must be false while unset");

    // …and like a normal variable once it has one.
    sh.set("v", String::new());
    assert_eq!(sh.lookup_var("v").as_deref(), Some(""));
    assert!(sh.is_set("v"), "an empty assignment SETS the variable");
}

/// `declare -p` prints the declaration with no `=…` while unset, and with it
/// once set. Attributes are real either way.
#[test]
fn declare_p_omits_the_value_while_unset() {
    let mut y = Variable::unset(Shape::Indexed);
    assert_eq!(crate::builtins::format_declare_line("y", &y), "declare -a y");
    y.readonly = true;
    assert_eq!(crate::builtins::format_declare_line("y", &y), "declare -ar y");

    let mut n = Variable::unset(Shape::Scalar);
    n.integer = true;
    assert_eq!(crate::builtins::format_declare_line("n", &n), "declare -i n");

    let mut v = Variable::unset(Shape::Scalar);
    v.exported = true;
    assert_eq!(crate::builtins::format_declare_line("v", &v), "declare -x v");

    // Plain, no attributes: bash prints `declare -- v`.
    let v = Variable::unset(Shape::Scalar);
    assert_eq!(crate::builtins::format_declare_line("v", &v), "declare -- v");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p huck-engine --locked --jobs 1 --lib -- --test-threads 1 unset_reads_as_absent declare_p_omits`
Expected: FAIL — `lookup_var` returns `Some("")`, `is_set` true, `format_declare_line` emits `=""`.

- [ ] **Step 3: Write minimal implementation**

`lookup_var`, at the point it reads the variable table (after the special-parameter block), returns `None` for an unset value:

```rust
        self.vars.get(name).and_then(|v| match &v.value {
            // #600: declared but valueless reads exactly like absent.
            VarValue::Unset(_) => None,
            other => Some(other.scalar_view().to_string()),
        })
```

`get` takes the same guard:

```rust
        self.vars.get(name).and_then(|v| match &v.value {
            VarValue::Unset(_) => None,
            other => Some(other.scalar_view()),
        })
```

`is_set`, at its variable-table arm:

```rust
        // #600: `[[ -v y ]]` is FALSE for a declared-but-unset variable; an
        // element form (`y[k]`) is answered by the map, which is empty.
        self.vars
            .get(name)
            .is_some_and(|v| !v.value.is_unset())
```

`format_declare_line` and `format_declare_bare_line`: the attribute letters already come from `var.integer`/`readonly`/etc., but the `a`/`A` letters read the value — extend them to the shape, and skip the `=…` tail when unset:

```rust
    if matches!(var.value.shape(), crate::shell_state::Shape::Indexed) {
        attrs.push('a');
    }
    if matches!(var.value.shape(), crate::shell_state::Shape::Associative) {
        attrs.push('A');
    }
```

and, where the function appends the value:

```rust
    // #600: a declared-but-unset variable prints WITHOUT `=…`.
    if var.value.is_unset() {
        return format!("declare -{attrs} {name}");
    }
```

(with `attrs` defaulting to `--` exactly as today when no letters were pushed).

The `set` and `compgen -v` listings iterate the variable table directly
(`var_names` / the `set` builtin's own walk). Both skip an unset entry:

```rust
        // #600: bash's `set` and `compgen -v` list only variables that HAVE a
        // value; a declared-but-unset name appears in `declare -p` and
        // `export -p`, but not here.
        .filter(|(_, v)| !v.value.is_unset())
```

nounset, in `crates/huck-engine/src/expand.rs` at the scalar raise: a scalar
read of an unset variable must raise `name: unbound variable`, and
`${y[@]}` / `${y[*]}` / `${#y[@]}` must NOT. The scalar path reaches
`lookup_var`, which now answers `None`, so the existing raise fires for free;
the array arms read the map, which for an unset array is absent and must be
treated as EMPTY rather than routed to the scalar raise. Prove both halves
NOW, before moving on:

```bash
cargo build -p huck --locked --jobs 1
cd /tmp && /home/john/projects/huck/target/debug/huck -c 'set -u; declare -a y; echo "[${y[@]}]"; echo "${#y[@]}"'
# expected: [] then 0, rc 0 — matching bash
cd /tmp && /home/john/projects/huck/target/debug/huck -c 'set -u; declare -i n; echo "$n"'
# expected: `n: unbound variable`, rc 1 — matching bash
```

If the array form raises, make the `${y[@]}`/`${y[*]}`/`${#y[@]}` arms read
the map (absent → empty) instead of falling through to the scalar view.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p huck-engine --locked --jobs 1 --lib -- --test-threads 1`
Expected: PASS. Some EXISTING tests may now fail — do not edit them here; note their names for Task 5, which adjudicates each against bash.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/shell_state.rs crates/huck-engine/src/builtins.rs crates/huck-engine/src/shell_state/unset_value_tests.rs
git commit -m "feat(#600,#33): an unset variable reads as absent and prints without a value

lookup_var/get answer None, is_set answers false, and declare -p emits the
declaration with no \`=…\`. The shape now supplies the a/A attribute letters,
so a valueless \`declare -a y\` still prints \`declare -a y\`.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Declaration paths create the unset state

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (`mark_readonly` at :2822, `mark_integer` at :2857, `set_case_fold`, the nameref marker, `declare_associative` at :3521 and its indexed twin)
- Modify: `crates/huck-engine/src/builtins.rs` (`builtin_declare`'s no-value path, `builtin_local` at :1816)
- Create: `tests/scripts/declared_unset_diff_check.sh`
- Test: the harness above

**Interfaces:**
- Consumes: `Variable::unset`, `VarValue::is_unset`, the Task 2 readers.
- Produces: user-visible behaviour for every row in #600's table; Task 4 extends the same harness file.

- [ ] **Step 1: Write the failing test**

Create `tests/scripts/declared_unset_diff_check.sh` (executable, `chmod +x`):

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #600/#225/#33/#691 (hub 2A of #765):
# a variable is attributes plus an OPTIONAL value. A valueless declaration
# records the attributes and leaves the value NULL; any assignment sets it;
# `unset` removes the entry outright.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
norm() { sed -E 's|^[^:]*: line |PROG: line |'; }
check() {
    local label="$1" frag="$2" b h
    b=$(cd "$tmp" && bash --norc --noprofile -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$(cd "$tmp" && "$HUCK_BIN" -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    compare "$label [-c]" "$b" "$h"
    printf '%s\n' "$frag" >"$tmp/f.sh"
    b=$(cd "$tmp" && bash --norc --noprofile f.sh 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$(cd "$tmp" && "$HUCK_BIN" f.sh 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    compare "$label [file]" "$b" "$h"
}

# --- #600's table: a valueless declaration has no value ---------------------
check "declare -A"          'declare -A x; declare -p x'
check "declare -a"          'declare -a y; declare -p y'
check "declare -i"          'declare -i n; declare -p n'
check "declare -l/-u"       'declare -l lo; declare -p lo; declare -u up; declare -p up'
check "declare -n"          'declare -n r; declare -p r'
check "declare -r (#225)"   'declare -r r; declare -p r; readonly q; declare -p q'
check "declare bare"        'declare v; declare -p v'
check "@A transform"        'declare -a y; echo "${y[@]@A}"; declare -i n; echo "${n@A}"'
check "[[ -v ]]"            'declare -a y; [[ -v y ]] && echo v || echo nv; declare -A x; [[ -v x ]] && echo v || echo nv'
check "set -u scalar"       'set -u; declare -i n; echo "$n"'
check "set -u array reads"  'set -u; declare -a y; echo "[${y[@]}]"; echo "[${y[*]}]"; echo "${#y[@]}"'
check "export no value"     'declare -x E; env | grep -c "^E="; declare -p E; export -p | grep -c "^declare -x E$"'
check "local bare"          'f(){ local v; declare -p v; echo "[${v+set}${v-unset}]"; }; f'
check "local -a/-i"         'f(){ local -a a; declare -p a; local -i i; declare -p i; }; f'

# --- becoming set -----------------------------------------------------------
check "scalar assign"       'declare v; echo "[${v+set}]"; v=; declare -p v; echo "[${v+set}]"'
check "indexed element"     'declare -a y; y[2]=x; declare -p y; [[ -v y ]] && echo v || echo nv'
check "indexed append"      'declare -a y; y+=(a); declare -p y'
check "assoc element"       'declare -A x; x[k]=v; declare -p x; [[ -v x ]] && echo v || echo nv; [[ -v x[k] ]] && echo vk || echo nvk'
check "integer append"      'declare -i n; n+=1; declare -p n'
check "export then assign"  'declare -x E; env | grep -c "^E="; E=1; env | grep "^E="; declare -p E'
check "case fold on assign" 'declare -l lo; lo=ABC; declare -p lo'
check "nameref on assign"   'declare -n r; r=x; declare -p r'

# --- still unset ------------------------------------------------------------
check "attribute churn"     'declare -i n; declare -p n; declare +i n; declare -p n; declare -x n; declare -p n'
check "shape survives"      'declare -a y; declare -A y; echo rc=$?; declare -p y'
check "readonly guard"      'declare -r r; r=1; echo rc=$?; declare -p r'
check "unset removes"       'declare -a y; unset y; declare -p y; echo rc=$?'
check "listings skip it"    'declare -a y; compgen -v y; echo ---; set | grep -c "^y="; declare -p | grep -c "^declare -a y$"'
check "empty-ish reads"     'declare -A x; echo "[${x[@]:-D}] [${x-D}] [${#x}]"; declare -a y; echo "[${!y[@]}] [${y[@]:0}]"'

# --- #691: a bare local inherits ONLY the export attribute ------------------
check "local over exported" 'declare -x V=1; f(){ local V; declare -p V; V=9; declare -p V; }; f; declare -p V'
check "local over plain"    'V=1; f(){ local V; declare -p V; }; f'
check "local over integer"  'declare -i N=1; f(){ local N; declare -p N; N=2+2; declare -p N; }; f'

harness_summary
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo build -p huck --locked --jobs 1
( ulimit -v 4000000; timeout 300 tests/scripts/declared_unset_diff_check.sh )
```

Expected: FAIL on the declaration rows (huck prints `declare -a y=()`, `[[ -v y ]]` true, `local v` → `not found`, …). Record the pass/fail counts.

- [ ] **Step 3: Write minimal implementation**

The attribute mutators create the entry WITHOUT a value. In `mark_readonly` (and the same shape in `mark_integer`, `set_case_fold`, the nameref marker):

```rust
    pub fn mark_readonly(&mut self, name: &str) {
        match self.vars.get_mut(name) {
            Some(v) => v.readonly = true,
            None => {
                // #225: the attribute exists without a value — bash stores
                // `declare -r r` with `value == NULL`, so `[ -v r ]` is false.
                let mut v = Variable::unset(Shape::Scalar);
                v.readonly = true;
                self.vars.insert(name.to_string(), v);
            }
        }
    }
```

`declare_associative` (and its indexed twin) create `Variable::unset(Shape::Associative)` / `Variable::unset(Shape::Indexed)` where they currently insert an empty map. Their conversion guards keep reading `value.shape()`, so `declare -a y; declare -A y` still refuses.

`builtin_declare`'s no-value path: where it currently materialises a value for a name that does not exist, insert `Variable::unset(shape)` with the requested attributes, `shape` coming from the `-a`/`-A` flags (default `Shape::Scalar`).

`builtin_local`'s no-value path: create `Variable::unset(shape)` in the current scope, and — per #691 — copy ONLY the `exported` flag from the binding being shadowed (the snapshot the frame just took), never `integer`, `readonly`, `case_fold` or the shape:

```rust
    // #691: bash's `local V` over an exported outer keeps the export
    // attribute and nothing else — the local is a FRESH variable whose
    // value is NULL, so `declare -p V` prints `declare -x V`.
    let inherited_export = shadowed.as_ref().is_some_and(|v| v.exported);
    let mut fresh = Variable::unset(shape);
    fresh.exported = inherited_export;
    self.vars.insert(name.to_string(), fresh);
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo build -p huck --locked --jobs 1
( ulimit -v 4000000; timeout 300 tests/scripts/declared_unset_diff_check.sh )
```

Expected: every row PASS except the export-set rows, which Task 4 owns. If a `set -u` array row fails, apply the note from Task 2 Step 3 (keep the array arms reading the map).

Also: `cargo test -p huck-engine --locked --jobs 1 --lib -- --test-threads 1` — note any newly failing pinned tests for Task 5; do not edit them here.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
chmod +x tests/scripts/declared_unset_diff_check.sh
git add crates/huck-engine/src/shell_state.rs crates/huck-engine/src/builtins.rs tests/scripts/declared_unset_diff_check.sh
git commit -m "feat(#600,#225,#33,#691): valueless declarations create the unset state

declare/local/readonly/export and the attribute mutators now record
attributes without materialising a value, so declare -p prints no \`=…\`,
[[ -v ]] is false, set -u raises on a scalar read, the listings skip the
name, and a bare \`local V\` inherits only the export attribute.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: The export set, and `local -x`

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (`exported_env` at :3714)
- Modify: `crates/huck-engine/src/builtins.rs` (`builtin_local`'s option table at ~:1839)
- Modify: `tests/scripts/declared_unset_diff_check.sh` (append rows)

**Interfaces:**
- Consumes: `VarValue::Unset`, `local_scopes` (`Vec<HashMap<String, Option<Variable>>>`, innermost last, each entry the binding that was SHADOWED).
- Produces: `exported_env(&self) -> Vec<(&str, &str)>` (was `impl Iterator`); its three production callers in `executor.rs` use `.envs(...)` and need no change beyond the type.

- [ ] **Step 1: Write the failing test**

Append to `tests/scripts/declared_unset_diff_check.sh`, above `harness_summary`:

```bash
# --- #692: the export set answers TWO different questions -------------------
# `export -p` reports the innermost VISIBLE binding if exported; a CHILD's
# environment takes the innermost binding that is exported AND has a value,
# walking past an unexported (or valueless) local to the outer binding.
check "unexported local"    'declare -x V=1; f(){ local +x V=2; env | grep "^V="; echo "[$V]"; declare -p V; }; f; env | grep "^V="'
check "export -p vs child"  'declare -x V=1; f(){ local +x V=2; export -p | grep -c "^declare -x V="; env | grep -c "^V=1$"; }; f'
check "bare local, valueless" 'declare -x V=1; f(){ local V; env | grep "^V="; declare -p V; }; f'
check "assign to unexported" 'declare -x V=1; f(){ local +x V=2; V=3; env | grep "^V="; declare -p V; }; f'
check "nested local"        'declare -x V=1; f(){ local +x V=2; g(){ local V=3; env | grep "^V="; declare -p V; }; g; }; f'
check "export inside"       'declare -x V=1; f(){ local +x V=2; export V; env | grep "^V="; }; f'
check "unexported outer"    'V=1; f(){ local +x V=2; env | grep "^V=" || echo none; }; f'
check "sibling unaffected"  'declare -x V=1 W=9; f(){ local +x V=2; env | grep -E "^(V|W)=" | sort; }; f'
check "local assign exports" 'declare -x A=1; f(){ local A; A=2; env | grep "^A="; declare -p A; }; f'

# --- #777: `local -x` -------------------------------------------------------
check "local -x"            'f(){ local -x L=5; declare -p L; env | grep "^L="; }; f; env | grep -c "^L="'
check "local -x over outer" 'declare -x V=1; f(){ local -x V=2; env | grep "^V="; declare -p V; }; f; env | grep "^V="'
check "local -x no value"   'declare -x V=1; f(){ local -x V; env | grep "^V="; declare -p V; }; f'
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo build -p huck --locked --jobs 1
( ulimit -v 4000000; timeout 300 tests/scripts/declared_unset_diff_check.sh )
```

Expected: the twelve new rows FAIL — huck's child sees nothing for `V` (it builds the environment from the single visible binding), and `local -x` errors with `local: -x: invalid option`.

- [ ] **Step 3: Write minimal implementation**

Replace `exported_env` with the outward walk:

```rust
    /// The environment a child process inherits. bash answers TWO different
    /// questions about exports, and this is the second one (#692): for each
    /// NAME, the innermost binding that is exported AND has a value. An
    /// unexported local does not remove the enclosing binding from the export
    /// set, so `declare -x V=1; f(){ local +x V=2; env; }` shows `V=1`; nor
    /// does a valueless one, so a bare `local V` (which inherits export, #691)
    /// also lets the outer value through. The FIRST question — what
    /// `export -p` lists — is the visible binding only, and lives in the
    /// export builtin, not here.
    ///
    /// `local_scopes` makes the walk cheap: each frame maps a name to the
    /// binding it SHADOWED, innermost frame last, so iterating the frames in
    /// reverse yields successively outer bindings.
    pub fn exported_env(&self) -> Vec<(&str, &str)> {
        fn exported_scalar(v: &Variable) -> Option<&str> {
            // bash never inherits arrays into a child's environment (#28), and
            // a valueless variable contributes nothing (#600).
            match (&v.value, v.exported) {
                (VarValue::Scalar(s), true) => Some(s.as_str()),
                _ => None,
            }
        }

        let mut env: std::collections::BTreeMap<&str, &str> = std::collections::BTreeMap::new();
        // Innermost first: the visible bindings…
        for (k, v) in &self.vars {
            if let Some(s) = exported_scalar(v) {
                env.entry(k.as_str()).or_insert(s);
            }
        }
        // …then outward through the shadowed snapshots. `or_insert` keeps the
        // innermost winner, so a name already resolved is never overwritten.
        for frame in self.local_scopes.iter().rev() {
            for (k, snapshot) in frame {
                if let Some(v) = snapshot
                    && let Some(s) = exported_scalar(v)
                {
                    env.entry(k.as_str()).or_insert(s);
                }
            }
        }
        // An inline prefix assignment over an array target wins outright (#28).
        for (k, v) in &self.inline_scalar_export {
            env.insert(k.as_str(), v.as_str());
        }
        env.into_iter().collect()
    }
```

Add `x` to `builtin_local`'s accepted option letters beside the existing `+x` handling, so `local -x NAME=value` marks the local exported for the call (#777). The letter joins the same `DeclArg` path `declare -x` already uses; no separate code.

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo build -p huck --locked --jobs 1
( ulimit -v 4000000; timeout 300 tests/scripts/declared_unset_diff_check.sh )
```

Expected: ALL rows PASS.

Run the neighbouring suites, which exercise the environment a child sees:

```bash
cargo test -p huck --locked --jobs 1 --test builtin_vars -- --test-threads 1
cargo test -p huck --locked --jobs 1 --test local_integration -- --test-threads 1
( ulimit -v 4000000; timeout 300 tests/scripts/local_shadow_diff_check.sh )
( ulimit -v 4000000; timeout 300 tests/scripts/export_diff_check.sh 2>/dev/null || true )
```

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/shell_state.rs crates/huck-engine/src/builtins.rs tests/scripts/declared_unset_diff_check.sh
git commit -m "feat(#692,#777): a child's environment walks outward to the innermost exported, valued binding

exported_env no longer builds from the single visible binding: for each name
it takes the innermost binding that is exported AND has a value, using the
local_scopes snapshots as the outward chain. So an unexported local no longer
hides an exported outer from a child. \`local -x\` joins the option table.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Adjudicate the pinned tests, then the full gates

**Files:**
- Modify: `tests/declare_integration.rs`, `tests/declare_integer_integration.rs`, `tests/builtin_vars.rs`, `tests/printf_v_array_integration.rs`, `crates/huck-engine/src/shell_state/array_value_tests.rs`, `crates/huck-engine/src/shell_state/assoc_value_tests.rs`, `crates/huck-engine/src/builtins/local_tests.rs`, `crates/huck-engine/src/builtins/array_declare_tests.rs` — only the assertions that fail.

**Interfaces:**
- Consumes: everything above.
- Produces: a green tree.

- [ ] **Step 1: List every failing assertion**

```bash
for c in huck-syntax huck-engine huck-cli; do
  cargo test -p $c --locked --jobs 1 --lib -- --test-threads 1 2>&1 | grep -E '^test .* FAILED'
done
for f in tests/*.rs; do t=$(basename "$f" .rs)
  cargo test -p huck --locked --jobs 1 --test "$t" -- --test-threads 1 2>&1 | grep -E '^test .* FAILED' | sed "s|^|$t: |"
done
```

Write the list down. Roughly 33 assertions are expected, concentrated in the four integration files above.

- [ ] **Step 2: Adjudicate each one against bash, individually**

For EACH failing assertion, run its fragment through real bash first:

```bash
cd /tmp && bash --norc --noprofile -c '<the fragment>'
```

Then classify:

- **bash agrees with the NEW behaviour** → the assertion pinned a divergence. Update it to bash's output and add a one-line comment naming #600 and what bash prints.
- **bash agrees with the OLD behaviour** → this is a REGRESSION in the implementation, not a stale test. Stop, fix the code, and re-run. Do not edit the test.
- **the assertion is about something else** that merely mentions `=()` → leave it untouched; the failure is incidental and means the code is wrong somewhere else.

Never bulk-replace. This project has repeatedly found that a pinned test was right and the change was wrong.

- [ ] **Step 3: Re-run every suite**

```bash
for c in huck-syntax huck-engine huck-cli; do
  cargo test -p $c --locked --jobs 1 --lib -- --test-threads 1 | tail -1
done
for f in tests/*.rs; do t=$(basename "$f" .rs)
  ( ulimit -v 6000000; cargo test -p huck --locked --jobs 1 --test "$t" -- --test-threads 1 ) | tail -1
done
for f in crates/huck-engine/tests/*.rs; do t=$(basename "$f" .rs)
  ( ulimit -v 6000000; cargo test -p huck-engine --locked --jobs 1 --test "$t" -- --test-threads 4 ) | tail -1
done
```

Expected: all green.

- [ ] **Step 4: The full gates**

```bash
cargo fmt --all --check
cargo +1.97.1 clippy --workspace --all-targets --locked --jobs 1 -- -D warnings
cargo build -p huck --locked --jobs 1
cargo build --release --locked --bin huck --jobs 1
( ulimit -v 4000000; tests/scripts/run_diff_checks.sh )
```

Expected: fmt clean, clippy clean, sweep green (332 harnesses — 331 plus the new one). Run the sweep with nothing else competing for the CPU; a harness that prints NOTHING has hit the per-harness timeout, not diverged.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "test(#600): adjudicate the assertions that pinned the materialised-empty value

Each failing assertion was re-measured against bash 5.2.21 individually and
updated only where bash agrees with the new behaviour.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Before the PR

- Update `docs/superpowers/plans/2026-09-17-tech-debt-hubs-roadmap.md`: tick hub 2A, record what the round found.
- Record the iteration in the memory files (`project_huck_iterations.md` + `MEMORY.md`).
- Write the blog entry: `site/content/blog/<slug>.mdx`, with before/after output taken from a binary built at the PRE-iteration commit in a throwaway worktree (`git worktree add <tmp> <sha>`), never from memory. Validate with `cd site && . ~/.nvm/nvm.sh && nvm use node && ( ulimit -v 12000000; node_modules/.bin/velite --strict )`.
- Open a PR against `main` with `Closes #600`, `Closes #225`, `Closes #33`, `Closes #691`, `Closes #692`, `Closes #777`, and HAND IT TO THE USER — a `vNN` iteration is not self-merged.
- File any neighbouring divergence the round turns up as its own `divergence` issue; do not smuggle it into this PR.
