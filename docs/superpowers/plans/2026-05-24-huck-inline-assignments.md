# v23: Inline Assignments — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Parse and execute leading `NAME=value` words on a simple command (`A=1 B=2 cmd args`) per POSIX semantics — externals and regular builtins restore prior var state after the command; special builtins, function calls, and command-less assignments persist.

**Architecture:** A leading run of assignment-shaped words is peeled off in `command.rs::finalize_stage` into `ExecCommand::inline_assignments`. Two new executor helpers (`apply_inline_assignments`, `restore_inline_assignments`) snapshot prior `(value, exported)` state per name, apply each assignment left-to-right via `shell.export_set`, and unwind for "temporary" targets. `run_exec_single` and `run_multi_stage` (both builtin and external stage paths) call them around their dispatch points. Two new tiny helpers — `Shell::is_exported` and `builtins::is_special_builtin` — round out the API surface.

**Tech Stack:** Rust 1.95, existing huck modules (`src/command.rs`, `src/executor.rs`, `src/builtins.rs`, `src/shell_state.rs`); existing `expand_assignment` (already has B-07 `$?` snapshot); existing test infrastructure (unit tests in-module, integration tests under `tests/*_integration.rs`).

**Spec:** `docs/superpowers/specs/2026-05-24-huck-inline-assignments-design.md`.

---

## File structure

- `src/command.rs` — `SimpleCommand::Assign` variant payload changes to `Vec<(String, Word)>`; `ExecCommand` grows `inline_assignments`; `finalize_stage` peels leading assignments.
- `src/shell_state.rs` — add `pub fn is_exported(&self, name: &str) -> bool`.
- `src/builtins.rs` — add `pub fn is_special_builtin(name: &str) -> bool`.
- `src/executor.rs` — add `apply_inline_assignments` + `restore_inline_assignments`; wire into `run_exec_single` and `run_multi_stage`'s builtin/external paths; update `Stage::Done`/Assign-stage handling to iterate the Vec.
- `tests/inline_assignments_integration.rs` (new) — end-to-end coverage.
- `docs/bash-divergences.md` — M-04 → `fixed`.
- `README.md` — add inline assignments under v23 / supported syntax (if a status table exists).

---

## Task 1: AST refactor — `Assign(Vec<…>)` + `inline_assignments` field (no behavior change)

Migrate the AST shape without altering parser or executor behavior. After this task, the parser still produces single-element `Vec`s for the old single-assignment case, and `inline_assignments` is always empty.

**Files:**
- Modify: `src/command.rs` (struct/enum defs + every `SimpleCommand::Assign { … }` match arm)
- Modify: `src/executor.rs` (every `SimpleCommand::Assign { … }` match arm)
- Modify: `src/command.rs` test helpers (the `assignment(...)` helper at line 1104, plus test bodies that destructure `Assign { name, value }`)

- [ ] **Step 1: Snapshot the baseline (all green)**

```bash
cargo test 2>&1 | tail -3
```
Expected: every result `0 failed`; total around 909.

- [ ] **Step 2: Change the AST**

In `src/command.rs`, replace:
```rust
pub enum SimpleCommand {
    Assign { name: String, value: Word },
    Exec(ExecCommand),
}

pub struct ExecCommand {
    pub program: Word,
    pub args: Vec<Word>,
    pub stdin: Option<Word>,
    pub stdout: Option<Redirect>,
    pub stderr: Option<Redirect>,
}
```
with:
```rust
pub enum SimpleCommand {
    /// `A=1 B=2 …` with no following command — every assignment
    /// persists in the shell. Single-element vec is the v22-style
    /// single-assignment case.
    Assign(Vec<(String, Word)>),
    Exec(ExecCommand),
}

pub struct ExecCommand {
    /// Leading `NAME=value` words preceding the command word. Empty
    /// when the user wrote `cmd args` with no assignment prefix.
    pub inline_assignments: Vec<(String, Word)>,
    pub program: Word,
    pub args: Vec<Word>,
    pub stdin: Option<Word>,
    pub stdout: Option<Redirect>,
    pub stderr: Option<Redirect>,
}
```

- [ ] **Step 3: Update `finalize_stage` to emit the new shapes**

In `src/command.rs::finalize_stage`, replace the body so:
- The `Assign` branch wraps `(name, value)` in a single-element vec: `SimpleCommand::Assign(vec![(name, value)])`.
- All `SimpleCommand::Exec(ExecCommand { … })` literals get `inline_assignments: Vec::new(),` as the first field.

- [ ] **Step 4: Fix the compile-time fanout — every match arm and constructor**

Run `cargo build 2>&1 | grep -E "error\[" | head -40` and walk each error:
- `SimpleCommand::Assign { name, value } => …` → `SimpleCommand::Assign(list) => …`. For single-element use cases, destructure as `if let [(name, value)] = list.as_slice()` or iterate. For executor's persistent-apply (covered in Task 4), iterate the vec calling `expand_assignment` + `shell.export_set` per pair.
- Every `SimpleCommand::Exec(ExecCommand { … })` literal grows `inline_assignments: Vec::new(),` — this includes tests under `src/command.rs::tests` and any test helpers in `src/executor.rs::tests`.

