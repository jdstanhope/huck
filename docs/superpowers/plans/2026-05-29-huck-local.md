# huck v52 — `local` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add bash's `local` builtin so functions can declare scoped
variables. Locals are restored to their pre-call state on function
exit (or unset if they were unset before).

**Architecture:** `Shell` gains a `local_scopes: Vec<HashMap<String,
Option<Variable>>>` stack. `call_function` pushes an empty frame
before the body, then pops and applies the saved state after. The
new `builtin_local` snapshots a variable's current state (once per
frame) and sets the new value. `Variable` becomes public so it can
be referenced in the field type.

**Tech Stack:** Rust. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-29-huck-local-design.md`

**Branch:** `v52-local` (created in preamble step P.1).

**Commit trailer convention**:

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble: Create the working branch

- [ ] **Step P.1: Create branch from main and check it out**

```bash
git checkout main
git pull --ff-only
git checkout -b v52-local
```

Expected: `Switched to a new branch 'v52-local'`.

The spec + this plan are committed as the first commit on this branch
(handled by the controller before Task 1 begins).

---

## Task 1: Foundation + builtin + 9 unit tests

**Files:**
- Modify: `src/shell_state.rs` — make `Variable` `pub`; add `pub
  local_scopes` field on `Shell`; init in `Shell::new`; add
  `snapshot_var` and `restore_var` methods.
- Modify: `src/executor.rs::call_function` — push/pop local_scopes
  frame around body; add 3 unit tests in the existing test module.
- Modify: `src/builtins.rs` — add `builtin_local`; add `"local"`
  to `BUILTIN_NAMES`; add dispatch arm; append `mod local_tests`
  with 6 tests.

### Step 1.1: Make `Variable` pub in `src/shell_state.rs`

In `src/shell_state.rs:8-12`, the current definition is:

```rust
#[derive(Debug, Clone)]
struct Variable {
    value: String,
    exported: bool,
}
```

Change to:

```rust
#[derive(Debug, Clone)]
pub struct Variable {
    pub value: String,
    pub exported: bool,
}
```

Fields become `pub` so the existing internal accessors (`set`,
`export`, etc.) keep working AND outside callers can inspect a
snapshotted `Variable` if they wish. The local-implementation paths
themselves treat `Variable` as opaque (snapshot + restore round-trip
via `self.vars.insert` / `remove`).

- [ ] **Step 1.1: Make Variable pub**

### Step 1.2: Add `local_scopes` field to `Shell`

In `src/shell_state.rs`, find the `pub struct Shell { ... }` block
(starts around line 18). Add the new field — natural position
alongside other call-frame state like `function_arg0` and
`positional_args`:

```rust
    /// Stack of `local`-snapshot frames. Pushed in `call_function`
    /// before the body runs; popped + restored after. Each frame
    /// maps `var_name` → the pre-`local` snapshot (None if the var
    /// was unset). Outside any function, this vec is empty —
    /// `builtin_local` checks for that.
    pub local_scopes: Vec<std::collections::HashMap<String, Option<Variable>>>,
```

Then in `impl Shell { pub fn new() -> Self { ... } }`, find the
struct literal and add the initializer:

```rust
            local_scopes: Vec::new(),
```

Position it alongside the other `Vec::new()` initializers (e.g. near
`positional_args: Vec::new()`).

- [ ] **Step 1.2: Add the field**

### Step 1.3: Add `snapshot_var` and `restore_var` methods

In `src/shell_state.rs`, find the `impl Shell { ... }` block. After
the existing `pub fn unset(&mut self, ...)` method (around line 197),
add:

```rust
    /// Returns a clone of the named variable's current state, or
    /// None if unset. Used by `local` to snapshot pre-local state.
    pub fn snapshot_var(&self, name: &str) -> Option<Variable> {
        self.vars.get(name).cloned()
    }

    /// Restores `name` to `snapshot`: Some → reinstall; None →
    /// remove. Used by `call_function` on exit to undo `local`s.
    pub fn restore_var(&mut self, name: &str, snapshot: Option<Variable>) {
        match snapshot {
            Some(v) => {
                self.vars.insert(name.to_string(), v);
            }
            None => {
                self.vars.remove(name);
            }
        }
    }
