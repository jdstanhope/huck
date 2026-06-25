# `declare -xF` export filter + FUNCNAME write protection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two independent `func`-category correctness fixes: (1) `declare -F`/`-xF` reflects the export attribute (`declare -fx NAME`) and `-x` filters to exported functions; (2) writes to `FUNCNAME` are silently discarded (matching bash 5.2.21).

**Architecture:** Component 1 is localized to the function-listing path in `builtins.rs` (`emit_function` + `declare_list_functions` + one dispatch call). Component 2 adds a FUNCNAME guard to the two user-facing variable setters in `shell_state.rs` (`set` + `assign`); `read`/`for`/`+=`/`[i]=` all funnel through these, and the call-stack rebuild bypasses them.

**Tech Stack:** Rust (`huck-engine`); bash diff harnesses under `tests/scripts/`.

## Global Constraints

- Commit trailer on EVERY commit, verbatim: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Run the FULL suite with `cargo test --workspace` (~3697 baseline) — plain `cargo test` skips most crates.
- Byte-faithfulness oracle is system `bash` (5.2.21).
- `cargo build --release --bin huck` is slow — long timeout (~480000ms).
- bash rules (verified): `declare -F` lists `declare -fx NAME` for exported, `declare -f NAME` otherwise; `declare -xF` lists only exported (nothing if none). Every write to FUNCNAME (`=`, `+=`, `[i]=`, `for`, `read`, inside-function) is silently discarded, rc 0, no error; `$FUNCNAME` is empty at top level.
- Do NOT change FUNCNAME population (the `rebuild_call_stack_vars`/`set_indexed_var` path) or protect BASH_SOURCE/BASH_LINENO (out of scope — they are top-level-populated).

---

### Task 1: `declare -F`/`-xF` export attribute format + `-x` filter

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (`emit_function` ~1070; `declare_list_functions` ~1037; dispatch call ~1807)
- Test: `crates/huck-engine/src/builtins.rs` tests module
- Create: `tests/scripts/declare_func_export_diff_check.sh`

**Interfaces:**
- `shell.is_function_exported(name: &str) -> bool` (shell_state.rs:2161) — existing.
- `shell.define_function(name: String, body: Box<Command>)` (shell_state.rs:2141) — for test setup.
- `shell.mark_function_exported(name: &str)` (shell_state.rs:2149).
- `declare_list_functions(names: &[String], names_only: bool, out: &mut dyn Write, shell: &mut Shell)` gains a `want_export: bool` parameter.

- [ ] **Step 1: Write the failing unit test**

Add to the `builtins.rs` tests module (mirrors the `let mut out: Vec<u8>` pattern):

```rust
    fn define_fn(shell: &mut crate::shell_state::Shell, src: &str) {
        let seq = crate::command::parse(crate::lexer::tokenize(src).unwrap()).unwrap().unwrap();
        let crate::command::Command::FunctionDef { name, body } = seq.first else { panic!("not a func def") };
        shell.define_function(name, body);
    }

    #[test]
    fn declare_big_f_listing_reflects_export_attr_and_filter() {
        let mut shell = crate::shell_state::Shell::new();
        define_fn(&mut shell, "a(){ :; }");
        define_fn(&mut shell, "zf(){ :; }");
        shell.mark_function_exported("zf");

        // -F listing (want_export=false): plain `declare -f a`, exported `declare -fx zf`.
        let mut out: Vec<u8> = Vec::new();
        declare_list_functions(&[], true, false, &mut out, &mut shell);
        assert_eq!(String::from_utf8(out).unwrap(), "declare -f a\ndeclare -fx zf\n");

        // -xF listing (want_export=true): only the exported function.
        let mut out2: Vec<u8> = Vec::new();
        declare_list_functions(&[], true, true, &mut out2, &mut shell);
        assert_eq!(String::from_utf8(out2).unwrap(), "declare -fx zf\n");
    }
```

(Note the new `want_export` arg appears as the 3rd positional in these calls — see Step 3 for the final signature order.)

- [ ] **Step 2: Run the test to confirm it FAILS**

Run: `cargo test -p huck-engine declare_big_f_listing_reflects_export_attr_and_filter`
Expected: FAIL to COMPILE first (arity mismatch — `want_export` not yet a parameter), which is the intended red state; after Step 3 it compiles and passes.

- [ ] **Step 3: Implement the format + filter**

In `emit_function` (~1070), the names-only listing branch — replace the `declare -f {name}` line:

```rust
    if names_only {
        if explicit {
            let _ = writeln!(out, "{name}");
        } else {
            // bash: listing form reflects the export attribute.
            if shell.is_function_exported(name) {
                let _ = writeln!(out, "declare -fx {name}");
            } else {
                let _ = writeln!(out, "declare -f {name}");
            }
        }
    } else if let Some(body) = shell.functions.get(name) {
        let _ = writeln!(out, "{}", crate::generate::function_to_source(name, body));
    }
```