For test helpers like `command.rs::tests::assignment`, update:
```rust
fn assignment(name: &str, value: Word) -> SimpleCommand {
    SimpleCommand::Assign(vec![(name.to_string(), value)])
}
```

For tests that destructure `SimpleCommand::Assign { name, value } => …`, switch to:
```rust
SimpleCommand::Assign(items) => {
    assert_eq!(items.len(), 1);
    let (name, value) = &items[0];
    …
}
```

- [ ] **Step 5: Verify zero behavior change**

```bash
cargo build 2>&1 | tail -3
cargo test 2>&1 | grep -E "^test result:" | awk '{sum += $4} END {print sum " tests pass"}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: build clean, ~909 tests pass, 0 warnings. (The test count may shift by 0–1 if any helper added/removed a test, but no `failed` lines should appear.)

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(ast): SimpleCommand::Assign carries Vec; ExecCommand gains inline_assignments

No behavior change. Parser still emits single-element Assign vecs and empty
inline_assignments. Sets up the AST shape for the v23 inline-assignment work
in subsequent tasks."
```

---

## Task 2: Tiny helpers — `Shell::is_exported` + `builtins::is_special_builtin`

Both helpers are pure and independent of each other; ship together so Task 4 has them ready.

**Files:**
- Modify: `src/shell_state.rs`
- Modify: `src/builtins.rs`
- Tests added inline in each module's `#[cfg(test)] mod tests`.

- [ ] **Step 1: Failing tests for `is_exported`**

In `src/shell_state.rs`, near the bottom of `mod tests` (or at the end of the file if a tests module exists — check first), add:
```rust
#[cfg(test)]
mod is_exported_tests {
    use super::*;

    #[test]
    fn is_exported_unset_var_is_false() {
        let shell = Shell::new();
        assert!(!shell.is_exported("DEFINITELY_NOT_SET"));
    }

    #[test]
    fn is_exported_after_set_is_false() {
        let mut shell = Shell::new();
        shell.set("FOO", "bar".to_string());
        assert!(!shell.is_exported("FOO"));
    }

    #[test]
    fn is_exported_after_export_set_is_true() {
        let mut shell = Shell::new();
        shell.export_set("FOO", "bar".to_string());
        assert!(shell.is_exported("FOO"));
    }
}
```

(If a `mod tests` already exists in this file, append the three `#[test] fn …` inside it instead of creating a new module.)

- [ ] **Step 2: Run, expect compile error**

```bash
cargo test --bin huck is_exported 2>&1 | tail -10
```
Expected: compile error — `no method named is_exported`.

- [ ] **Step 3: Implement `is_exported`**

In `src/shell_state.rs`, in the `impl Shell` block — between `unset` and `last_status`:
```rust
/// True if `name` is set and marked exported.
pub fn is_exported(&self, name: &str) -> bool {
    self.vars.get(name).is_some_and(|v| v.exported)
}
```

- [ ] **Step 4: Run, expect pass**

```bash
cargo test --bin huck is_exported 2>&1 | tail -5
```
Expected: 3 passed.

- [ ] **Step 5: Failing tests for `is_special_builtin`**

In `src/builtins.rs::tests` mod, append:
```rust
#[test]
fn is_special_builtin_recognises_posix_specials() {
    for name in ["break", "continue", "exit", "export", "return", "unset"] {
        assert!(is_special_builtin(name), "expected {name} to be special");
    }
}

#[test]
fn is_special_builtin_rejects_regular_builtins() {
    for name in ["cd", "pwd", "echo", "jobs", "wait", "fg", "bg", "kill", "disown", "history", "test", "["] {
        assert!(!is_special_builtin(name), "expected {name} to be regular");
    }
}

#[test]
fn is_special_builtin_rejects_unknowns() {
    assert!(!is_special_builtin("not_a_builtin"));
    assert!(!is_special_builtin(""));
}
```

- [ ] **Step 6: Run, expect compile error**

```bash
cargo test --bin huck is_special_builtin 2>&1 | tail -10
```
Expected: compile error.

- [ ] **Step 7: Implement `is_special_builtin`**

In `src/builtins.rs`, near `is_builtin`:
```rust
/// True for POSIX "special builtins" (2.14). Inline assignments preceding a
/// special builtin persist in the shell; assignments preceding a regular
/// builtin or external command are scoped to the command. The set is huck's
/// existing builtins intersected with the POSIX special list; expand here as
/// huck adds `set`/`shift`/`trap`/`eval`/`exec`/`:`/`readonly`/`.`.
pub fn is_special_builtin(name: &str) -> bool {
    matches!(name, "break" | "continue" | "exit" | "export" | "return" | "unset")
}
```

- [ ] **Step 8: Run, expect pass**

```bash
cargo test --bin huck is_special_builtin 2>&1 | tail -5
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: 3 passed, 0 warnings.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat: Shell::is_exported and builtins::is_special_builtin helpers

Read-only accessors used by the v23 inline-assignment apply/restore path.
Special-builtin set is POSIX 2.14 intersected with huck's current builtins."
```