```

- [ ] **Step 1.3: Add the methods**

### Step 1.4: Build to confirm shell_state compiles

Run: `cargo build`
Expected: clean. `Variable` going pub may surface unused warnings
elsewhere — none expected since `Variable` was previously
module-private but only the methods exposed its contents.

- [ ] **Step 1.4: Build clean**

### Step 1.5: Integrate push/pop in `call_function`

In `src/executor.rs:1378-1406`, the current `call_function` reads:

```rust
fn call_function(
    name: &str,
    body: Box<crate::command::Command>,
    args: Vec<String>,
    shell: &mut Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
    let saved = std::mem::take(&mut shell.positional_args);
    shell.positional_args = args;
    shell.function_arg0.push(name.to_string());
    let result = run_command(&body, shell, sink);
    let status_for_trap = match &result {
        ExecOutcome::FunctionReturn(n) => *n,
        ExecOutcome::Continue(c) => *c,
        _ => shell.last_status(),
    };
    shell.set_last_status(status_for_trap);
    crate::traps::fire_return_trap(shell);
    shell.function_arg0.pop();
    shell.positional_args = saved;
    match result {
        ExecOutcome::FunctionReturn(n) => ExecOutcome::Continue(n),
        other => other,
    }
}
```

Replace with:

```rust
fn call_function(
    name: &str,
    body: Box<crate::command::Command>,
    args: Vec<String>,
    shell: &mut Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
    let saved = std::mem::take(&mut shell.positional_args);
    shell.positional_args = args;
    shell.function_arg0.push(name.to_string());
    shell.local_scopes.push(std::collections::HashMap::new());

    let result = run_command(&body, shell, sink);

    let status_for_trap = match &result {
        ExecOutcome::FunctionReturn(n) => *n,
        ExecOutcome::Continue(c) => *c,
        _ => shell.last_status(),
    };
    shell.set_last_status(status_for_trap);
    crate::traps::fire_return_trap(shell);

    // Pop local scope and restore each snapshotted variable.
    if let Some(frame) = shell.local_scopes.pop() {
        for (var_name, snapshot) in frame {
            shell.restore_var(&var_name, snapshot);
        }
    }

    shell.function_arg0.pop();
    shell.positional_args = saved;
    match result {
        ExecOutcome::FunctionReturn(n) => ExecOutcome::Continue(n),
        other => other,
    }
}
```

The local-scope restore happens AFTER the RETURN trap fires so the
trap action sees the function's locals (consistent with how
positional_args + function_arg0 are still in scope for the trap).

- [ ] **Step 1.5: Update call_function**

### Step 1.6: Build

Run: `cargo build`
Expected: clean.

- [ ] **Step 1.6: Build clean**

### Step 1.7: Add `"local"` to `BUILTIN_NAMES`

In `src/builtins.rs:18-23`, the current array (post-v51) reads:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return", "trap", "alias", "unalias",
    "set", "shift", ".", "source",
];
```

