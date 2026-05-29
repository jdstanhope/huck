# huck v54 — `readonly` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add bash's `readonly` builtin. A var marked readonly
rejects all subsequent writes (assignment, inline-prefix, for-loop
iter, `${var:=…}`, `$((var=…))`, `export`, `local`, `unset`).

**Architecture:** Add `readonly: bool` to `Variable`. Add helpers
`is_readonly` / `try_set` / `try_unset` / `mark_readonly` /
`readonly_names` to `Shell`. Add `builtin_readonly`. Plumb the
readonly check into 8 write sites: 3 in builtins (`unset`,
`export`, `local`) and 5 in executor/expansion (top-level
`Assign`, inline-assignments, for-loop iter, `${var:=…}`, arith
assignment).

**Tech Stack:** Rust. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-29-huck-readonly-design.md`

**Branch:** `v54-readonly` (created in preamble step P.1).

**Commit trailer convention:**

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble: Create the working branch

- [ ] **Step P.1: Create branch from main**

```bash
git checkout main
git pull --ff-only
git checkout -b v54-readonly
```

The spec + this plan are committed as the first commit on this
branch by the controller before Task 1 begins.

---

## Task 1: Foundation + builtins-layer enforcement + 10 unit tests

**Files:**
- Modify `src/shell_state.rs` — add `readonly: bool` to
  `Variable`; ensure all `Variable { ... }` literals initialize
  it; add 4 new methods (`is_readonly`, `try_set`, `try_unset`,
  `mark_readonly`, `readonly_names`).
- Modify `src/builtins.rs` — add `builtin_readonly`; readonly
  checks in `builtin_unset` and `builtin_export` and
  `builtin_local`; add `"readonly"` to `BUILTIN_NAMES` and
  `is_special_builtin`; dispatch; new `mod readonly_tests` (10
  tests).

### Step 1.1: Add `readonly` field to `Variable`

`src/shell_state.rs:7-12` currently:

```rust
#[derive(Debug, Clone)]
pub struct Variable {
    pub value: String,
    pub exported: bool,
}
```

Change to:

```rust
#[derive(Debug, Clone)]
pub struct Variable {
    pub value: String,
    pub exported: bool,
    pub readonly: bool,
}
```

- [ ] **Step 1.1**

### Step 1.2: Initialize `readonly: false` everywhere `Variable {…}` is constructed

Search the codebase for `Variable {` and update each literal:

```bash
grep -rn "Variable {" /home/john/projects/shuck/src/
```

For each occurrence, add `readonly: false,` alongside `exported: …`.
Don't change semantics — every existing construction defaults
the new field to `false`. The set/unset internals don't need to
change beyond this.

- [ ] **Step 1.2**

### Step 1.3: Build

`cargo build`. Expected: clean. If any place constructs `Variable
{…}` without `..Default::default()` and you missed it, the
compiler tells you exactly where.

- [ ] **Step 1.3**

### Step 1.4: Add `is_readonly` / `try_set` / `try_unset` / `mark_readonly` / `readonly_names` to `Shell`

In `src/shell_state.rs`, in `impl Shell { ... }`, after the
existing `set` / `unset` / `snapshot_var` / `restore_var` methods
(roughly around line 215-230, after the v52 helpers), add:

```rust
    pub fn is_readonly(&self, name: &str) -> bool {
        self.vars.get(name).map(|v| v.readonly).unwrap_or(false)
    }

    pub fn try_set(&mut self, name: &str, value: String) -> Result<(), ()> {
        if self.is_readonly(name) { return Err(()); }
        self.set(name, value);
        Ok(())
    }

    pub fn try_unset(&mut self, name: &str) -> Result<(), ()> {
        if self.is_readonly(name) { return Err(()); }
        self.unset(name);
        Ok(())
    }

    pub fn mark_readonly(&mut self, name: &str) {
        if let Some(v) = self.vars.get_mut(name) {
            v.readonly = true;
        } else {
            self.vars.insert(name.to_string(), Variable {
                value: String::new(),
                exported: false,
                readonly: true,
            });
        }
    }

    pub fn readonly_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .vars
            .iter()
            .filter(|(_, v)| v.readonly)
            .map(|(k, _)| k.clone())
            .collect();
        names.sort();
        names
    }