---

## Task 3: Parser collects leading inline assignments + rejects assign-then-compound

Pre-task: every test in Task 1's baseline still passes. After this task, the parser produces non-empty `inline_assignments` for `A=1 cmd` shapes and rejects `A=1 if …`.

**Files:**
- Modify: `src/command.rs::finalize_stage` (peel leading assignments from `program + args`).
- Modify: `src/command.rs` (the top-level parse loop — add a check that an assignment-only run not followed by a simple command can't precede a compound-command keyword; concretely, in the keyword-dispatch path, emit a `ParseError::AssignmentBeforeCompound` when the prior simple-command-prefix accumulator is non-empty).
- Modify: `src/command.rs::ParseError` enum — add `AssignmentBeforeCompound`.
- Tests: in `src/command.rs::tests`.

Note on where the keyword check lives: the existing parser dispatches to `parse_if` / `parse_while` / `parse_for` / `parse_case` / `parse_brace_group` / `parse_function_def` based on the first token of a simple command. The leading-assignment accumulator only exists inside `finalize_stage`, which is called per simple-command. So the cleanest "reject" path is: when the dispatcher sees a compound-command keyword as the first token of a command, *but* the lexer's token stream contains a preceding assignment-shaped word that the dispatcher would otherwise have classified as a simple-command start — that's the error case. In practice, the simpler implementation is: when `parse_pipeline_with_first` collects words for a stage, if the stage's first word is an assignment AND a later word is a reserved keyword token, the whole thing is currently a parse error already (since `if`/`while`/etc. tokens can't appear inside a simple command). Adding `AssignmentBeforeCompound` makes the error message clearer; if the implementer finds the existing error sufficiently clear, the new variant can be skipped — just add a test that confirms the error case rejects.

- [ ] **Step 1: Failing tests**

In `src/command.rs::tests`, append:
```rust
#[test]
fn parse_inline_assignments_collect_into_exec() {
    let tokens = tokenize("A=1 B=2 cmd arg").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!("expected Pipeline") };
    assert_eq!(p.commands.len(), 1);
    let SimpleCommand::Exec(e) = &p.commands[0] else {
        panic!("expected Exec, got {:?}", p.commands[0])
    };
    assert_eq!(e.inline_assignments.len(), 2);
    assert_eq!(e.inline_assignments[0].0, "A");
    assert_eq!(e.inline_assignments[1].0, "B");
    assert_eq!(e.program, word_lit("cmd"));
    assert_eq!(e.args, vec![word_lit("arg")]);
}

#[test]
fn parse_assign_only_multiple_vars() {
    let tokens = tokenize("A=1 B=2").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    assert_eq!(p.commands.len(), 1);
    let SimpleCommand::Assign(items) = &p.commands[0] else {
        panic!("expected Assign(Vec), got {:?}", p.commands[0])
    };
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].0, "A");
    assert_eq!(items[1].0, "B");
}

#[test]
fn parse_assign_only_single_var_still_works() {
    let tokens = tokenize("FOO=bar").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    let SimpleCommand::Assign(items) = &p.commands[0] else { panic!() };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].0, "FOO");
}

#[test]
fn parse_mid_command_assignment_word_stays_literal() {
    let tokens = tokenize("cmd A=1").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    let SimpleCommand::Exec(e) = &p.commands[0] else { panic!() };
    assert!(e.inline_assignments.is_empty());
    assert_eq!(e.program, word_lit("cmd"));
    assert_eq!(e.args, vec![word_lit("A=1")]);
}

#[test]
fn parse_invalid_identifier_lhs_is_not_assignment() {
    let tokens = tokenize("1FOO=bar cmd").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    let SimpleCommand::Exec(e) = &p.commands[0] else { panic!() };
    assert!(e.inline_assignments.is_empty());
    assert_eq!(e.program, word_lit("1FOO=bar"));
    assert_eq!(e.args, vec![word_lit("cmd")]);
}

#[test]
fn parse_assignment_before_compound_command_errors() {
    let tokens = tokenize("A=1 if true; then echo hi; fi").unwrap();
    let err = parse(tokens).err().expect("expected parse error");
    // Either AssignmentBeforeCompound (if the implementer adds the variant)
    // or whatever the existing error path produces for `A=1 if` — the test
    // just confirms parsing fails rather than silently dropping the prefix.
    let msg = format!("{err:?}");
    assert!(!msg.is_empty(), "got: {err:?}");
}

/// Test helper if not already defined in scope.
fn word_lit(s: &str) -> Word {
    Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }])
}
```

(If `word_lit` is already defined in this `tests` mod, drop the helper definition.)

- [ ] **Step 2: Run, expect failures**

```bash
cargo test --bin huck parse_inline_assignments parse_assign_only parse_mid_command_assignment parse_invalid_identifier_lhs parse_assignment_before_compound 2>&1 | tail -25
```
Expected: 6 tests, several failures (current parser produces empty `inline_assignments` for the `A=1 B=2 cmd` case and treats `A=1 B=2` as a single Exec of program=`A=1` arg=`B=2`).

- [ ] **Step 3: Update `finalize_stage` to peel leading assignments**