In `declare_list_functions` (~1037), add `want_export: bool` (place it right after `names_only`) and skip non-exported in the no-names listing:

```rust
fn declare_list_functions(
    names: &[String],
    names_only: bool,
    want_export: bool,
    out: &mut dyn std::io::Write,
    shell: &mut Shell,
) -> ExecOutcome {
    if names.is_empty() {
        let mut fnames: Vec<String> = shell.functions.keys().cloned().collect();
        fnames.sort();
        for n in &fnames {
            if want_export && !shell.is_function_exported(n) {
                continue;
            }
            emit_function(n, names_only, false, out, shell);
        }
        return ExecOutcome::Continue(0);
    }
    // (explicit-name branch unchanged — bash applies the export filter only to the bulk listing)
    let mut exit: i32 = 0;
    for name in names {
        if shell.functions.contains_key(name) {
            emit_function(name, names_only, true, out, shell);
        } else {
            exit = 1;
        }
    }
    ExecOutcome::Continue(exit)
}
```

At the dispatch call (~1807) pass the already-parsed export flag:

```rust
        return declare_list_functions(&plain_names, function_names_only, want_export, out, shell);
```

(The local flag is named `want_export` in this function — confirm the exact identifier in scope at that call site and use it.)

- [ ] **Step 4: Run the unit test to confirm it PASSES**

Run: `cargo test -p huck-engine declare_big_f_listing_reflects_export_attr_and_filter`
Expected: PASS.

- [ ] **Step 5: Add the diff harness**

Create `tests/scripts/declare_func_export_diff_check.sh`, mirroring
`tests/scripts/declare_f_diff_check.sh`'s structure (shebang, `HUCK_BIN` →
`target/release/huck`, bash-absent SKIP, a `fragments` array, PASS/FAIL loop
comparing combined stdout+exit of `bash --norc --noprofile` vs `"$HUCK_BIN"`,
`exit $(( FAIL>0 ? 1 : 0 ))`). Fragments:

```bash
fragments=(
  'a(){ :; }; b(){ :; }; declare -xF; echo END'
  'a(){ :; }; b(){ :; }; declare -xf; echo END'
  'a(){ :; }; zf(){ echo z; }; export -f zf; declare -F'
  'a(){ :; }; zf(){ echo z; }; export -f zf; declare -xF'
  'a(){ :; }; zf(){ echo z; }; export -f zf; declare -xf'
  'a(){ :; }; declare -F a'
  'a(){ :; }; zf(){ :; }; export -f zf; declare -F zf'
)
```

Run:
```bash
cargo build --release --bin huck   # slow, ~480000ms timeout
bash tests/scripts/declare_func_export_diff_check.sh | tail -2
```
Expected: `Fail: 0`.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/builtins.rs tests/scripts/declare_func_export_diff_check.sh
git commit -m "$(printf 'v223 task 1: declare -F/-xF export attribute format + -x filter\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: FUNCNAME write protection

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (`set` ~999; `assign` ~1595; add predicate)
- Test: `crates/huck-engine/src/shell_state.rs` tests module
- Create: `tests/scripts/funcname_assign_diff_check.sh`

**Interfaces:**
- `fn is_write_protected_var(name: &str) -> bool` — new free function (returns `name == "FUNCNAME"`); single point so siblings can be added later.
- `Shell::set(&mut self, name: &str, value: String)` and `Shell::assign(&mut self, dest, op, source) -> Result<(), AssignErr>` gain an early FUNCNAME guard.

- [ ] **Step 1: Write the failing unit tests**

Add to the `shell_state.rs` tests module:

```rust
    #[test]
    fn funcname_assignment_is_silently_discarded() {
        let mut sh = Shell::new();
        // `set` path (used by `for`, internal writers).
        sh.set("FUNCNAME", "7".to_string());
        assert_eq!(sh.lookup_var("FUNCNAME"), None, "set must not write FUNCNAME");
        // `assign` path (used by `FOO=v`, inline, declare, read via try_set).
        let _ = sh.try_set("FUNCNAME", "9".to_string());
        assert_eq!(sh.lookup_var("FUNCNAME"), None, "assign must not write FUNCNAME");
    }

    #[test]
    fn non_protected_var_still_writes() {
        let mut sh = Shell::new();
        sh.set("FOO", "x".to_string());
        assert_eq!(sh.lookup_var("FOO"), Some("x".to_string()));
        let _ = sh.try_set("BAR", "y".to_string());
        assert_eq!(sh.lookup_var("BAR"), Some("y".to_string()));
    }
```

(The existing call-stack tests around shell_state.rs:2850 already prove
`rebuild_call_stack_vars` still POPULATES FUNCNAME — they must keep passing,
confirming the guard does not block the rebuild.)

- [ ] **Step 2: Run the tests to confirm they FAIL**