```

- [ ] **Step 1.4**

### Step 1.5: Build

`cargo build`. Expected: clean.

- [ ] **Step 1.5**

### Step 1.6: Add `"readonly"` to `BUILTIN_NAMES`

In `src/builtins.rs:18-24`, current value (post-v53):

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return", "trap", "alias", "unalias",
    "set", "shift", ".", "source", "local",
    ":", "true", "false", "command",
];
```

Append `"readonly"`:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return", "trap", "alias", "unalias",
    "set", "shift", ".", "source", "local",
    ":", "true", "false", "command",
    "readonly",
];
```

- [ ] **Step 1.6**

### Step 1.7: Add `"readonly"` to `is_special_builtin`

Current matches at `src/builtins.rs:35-38`:

```rust
matches!(name,
    ":" | "." | "break" | "continue" | "exit" | "export" | "return"
    | "set" | "shift" | "source" | "trap" | "unset"
)
```

Add `"readonly"`:

```rust
matches!(name,
    ":" | "." | "break" | "continue" | "exit" | "export" | "readonly" | "return"
    | "set" | "shift" | "source" | "trap" | "unset"
)
```

(Position keeps roughly alphabetical, but exact order doesn't
matter for `matches!`.)

- [ ] **Step 1.7**

### Step 1.8: Add `builtin_readonly`

Insert near `builtin_export` and `builtin_unset` (natural
neighbors). Full code:

```rust
fn builtin_readonly(
    args: &[String],
    out: &mut dyn std::io::Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let mut want_list = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-p" => { want_list = true; i += 1; }
            "--" => { i += 1; break; }
            s if s.starts_with('-') && s.len() > 1 => {
                eprintln!("huck: readonly: {s}: invalid option");
                return ExecOutcome::Continue(2);
            }
            _ => break,
        }
    }
    let names = &args[i..];

    if names.is_empty() || want_list {
        for name in shell.readonly_names() {
            let value = shell.lookup_var(&name).unwrap_or_default();
            let _ = writeln!(out, "readonly {name}='{}'",
                escape_alias_value(&value));
        }
        return ExecOutcome::Continue(0);
    }

    let mut exit = 0;
    for arg in names {
        let (name, value): (&str, Option<String>) = match arg.find('=') {
            Some(eq) => (&arg[..eq], Some(arg[eq + 1..].to_string())),
            None => (arg.as_str(), None),
        };
        if !is_valid_name(name) {
            eprintln!("huck: readonly: `{arg}': not a valid identifier");
            exit = 1;
            continue;
        }
        if let Some(v) = value {
            // Refuse to overwrite an already-readonly variable.
            if shell.is_readonly(name) {
                eprintln!("huck: readonly: {name}: readonly variable");
                exit = 1;
                continue;
            }
            shell.set(name, v);
        }
        shell.mark_readonly(name);
    }
    ExecOutcome::Continue(exit)
}
```

`escape_alias_value` is defined in the same file (used by
`builtin_alias` and v53's `builtin_command`).

- [ ] **Step 1.8**

### Step 1.9: Add dispatch arm

In `run_builtin`'s match block, add:

```rust
"readonly" => builtin_readonly(args, out, shell),
```

Position near `"export"`/`"unset"`. The output writer parameter is
named `out` (verify against the actual signature).

- [ ] **Step 1.9**

### Step 1.10: Add readonly check to `builtin_unset`

Current `builtin_unset` (around `src/builtins.rs:318`): loops
`shell.unset(arg)` for each arg. Wrap each call:

```rust
let mut exit: i32 = 0;
for arg in args {
    if shell.is_readonly(arg) {
        eprintln!("huck: unset: {arg}: readonly variable");
        exit = 1;
        continue;
    }
    shell.unset(arg);
}
ExecOutcome::Continue(exit)
```

(Verify the function's existing signature and return type; adapt
the exit-status variable name. The original may have returned a
fixed `Continue(0)` — replace with the running `exit`.)

- [ ] **Step 1.10**

### Step 1.11: Add readonly check to `builtin_export`

In `builtin_export`, for each arg that has a `=` (NAME=value
form): check `shell.is_readonly(name)` BEFORE calling
`shell.export_set(name, value)`. If violated: print "huck: export:
NAME: readonly variable", set `exit = 1`, continue.

For bare `NAME` (no `=`): bash allows flipping the export flag on
a readonly var without changing the value. Don't add a check on
this path.

(Read the current `builtin_export` body to find the exact loop
structure — adapt accordingly.)

- [ ] **Step 1.11**

### Step 1.12: Add readonly check to `builtin_local`

In `builtin_local` (the v52 addition), for each arg-iteration,
BEFORE `shell.set(name, value.unwrap_or_default())`, check
`shell.is_readonly(name)`. If violated: print "huck: local: NAME:
readonly variable", set `exit = 1`, continue. Do NOT snapshot in
this case.

The current code already does an idempotency check
(`already_saved`). The readonly check fits before the snapshot:

```rust
if shell.is_readonly(name) {
    eprintln!("huck: local: {name}: readonly variable");
    exit = 1;     // introduce a running exit var
    continue;
}
```

You'll need to convert `builtin_local` from its current always-
returns-`Continue(0)` form (if it is) into a running-exit form so
the readonly error propagates. Verify the existing return shape.

- [ ] **Step 1.12**

### Step 1.13: Build

`cargo build`. Expected: clean.

- [ ] **Step 1.13**

### Step 1.14: Append `mod readonly_tests` with 10 unit tests

At the end of `src/builtins.rs` (after the v53 `mod
command_tests`), append:

```rust
#[cfg(test)]
mod readonly_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn readonly_with_value_sets_and_locks() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["X=hi".to_string()];
        let outcome = run_builtin("readonly", &args, &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X").as_deref(), Some("hi"));
        assert!(shell.is_readonly("X"));
    }

    #[test]
    fn readonly_no_value_creates_empty_and_locks() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["X".to_string()];
        let outcome = run_builtin("readonly", &args, &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X").as_deref(), Some(""));
        assert!(shell.is_readonly("X"));
    }

    #[test]
    fn readonly_no_value_keeps_existing_value() {
        let mut shell = Shell::new();
        shell.set("X", "prev".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["X".to_string()];
        let outcome = run_builtin("readonly", &args, &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X").as_deref(), Some("prev"));
        assert!(shell.is_readonly("X"));
    }

    #[test]
    fn readonly_multi_arg_mixed_forms() {
        let mut shell = Shell::new();
        shell.set("B", "had".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["A=1".to_string(), "B".to_string(), "C=3".to_string()];
        let outcome = run_builtin("readonly", &args, &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("A").as_deref(), Some("1"));
        assert_eq!(shell.lookup_var("B").as_deref(), Some("had"));
        assert_eq!(shell.lookup_var("C").as_deref(), Some("3"));
        assert!(shell.is_readonly("A"));
        assert!(shell.is_readonly("B"));
        assert!(shell.is_readonly("C"));
    }

    #[test]
    fn readonly_invalid_identifier_errors() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["1foo=bar".to_string()];
        let outcome = run_builtin("readonly", &args, &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        assert!(shell.lookup_var("1foo").is_none());
    }

    #[test]
    fn readonly_listing_no_args() {
        let mut shell = Shell::new();
        shell.set("X", "v".to_string());
        shell.mark_readonly("X");
        shell.set("Y", "w".to_string());
        shell.mark_readonly("Y");
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("readonly", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        // Sorted; POSIX-escape format.
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines.contains(&"readonly X='v'"));
        assert!(lines.contains(&"readonly Y='w'"));
    }

    #[test]
    fn readonly_dash_p_same_as_no_args() {
        let mut shell = Shell::new();
        shell.set("X", "v".to_string());
        shell.mark_readonly("X");
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("readonly", &["-p".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.lines().any(|l| l == "readonly X='v'"));
    }

    #[test]
    fn readonly_overwrite_existing_readonly_errors() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        run_builtin("readonly", &["X=first".to_string()], &mut buf, &mut shell);
        let outcome = run_builtin(
            "readonly",
            &["X=second".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        assert_eq!(shell.lookup_var("X").as_deref(), Some("first"));
        assert!(shell.is_readonly("X"));
    }

    #[test]
    fn unset_readonly_errors_status_1() {
        let mut shell = Shell::new();
        shell.set("X", "v".to_string());
        shell.mark_readonly("X");
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("unset", &["X".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        assert_eq!(shell.lookup_var("X").as_deref(), Some("v"));
    }

    #[test]
    fn export_readonly_value_errors_but_bare_export_succeeds() {
        let mut shell = Shell::new();
        shell.set("X", "v".to_string());
        shell.mark_readonly("X");
        let mut buf: Vec<u8> = Vec::new();
        // `export X=newval` should error and not overwrite.
        let bad = run_builtin(
            "export",
            &["X=newval".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(bad, ExecOutcome::Continue(1)));
        assert_eq!(shell.lookup_var("X").as_deref(), Some("v"));
        // `export X` (bare) should succeed and flip the export flag.
        let bare = run_builtin("export", &["X".to_string()], &mut buf, &mut shell);
        assert!(matches!(bare, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X").as_deref(), Some("v"));
        assert!(shell.is_readonly("X"));
    }
}
```

- [ ] **Step 1.14**

### Step 1.15: Run new tests

```bash
cargo test --bin huck readonly_tests
```

Expected: 10 pass.

- [ ] **Step 1.15**

### Step 1.16: Full unit suite

`cargo test --bin huck`. Expected: green.

- [ ] **Step 1.16**

### Step 1.17: Clippy

`cargo clippy --all-targets -- -D warnings`. Expected: zero
warnings.

- [ ] **Step 1.17**

### Step 1.18: Commit Task 1

```bash
git add src/shell_state.rs src/builtins.rs
git commit -m "$(cat <<'EOF'
builtin: readonly + builtins-layer enforcement (v54 task 1)

Foundation:
- src/shell_state.rs: `Variable` gains a `pub readonly: bool`
  field; the existing field literals all default it to false. New
  Shell methods: `is_readonly` (query), `try_set` /  `try_unset`
  (checked writes returning Err(()) on violation; caller prints
  diagnostic), `mark_readonly` (sets the flag; creates an empty
  Variable if the name is unset, matching bash), `readonly_names`
  (sorted list).

Builtin:
- src/builtins.rs::builtin_readonly: bare and `-p` list all
  readonly vars in POSIX `readonly NAME='escaped'` format
  (reuses escape_alias_value). Per arg, `NAME=value` sets +
  marks readonly; `NAME` alone marks readonly (creating empty
  if unset, preserving existing value otherwise). Refuses to
  overwrite an already-readonly variable. Invalid identifiers
  → status 1 (other args still processed). `-X` unknown flag →
  status 2. `readonly` added to `BUILTIN_NAMES` and to
  `is_special_builtin` (POSIX classification); dispatched after
  `"unset"` in `run_builtin`.

Builtin-layer enforcement (3 sites):
- builtin_unset: per-arg readonly check; "huck: unset: NAME:
  readonly variable" + status 1 on violation; continues to next
  arg.
- builtin_export: `export NAME=value` checks readonly before
  the write; `export NAME` (bare) is exempt (bash allows it to
  flip the export flag without changing value).
- builtin_local: `local NAME=value` / `local NAME` check
  readonly before snapshotting + setting; "huck: local: NAME:
  readonly variable" + status 1 on violation.

10 unit tests in `mod readonly_tests`: set+lock with value,
no-value creates empty, no-value keeps existing, multi-arg
mixed forms, invalid identifier, listing format, -p alias,
overwrite-already-readonly errors, unset-readonly errors,
export-readonly errors but bare export succeeds.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Stage exactly: `src/shell_state.rs src/builtins.rs`.

- [ ] **Step 1.18**

---

## Task 2: Executor-layer enforcement + 6 executor tests + 6 integration tests

**Files:**
- Modify `src/executor.rs` — readonly check in for-loop iter,
  top-level `Assign`, `apply_inline_assignments` (signature
  change), callers of `apply_inline_assignments`. Add 6 new
  executor unit tests.
- Modify `src/param_expansion.rs` — readonly check in the
  `AssignDefault` modifier (`${var:=…}` / `${var=…}`).
- Modify `src/arith.rs` — make `write_var_i64` (line 591) return
  `Result<(), ArithError>`; add `ReadonlyVar(String)` variant to
  `ArithError`; update Display + all callers.
- Create `tests/readonly_integration.rs` with 6 tests.

### Step 2.1: Add `ReadonlyVar` to `ArithError`

In `src/arith.rs:354`, current enum:

```rust
pub enum ArithError {
    Parse(String),
    DivisionByZero,
    ModuloByZero,
    NotAnInteger { var: String, value: String },
    NegativeExponent,
    ShiftCountOutOfRange { count: i64 },
}
```

Add a variant:

```rust
pub enum ArithError {
    Parse(String),
    DivisionByZero,
    ModuloByZero,
    NotAnInteger { var: String, value: String },
    NegativeExponent,
    ShiftCountOutOfRange { count: i64 },
    ReadonlyVar(String),
}
```

Add to the Display impl:

```rust
Self::ReadonlyVar(name) => write!(f, "{name}: readonly variable"),
```

- [ ] **Step 2.1**

### Step 2.2: Make `write_var_i64` checked

`src/arith.rs:590-593`:

```rust
fn write_var_i64(shell: &mut Shell, name: &str, value: i64) {
    shell.set(name, value.to_string());
}
```

Change to:

```rust
fn write_var_i64(shell: &mut Shell, name: &str, value: i64) -> Result<(), ArithError> {
    shell.try_set(name, value.to_string())
        .map_err(|_| ArithError::ReadonlyVar(name.to_string()))
}
```

Update every caller — propagate with `?` inside functions returning
`Result<i64, ArithError>` (Assign and compound-assign paths). The
callers' surrounding function signatures already return that type
(they're in the arithmetic evaluator). Compiler errors will list
each site to update.

- [ ] **Step 2.2**

### Step 2.3: Build

`cargo build`. Expected: clean.

- [ ] **Step 2.3**

### Step 2.4: Add readonly check in `param_expansion.rs::AssignDefault`

`src/param_expansion.rs:42-49`:

```rust
ParamModifier::AssignDefault { word, colon } => {
    let raw = shell.get(name).map(|s| s.to_string());
    if condition_is_null(raw.as_deref(), *colon) {
        let v = expand_word_to_string(word, shell);
        shell.set(name, v.clone());
        ExpansionResult::Value(v)
    } else {
        ExpansionResult::Value(raw.unwrap_or_default())
    }
}
```

Change the inner `shell.set` to a try_set + emit fatal on err:

```rust
ParamModifier::AssignDefault { word, colon } => {
    let raw = shell.get(name).map(|s| s.to_string());
    if condition_is_null(raw.as_deref(), *colon) {
        let v = expand_word_to_string(word, shell);
        if shell.try_set(name, v.clone()).is_err() {
            eprintln!("huck: {name}: readonly variable");
            return ExpansionResult::Fatal { status: 1 };
        }
        ExpansionResult::Value(v)
    } else {
        ExpansionResult::Value(raw.unwrap_or_default())
    }
}
```

- [ ] **Step 2.4**

### Step 2.5: Add readonly check in for-loop iter (`executor.rs:291`)

Current shape around lines 289-292:

```rust
shell.set(&clause.var, value);
```

Change to:

```rust
if shell.try_set(&clause.var, value).is_err() {
    eprintln!("huck: {}: readonly variable", clause.var);
    return ExecOutcome::Continue(1);
}
```

- [ ] **Step 2.5**

### Step 2.6: Add readonly check in top-level `Assign` (`executor.rs:1358`)

Current shape:

```rust
SimpleCommand::Assign(items) => {
    for (name, value) in items {
        let v = expand_assignment(value, shell);
        shell.set(name, v);
    }
    ExecOutcome::Continue(0)
}
```

Change to:

```rust
SimpleCommand::Assign(items) => {
    for (name, value) in items {
        let v = expand_assignment(value, shell);
        if shell.try_set(name, v).is_err() {
            eprintln!("huck: {name}: readonly variable");
            return ExecOutcome::Continue(1);
        }
    }
    ExecOutcome::Continue(0)
}
```

- [ ] **Step 2.6**

### Step 2.7: Change `apply_inline_assignments` to return `Result<Snapshot, Snapshot>`

`src/executor.rs:2518-2532`:

```rust
fn apply_inline_assignments(
    assignments: &[(String, crate::lexer::Word)],
    shell: &mut Shell,
) -> AssignmentSnapshot {
    let mut snap: AssignmentSnapshot = Vec::with_capacity(assignments.len());
    for (name, rhs) in assignments {
        let prior_value = shell.get(name).map(str::to_string);
        let prior_exported = shell.is_exported(name);
        let value = expand_assignment(rhs, shell);
        shell.export_set(name, value);
        snap.push((name.clone(), prior_value, prior_exported));
    }
    snap
}
```

Change to:

```rust
fn apply_inline_assignments(
    assignments: &[(String, crate::lexer::Word)],
    shell: &mut Shell,
) -> Result<AssignmentSnapshot, AssignmentSnapshot> {
    let mut snap: AssignmentSnapshot = Vec::with_capacity(assignments.len());
    for (name, rhs) in assignments {
        let prior_value = shell.get(name).map(str::to_string);
        let prior_exported = shell.is_exported(name);
        if shell.is_readonly(name) {
            eprintln!("huck: {name}: readonly variable");
            return Err(snap);
        }
        let value = expand_assignment(rhs, shell);
        shell.export_set(name, value);
        snap.push((name.clone(), prior_value, prior_exported));
    }
    Ok(snap)
}
```

Now every caller must handle the Err arm.

#### Update `run_double_bracket` caller (`executor.rs:422`):

```rust
let snap = match apply_inline_assignments(inline_assignments, shell) {
    Ok(s) => s,
    Err(s) => {
        restore_inline_assignments(s, shell);
        return ExecOutcome::Continue(1);
    }
};
```

#### Update the main pipeline-stage caller (`executor.rs:691`):

```rust
let snap = match apply_inline_assignments(inline_assignments, shell) {
    Ok(s) => s,
    Err(s) => {
        restore_inline_assignments(s, shell);
        return ExecOutcome::Continue(1);
    }
};
```

(Verify each caller carefully — there may also be cleanup code
that runs on the error path. Mirror the existing error-path
cleanup in each location. Search for all occurrences:

```bash
grep -n "apply_inline_assignments" src/executor.rs
```

Update every one.)

- [ ] **Step 2.7**

### Step 2.8: Build

`cargo build`. Expected: clean. May surface several call sites
the compiler points to.

- [ ] **Step 2.8**

### Step 2.9: Append 6 executor unit tests

At the end of `src/executor.rs::#[cfg(test)] mod tests { … }`,
append:

```rust
    #[test]
    fn top_level_assign_to_readonly_errors() {
        let mut shell = Shell::new();
        shell.set("X", "outer".to_string());
        shell.mark_readonly("X");
        exec_script("X=new\n", &mut shell);
        assert_eq!(shell.lookup_var("X").as_deref(), Some("outer"));
        assert_eq!(shell.last_status(), 1);
    }

    #[test]
    fn inline_assignment_to_readonly_aborts_command() {
        let mut shell = Shell::new();
        shell.set("X", "outer".to_string());
        shell.mark_readonly("X");
        // Inline `X=new echo hi` — bash aborts the command. Use a
        // builtin (echo) to keep the assertion deterministic.
        exec_script("X=new echo hi\n", &mut shell);
        // X is still its original value (not changed by the failed
        // inline). The echo should NOT have run. Status is 1.
        assert_eq!(shell.lookup_var("X").as_deref(), Some("outer"));
        assert_eq!(shell.last_status(), 1);
    }

    #[test]
    fn for_loop_iter_var_readonly_aborts_at_first_iter() {
        let mut shell = Shell::new();
        shell.set("X", "outer".to_string());
        shell.mark_readonly("X");
        exec_script(
            "for X in a b c; do echo got=$X; done\n",
            &mut shell,
        );
        // X unchanged; status 1; body should not have executed.
        assert_eq!(shell.lookup_var("X").as_deref(), Some("outer"));
        assert_eq!(shell.last_status(), 1);
    }

    #[test]
    fn param_expansion_default_assign_to_readonly_errors() {
        let mut shell = Shell::new();
        shell.set("X", "".to_string());
        shell.mark_readonly("X");
        // `: ${X:=hello}` — colon command + AssignDefault that
        // tries to write hello to readonly X.
        exec_script(": ${X:=hello}\n", &mut shell);
        assert_eq!(shell.lookup_var("X").as_deref(), Some(""));
        assert_eq!(shell.last_status(), 1);
    }

    #[test]
    fn arith_assign_to_readonly_errors() {
        let mut shell = Shell::new();
        shell.set("X", "0".to_string());
        shell.mark_readonly("X");
        exec_script("echo $((X=5))\n", &mut shell);
        assert_eq!(shell.lookup_var("X").as_deref(), Some("0"));
        assert!(shell.last_status() != 0);
    }

    #[test]
    fn local_readonly_in_function_errors() {
        let mut shell = Shell::new();
        shell.set("X", "outer".to_string());
        shell.mark_readonly("X");
        exec_script(
            "f() { local X=inner; }\nf\n",
            &mut shell,
        );
        // local should have errored; X unchanged.
        assert_eq!(shell.lookup_var("X").as_deref(), Some("outer"));
        assert_eq!(shell.last_status(), 1);
    }
```

The `exec_script` helper already exists.

- [ ] **Step 2.9**

### Step 2.10: Run new executor tests

```bash
cargo test --bin huck top_level_assign_to_readonly_errors \
  inline_assignment_to_readonly_aborts_command \
  for_loop_iter_var_readonly_aborts_at_first_iter \
  param_expansion_default_assign_to_readonly_errors \
  arith_assign_to_readonly_errors \
  local_readonly_in_function_errors
```

Expected: 6 pass.

- [ ] **Step 2.10**

### Step 2.11: Create `tests/readonly_integration.rs`

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
fn readonly_basic_blocks_reassignment() {
    let (out, err) = run_capture(
        "readonly X=1\nX=2\nrc=$?\necho rc=$rc\necho X=$X\nexit\n",
    );
    assert!(
        err.contains("readonly"),
        "expected stderr to mention readonly, got: {err:?}",
    );
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
    assert!(out.lines().any(|l| l == "X=1"), "stdout: {out:?}");
}

#[test]
fn readonly_lists_in_posix_format() {
    let (out, _) = run_capture("readonly X='a b'\nreadonly\nexit\n");
    assert!(
        out.lines().any(|l| l == "readonly X='a b'"),
        "stdout: {out:?}",
    );
}

#[test]
fn readonly_blocks_unset() {
    let (out, err) = run_capture(
        "readonly X=1\nunset X\nrc=$?\necho rc=$rc\nexit\n",
    );
    assert!(err.contains("readonly"), "stderr: {err:?}");
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
}

#[test]
fn readonly_blocks_inline_assignment() {
    let (out, err) = run_capture(
        "readonly X=1\nX=2 echo hi\nrc=$?\necho rc=$rc\nexit\n",
    );
    assert!(err.contains("readonly"), "stderr: {err:?}");
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
    assert!(
        !out.lines().any(|l| l == "hi"),
        "echo should not have run; stdout: {out:?}",
    );
}

#[test]
fn readonly_blocks_for_loop() {
    let (out, err) = run_capture(
        "readonly X=outer\nfor X in a b c; do echo got=$X; done\nrc=$?\necho rc=$rc\nexit\n",
    );
    assert!(err.contains("readonly"), "stderr: {err:?}");
    assert!(
        !out.lines().any(|l| l.starts_with("got=")),
        "loop body should not run; stdout: {out:?}",
    );
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
}

#[test]
fn readonly_with_single_quote_listing_escapes() {
    let (out, _) = run_capture(
        "readonly X=\"a'b\"\nreadonly\nexit\n",
    );
    assert!(
        out.lines().any(|l| l == r"readonly X='a'\''b'"),
        "stdout: {out:?}",
    );
}
```

- [ ] **Step 2.11**

### Step 2.12: Run integration suite

```bash
cargo test --test readonly_integration -- --nocapture
cargo test --tests
```

Expected: 6 new + full integration suite green (PTY flake
tolerated).

- [ ] **Step 2.12**

### Step 2.13: Clippy

`cargo clippy --all-targets -- -D warnings`. Expected: clean.

- [ ] **Step 2.13**

### Step 2.14: Commit Task 2

```bash
git add src/executor.rs src/param_expansion.rs src/arith.rs tests/readonly_integration.rs
git commit -m "$(cat <<'EOF'
exec: readonly enforcement in 5 write paths (v54 task 2)

Executor-layer plumbing for `readonly` (M-71):

- src/executor.rs::call_function for-loop branch: try_set on each
  iter; on Err print "huck: NAME: readonly variable" and return
  Continue(1) (aborts loop, body NOT run on the failing iter).
- src/executor.rs SimpleCommand::Assign branch (top-level
  NAME=value): try_set; on Err print + Continue(1).
- src/executor.rs::apply_inline_assignments: signature changes to
  Result<AssignmentSnapshot, AssignmentSnapshot>. Pre-checks
  is_readonly before each export_set; on violation, returns
  Err(partial-snapshot) so callers can restore_inline_assignments
  and return Continue(1) without running the command. All callers
  updated (run_double_bracket; main pipeline-stage path).
- src/param_expansion.rs ParamModifier::AssignDefault arm: try_set
  the computed value; on Err print + return Fatal{status: 1}
  through the existing expansion-error pathway.
- src/arith.rs: new ArithError::ReadonlyVar(String) variant with
  Display "{name}: readonly variable". write_var_i64 now returns
  Result<(), ArithError> via try_set; arithmetic Assign/compound
  callers propagate with `?`.

6 new executor unit tests covering: top-level Assign,
inline-assignment-aborts-command, for-loop-aborts-at-first-iter,
${var:=…} default-assign error, arith assignment error, local
readonly in function.

6 new binary-driven integration tests in
tests/readonly_integration.rs: basic blocks-reassignment, listing
in POSIX format, blocks-unset, blocks-inline-assignment,
blocks-for-loop, single-quote escaping in listing.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Stage exactly: `src/executor.rs src/param_expansion.rs
src/arith.rs tests/readonly_integration.rs`.

- [ ] **Step 2.14**

---

## Task 3: Docs

**Files:**
- Modify `docs/bash-divergences.md` — add M-71 entry + change-log.
- Modify `README.md` — add v54 row.

### Step 3.1: Add M-71 entry

In `docs/bash-divergences.md`, find the M-70 entry (v53 `command`).
Insert IMMEDIATELY after it:

```markdown
- **M-71: `readonly`** — `[fixed v54]` medium. POSIX special builtin. `readonly NAME=value` sets and locks; `readonly NAME` locks an existing var (or creates empty + locks if unset); multiple per call. `readonly` and `readonly -p` list all readonly vars in POSIX `readonly NAME='value'` format (with single-quote escaping via the existing `escape_alias_value` helper). Eight write paths enforce the readonly flag: top-level `NAME=value`, inline `NAME=v cmd` (aborts the command via the existing snapshot+restore cycle), for-loop iter var, `${var:=...}` default-assignment, `$((var=...))` arithmetic, `export NAME=value` (bare `export NAME` is exempt — matches bash), `local NAME[=value]` in a function, and `unset NAME`. Each violation prints "huck: <context>: NAME: readonly variable" and returns status 1; for-loop and inline assignment additionally abort the body/command. Attribute extensions deferred: `readonly -f` (function readonly), `readonly -a`/`-A` (huck has no arrays). **Known limitation**: internal-mechanism writes (cd updating PWD/OLDPWD, signal-state) bypass the check by design — `readonly PWD; cd /tmp` succeeds in huck where bash would error. Acceptable for now.
```

- [ ] **Step 3.1**

### Step 3.2: Add v54 change-log entry

In `## Change log` after the v53 entry:

```markdown
- **2026-05-29**: M-71 (`readonly`) shipped as v54. `Variable` gains a `pub readonly: bool` field; all existing literals defaulted to false. New Shell methods: `is_readonly`, `try_set`, `try_unset`, `mark_readonly`, `readonly_names`. `builtin_readonly` (POSIX special; added to `is_special_builtin` and `BUILTIN_NAMES`) parses `-p` and `--`; with no names lists; with names sets value (if `=`) and marks readonly; invalid identifiers → status 1; overwriting already-readonly → status 1. Enforcement plumbed into 8 write paths: `builtin_unset`, `builtin_export` (value form only — bare `export NAME` exempt), `builtin_local` (both forms), top-level `SimpleCommand::Assign`, `apply_inline_assignments` (signature now `Result<Snapshot, Snapshot>`; callers updated), for-loop iter, `${var:=…}` (`ExpansionResult::Fatal { status: 1 }`), and arithmetic assignment (new `ArithError::ReadonlyVar` variant; `write_var_i64` returns `Result<(), ArithError>`). 16 unit tests + 6 integration tests. No new L-* divergences.
```

- [ ] **Step 3.2**

### Step 3.3: Add v54 row to README

After the v53 row:

```markdown
| v54       | `readonly` (M-71)                                              |
```

Match v53's column padding exactly.

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
docs: add M-71 (readonly) fixed v54

New M-71 entry in docs/bash-divergences.md tracks bash's
`readonly` builtin as [fixed v54]. Covers value/no-value forms,
multi-arg, listing, the 8 enforcement sites (3 builtin + 5
executor/expansion), the `export NAME` bare-form exemption, and
the known limitation that internal-mechanism writes (cd's
PWD/OLDPWD, signal-state) bypass the check.

Change log: 2026-05-29 v54 entry summarizing the Variable.readonly
field, the new Shell helpers, the builtin, and each enforcement
site.

README: v54 row added to the version table.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3.6**

---

## Final verification (controller)

1. `cargo test --all-targets` once more.
2. `cargo clippy --all-targets -- -D warnings`.
3. Branch is four commits ahead of `main`: docs preamble, task 1,
   task 2, task 3.
4. Dispatch a final cross-task code-reviewer subagent over
   `main..v54-readonly`.
5. Merge to `main` with `--no-ff`, push, delete the branch, update
   the `huck iterations` memory.