In `src/command.rs::finalize_stage`, replace the body with:
```rust
fn finalize_stage(
    program: crate::lexer::Word,
    args: Vec<crate::lexer::Word>,
    stdin: Option<crate::lexer::Word>,
    stdout: Option<Redirect>,
    stderr: Option<Redirect>,
) -> SimpleCommand {
    let no_redirs = stdin.is_none() && stdout.is_none() && stderr.is_none();

    // Walk [program, args…] peeling leading assignments. Stops at the first
    // word that isn't a valid `NAME=value` (per try_split_assignment).
    let mut inline: Vec<(String, Word)> = Vec::new();
    let mut iter = std::iter::once(program).chain(args.into_iter()).peekable();
    loop {
        let Some(w) = iter.peek().cloned() else { break };
        match try_split_assignment(w) {
            Ok((name, value)) => {
                inline.push((name, value));
                iter.next();
            }
            Err(_) => break,
        }
    }
    let remaining: Vec<Word> = iter.collect();

    if remaining.is_empty() && no_redirs && !inline.is_empty() {
        return SimpleCommand::Assign(inline);
    }
    // If `inline` is empty AND remaining is empty AND there are redirects,
    // there's no program — fall through to the legacy "program is the
    // first word, args is the rest" path by re-establishing them. Since
    // remaining is empty, this is a redirect-only command; restore an
    // empty Word as program (which the executor will treat as a no-op).
    if remaining.is_empty() && inline.is_empty() {
        // Shouldn't happen — caller always passes at least one word.
        // Defensive: produce an Exec with an empty program.
        return SimpleCommand::Exec(ExecCommand {
            inline_assignments: Vec::new(),
            program: Word(Vec::new()),
            args: Vec::new(),
            stdin,
            stdout,
            stderr,
        });
    }
    let mut remaining = remaining.into_iter();
    let program = remaining.next().expect("non-empty after peel");
    let args: Vec<Word> = remaining.collect();
    SimpleCommand::Exec(ExecCommand {
        inline_assignments: inline,
        program,
        args,
        stdin,
        stdout,
        stderr,
    })
}
```

- [ ] **Step 4: Run, expect new parser tests pass**

```bash
cargo test --bin huck parse_inline_assignments parse_assign_only parse_mid_command_assignment parse_invalid_identifier_lhs 2>&1 | tail -10
cargo test --bin huck parse_assignment_before_compound 2>&1 | tail -10
```
Expected: 5 of 6 pass (the `parse_assignment_before_compound` test should also pass — the existing parser already errors on `A=1 if`, just with a generic message). If it does NOT fail-with-error, the implementer can either add a `ParseError::AssignmentBeforeCompound` variant and detect it in `finalize_stage` (set a flag when assignments accumulated, then check at the caller for compound-keyword tokens following), or convert the test to whatever the actual observable error is.

- [ ] **Step 5: Verify full suite + clippy**

```bash
cargo test 2>&1 | grep -E "^test result:" | awk '{sum+=$4; if($6 > 0) f+=$6} END {print "Pass: " sum ", Fail: " (f+0)}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: 0 fails, 0 warnings. Total goes up by ~6 from the new tests.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "parse: collect leading NAME=value words into ExecCommand::inline_assignments

finalize_stage peels the leading assignment run from program+args. A
trailing command word produces Exec with non-empty inline_assignments;
no command produces Assign(vec). Mid-command assignment-shaped words and
invalid identifiers stay literal, per POSIX."
```

---

## Task 4: Executor — apply/restore helpers + `run_exec_single` integration

Add the two helpers, then wire them into the single-command execution path. After this task, `FOO=bar /bin/true` works for externals AND `FOO=bar test -n "$FOO"` works for builtins, with restoration verified.

**Files:**
- Modify: `src/executor.rs` (new helpers + integration in `run_exec_single`).
- Tests added in `src/executor.rs::tests` plus a few integration tests in the new `tests/inline_assignments_integration.rs` (created in this task; expanded in Task 6).

- [ ] **Step 1: Failing unit tests for helpers**

