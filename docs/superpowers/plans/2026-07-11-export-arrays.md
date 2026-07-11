# v282 — Fix #82 (`export name=(array)`) + #28 (exported-array env leak) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `export a=(1 2 3)` assign the array and mark it exported (matching bash, #82), and stop exported arrays from leaking their `[0]` element into a child's environment (#28).

**Architecture:** Two small independent fixes. (1) In `builtin_export_decl`, delete the array-assignment rejection so array RHS flows through the existing `apply_one_assignment` + `shell.export` path scalars already use. (2) In `exported_env`, emit only `VarValue::Scalar` values so `Indexed`/`Associative` arrays are omitted from the child environment.

**Tech Stack:** Rust (huck-engine builtins + shell_state), bash diff-check harness.

## Global Constraints

- **Files touched:** `crates/huck-engine/src/builtins.rs`, `crates/huck-engine/src/shell_state.rs`, their test modules (`crates/huck-engine/src/builtins/array_declare_tests.rs`), and a new `tests/scripts/export_array_diff_check.sh`. No other files.
- **Run tests per-crate, single-threaded** (box OOMs on `--workspace`): `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. Build the binary with `cargo build -p huck` (and `cargo build --release -p huck` before the sweep).
- **`cargo fmt --all` before each commit**; CI enforces `cargo fmt --all --check`.
- **Every commit** ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Branch `v282-export-arrays`; do not push to main / do not merge.** PR (`Closes #82`, `Closes #28`) is for the user.
- `VarValue` variants (shell_state.rs): `Scalar(String)`, `Indexed(BTreeMap<usize,String>)`, `Associative(Vec<(String,String)>)`.

---

### Task 1: #82 — let `export name=(array)` assign + mark exported

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (`builtin_export_decl` ~1461-1465; dead helper `assign_value_is_array` ~1314-1322; doc comment ~1319)
- Test: `crates/huck-engine/src/builtins/array_declare_tests.rs` (rewrite `export_array_rejects`, ~line 60)

**Interfaces:**
- Consumes: `crate::executor::apply_one_assignment` (handles array literals), `shell.export(name)`, the `run(shell, line)` test helper (`process_line`), `s.get_indexed(name)`, `s.iter_vars()` (`v.exported`, `v.value`).
- Produces: `export a=(…)` returns rc 0, creates the `Indexed` value, sets the export bit.

- [ ] **Step 1: Rewrite the failing test to assert the NEW behavior**

In `crates/huck-engine/src/builtins/array_declare_tests.rs`, REPLACE the existing `export_array_rejects` test (lines ~60-68) with:
```rust
#[test]
fn export_array_assigns_and_exports() {
    // #82: bash accepts `export a=(x y)` — assign the indexed array AND mark it
    // exported (declare -ax), rc 0. huck used to reject with "cannot export
    // arrays" and not create the variable.
    let mut s = Shell::new();
    let outcome = run(&mut s, "export a=(x y)");
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let m = s.get_indexed("a").expect("array a created");
    assert_eq!(m.get(&0).map(String::as_str), Some("x"));
    assert_eq!(m.get(&1).map(String::as_str), Some("y"));
    let (_, v) = s
        .iter_vars()
        .find(|(n, _)| n.as_str() == "a")
        .expect("a is set");
    assert!(v.exported, "a must carry the export attribute");
    assert!(matches!(v.value, crate::shell_state::VarValue::Indexed(_)));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 export_array_assigns_and_exports`
Expected: FAIL — current code rejects with rc 1 and does not create `a`.

- [ ] **Step 3: Remove the array rejection in `builtin_export_decl`**

In `crates/huck-engine/src/builtins.rs`, in the `DeclArg::Assign(a) =>` arm, DELETE the 5-line rejection block (keep everything after it — the `AssignTarget::Indexed` check, readonly check, `apply_one_assignment`, and export):
```rust
                if assign_value_is_array(a) {
                    crate::sh_error_to!(shell, err, None, "export: cannot export arrays");
                    any_error = true;
                    continue;
                }
```

- [ ] **Step 4: Delete the now-dead `assign_value_is_array` helper**

Its only caller was the block just deleted, so it now triggers `dead_code`. Remove the helper and its doc comment (~builtins.rs:1314-1322):
```rust
/// True iff the `Word` value of an Assignment carries a trailing
/// `ArrayLiteral` (i.e. it's a compound-RHS form like `name=(x y)`).
fn assign_value_is_array(a: &crate::command::Assignment) -> bool {
    matches!(
        a.value.0.last(),
        Some(crate::lexer::WordPart::ArrayLiteral(_))
    )
}
```

- [ ] **Step 5: Update the `builtin_export_decl` doc comment**

Change the comment above `builtin_export_decl` (currently begins "`export` entry point with DeclArg input. Rejects array compound-RHS; otherwise mirrors …") to drop the "Rejects array compound-RHS" clause. New text:
```rust
/// `export` entry point with DeclArg input. Mirrors the legacy `builtin_export`
/// behavior: scalar `=` assigns + exports; array compound-RHS (`name=(x y)`)
/// assigns the array via `apply_one_assignment` and sets the export attribute
/// (bash `declare -ax`); bare `NAME` flips the export bit without checking
/// readonly.
```

- [ ] **Step 6: Run the test + the full crate suite**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`
Expected: `export_array_assigns_and_exports` PASSES and the whole `huck-engine` suite is green (no other test asserted the old rejection — verified: `export_array_rejects` was the only one).

- [ ] **Step 7: Format and commit**

```bash
cargo fmt --all && cargo fmt --all --check
git add crates/huck-engine/src/builtins.rs crates/huck-engine/src/builtins/array_declare_tests.rs
git commit -m "fix: export name=(array) assigns the array and marks it exported (#82)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: #28 — omit array-typed variables from the exported environment

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (`exported_env` ~line 2998)
- Test: `crates/huck-engine/src/builtins/array_declare_tests.rs` (add one test)
- Create: `tests/scripts/export_array_diff_check.sh`

