# huck v65 — integer attribute Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `declare -i` / `declare +i` (integer
attribute) — completes one row of v64's M-79 deferred list.

**Architecture:** Add `integer: bool` to `Variable`. Extend
`Shell::try_set` to arith-evaluate the RHS when the target is
integer-flagged (silent coerce to 0 on parse/eval failure).
Wire `-i`/`+i` into `builtin_declare`. Update
`format_declare_line` to emit `i` first in the attribute order
(`irx`).

**Tech Stack:** Rust. No new deps. Reuses v54's `try_set`,
v22's `arith::parse`/`arith::eval`.

**Spec:** `docs/superpowers/specs/2026-06-01-huck-integer-attr-design.md`

**Branch:** `v65-integer-attr`.

**Commit trailer:**

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble: Create the working branch

- [ ] **Step P.1**

```bash
git checkout main
git pull --ff-only
git checkout -b v65-integer-attr
```

Spec + this plan are committed before Task 1.

---

## Task 1: Foundation + try_set extension + declare wiring + 10 unit tests

**Files:**
- Modify `src/shell_state.rs` — add `integer` field to
  `Variable`; update existing `Variable { … }` literals; add
  3 new Shell methods; extend `try_set`.
- Modify `src/builtins.rs` — remove `-i` from deferred-flag
  list; add `-i`/`+i` paths in `builtin_declare`; update
  `format_declare_line` to put `i` first; readonly check for
  `declare -i X`; `mod integer_attr_tests` with 10 tests.

### Step 1.1: Add `integer` field to `Variable`

In `src/shell_state.rs:7-12`:

```rust
#[derive(Debug, Clone)]
pub struct Variable {
    pub value: String,
    pub exported: bool,
    pub readonly: bool,
    pub integer: bool,    // NEW
}
```

- [ ] **Step 1.1**

### Step 1.2: Update existing `Variable { … }` literals

Search the codebase:

```bash
grep -rn "Variable {" /home/john/projects/shuck/src/
```

For each construction (likely 4-5 spots in `shell_state.rs` and
maybe `builtins.rs`), add `integer: false,` alongside the other
fields. Compiler errors will list anywhere we miss.

- [ ] **Step 1.2**

### Step 1.3: Build

`cargo build`. Expected: clean once all literals are updated.

- [ ] **Step 1.3**

### Step 1.4: Add new `Shell` methods

In `src/shell_state.rs`, after the existing readonly helpers
(around line 260):

```rust
pub fn is_integer(&self, name: &str) -> bool {
    self.vars.get(name).map(|v| v.integer).unwrap_or(false)
}

pub fn mark_integer(&mut self, name: &str) {
    if let Some(v) = self.vars.get_mut(name) {
        v.integer = true;
    } else {
        self.vars.insert(name.to_string(), Variable {
            value: String::new(),
            exported: false,
            readonly: false,
            integer: true,
        });
    }
}

pub fn unmark_integer(&mut self, name: &str) {
    if let Some(v) = self.vars.get_mut(name) {
        v.integer = false;
    }
}
```

- [ ] **Step 1.4**

### Step 1.5: Extend `try_set`

Find the existing `pub fn try_set` (added in v54). Replace
with:

```rust
pub fn try_set(&mut self, name: &str, value: String) -> Result<(), ()> {
    if self.is_readonly(name) {
        return Err(());
    }
    let final_value = if self.is_integer(name) {
        eval_integer_coerce(self, &value)
    } else {
        value
    };
    self.set(name, final_value);
    Ok(())
}
```

Add the helper just below (or as a sibling `fn`):

```rust
/// Evaluate `value` as a bash arithmetic expression and return
/// the decimal-string result, or "0" on parse/eval failure
/// (bash's silent-coerce-to-zero semantics for integer-flagged
/// variable writes).
fn eval_integer_coerce(shell: &mut Shell, value: &str) -> String {
    match crate::arith::parse(value) {
        Ok(expr) => match crate::arith::eval(&expr, shell) {
            Ok(n) => n.to_string(),
            Err(_) => "0".to_string(),
        },
        Err(_) => "0".to_string(),
    }
}
```

Note: `eval_integer_coerce` is at module scope (not inside
`impl Shell`) because it takes `&mut Shell` and is used by
`try_set` after the readonly check. This is fine.