In `src/executor.rs::tests`, append:
```rust
#[test]
fn apply_inline_assignments_sets_and_exports_left_to_right() {
    let mut shell = Shell::new();
    shell.export_set("HOME", "/home/test".to_string()); // ensure baseline
    let assigns = vec![
        ("A".to_string(), word_lit("1")),
        ("B".to_string(), Word(vec![WordPart::Var { name: "A".to_string(), quoted: false }])),
    ];
    let snap = apply_inline_assignments(&assigns, &mut shell);
    assert_eq!(shell.get("A"), Some("1"));
    assert_eq!(shell.get("B"), Some("1"));
    assert!(shell.is_exported("A"));
    assert!(shell.is_exported("B"));
    assert_eq!(snap.len(), 2);
}

#[test]
fn restore_inline_assignments_restores_prior_unset_state() {
    let mut shell = Shell::new();
    let assigns = vec![("FOO".to_string(), word_lit("bar"))];
    let snap = apply_inline_assignments(&assigns, &mut shell);
    assert_eq!(shell.get("FOO"), Some("bar"));
    restore_inline_assignments(snap, &mut shell);
    assert_eq!(shell.get("FOO"), None);
}

#[test]
fn restore_inline_assignments_restores_prior_value_unexported() {
    let mut shell = Shell::new();
    shell.set("FOO", "outer".to_string());
    assert!(!shell.is_exported("FOO"));
    let assigns = vec![("FOO".to_string(), word_lit("inner"))];
    let snap = apply_inline_assignments(&assigns, &mut shell);
    assert_eq!(shell.get("FOO"), Some("inner"));
    assert!(shell.is_exported("FOO"));
    restore_inline_assignments(snap, &mut shell);
    assert_eq!(shell.get("FOO"), Some("outer"));
    assert!(!shell.is_exported("FOO"));
}

#[test]
fn restore_inline_assignments_restores_prior_value_exported() {
    let mut shell = Shell::new();
    shell.export_set("FOO", "outer".to_string());
    let assigns = vec![("FOO".to_string(), word_lit("inner"))];
    let snap = apply_inline_assignments(&assigns, &mut shell);
    restore_inline_assignments(snap, &mut shell);
    assert_eq!(shell.get("FOO"), Some("outer"));
    assert!(shell.is_exported("FOO"));
}

#[test]
fn restore_inline_assignments_handles_repeated_name() {
    let mut shell = Shell::new();
    shell.set("FOO", "outer".to_string());
    let assigns = vec![
        ("FOO".to_string(), word_lit("a")),
        ("FOO".to_string(), word_lit("b")),
    ];
    let snap = apply_inline_assignments(&assigns, &mut shell);
    assert_eq!(shell.get("FOO"), Some("b"));
    restore_inline_assignments(snap, &mut shell);
    assert_eq!(shell.get("FOO"), Some("outer"));
    assert!(!shell.is_exported("FOO"));
}

fn word_lit(s: &str) -> Word {
    Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }])
}
```

(Remove the `word_lit` helper if already in scope.)

- [ ] **Step 2: Run, expect compile error**

```bash
cargo test --bin huck apply_inline_assignments restore_inline_assignments 2>&1 | tail -10
```
Expected: `cannot find function apply_inline_assignments`.

- [ ] **Step 3: Implement the helpers**

In `src/executor.rs`, near the other private helpers (e.g., right after `forget_process_children` from B-09):

```rust
/// Snapshot entry for one applied inline assignment: name, prior value
/// (None if the var was unset), prior export flag.
type AssignmentSnapshot = Vec<(String, Option<String>, bool)>;

/// Expands and applies `assignments` left-to-right, exporting each, and
/// returns a snapshot the caller can pass to `restore_inline_assignments`
/// (for temporary-scope targets) or discard (for persistent-scope targets).
fn apply_inline_assignments(
    assignments: &[(String, Word)],
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

/// Restores each snapshot entry in reverse order, so repeated names
/// unwind LIFO and end up at their pre-prefix value.
fn restore_inline_assignments(snap: AssignmentSnapshot, shell: &mut Shell) {
    for (name, prior_value, prior_exported) in snap.into_iter().rev() {
        match (prior_value, prior_exported) {
            (Some(v), true) => shell.export_set(&name, v),
            (Some(v), false) => {
                // `shell.set` preserves the existing export flag; we just
                // wrote with export=true via export_set during apply, so
                // we have to unset-then-set to land at unexported.
                shell.unset(&name);
                shell.set(&name, v);
            }
            (None, _) => shell.unset(&name),
        }
    }
}
```

- [ ] **Step 4: Run, expect helper tests pass**

```bash
cargo test --bin huck apply_inline_assignments restore_inline_assignments 2>&1 | tail -10
```
Expected: 5 passed.

- [ ] **Step 5: Failing executor test for the integration**