Replace with:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return", "trap", "alias", "unalias",
    "set", "shift", ".", "source", "local",
];
```

Note: `local` is NOT added to `is_special_builtin`. Bash classifies
it as a regular builtin (it can fail when called outside a function;
special builtins are not supposed to fail in ways that abort the
shell).

- [ ] **Step 1.7: Update BUILTIN_NAMES**

### Step 1.8: Add `builtin_local` function

In `src/builtins.rs`, find a natural insertion point — after
`builtin_source` (the v51 addition) or alongside other "scope
manipulation" builtins. Insert:

```rust
fn builtin_local(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if shell.local_scopes.is_empty() {
        eprintln!("huck: local: can only be used in a function");
        return ExecOutcome::Continue(1);
    }
    for arg in args {
        let (name, value): (&str, Option<String>) = match arg.find('=') {
            Some(eq) => (&arg[..eq], Some(arg[eq + 1..].to_string())),
            None => (arg.as_str(), None),
        };
        if !is_valid_name(name) {
            eprintln!("huck: local: `{arg}': not a valid identifier");
            return ExecOutcome::Continue(1);
        }
        // Snapshot pre-local state only if NAME is not already saved
        // in this frame. Compute the snapshot via shell.snapshot_var
        // BEFORE taking the mutable borrow on local_scopes.
        let already_saved = shell
            .local_scopes
            .last()
            .map(|f| f.contains_key(name))
            .unwrap_or(false);
        if !already_saved {
            let snap = shell.snapshot_var(name);
            shell
                .local_scopes
                .last_mut()
                .unwrap()
                .insert(name.to_string(), snap);
        }
        shell.set(name, value.unwrap_or_default());
    }
    ExecOutcome::Continue(0)
}
```

`is_valid_name` already exists in `src/builtins.rs:258` and is the
same identifier check used by `export`. Reuse it.

- [ ] **Step 1.8: Add builtin_local**

### Step 1.9: Add dispatch arm

In `src/builtins.rs`, find `run_builtin`'s match block. Add the
`"local"` arm — natural position is near `"export"` and `"unset"`
since they all manipulate vars:

```rust
        "unset" => builtin_unset(args, shell),
        "local" => builtin_local(args, shell),