- [ ] **Step 1.5**

### Step 1.6: Build + run existing tests

`cargo build`. Then `cargo test --bin huck` to ensure the
Variable field addition + try_set extension hasn't broken
anything (especially the v54 readonly tests and v52 local_scopes
tests). Expected: no regressions.

- [ ] **Step 1.6**

### Step 1.7: Remove `-i` from deferred list in `builtin_declare`

Find the deferred-flag arm in `src/builtins.rs::builtin_declare`
(introduced in v64):

```rust
b'i' | b'l' | b'u' | b'a' | b'A' | b'n' | b'g' if minus => {
    eprintln!(
        "huck: declare: -{}: not yet implemented in this version",
        c as char
    );
    return ExecOutcome::Continue(1);
}
```

Remove `b'i' | ` so the arm becomes:

```rust
b'l' | b'u' | b'a' | b'A' | b'n' | b'g' if minus => {
    eprintln!(
        "huck: declare: -{}: not yet implemented in this version",
        c as char
    );
    return ExecOutcome::Continue(1);
}
```

- [ ] **Step 1.7**

### Step 1.8: Add `-i` / `+i` flag tracking

In the flag-parser section, add two new state variables (near
`want_readonly`, `want_export`, etc.):

```rust
let mut want_integer = false;
let mut want_remove_integer = false;
```

In the cluster loop, add the two new arms:

```rust
b'i' if minus => want_integer = true,
b'i' if plus => want_remove_integer = true,
```

Position them near `b'r'` / `b'x'` arms.

- [ ] **Step 1.8**

### Step 1.9: Handle `-i` and `+i` in the per-name mutation block

In `builtin_declare`, the per-name loop currently handles
`-r`/`-x`/`+x` paths. Add `-i`/`+i` handling.

**Order of operations** for `-i NAME[=val]`:
1. `snapshot_for_local_scope` (already in place from v64).
2. If NAME is readonly AND `-i` was requested but `-r` was not:
   error + exit 1 + continue. (Matches bash: integer flag
   transition on readonly is denied.)
3. If `-i`: `shell.mark_integer(name)` BEFORE the value-set
   so the subsequent `try_set` routes through integer-eval.
4. If value is present: call `try_set` (which now evaluates).
5. Other flags (`-r`/`-x`) apply per the existing v64 logic.

For `+i`:
- After snapshot, call `shell.unmark_integer(name)`.
- Don't allow `+i` on a readonly var (bash errors). Same guard
  as `-i`.

Rough sketch (slot into the existing per-name loop):

```rust
// Already in the loop after snapshot_for_local_scope(shell, name):

// Reject integer-attribute transitions on readonly vars
// (bash-compat).
if (want_integer || want_remove_integer)
    && shell.is_readonly(name)
{
    eprintln!("huck: declare: {name}: readonly variable");
    exit = 1;
    continue;
}

if want_integer {
    shell.mark_integer(name);
    // Fall through to the set-value path below.
}
if want_remove_integer {
    shell.unmark_integer(name);
    // No value-set in the +i case (matches +x's no-set
    // semantic). If the user wrote `declare +i X=val` we still
    // process the value below — bash's behavior is to set the
    // value (without integer coercion since we just unflagged).
}

// ... existing -r / -x / try_set logic continues ...
```

Place this block BEFORE the existing `-r`/`-x` handling so the
integer flag is set BEFORE `try_set` runs. Adjust the existing
per-flag branches to fall through correctly (avoid double-
processing).

The simplest structure (since v64's per-name loop already has
sequential branches): add a new branch for `-r` + value AND
`-i` + value combinations. The flag-state mutation happens
first (mark_integer / unmark_integer / mark_readonly); the
value-set happens last via `try_set` (which now respects both
flags).

Refactored skeleton:

```rust
// 1. Snapshot for unwinding.
snapshot_for_local_scope(shell, name);

// 2. Reject integer transition on readonly.
if (want_integer || want_remove_integer)
    && shell.is_readonly(name)
    && !want_readonly
{
    eprintln!("huck: declare: {name}: readonly variable");
    exit = 1;
    continue;
}

// 3. Apply attribute mutations (flag flips) first.
if want_integer { shell.mark_integer(name); }
if want_remove_integer { shell.unmark_integer(name); }

// 4. Apply -r and -x mutations.
//    For -r with value: check is_readonly, set, mark_readonly.
//    For -x: existing logic.
//    (See v64's flow; -i doesn't change these branches.)

// 5. Plain value assignment for unflagged paths.
//    try_set handles integer-coercion automatically because we
//    set the integer flag in step 3.
```

The cleanest way is to do attribute-flag flips FIRST, then
unified value-set via `try_set` at the end. Refactor the v64
per-name block accordingly.

- [ ] **Step 1.9**

### Step 1.10: Update `format_declare_line` for `irx` order

Current (v64):
```rust
fn format_declare_line(name: &str, var: &Variable) -> String {
    let mut attrs = String::new();
    if var.readonly { attrs.push('r'); }
    if var.exported { attrs.push('x'); }
    // ...
}
```