In `src/executor.rs::tests` append:
```rust
#[test]
fn run_exec_single_external_command_inline_assignment_restores_after() {
    use std::process::Stdio;
    // Use /bin/true (or just `true`) — a tiny external. The assertion is on
    // the shell state, not the child's behaviour.
    let mut shell = Shell::new();
    shell.set("FOO", "outer".to_string());
    let cmd = SimpleCommand::Exec(ExecCommand {
        inline_assignments: vec![("FOO".to_string(), word_lit("inner"))],
        program: word_lit("true"),
        args: vec![],
        stdin: None,
        stdout: None,
        stderr: None,
    });
    let pipeline = Pipeline { commands: vec![cmd] };
    let seq = Sequence { first: Command::Pipeline(pipeline), rest: vec![], background: false };
    let _ = execute(&seq, &mut shell, "FOO=inner true");
    assert_eq!(shell.get("FOO"), Some("outer"));
    assert!(!shell.is_exported("FOO"));
    let _ = Stdio::null(); // silence unused-import in case the implementer reorganizes
}

#[test]
fn run_exec_single_function_call_inline_assignment_persists() {
    let mut shell = Shell::new();
    // Define a no-op function via the parser so it's stored in shell.functions.
    let tokens = crate::lexer::tokenize("myfunc() { :; }").ok();
    if let Some(tokens) = tokens {
        if let Ok(Some(seq)) = crate::command::parse(tokens) {
            let _ = execute(&seq, &mut shell, "myfunc() { :; }");
        }
    }
    // Note: huck doesn't have a `:` builtin; if function definition fails
    // for that reason, replace the body with `echo > /dev/null` or similar.
    // The implementer should pick a no-op the parser accepts.

    let cmd = SimpleCommand::Exec(ExecCommand {
        inline_assignments: vec![("FOO".to_string(), word_lit("val"))],
        program: word_lit("myfunc"),
        args: vec![],
        stdin: None,
        stdout: None,
        stderr: None,
    });
    let pipeline = Pipeline { commands: vec![cmd] };
    let seq = Sequence { first: Command::Pipeline(pipeline), rest: vec![], background: false };
    let _ = execute(&seq, &mut shell, "FOO=val myfunc");
    assert_eq!(shell.get("FOO"), Some("val"));
}

#[test]
fn run_exec_single_special_builtin_inline_assignment_persists() {
    let mut shell = Shell::new();
    let cmd = SimpleCommand::Exec(ExecCommand {
        inline_assignments: vec![("FOO".to_string(), word_lit("val"))],
        program: word_lit("export"),
        args: vec![word_lit("FOO")],
        stdin: None,
        stdout: None,
        stderr: None,
    });
    let pipeline = Pipeline { commands: vec![cmd] };
    let seq = Sequence { first: Command::Pipeline(pipeline), rest: vec![], background: false };
    let _ = execute(&seq, &mut shell, "FOO=val export FOO");
    assert_eq!(shell.get("FOO"), Some("val"));
    assert!(shell.is_exported("FOO"));
}
```

- [ ] **Step 6: Run, expect at least one failure**

```bash
cargo test --bin huck run_exec_single 2>&1 | tail -25
```
Expected: failures — `run_exec_single` doesn't yet call the apply/restore helpers.

- [ ] **Step 7: Wire into `run_exec_single`**

Find `run_exec_single` (around line 640 of executor.rs). Just before the dispatch (function → builtin → external), add:

```rust
let snap = apply_inline_assignments(&cmd.inline_assignments, shell);
```

After the dispatch returns its outcome, before returning to the caller, determine persistence and conditionally restore. Concretely, replace the per-target dispatch with something like:

```rust
let resolved = resolve(cmd, shell)?;
let persistent = resolved_is_persistent(&resolved);
let snap = apply_inline_assignments(&cmd.inline_assignments, shell);
let outcome = match resolved {
    Resolved::Function(body) => call_function(body, …, shell, …),
    Resolved::Builtin(name) => builtins::run_builtin(name, …, shell),
    Resolved::External(prog) => spawn_and_wait(prog, …, shell),
};
if !persistent {
    restore_inline_assignments(snap, shell);
}
outcome
```

Where `resolved_is_persistent` is a small helper:
```rust
fn resolved_is_persistent(r: &Resolved) -> bool {
    match r {
        Resolved::Function(_) => true,
        Resolved::Builtin(name) => crate::builtins::is_special_builtin(name),
        Resolved::External(_) => false,
    }
}
```

(The exact resolved-enum names depend on what's already in `executor.rs`; the implementer should read the existing dispatch to find the right symbols. If there's no resolver enum and the dispatch is inline `if`/`else`, conditionally call `restore_inline_assignments` in each else-branch path.)

For the empty-program (no command) Assign path, the `SimpleCommand::Assign(items)` branch elsewhere in the executor should iterate `items` and apply each via `expand_assignment` + `shell.export_set`. No snapshot/restore — persistent by definition. This may already be Task 1's "Fix the compile-time fanout" step.

- [ ] **Step 8: Run executor tests, expect pass**

```bash
cargo test --bin huck run_exec_single 2>&1 | tail -15
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print "Pass: " p ", Fail: " (f+0)}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: 3 new tests pass, 0 fail in full suite, 0 warnings.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "exec: apply/restore inline assignments around run_exec_single dispatch

New apply_inline_assignments + restore_inline_assignments helpers snapshot
prior (value, exported) per name, set + export each assignment left-to-right,
and (for temporary-scope targets — regular builtins and externals) unwind
in reverse order so repeated names land back at their pre-prefix state.
Function calls and special builtins skip the restore step per POSIX 2.9.1
and 2.14."
```

---

## Task 5: Executor — `run_multi_stage` integration (per-stage scoping)

Per-stage scoping: each pipeline stage gets its own snapshot, applied just before the stage runs and restored before the next stage begins. This avoids leaking stage N's assignments into stage N+1's env.

**Files:**
- Modify: `src/executor.rs::run_multi_stage`.
- Tests added in `src/executor.rs::tests` + the integration file from Task 4.

- [ ] **Step 1: Failing integration test for per-stage scoping**