```

(Or place it after `"source"`; either works.)

- [ ] **Step 1.9: Add dispatch arm**

### Step 1.10: Build

Run: `cargo build`
Expected: clean.

- [ ] **Step 1.10: Build clean**

### Step 1.11: Append `mod local_tests` with 6 unit tests

At the end of `src/builtins.rs` (after the most recent v51 `mod
source_tests`), append:

```rust
#[cfg(test)]
mod local_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn local_outside_function_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        // local_scopes is empty (we never pushed a frame).
        let outcome = run_builtin(
            "local",
            &["X=hi".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn local_with_value_sets_and_records_snapshot() {
        let mut shell = Shell::new();
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "local",
            &["XYZ_LOCAL_T1=hi".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("XYZ_LOCAL_T1").as_deref(), Some("hi"));
        // Snapshot recorded: X was unset before, so snapshot is None.
        let frame = shell.local_scopes.last().unwrap();
        assert!(frame.contains_key("XYZ_LOCAL_T1"));
        assert!(frame["XYZ_LOCAL_T1"].is_none());
    }

    #[test]
    fn local_without_value_sets_empty() {
        let mut shell = Shell::new();
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "local",
            &["XYZ_LOCAL_T2".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("XYZ_LOCAL_T2").as_deref(), Some(""));
    }

    #[test]
    fn local_snapshots_existing_var() {
        let mut shell = Shell::new();
        shell.set("XYZ_LOCAL_T3", "outer".to_string());
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "local",
            &["XYZ_LOCAL_T3=inner".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        // After `local`, the var has the inner value.
        assert_eq!(shell.lookup_var("XYZ_LOCAL_T3").as_deref(), Some("inner"));
        // The frame holds the snapshot of the outer value.
        let snapshot = shell
            .local_scopes
            .last()
            .unwrap()
            .get("XYZ_LOCAL_T3")
            .cloned()
            .unwrap();
        let v = snapshot.expect("expected Some snapshot for previously-set var");
        assert_eq!(v.value, "outer");
    }

    #[test]
    fn local_idempotent_in_same_frame() {
        let mut shell = Shell::new();
        shell.set("XYZ_LOCAL_T4", "outer".to_string());
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        // First `local`: snapshot the outer value.
        let _ = run_builtin(
            "local",
            &["XYZ_LOCAL_T4=first".to_string()],
            &mut buf,
            &mut shell,
        );
        // Second `local` for the same name in the same frame: must NOT
        // re-snapshot (otherwise it would overwrite the outer snapshot
        // with "first").
        let _ = run_builtin(
            "local",
            &["XYZ_LOCAL_T4=second".to_string()],
            &mut buf,
            &mut shell,
        );
        // Current value reflects the second assignment.
        assert_eq!(shell.lookup_var("XYZ_LOCAL_T4").as_deref(), Some("second"));
        // Snapshot still holds the original outer value.
        let snapshot = shell
            .local_scopes
            .last()
            .unwrap()
            .get("XYZ_LOCAL_T4")
            .cloned()
            .unwrap();
        let v = snapshot.expect("expected Some outer snapshot");
        assert_eq!(v.value, "outer");
    }

    #[test]
    fn local_invalid_identifier_errors() {
        let mut shell = Shell::new();
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "local",
            &["1foo=bar".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }
}
```

- [ ] **Step 1.11: Append local_tests**

### Step 1.12: Add 3 executor tests covering call_function integration

In `src/executor.rs`, find the existing `#[cfg(test)] mod tests {
... }` block. Within that mod (look near other `call_function_*` tests
around line 3966), append:

```rust
    #[test]
    fn function_with_local_does_not_leak_var() {
        let mut shell = Shell::new();
        exec_script("f() { local XYZ_LOCAL_E1=in; }\nf\n", &mut shell);
        assert!(shell.lookup_var("XYZ_LOCAL_E1").is_none());
    }

    #[test]
    fn function_local_restores_outer_var() {
        let mut shell = Shell::new();
        shell.set("XYZ_LOCAL_E2", "outer".to_string());
        exec_script("f() { local XYZ_LOCAL_E2=inner; }\nf\n", &mut shell);
        assert_eq!(shell.lookup_var("XYZ_LOCAL_E2").as_deref(), Some("outer"));
    }

    #[test]
    fn nested_function_calls_have_isolated_locals() {
        let mut shell = Shell::new();
        shell.set("XYZ_LOCAL_E3", "top".to_string());
        let script = "outer() { local XYZ_LOCAL_E3=outer_val; inner; }\n\
                      inner() { local XYZ_LOCAL_E3=inner_val; }\n\
                      outer\n";
        exec_script(script, &mut shell);
        // After both functions return, the outer `top` value is restored.
        assert_eq!(shell.lookup_var("XYZ_LOCAL_E3").as_deref(), Some("top"));
    }
```

`exec_script` is the existing test helper at `src/executor.rs:3927`
that drives the multi-line parse+execute loop.

- [ ] **Step 1.12: Append executor tests**

### Step 1.13: Run the new tests

Run: `cargo test --bin huck local_tests:: function_with_local function_local_restores nested_function_calls_have_isolated`
Expected: 9 tests pass (6 in local_tests + 3 in executor tests).

If `nested_function_calls_have_isolated_locals` fails: most likely
cause is a borrow-checker issue in `call_function`'s restore loop —
the `for (var_name, snapshot) in frame` iter takes ownership of
`frame`, and `shell.restore_var` mutably borrows `shell`. That's
fine (frame is a separate HashMap, not held inside shell anymore
after `pop()` returns it by value). Verify Task 1 Step 1.5.

- [ ] **Step 1.13: 9 tests pass**

### Step 1.14: Full unit suite

Run: `cargo test --bin huck`
Expected: all unit tests pass.

- [ ] **Step 1.14: Full unit suite passes**

### Step 1.15: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 1.15: Clippy clean**

### Step 1.16: Commit

```bash
git add src/shell_state.rs src/executor.rs src/builtins.rs
git commit -m "$(cat <<'EOF'
builtin: local (v52 task 1)

Add bash's `local` builtin so functions can declare scoped
variables that are restored on function return.

Foundation:
- src/shell_state.rs: `Variable` struct (and its `value`/`exported`
  fields) become pub so the new `local_scopes` field on Shell can
  reference it from outside callers. New
  `local_scopes: Vec<HashMap<String, Option<Variable>>>` field
  (stack of "to-restore-on-function-exit" snapshots; each frame
  maps var_name → pre-`local` state, with None meaning the var
  was unset before). Two new Shell methods: `snapshot_var(name)`
  clones the current state; `restore_var(name, snapshot)` reinstalls
  Some or removes None.

Integration:
- src/executor.rs::call_function: push an empty HashMap onto
  `local_scopes` before running the body; after the body (and
  after the RETURN trap fires, so the trap action still sees the
  function's locals), pop the frame and restore each entry via
  `restore_var`. Mirrors the existing positional_args + function_arg0
  save/restore pattern.

Builtin:
- src/builtins.rs::builtin_local: errors with "can only be used in
  a function" + status 1 when `local_scopes` is empty. For each
  arg, parses `NAME=value` (or `NAME` alone → empty value), validates
  via the existing is_valid_name, snapshots pre-local state into
  the current frame (once per name, so `local X=1; local X=2`
  preserves the outer snapshot), then `shell.set(name, value)`.
  Invalid identifier → status 1.
- `local` added to BUILTIN_NAMES and dispatched after `"unset"` in
  run_builtin. NOT added to is_special_builtin (bash classifies as
  regular).

9 new unit tests: 6 in mod local_tests (outside-function error,
with-value sets and snapshots None, without-value sets empty,
snapshots existing-var, idempotent in same frame, invalid
identifier) + 3 in executor tests (function does not leak var to
caller, function restores outer var to outer value on return,
nested function calls have isolated local snapshots).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 1.16: Commit Task 1**

---

## Task 2: Integration tests

**Files:**
- Create: `tests/local_integration.rs`

Two binary-driven tests.

### Step 2.1: Create the integration test file

Create `tests/local_integration.rs` with this exact content:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn local_scopes_to_function() {
    // X is set at top level; the function sets a local X; on return
    // the outer X must be restored.
    let script = "X=outer\n\
                  f() { local X=in; echo \"in=$X\"; }\n\
                  f\n\
                  echo \"out=$X\"\n\
                  exit\n";
    let (out, _) = run_capture(script);
    assert!(
        out.lines().any(|l| l == "in=in"),
        "expected `in=in` in: {:?}",
        out
    );
    assert!(
        out.lines().any(|l| l == "out=outer"),
        "expected `out=outer` (locals restored) in: {:?}",
        out
    );
}

#[test]
fn local_outside_function_errors() {
    // `local` at top level should error and produce a non-zero $?.
    let script = "local X=1\nrc=$?\necho rc=$rc\nexit\n";
    let (out, err) = run_capture(script);
    let rc_line = out
        .lines()
        .find(|l| l.starts_with("rc="))
        .unwrap_or_else(|| panic!("no rc= line in stdout: {:?}; stderr: {:?}", out, err));
    let rc = rc_line.strip_prefix("rc=").unwrap();
    assert_ne!(rc, "0", "expected non-zero rc, got {rc}; stderr: {:?}", err);
    assert!(
        err.contains("can only be used in a function"),
        "expected stderr to mention 'can only be used in a function': {:?}",
        err
    );
}
```

- [ ] **Step 2.1: Create the file**

### Step 2.2: Run the integration suite

Run: `cargo test --test local_integration -- --nocapture`
Expected: both tests pass.

If `local_outside_function_errors` fails because `$?` reads 0
instead of 1: that's the same `$?`-clobbering pattern from prior
iterations. The script captures `rc=$?` IMMEDIATELY after `local`,
then prints — should be safe. If it still fails, investigate the
order of `set_last_status` calls in `process_line`.

- [ ] **Step 2.2: Tests pass**

### Step 2.3: Full integration suite

Run: `cargo test --tests`
Expected: all integration tests pass. PTY flake tolerated.

- [ ] **Step 2.3: Full integration suite green**

### Step 2.4: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 2.4: Clippy clean**

### Step 2.5: Commit

```bash
git add tests/local_integration.rs
git commit -m "$(cat <<'EOF'
test: local integration coverage (v52 task 2)

Two binary-driven tests verifying `local` end-to-end through the
huck binary:
- local_scopes_to_function: X=outer; f() { local X=in; echo $X; };
  f; echo $X → stdout has both `in=in` and `out=outer` (local
  shadows during the call, restored after).
- local_outside_function_errors: top-level `local X=1` → non-zero
  rc captured immediately as $?, stderr contains "can only be used
  in a function".

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2.5: Commit Task 2**

---

## Task 3: Docs

**Files:**
- Modify: `docs/bash-divergences.md` — add new M-67 entry,
  change-log entry.
- Modify: `README.md` — v52 row.

### Step 3.1: Add M-67 entry in `docs/bash-divergences.md`

Find a Tier 2 section that fits — "Builtins (other)" or similar
where v50/v51 added M-65/M-66 entries. Add the new entry right
after M-66 (the v51 source entry):

```markdown
- **M-67: `local`** — `[fixed v52]` medium. Bash's `local` builtin for function-scoped variables. `local NAME=value` and `local NAME` (sets to empty) supported; multiple per call. On function return, each local is restored to its caller-side state (or unset if it was unset before). Outside a function → "can only be used in a function" + status 1. Idempotent within a single frame: `local X=1; local X=2` preserves the outer-caller's pre-`local` snapshot. Nested function calls have isolated local snapshots. Attribute flags (`local -p`/`-i`/`-a`/`-A`) deferred.
```

- [ ] **Step 3.1: Add M-67 entry**

### Step 3.2: Add v52 change-log entry

In `docs/bash-divergences.md`, find `## Change log` and the most
recent `**2026-05-29**` entry (v51, M-66 source). Add IMMEDIATELY
after it:

```markdown
- **2026-05-29**: M-67 (`local`) shipped as v52. New `Shell.local_scopes: Vec<HashMap<String, Option<Variable>>>` stack. `call_function` pushes an empty frame before running the body; after the body (and the RETURN trap), pops the frame and replays each saved snapshot via the new `restore_var` method. `Variable` becomes pub so it can be referenced in the field type. `builtin_local` errors if `local_scopes` is empty, else parses `NAME=value` / `NAME`, snapshots pre-state once per name via the new `snapshot_var` method, and `shell.set(name, value)`s. Added to `BUILTIN_NAMES` and `run_builtin` dispatch; NOT added to `is_special_builtin` (bash classifies as regular). No new L-* divergences.
```

- [ ] **Step 3.2: Add change-log entry**

### Step 3.3: Add v52 row to README

In `README.md`, find the version table. After the v51 row (search
for `| v51       |`), add IMMEDIATELY after it:

```markdown
| v52       | `local` (M-67)                                                 |
```

Match column padding to v50/v51 (count actual trailing spaces in
the file).

- [ ] **Step 3.3: Add README v52 row**

### Step 3.4: Full suite

Run: `cargo test --all-targets`
Expected: all tests pass (modulo PTY flake).

- [ ] **Step 3.4: Full suite green**

### Step 3.5: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 3.5: Clippy clean**

### Step 3.6: Commit

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: add M-67 (local) fixed v52

New M-67 entry in docs/bash-divergences.md tracks bash's `local`
builtin as [fixed v52]. Covers value/no-value forms, multiple per
call, restore-on-return, outside-function error, in-frame
idempotency, and nested-function isolation. Attribute flags
(-p/-i/-a/-A) deferred.

Change log: 2026-05-29 v52 entry summarizing the Shell.local_scopes
stack, the call_function push/pop+restore integration, the new
snapshot_var/restore_var Shell methods, and the Variable visibility
bump.

README: v52 row added to the version table.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3.6: Commit Task 3**

---

## Final verification (controller, not a task)

After the three task commits land:

1. Run `cargo test --all-targets` once more.
2. Run `cargo clippy --all-targets -- -D warnings`.
3. Confirm the branch has exactly four commits ahead of `main`:
   docs preamble (spec + plan), task 1, task 2, task 3.
4. Dispatch a final cross-task code-reviewer subagent over the
   full diff (`main..v52-local`).
5. Merge to `main` with `--no-ff`, push, delete the branch, update
   the `huck iterations` memory with v52.