Run: `cargo test -p huck-engine funcname_assignment_is_silently_discarded non_protected_var_still_writes`
Expected: `funcname_assignment_is_silently_discarded` FAILS (FUNCNAME currently persists); `non_protected_var_still_writes` passes.

- [ ] **Step 3: Add the predicate + guards**

Add the predicate near the other variable helpers in `shell_state.rs`:

```rust
/// Variables the shell maintains itself and whose user writes bash silently
/// discards (rc 0, no error). Currently FUNCNAME only; BASH_SOURCE/BASH_LINENO
/// share the behavior but are top-level-populated and deferred.
fn is_write_protected_var(name: &str) -> bool {
    name == "FUNCNAME"
}
```

In `Shell::set` (~999), guard at the very top (before the restricted check):

```rust
    pub fn set(&mut self, name: &str, value: String) {
        if is_write_protected_var(name) {
            return; // bash silently discards writes to FUNCNAME
        }
        if self.restricted
            && let Err(msg) = crate::restricted::check_special_assign(name)
        { /* …unchanged… */ }
        self.store_scalar(name, value);
    }
```

In `Shell::assign` (~1595), guard right after the resolved `name` is computed
(so `FUNCNAME[0]=…` and a nameref to FUNCNAME are caught), before the
restricted/readonly checks:

```rust
        let name = dest.name().to_string();
        if is_write_protected_var(&name) {
            return Ok(()); // bash silently discards writes to FUNCNAME
        }
        // …restricted gate, readonly check, stores unchanged…
```

- [ ] **Step 4: Run the unit tests to confirm they PASS**

Run: `cargo test -p huck-engine funcname_assignment_is_silently_discarded non_protected_var_still_writes`
Expected: both PASS.

- [ ] **Step 5: Add the diff harness**

Create `tests/scripts/funcname_assign_diff_check.sh`, mirroring
`declare_f_diff_check.sh`'s structure (compare combined stdout+exit of
`bash --norc --noprofile` vs `target/release/huck`). Fragments:

```bash
fragments=(
  'FUNCNAME=7; echo "[$FUNCNAME]"'
  'FUNCNAME=7; echo $?'
  'for FUNCNAME in x y; do :; done; echo "[$FUNCNAME]"'
  'read FUNCNAME <<< hello; echo "[$FUNCNAME]"'
  'f(){ FUNCNAME=x; echo "[$FUNCNAME]"; }; f'
  'f(){ echo "[$FUNCNAME]"; }; f; echo "[$FUNCNAME]"'
  'FUNCNAME+=z; echo "[$FUNCNAME]"'
)
```

Run:
```bash
cargo build --release --bin huck   # slow if not already built
bash tests/scripts/funcname_assign_diff_check.sh | tail -2
```
Expected: `Fail: 0`.

- [ ] **Step 6: Full suite + func re-measure + no-regress guard**

Run: `cargo test --workspace`
Expected: PASS (~3697+). If any existing test breaks, check whether it encoded
the old behavior (a test asserting `FUNCNAME` is writable, or `declare -F` of an
exported function as `declare -f`) — update to bash-faithful — or is a genuine
regression (STOP, report BLOCKED).

Re-measure the categories (both Task 1 + Task 2 fixes now present):
```bash
for cat in func cprint herestr; do
  BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_BASH_TEST_HELPERS=/tmp/bash-test-helpers \
    HUCK_BASH_TEST_CATEGORY=$cat bash tests/bash-test-suite/runner.sh 2>&1 | grep -E "\| $cat \|"
done
```
Expected: `cprint` + `herestr` stay PASS (no regression). `func` stays FAIL but
its diff SHRANK — capture the scratch `func.diff` and confirm the
`declare -f a … f1` block (the `declare -xF` hunk) and the
`outside: FUNCNAME = 7` hunk are GONE; the only residual is FUNCNEST (func4.sub).
Record whether any category incidentally flipped (not predicted).

- [ ] **Step 7: Commit**

```bash
git add crates/huck-engine/src/shell_state.rs tests/scripts/funcname_assign_diff_check.sh
git commit -m "$(printf 'v223 task 2: FUNCNAME writes are silently discarded\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Notes for the implementer

- Task 1: the export filter applies ONLY to the no-names bulk listing; leave the
  explicit-name branch (`declare -F NAME`) untouched — bash does too.
- Task 2: guarding `set` + `assign` is COMPLETE — `read`/`+=`/`[i]=` route through
  `assign` (via `try_set`/`replace_indexed`/`apply_one_assignment`) and `for` uses
  `set`. The rebuild uses `set_indexed_var` (direct insert) and is NOT guarded, so
  FUNCNAME population is preserved (existing call-stack tests prove it).
- Do NOT protect BASH_SOURCE/BASH_LINENO (top-level-populated — out of scope).
- func will NOT flip (FUNCNEST remains); the success criterion is the two diff
  hunks gone + no PASS-category regression.