In `tests/inline_assignments_integration.rs` (created in Task 4), append:
```rust
#[test]
fn pipeline_stage_inline_assignments_are_scoped_per_stage() {
    let (out, _) = run("FOO=stage1 env | FOO=stage2 grep ^FOO=\n");
    // grep's env has FOO=stage2; stage1's env had FOO=stage1.
    // `env` outputs all its env vars; `grep` filters to ^FOO=.
    // The line we expect is `FOO=stage1` because that's what `env`
    // saw — `grep` sees its own FOO=stage2 but only prints from stdin.
    assert!(out.contains("FOO=stage1"), "got: {out}");
    assert!(!out.contains("FOO=stage2"), "stage2 should not leak: {out}");
}
```

(Use the existing `run` helper from the file, defined in Task 4. If not yet present, copy the pattern from `tests/while_integration.rs` or similar — a small helper that spawns `target/debug/huck` with piped stdin and returns (stdout, status).)

- [ ] **Step 2: Run, expect failure**

```bash
cargo test --test inline_assignments_integration pipeline_stage_inline_assignments 2>&1 | tail -15
```
Expected: failure — `run_multi_stage` doesn't apply per-stage assignments yet.

- [ ] **Step 3: Wire apply/restore into `run_multi_stage` (external stages)**

In `src/executor.rs::run_multi_stage`, find the stage-iteration loop (~line 920). For each non-Assign stage, just before `process.spawn()`, call `apply_inline_assignments(&cmd.inline_assignments, shell)` and hold the snapshot in a local. After `process.spawn()` returns (the child has captured its env at fork time), immediately call `restore_inline_assignments(snap, shell)` — the shell is back to its prior state before the next stage's setup begins.