Change order to `irx` (bash's display order):

```rust
fn format_declare_line(name: &str, var: &Variable) -> String {
    let mut attrs = String::new();
    if var.integer { attrs.push('i'); }
    if var.readonly { attrs.push('r'); }
    if var.exported { attrs.push('x'); }
    // ...
}
```

Existing v64 tests checked `declare --` and `declare -rx`
formats. The `-rx` format stays valid (no integer flag). New
combos: `-i`, `-ir`, `-irx`.

- [ ] **Step 1.10**

### Step 1.11: Build

`cargo build`. Expected: clean.

- [ ] **Step 1.11**

### Step 1.12: Append `mod integer_attr_tests` (10 tests)

At end of `src/builtins.rs` (after `mod declare_tests`):

```rust
#[cfg(test)]
mod integer_attr_tests {
    use super::*;
    use crate::shell_state::Shell;

    // ── try_set integer-eval ────────────────────────────────

    #[test]
    fn try_set_non_integer_passes_through() {
        let mut shell = Shell::new();
        assert!(shell.try_set("X_INT_T1", "2+3".to_string()).is_ok());
        assert_eq!(shell.lookup_var("X_INT_T1").as_deref(), Some("2+3"));
    }

    #[test]
    fn try_set_integer_simple_arith() {
        let mut shell = Shell::new();
        shell.mark_integer("X_INT_T2");
        assert!(shell.try_set("X_INT_T2", "2+3".to_string()).is_ok());
        assert_eq!(shell.lookup_var("X_INT_T2").as_deref(), Some("5"));
    }

    #[test]
    fn try_set_integer_negative_result() {
        let mut shell = Shell::new();
        shell.mark_integer("X_INT_T3");
        assert!(shell.try_set("X_INT_T3", "0-5".to_string()).is_ok());
        assert_eq!(shell.lookup_var("X_INT_T3").as_deref(), Some("-5"));
    }

    #[test]
    fn try_set_integer_invalid_silently_zero() {
        let mut shell = Shell::new();
        shell.mark_integer("X_INT_T4");
        assert!(shell.try_set("X_INT_T4", "abc".to_string()).is_ok());
        assert_eq!(shell.lookup_var("X_INT_T4").as_deref(), Some("0"));
    }

    #[test]
    fn try_set_integer_with_var_ref() {
        let mut shell = Shell::new();
        shell.set("Y_INT_T5", "10".to_string());
        shell.mark_integer("X_INT_T5");
        assert!(shell.try_set("X_INT_T5", "Y_INT_T5*2".to_string()).is_ok());
        assert_eq!(shell.lookup_var("X_INT_T5").as_deref(), Some("20"));
    }

    #[test]
    fn try_set_readonly_checked_before_integer() {
        let mut shell = Shell::new();
        shell.set("X_INT_T6", "outer".to_string());
        shell.mark_readonly("X_INT_T6");
        shell.mark_integer("X_INT_T6");
        // try_set must return Err on readonly; value should NOT
        // change to "5".
        assert!(shell.try_set("X_INT_T6", "5".to_string()).is_err());
        assert_eq!(shell.lookup_var("X_INT_T6").as_deref(), Some("outer"));
    }

    // ── builtin_declare wiring ──────────────────────────────

    fn run_declare(args: &[&str], shell: &mut Shell) -> (ExecOutcome, String) {
        let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("declare", &args_owned, &mut buf, shell);
        (outcome, String::from_utf8(buf).unwrap())
    }

    #[test]
    fn declare_i_marks_and_evals() {
        let mut shell = Shell::new();
        let (oc, _) = run_declare(&["-i", "X_INT_D1=2+3"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X_INT_D1").as_deref(), Some("5"));
        assert!(shell.is_integer("X_INT_D1"));
    }

    #[test]
    fn declare_plus_i_unmarks() {
        let mut shell = Shell::new();
        run_declare(&["-i", "X_INT_D2=5"], &mut shell);
        assert!(shell.is_integer("X_INT_D2"));
        let (oc, _) = run_declare(&["+i", "X_INT_D2"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(!shell.is_integer("X_INT_D2"));
        // Value preserved.
        assert_eq!(shell.lookup_var("X_INT_D2").as_deref(), Some("5"));
    }

    #[test]
    fn declare_i_existing_var_no_reeval() {
        let mut shell = Shell::new();
        shell.set("X_INT_D3", "2+3".to_string());
        let (oc, _) = run_declare(&["-i", "X_INT_D3"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        // Value preserved; no re-eval on flag set without =.
        assert_eq!(shell.lookup_var("X_INT_D3").as_deref(), Some("2+3"));
        assert!(shell.is_integer("X_INT_D3"));
    }

    #[test]
    fn declare_i_on_readonly_errors() {
        let mut shell = Shell::new();
        shell.set("X_INT_D4", "outer".to_string());
        shell.mark_readonly("X_INT_D4");
        let (oc, _) = run_declare(&["-i", "X_INT_D4"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
        // Integer flag NOT set on a readonly var.
        assert!(!shell.is_integer("X_INT_D4"));
    }
}
```

Use unique variable names per test to avoid cross-test
pollution (the `_INT_T1`/`_INT_T2`/etc. suffixes).

- [ ] **Step 1.12**

### Step 1.13: Run new tests

```bash
cargo test --bin huck integer_attr_tests
```

Expected: 10 pass.

- [ ] **Step 1.13**

### Step 1.14: Full unit suite

`cargo test --bin huck`. Expected: green. Especially watch the
v54 readonly tests (we extended their try_set) and v52 local
tests (they use Variable struct construction).

- [ ] **Step 1.14**

### Step 1.15: Clippy

`cargo clippy --all-targets -- -D warnings`. Expected: clean.

Potential lints:
- `Variable { value, exported: false, readonly: false, integer: true }` — clippy may want field init order to match struct decl. The struct decl puts integer last, so this is fine.
- Otherwise no obvious lints expected.

- [ ] **Step 1.15**

### Step 1.16: Commit Task 1

```bash
git add src/shell_state.rs src/builtins.rs
git commit -m "$(cat <<'EOF'
declare: integer attribute -i / +i (v65 task 1)

Add the integer variable attribute, completing one row of v64's
M-79 deferred list.

Foundation (src/shell_state.rs):
- New \`pub integer: bool\` field on \`Variable\`. All existing
  literals updated to set \`integer: false\`.
- New Shell methods: \`is_integer\`, \`mark_integer\`
  (creates empty if name unset, mirrors mark_readonly),
  \`unmark_integer\`.
- \`try_set\` extended: when the target var is integer-flagged
  AND not readonly, route the RHS through \`arith::parse\` +
  \`arith::eval\` and store the decimal result. On parse/eval
  failure, silently coerce to "0" (matches bash's silent-coerce
  semantics for all integer-write paths except the loud
  \`declare -i NAME=value\` diagnostic which we document as
  divergent).

builtin_declare wiring (src/builtins.rs):
- Removed \`b'i'\` from the deferred-flag arm; new \`-i\` and
  \`+i\` cluster bytes are handled.
- Per-name mutation block: rejects integer-attribute
  transitions on readonly vars (matches bash); applies
  mark_integer/unmark_integer BEFORE the value set so try_set
  sees the flag.
- \`format_declare_line\` updated to put \`i\` first in the
  attribute order (matches bash's \`irx\` display: \`declare
  -ir X="5"\`).

Because all user-facing variable-write paths route through
try_set (per v54: top-level Assign, inline-prefix, for-loop
iter, \${var:=...}, arith assignment, read, declare), the
integer-coerce takes effect everywhere for free.

Inside-function semantics work via v52's local_scopes snapshot
(Variable is snapshotted in full, including the new integer
flag), so \`declare -i X=5\` inside a function unwinds the
integer flag along with the value on function return.

10 unit tests in \`mod integer_attr_tests\`: try_set non-integer
passes through; integer simple arith; integer negative result;
integer invalid silently zero; integer with var-ref;
readonly-checked-before-integer; declare -i marks and evals;
+i unmarks (value preserved); -i on existing var no re-eval;
-i on readonly errors.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Stage exactly: `src/shell_state.rs src/builtins.rs`.

- [ ] **Step 1.16**

---

## Task 2: Integration tests

**Files:**
- Create `tests/declare_integer_integration.rs`.

6 binary-driven tests.

### Step 2.1: Create the test file

Use the standard helper shape:

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
fn integer_assign_evaluates() {
    let (out, _, _) = run_capture("declare -i X=2+3\necho $X\nexit\n");
    assert!(out.lines().any(|l| l == "5"), "stdout: {out:?}");
}

#[test]
fn integer_reassign_evaluates() {
    let (out, _, _) = run_capture("declare -i X\nX=10*5\necho $X\nexit\n");
    assert!(out.lines().any(|l| l == "50"), "stdout: {out:?}");
}

#[test]
fn integer_garbage_becomes_zero() {
    let (out, _, _) = run_capture("declare -i X=abc\necho $X\nexit\n");
    assert!(out.lines().any(|l| l == "0"), "stdout: {out:?}");
}

#[test]
fn integer_p_format() {
    let (out, _, _) = run_capture("declare -i X=42\ndeclare -p X\nexit\n");
    assert!(
        out.lines().any(|l| l == "declare -i X=\"42\""),
        "stdout: {out:?}",
    );
}

#[test]
fn plus_i_unmarks() {
    let (out, _, _) = run_capture(
        "declare -i X=10\ndeclare +i X\nX=2+3\necho $X\nexit\n",
    );
    // After +i, X=2+3 stores literally.
    assert!(out.lines().any(|l| l == "2+3"), "stdout: {out:?}");
}

#[test]
fn integer_in_for_loop() {
    let (out, _, _) = run_capture(
        "declare -i X\nfor X in 2+3 7-1; do echo $X; done\nexit\n",
    );
    let collected: Vec<&str> = out.lines().take(2).collect();
    assert_eq!(collected, vec!["5", "6"], "stdout: {out:?}");
}
```

- [ ] **Step 2.1**

### Step 2.2: Run integration tests

```bash
cargo test --test declare_integer_integration -- --nocapture
```

Expected: 6 pass.

- [ ] **Step 2.2**

### Step 2.3: Full integration suite

`cargo test --tests`. Expected: green (PTY flake tolerated).

- [ ] **Step 2.3**

### Step 2.4: Clippy

`cargo clippy --all-targets -- -D warnings`. Expected: clean.

- [ ] **Step 2.4**

### Step 2.5: Commit Task 2

```bash
git add tests/declare_integer_integration.rs
git commit -m "$(cat <<'EOF'
test: declare -i integration coverage (v65 task 2)

6 binary-driven tests exercising the integer attribute
end-to-end through the huck binary:

- integer_assign_evaluates — declare -i X=2+3 → X=5.
- integer_reassign_evaluates — declare -i X; X=10*5 → X=50.
- integer_garbage_becomes_zero — declare -i X=abc → X=0.
- integer_p_format — declare -p X → declare -i X="42".
- plus_i_unmarks — declare +i; X=2+3 stays literal.
- integer_in_for_loop — for X in 2+3 7-1; do echo $X; done →
  5, 6 (per-iter integer-coerce through the for-loop
  try_set path).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2.5**

---

## Task 3: Docs

**Files:**
- Modify `docs/bash-divergences.md` — update M-79 to drop `-i`
  from deferred list + add v65 update; add v65 change-log
  entry.
- Modify `README.md` — v65 row.

### Step 3.1: Update M-79 entry

Find M-79's "Deferred" line. Currently lists `-i` among the
deferred attributes. Remove `-i` from that list and add an
"Updates" sentence at the end of the entry covering v65.

The exact text in M-79 (post-v64):

```markdown
... **Deferred** ("not yet implemented" + exit 1 if used): `-i` (integer coercion), `-l` (lowercase), `-u` (uppercase), `-a` (indexed array — huck has no arrays), `-A` (associative array), `-n` (nameref), `-g` (force global). Also deferred: function-body printing in `-f` output ...
```

Change to:

```markdown
... **Deferred** ("not yet implemented" + exit 1 if used): `-l` (lowercase), `-u` (uppercase), `-a` (indexed array — huck has no arrays), `-A` (associative array), `-n` (nameref), `-g` (force global). Also deferred: function-body printing in `-f` output ... **Updates**: v65 ships `-i` / `+i` (integer attribute). New `Variable.integer` flag + `Shell::mark_integer` / `unmark_integer` / `is_integer` helpers. `try_set` extended to arith-evaluate the RHS when the target is integer-flagged (silent coerce to "0" on parse/eval failure; bash's `declare -i NAME=val` loud-error diagnostic deferred — small divergence). Affects all user-facing write paths automatically (top-level Assign, inline-prefix, for-loop iter, `${var:=...}`, arith assign, `read`, `declare`). `format_declare_line` updated to put `i` first in attribute order (matches bash's `irx` display).
```

- [ ] **Step 3.1**

### Step 3.2: Add v65 change-log entry

In `## Change log` after v64:

```markdown
- **2026-06-01**: v65 finishes the `-i` row M-79 left deferred. New `Variable.integer` flag + three Shell helpers (`is_integer`, `mark_integer`, `unmark_integer`). `Shell::try_set` extended with integer-coerce: when the target var is integer-flagged AND not readonly, the RHS is parsed+evaluated via `arith::parse`/`arith::eval` and the decimal result is stored. On parse/eval failure, silently coerces to "0" (matches bash for non-declare write paths; for `declare -i NAME=val` bash emits a loud diagnostic that huck does NOT — small divergence documented in M-79). Because all user-facing write paths route through `try_set` (per v54), the integer-coerce takes effect everywhere without further wiring: top-level `X=expr`, inline-prefix `X=expr cmd`, `for X in ...; do done`, `${X:=expr}`, `$((X = expr))` (already evaluated), `read X`, `declare -i X=expr`. `builtin_declare`'s flag parser now handles `-i` (set integer + optionally set value via the integer-coerce path) and `+i` (remove integer flag; value preserved). Attribute transitions (`-i` / `+i`) on a readonly variable error with exit 1 (matches bash's readonly-blocks-integer-flag-change rule). `format_declare_line` updated to put `i` first in the displayed attribute order (`declare -ir X="5"` etc.). 10 unit tests in `mod integer_attr_tests` + 6 binary-driven integration tests. No new L-* divergences.
```

- [ ] **Step 3.2**

### Step 3.3: Add v65 row to README

After v64:

```markdown
| v65       | `declare -i` integer attribute (M-79 cont.)                    |
```

Match v64 column padding.

- [ ] **Step 3.3**

### Step 3.4: Full suite

`cargo test --all-targets`. Expected: green (PTY flake tolerated).

- [ ] **Step 3.4**

### Step 3.5: Clippy

`cargo clippy --all-targets -- -D warnings`. Expected: clean.

- [ ] **Step 3.5**

### Step 3.6: Commit Task 3

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: declare -i integer attribute shipped v65

M-79's deferred list no longer includes -i. An "Updates"
sentence at the end of M-79 covers v65's additions:
Variable.integer flag, the three new Shell helpers, the
try_set integer-coerce extension, the silent-coerce-to-0
divergence, and the irx attribute display order.

Change log: 2026-06-01 v65 entry summarizing the foundation
work, the try_set extension, the builtin_declare wiring, and
the test counts.

README: v65 row added.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3.6**

---

## Final verification (controller)

1. `cargo test --all-targets` once more.
2. `cargo clippy --all-targets -- -D warnings`.
3. Branch is four commits ahead of `main`: docs preamble + 3
   task commits.
4. Dispatch a final cross-task reviewer.
5. Merge to `main` with `--no-ff`, push, delete branch, update
   memory.