**Interfaces:**
- Consumes: `VarValue` (in scope in shell_state.rs); `shell.exported_env()`; the `run` helper.
- Produces: `exported_env` yields only `Scalar` exported vars.

- [ ] **Step 1: Write the failing exported_env test**

In `crates/huck-engine/src/builtins/array_declare_tests.rs`, add:
```rust
#[test]
fn exported_array_omitted_from_child_env_but_scalar_kept() {
    // #28: bash never puts an array into a child's environment; huck used to
    // leak the [0] element as a scalar. An exported scalar is still inherited.
    let mut s = Shell::new();
    let _ = run(&mut s, "export a=(x y z)");
    let _ = run(&mut s, "export s=hi");
    assert!(
        !s.exported_env().any(|(k, _)| k == "a"),
        "exported array must NOT appear in the child environment"
    );
    assert!(
        s.exported_env().any(|(k, v)| k == "s" && v == "hi"),
        "exported scalar must still be inherited"
    );
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 exported_array_omitted_from_child_env_but_scalar_kept`
Expected: FAIL — `exported_env` currently emits `a` (its `[0]` = "x").

- [ ] **Step 3: Filter arrays out of `exported_env`**

In `crates/huck-engine/src/shell_state.rs`, replace the body of `exported_env` (~line 2998):
```rust
    pub fn exported_env(&self) -> impl Iterator<Item = (&str, &str)> {
        self.vars
            .iter()
            .filter(|(_, v)| v.exported)
            .map(|(k, v)| (k.as_str(), v.value.scalar_view()))
    }
```
with:
```rust
    pub fn exported_env(&self) -> impl Iterator<Item = (&str, &str)> {
        self.vars
            .iter()
            .filter(|(_, v)| v.exported)
            // bash never inherits array variables into a child's environment;
            // emit only true scalars (skip Indexed/Associative). See #28.
            .filter_map(|(k, v)| match &v.value {
                VarValue::Scalar(s) => Some((k.as_str(), s.as_str())),
                VarValue::Indexed(_) | VarValue::Associative(_) => None,
            })
    }
```

- [ ] **Step 4: Run the test + the full crate suite**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`
Expected: `exported_array_omitted_from_child_env_but_scalar_kept` PASSES; the whole `huck-engine` suite green (no test relied on an array being present in `exported_env`).

- [ ] **Step 5: Create the bash-diff harness**

Create `tests/scripts/export_array_diff_check.sh` (mode 0755) covering both fixes:
```bash
#!/usr/bin/env bash
# v282: byte-identical bash<->huck for exported arrays.
#   #82 — `export a=(...)` assigns the indexed array + marks it exported
#         (declare -ax), rc 0 (huck used to error "cannot export arrays").
#   #28 — an exported array is NOT inherited by a child process (bash puts no
#         array in the environment); an exported scalar IS. `printenv` is an
#         ordinary external child, so the same fragment runs under both shells.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
norm() { sed -E 's#^([^:]*/)?(bash|huck): (line [0-9]+: )?##'; }
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?"); b=$(printf '%s\n' "$b" | norm)
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?"); h=$(printf '%s\n' "$h" | norm)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "export array assigns"     'export a=(1 2 3); declare -p a'
check "export array rc"          'export a=(1 2 3); echo "rc=$?"'
check "export existing array"    'a=(x y); export a; declare -p a'
check "export array append"      'a=(1 2 3); export a+=(4 5); declare -p a'
check "array not in child env"   'export a=(x y z); printenv a; echo "rc=$?"'
check "scalar IS in child env"   'export s=hi; printenv s; echo "rc=$?"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 6: Run the new harness — expect all green**

```bash
cargo build -p huck
HUCK_BIN="$(pwd)/target/debug/huck" bash tests/scripts/export_array_diff_check.sh; echo "exit=$?"
```
Expected: every case `PASS:`, `Total: 6, Pass: 6, Fail: 0`, `exit=0`.

- [ ] **Step 7: Run the full diff-check sweep**

```bash
cargo build -p huck && cargo build --release -p huck
tests/scripts/run_diff_checks.sh; echo "exit=$?"
```
Expected: `Diff-check sweep: 181 passed, 0 failed`, `exit=0` (180 prior + the new harness).

- [ ] **Step 8: Format and commit**

```bash
cargo fmt --all && cargo fmt --all --check
chmod +x tests/scripts/export_array_diff_check.sh
git add crates/huck-engine/src/shell_state.rs crates/huck-engine/src/builtins/array_declare_tests.rs tests/scripts/export_array_diff_check.sh
git commit -m "fix: exported arrays no longer leak [0] into a child's environment (#28)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final verification (after all tasks)

- [ ] `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` green.
- [ ] End-to-end vs bash:
```bash
HUCK="$(pwd)/target/debug/huck"
"$HUCK" -c 'export a=(1 2 3); declare -p a; echo rc=$?'     # declare -ax a=([0]="1"…), rc 0
"$HUCK" -c 'export a=(x y); printenv a; echo rc=$?'          # (empty), rc 1
```
Expected: identical to `bash -c` of the same.

## Notes for the whole-branch review

- The two fixes are independent (different files); confirm neither regresses the other's tests.
- `scalar_view` is intentionally retained (used elsewhere for display); only `exported_env` stops calling it.
- Expected sweep count rises 180 → 181 (one new harness).
- Out of scope (per spec): array display, `exported_function_env`, other `export`/`readonly` edges (#65/#23/#67).