For builtin stages (the `is_builtin` branch around line 938), apply before `run_builtin`, restore after — same persistence rules as Task 4 (special builtin → don't restore; regular builtin → restore).

Concretely for the external branch:
```rust
let snap = apply_inline_assignments(&cmd.inline_assignments, shell);
process.env_clear();
process.envs(shell.exported_env());
// ... rest of spawn config ...
let child = process.spawn()?;
restore_inline_assignments(snap, shell);
```

For the builtin branch:
```rust
let snap = apply_inline_assignments(&cmd.inline_assignments, shell);
let persistent = crate::builtins::is_special_builtin(&cmd.program);
let outcome = builtins::run_builtin(&cmd.program, &cmd.args, &mut buffer, shell);
if !persistent {
    restore_inline_assignments(snap, shell);
}
```

For the `cd`/`exit` early-skip branch (line 941), apply isn't needed because the branch immediately produces `Stage::Done(0)` without running anything. To avoid a state leak, either include the same apply/restore pair around that branch too, or just skip the inline-assignment handling for it (the user has written something like `FOO=val cd` in a pipeline, which is weird; leaving assignments unapplied is acceptable). The implementer should pick one and document it.

- [ ] **Step 4: Run, expect pass**

```bash
cargo test --test inline_assignments_integration 2>&1 | tail -10
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print "Pass: " p ", Fail: " (f+0)}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: integration test passes, 0 fail full suite, 0 warnings.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "exec: per-stage inline-assignment scoping in run_multi_stage

External-stage spawn captures the assignments via shell.exported_env() at
fork time, then the parent restores immediately so the next stage's setup
sees the original state. Builtin stages apply/restore around run_builtin
following the same special-vs-regular persistence rule used by
run_exec_single."
```

---

## Task 6: Integration test suite + doc updates

Lock down end-to-end behavior and mark the audit-doc entry fixed.

**Files:**
- Create/extend: `tests/inline_assignments_integration.rs`.
- Modify: `docs/bash-divergences.md` (M-04 → fixed).
- Modify: `README.md` (if it has a status/feature list, add inline assignments under the v22-or-later block; otherwise skip).

- [ ] **Step 1: Add the full integration-test coverage from the spec**

In `tests/inline_assignments_integration.rs`, add tests covering every row of the spec's Tests table that isn't already covered:

```rust
//! End-to-end tests for v23 inline assignments. Run `huck` as a subprocess
//! with piped stdin so the full lex/parse/execute path is exercised.

use std::io::Write;
use std::process::{Command, Stdio};

fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_huck"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        output.status.code().unwrap_or(-1),
    )
}

#[test]
fn inline_assignment_external_command_sees_var() {
    let (out, _) = run("FOO=hi env | grep ^FOO=\nexit\n");
    assert!(out.contains("FOO=hi"), "got: {out}");
}

#[test]
fn inline_assignment_external_command_restores_after() {
    let (out, _) = run("unset FOO\nFOO=hi /bin/true\necho \"[$FOO]\"\nexit\n");
    assert!(out.contains("[]"), "got: {out}");
}

#[test]
fn inline_assignment_left_to_right_visibility() {
    let (out, _) = run("A=1 B=$A env | grep -E '^[AB]='\nexit\n");
    assert!(out.contains("A=1"), "got: {out}");
    assert!(out.contains("B=1"), "got: {out}");
}

#[test]
fn inline_assignment_unset_before_restores_to_unset() {
    let (out, status) = run("unset FOO\nFOO=hi /bin/true\nprintenv FOO\necho status=$?\nexit\n");
    let _ = status;
    // printenv exits 1 when the var isn't set; we expect that line.
    assert!(out.contains("status=1"), "got: {out}");
}

#[test]
fn inline_assignment_set_unexported_before_keeps_unexported_after() {
    let (out, _) = run("FOO=outer\nFOO=inner /bin/true\nenv | grep ^FOO= || echo not-exported\nexit\n");
    assert!(out.contains("not-exported"), "got: {out}");
}

#[test]
fn inline_assignment_regular_builtin_restores() {
    let (out, _) = run("FOO=outer\nFOO=inner test -n \"$FOO\"\necho $FOO\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "outer"), "got: {out}");
}

#[test]
fn inline_assignment_special_builtin_persists() {
    let (out, _) = run("FOO=val export FOO\necho $FOO\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "val"), "got: {out}");
}

#[test]
fn inline_assignment_function_call_persists() {
    let (out, _) = run("myfunc() { :; }\nFOO=val myfunc\necho $FOO\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "val"), "got: {out}");
}

#[test]
fn inline_assignment_function_mutation_persists() {
    let (out, _) = run("myfunc() { FOO=\"$FOO-modified\"; }\nFOO=initial myfunc\necho $FOO\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "initial-modified"), "got: {out}");
}

#[test]
fn inline_assignment_dollar_question_snapshot() {
    let (out, _) = run("false\nFOO=$? true\necho $FOO\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "1"), "got: {out}");
}

#[test]
fn inline_assignment_empty_rhs() {
    let (out, _) = run("FOO= env | grep ^FOO=\nexit\n");
    assert!(out.contains("FOO=\n") || out.contains("FOO="), "got: {out}");
}

#[test]
fn inline_assignment_tilde_expands() {
    let (out, _) = run("HOME=/tmp/x FOO=~ env | grep ^FOO=\nexit\n");
    assert!(out.contains("FOO=/tmp/x"), "got: {out}");
}

#[test]
fn inline_assignment_repeated_name_restores_original() {
    let (out, _) = run("FOO=outer\nFOO=a FOO=b /bin/true\necho $FOO\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "outer"), "got: {out}");
}

// pipeline_stage_inline_assignments_are_scoped_per_stage was added in Task 5
```

- [ ] **Step 2: Run, expect pass**

```bash
cargo test --test inline_assignments_integration 2>&1 | tail -20
```
Expected: every test passes. If something fails because of a test environment quirk (e.g., `/bin/true` not at that path on some system), adjust to use the builtin `true` if huck has one, or skip with a clear comment.

- [ ] **Step 3: Verify full suite + clippy**

```bash
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print "Pass: " p ", Fail: " (f+0)}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: 0 fail, 0 warnings.

- [ ] **Step 4: Update `docs/bash-divergences.md`**

Find the M-04 line in the "Functions & scoping" section and replace its bullet with:
```markdown
- **M-04: Inline assignments `VAR=val cmd`** — `[fixed (2026-05-24)]` high. Now supported: leading `NAME=value` words on a simple command are applied left-to-right with the export flag, then restored (for external commands and regular builtins) or persisted (for special builtins, functions, and command-less assignment lists) per POSIX 2.14 / 2.9.1.
```

Also add to the change log at the bottom:
```markdown
- **2026-05-24**: M-04 (inline assignments) shipped as v23.
```

- [ ] **Step 5: Update README if relevant**

`grep -n "Status\|implemented\|v22\|inline" README.md | head -20` — if the README has a status table or feature list, add a v23 entry: "Inline assignments (`VAR=val cmd`)". If not, skip this step.

- [ ] **Step 6: Final commit**

```bash
git add -A
git commit -m "v23: inline assignments — integration tests + docs

Covers external commands, regular builtins, special builtins, function
calls, function-internal mutations, \$? snapshot, empty RHS, tilde RHS,
repeated-name LIFO restoration, and per-pipeline-stage scoping. Audit
doc M-04 marked fixed; change log updated."
```

---

## Final verification (no separate task — runs after Task 6)

```bash
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print "Pass: " p ", Fail: " (f+0)}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
cargo build 2>&1 | tail -3
```

Acceptance: 0 failures, 0 warnings, clean build, integration suite for inline assignments passes, full suite passes (target ~925 tests; +6 parser + ~5 unit + ~14 integration ≈ +25 from baseline 909).

---

## Self-review checklist

1. **Spec coverage** — every spec section maps to a task:
   - Semantics → Task 2 (helpers) + Task 4 (apply/restore + run_exec_single) + Task 5 (run_multi_stage).
   - AST changes → Task 1.
   - Parser changes → Task 3.
   - Executor changes → Tasks 4 + 5.
   - Edge cases → Task 6 (integration tests) + spec test table coverage.
   - Out-of-scope items → noted in Task 3 (compound-after-assignment rejection).
2. **Placeholders** — no TBDs; the only "implementer judgment" calls are explicitly flagged (resolved-enum names in Task 4, cd/exit-in-pipeline behaviour in Task 5).
3. **Type consistency** — `Vec<(String, Word)>` for `inline_assignments` and the `Assign` variant; `AssignmentSnapshot = Vec<(String, Option<String>, bool)>` for the snapshot helpers, used consistently across Tasks 4 and 5.
