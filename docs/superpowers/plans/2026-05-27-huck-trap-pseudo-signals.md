# huck v36 — `trap` Pseudo-Signals (ERR + DEBUG + RETURN) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close M-22 by adding the three remaining bash pseudo-signals
to v35's `trap` builtin: ERR (fires after any command's non-zero exit
with bash 5.x exemptions), DEBUG (before each simple command),
RETURN (after function returns with `$?` set to the function's status).

**Architecture:** Three new `TrapSignal` variants share v35's storage
(`shell.traps: HashMap<TrapSignal, Option<String>>`) — what's new is
the per-event firing helpers (`fire_err_trap` / `fire_debug_trap` /
`fire_return_trap`), a recursion guard (`Shell::firing_trap:
Option<TrapSignal>`), the ERR exemption depth counter
(`Shell::err_suppressed_depth: u32`), and the executor hook points.
ERR fires from `execute_sequence_body` after each command using a
next-connector-Or peek; DEBUG fires at the entry of `run_exec_single`;
RETURN fires in `call_function` between the body run and the
positional-args restore, with `$?` set to the function's status.

**Tech Stack:** Rust. Reuses v35's `src/traps.rs` infrastructure.
No new external deps.

**Spec:** `docs/superpowers/specs/2026-05-27-huck-trap-pseudo-signals-design.md`

**Branch:** `v36-trap-pseudo-signals` (already created and checked out).

---

### Task 1: `TrapSignal::{Err,Debug,Return}` variants + parse + install/reset + print_active_traps sort key

**Files:**
- Modify: `src/traps.rs` (`TrapSignal` enum at line 180; `parse_trap_signal` at line 212+; `install` arm at line 107; `reset` arm at line 163; tests module at the bottom)
- Modify: `src/builtins.rs` (`print_active_traps` sort key around line 870-880; tests module)

**Note for implementer:** Pure data extension. The four pseudo-signals
all share the same simple `traps.insert` / `traps.remove` path in
`install` / `reset` — extend the existing match arm to include the
three new variants.

- [ ] **Step 1: Write the failing parse tests**

Append to `src/traps.rs` tests module (search for `fn parse_trap_signal_exit` to find the right area):

```rust
    #[test]
    fn parse_trap_signal_err() {
        assert_eq!(parse_trap_signal("ERR"), Ok(TrapSignal::Err));
    }

    #[test]
    fn parse_trap_signal_debug() {
        assert_eq!(parse_trap_signal("DEBUG"), Ok(TrapSignal::Debug));
    }

    #[test]
    fn parse_trap_signal_return() {
        assert_eq!(parse_trap_signal("RETURN"), Ok(TrapSignal::Return));
    }
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cargo test --lib traps::tests::parse_trap_signal_err traps::tests::parse_trap_signal_debug traps::tests::parse_trap_signal_return 2>&1 | tail -10`
Expected: 3 fails (`Err/Debug/Return` variants don't exist yet).

- [ ] **Step 3: Add the three variants to `TrapSignal`**

In `src/traps.rs` at line 180, replace:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrapSignal {
    Exit,
    Real(i32),
}
```

with:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrapSignal {
    Exit,
    Err,
    Debug,
    Return,
    Real(i32),
}
```

- [ ] **Step 4: Extend `parse_trap_signal`**

In `src/traps.rs::parse_trap_signal` (around line 212), find the `if name == "EXIT"` line and add three more right after it:

```rust
    if name == "EXIT" {
        return Ok(TrapSignal::Exit);
    }
    if name == "ERR" {
        return Ok(TrapSignal::Err);
    }
    if name == "DEBUG" {
        return Ok(TrapSignal::Debug);
    }
    if name == "RETURN" {
        return Ok(TrapSignal::Return);
    }
```

- [ ] **Step 5: Extend `install`'s match arm**

In `src/traps.rs::install` (around line 107), find:

```rust
    match sig {
        TrapSignal::Exit => {
            shell.traps.insert(TrapSignal::Exit, action);
            Ok(())
        }
        TrapSignal::Real(signum) => {
            // ... existing OS-handler path ...
```

Replace the `TrapSignal::Exit` arm with a unified pseudo-signal arm covering all four:

```rust
    match sig {
        TrapSignal::Exit | TrapSignal::Err | TrapSignal::Debug | TrapSignal::Return => {
            shell.traps.insert(sig, action);
            Ok(())
        }
        TrapSignal::Real(signum) => {
            // ... existing OS-handler path unchanged ...
```

- [ ] **Step 6: Extend `reset`'s match arm**

In `src/traps.rs::reset` (around line 163), find:

```rust
    match sig {
        TrapSignal::Exit => {
            shell.traps.remove(&TrapSignal::Exit);
            Ok(())
        }
        TrapSignal::Real(signum) => {
            // ... existing path ...
```

Replace with:

```rust
    match sig {
        TrapSignal::Exit | TrapSignal::Err | TrapSignal::Debug | TrapSignal::Return => {
            shell.traps.remove(&sig);
            Ok(())
        }
        TrapSignal::Real(signum) => {
            // ... existing path unchanged ...
```

- [ ] **Step 7: Update `print_active_traps` sort key in `src/builtins.rs`**

Find `print_active_traps` (around line 856). It builds an `entries` Vec with `(sort_key, signal, action)` tuples. The current sort logic:

```rust
        let key = match sig {
            TrapSignal::Exit => 0,
            TrapSignal::Real(n) => *n,
        };
```

Replace with:

```rust
        let key = match sig {
            TrapSignal::Exit => 0,
            TrapSignal::Err => 1,
            TrapSignal::Debug => 2,
            TrapSignal::Return => 3,
            TrapSignal::Real(n) => 100 + *n,
        };
```

The `100 + *n` offset ensures all real signals sort AFTER the four pseudo-signals (since real signals are at most ~31).

Find the display-name logic in the SAME function (it maps `TrapSignal` to a name string). Currently:

```rust
        let name = match sig {
            TrapSignal::Exit => "EXIT".to_string(),
            TrapSignal::Real(n) => signal_number_to_name(*n).unwrap_or_else(|| n.to_string()),
        };
```

Replace with:

```rust
        let name = match sig {
            TrapSignal::Exit => "EXIT".to_string(),
            TrapSignal::Err => "ERR".to_string(),
            TrapSignal::Debug => "DEBUG".to_string(),
            TrapSignal::Return => "RETURN".to_string(),
            TrapSignal::Real(n) => signal_number_to_name(*n).unwrap_or_else(|| n.to_string()),
        };
```

- [ ] **Step 8: Write the builtin unit tests**

Append to `src/builtins.rs` tests module (search for `fn trap_exit_action_signal_registers` to find the right area):

```rust
    #[test]
    fn trap_err_pseudo_signal_registers() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["echo err".to_string(), "ERR".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(shell.traps.contains_key(&crate::traps::TrapSignal::Err));
    }

    #[test]
    fn trap_debug_pseudo_signal_registers() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["echo dbg".to_string(), "DEBUG".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(shell.traps.contains_key(&crate::traps::TrapSignal::Debug));
    }

    #[test]
    fn trap_return_pseudo_signal_registers() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["echo ret".to_string(), "RETURN".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(shell.traps.contains_key(&crate::traps::TrapSignal::Return));
    }

    #[test]
    fn trap_p_lists_pseudo_signals_in_order() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        // Register four pseudo-signals (intentionally not in EXIT/ERR/DEBUG/RETURN order).
        for (action, sig) in [
            ("a-return", "RETURN"),
            ("a-debug", "DEBUG"),
            ("a-exit", "EXIT"),
            ("a-err", "ERR"),
        ] {
            let _ = run_builtin(
                "trap",
                &[action.to_string(), sig.to_string()],
                &mut buf,
                &mut shell,
            );
        }
        buf.clear();
        let outcome = run_builtin("trap", &["-p".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        // The four pseudo-signals should appear in EXIT, ERR, DEBUG, RETURN order.
        let pseudo_lines: Vec<&&str> = lines.iter()
            .filter(|l| l.contains("EXIT") || l.contains("ERR") || l.contains("DEBUG") || l.contains("RETURN"))
            .collect();
        assert_eq!(pseudo_lines.len(), 4, "expected 4 pseudo-signal lines, got: {out}");
        assert!(pseudo_lines[0].contains("EXIT"), "first line should be EXIT: {}", pseudo_lines[0]);
        assert!(pseudo_lines[1].contains("ERR"), "second line should be ERR: {}", pseudo_lines[1]);
        assert!(pseudo_lines[2].contains("DEBUG"), "third line should be DEBUG: {}", pseudo_lines[2]);
        assert!(pseudo_lines[3].contains("RETURN"), "fourth line should be RETURN: {}", pseudo_lines[3]);
    }
```

- [ ] **Step 9: Run all Task 1 tests to verify they pass**

Run: `cargo test --lib traps::tests::parse_trap_signal traps::tests::trap_ 2>&1 | tail -10`
Run: `cargo test --lib builtins::tests::trap_ 2>&1 | tail -10`
Expected: all Task 1 tests pass; existing v35 tests still pass.

- [ ] **Step 10: Run the full lib suite + clippy**

Run: `cargo test --lib 2>&1 | tail -5`
Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: 0 failures; clippy clean.

- [ ] **Step 11: Commit**

```bash
git add src/traps.rs src/builtins.rs
git commit -m "$(cat <<'EOF'
traps: TrapSignal::{Err,Debug,Return} variants + parse + dispatch (v36 task 1)

Extends v35's TrapSignal enum with three new pseudo-signals. parse_trap_signal
accepts ERR/DEBUG/RETURN names; install/reset arms unified for all four
pseudo-signals via simple traps.insert / traps.remove. print_active_traps
sort key updated: EXIT=0, ERR=1, DEBUG=2, RETURN=3, Real(n)=100+n.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: `Shell::firing_trap` + `Shell::err_suppressed_depth` + fire_err/debug/return helpers + recursion-guard tests

**Files:**
- Modify: `src/shell_state.rs` (add two new pub fields + init in `Shell::new()`)
- Modify: `src/traps.rs` (add `fire_err_trap`, `fire_debug_trap`, `fire_return_trap`, shared `fire_pseudo_trap`; update `clear_for_subshell`; unit tests)

**Note for implementer:** Storage + fire mechanism. No executor wiring
yet — Task 3 and Task 4 handle the hook-point insertions. Unit tests
exercise the helpers directly (by manually pre-setting Shell flags +
calling the fire functions).

- [ ] **Step 1: Add the two new `Shell` fields**

In `src/shell_state.rs`, after the v35 trap fields (`traps`,
`trap_pending`, `trap_sigids`), add:

```rust
    /// Currently-firing pseudo-trap, if any. Set on entry to
    /// fire_err/fire_debug/fire_return; cleared on exit. Used to
    /// suppress re-firing of the SAME trap from within its own action.
    /// Different signals do NOT cross-suppress (a DEBUG action that
    /// triggers ERR still fires ERR).
    #[allow(dead_code)]  // used by traps module + executor; remove in Task 3
    pub firing_trap: Option<crate::traps::TrapSignal>,

    /// Depth counter for ERR-suppression contexts (if/elif/while/until
    /// conditions). ERR trap only fires when this is 0.
    #[allow(dead_code)]  // used by executor; remove in Task 4
    pub err_suppressed_depth: u32,
```

- [ ] **Step 2: Initialise in `Shell::new()`**

In `Shell::new()`'s struct literal, add at the end (after the v35
trap field initialisers):

```rust
            firing_trap: None,
            err_suppressed_depth: 0,
```

- [ ] **Step 3: Write the failing fire-helper tests**

Append to `src/traps.rs` tests module:

```rust
    #[test]
    fn fire_err_trap_runs_action_without_remove() {
        let mut shell = Shell::new();
        shell.traps.insert(TrapSignal::Err, Some("FOO=err_ran".to_string()));
        fire_err_trap(&mut shell);
        assert_eq!(shell.get("FOO"), Some("err_ran"));
        // Trap entry MUST still be present (unlike EXIT which self-removes).
        assert!(shell.traps.contains_key(&TrapSignal::Err));
    }

    #[test]
    fn fire_debug_trap_runs_action_without_remove() {
        let mut shell = Shell::new();
        shell.traps.insert(TrapSignal::Debug, Some("FOO=dbg_ran".to_string()));
        fire_debug_trap(&mut shell);
        assert_eq!(shell.get("FOO"), Some("dbg_ran"));
        assert!(shell.traps.contains_key(&TrapSignal::Debug));
    }

    #[test]
    fn fire_return_trap_runs_action_without_remove() {
        let mut shell = Shell::new();
        shell.traps.insert(TrapSignal::Return, Some("FOO=ret_ran".to_string()));
        fire_return_trap(&mut shell);
        assert_eq!(shell.get("FOO"), Some("ret_ran"));
        assert!(shell.traps.contains_key(&TrapSignal::Return));
    }

    #[test]
    fn fire_err_trap_recursion_guard_suppresses_reentry() {
        let mut shell = Shell::new();
        shell.firing_trap = Some(TrapSignal::Err);
        shell.traps.insert(TrapSignal::Err, Some("FOO=should_not_run".to_string()));
        fire_err_trap(&mut shell);
        // Action should NOT have run because firing_trap was already set.
        assert_eq!(shell.get("FOO"), None);
    }

    #[test]
    fn fire_debug_trap_recursion_guard_suppresses_reentry() {
        let mut shell = Shell::new();
        shell.firing_trap = Some(TrapSignal::Debug);
        shell.traps.insert(TrapSignal::Debug, Some("FOO=should_not_run".to_string()));
        fire_debug_trap(&mut shell);
        assert_eq!(shell.get("FOO"), None);
    }

    #[test]
    fn fire_return_trap_recursion_guard_suppresses_reentry() {
        let mut shell = Shell::new();
        shell.firing_trap = Some(TrapSignal::Return);
        shell.traps.insert(TrapSignal::Return, Some("FOO=should_not_run".to_string()));
        fire_return_trap(&mut shell);
        assert_eq!(shell.get("FOO"), None);
    }

    #[test]
    fn fire_err_trap_different_signal_in_flight_does_not_suppress() {
        // firing_trap is Some(Debug), but we're firing Err — should fire.
        let mut shell = Shell::new();
        shell.firing_trap = Some(TrapSignal::Debug);
        shell.traps.insert(TrapSignal::Err, Some("FOO=err_ran".to_string()));
        fire_err_trap(&mut shell);
        assert_eq!(shell.get("FOO"), Some("err_ran"));
        // firing_trap restored to its previous value (Debug) after the
        // ERR action finished.
        assert_eq!(shell.firing_trap, Some(TrapSignal::Debug));
    }

    #[test]
    fn clear_for_subshell_resets_firing_trap_and_err_depth() {
        let mut shell = Shell::new();
        shell.firing_trap = Some(TrapSignal::Err);
        shell.err_suppressed_depth = 5;
        clear_for_subshell(&mut shell);
        assert_eq!(shell.firing_trap, None);
        assert_eq!(shell.err_suppressed_depth, 0);
    }
```

- [ ] **Step 4: Run the new tests to verify they fail**

Run: `cargo test --lib traps::tests::fire_ traps::tests::clear_for_subshell_resets 2>&1 | tail -10`
Expected: 8 fails (the fire_* helpers don't exist yet).

- [ ] **Step 5: Implement the three public fire helpers + shared body**

In `src/traps.rs`, near `fire_exit_trap` (around line 77), add:

```rust
/// Fires the ERR pseudo-signal trap. Repeatable: the trap entry is
/// NOT removed after firing (unlike EXIT). Respects the recursion
/// guard via `Shell::firing_trap`.
pub fn fire_err_trap(shell: &mut Shell) {
    fire_pseudo_trap(shell, TrapSignal::Err);
}

/// Fires the DEBUG pseudo-signal trap. Repeatable; recursion-guarded.
pub fn fire_debug_trap(shell: &mut Shell) {
    fire_pseudo_trap(shell, TrapSignal::Debug);
}

/// Fires the RETURN pseudo-signal trap. Repeatable; recursion-guarded.
pub fn fire_return_trap(shell: &mut Shell) {
    fire_pseudo_trap(shell, TrapSignal::Return);
}

/// Shared body for the three repeating pseudo-signal traps. Returns
/// immediately if `shell.firing_trap == Some(sig)` (recursion guard).
/// Looks up the action via `traps.get` (NOT remove), executes via
/// `process_line`. Save-and-restore of `firing_trap` allows different
/// pseudo-signals to nest (e.g. a DEBUG action that triggers ERR).
fn fire_pseudo_trap(shell: &mut Shell, sig: TrapSignal) {
    if shell.firing_trap == Some(sig) {
        return;
    }
    let action = match shell.traps.get(&sig) {
        Some(Some(text)) => text.clone(),
        _ => return,
    };
    let prev = shell.firing_trap.replace(sig);
    let _ = crate::shell::process_line(&action, shell);
    shell.firing_trap = prev;
}
```

- [ ] **Step 6: Update `clear_for_subshell` to reset the new fields**

In `src/traps.rs::clear_for_subshell`, find the existing body (it
unregisters sigids, clears `shell.traps`, resets `shell.trap_pending`).
Add at the end:

```rust
pub fn clear_for_subshell(shell: &mut Shell) {
    // ... existing SigId unregistration + traps.clear + trap_pending reset ...
    shell.firing_trap = None;
    shell.err_suppressed_depth = 0;
}
```

- [ ] **Step 7: Run the new tests to verify they pass**

Run: `cargo test --lib traps::tests::fire_ traps::tests::clear_for_subshell_resets 2>&1 | tail -15`
Expected: 8 tests pass.

- [ ] **Step 8: Run the full lib suite + clippy**

Run: `cargo test --lib 2>&1 | tail -5`
Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: 0 failures; clippy clean.

- [ ] **Step 9: Commit**

```bash
git add src/shell_state.rs src/traps.rs
git commit -m "$(cat <<'EOF'
traps: fire_err/debug/return + recursion guard + err_suppressed_depth (v36 task 2)

Shell gains firing_trap (recursion guard) + err_suppressed_depth (ERR
exemption counter). Three new pub fire helpers share a fire_pseudo_trap
body that respects the guard and uses traps.get (not remove) so the
action stays registered for repeated firing. clear_for_subshell now
also resets the two new fields. 8 unit tests cover the firing path +
recursion guard + cross-signal nesting + subshell reset.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: DEBUG hook in `run_exec_single` + RETURN hook in `call_function`

**Files:**
- Modify: `src/executor.rs` (`run_exec_single` at line 1363; `call_function` at line 1344)

**Note for implementer:** Two single-site insertions plus the
`$?`-set-before-RETURN pattern. After this task, `Shell::firing_trap`
is reachable from the executor, so the `#[allow(dead_code)]`
annotation on that field can be removed.

- [ ] **Step 1: Add the DEBUG hook in `run_exec_single`**

In `src/executor.rs::run_exec_single` (line 1363), the function body
currently opens:

```rust
fn run_exec_single(cmd: &ExecCommand, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    let resolved = match resolve(cmd, shell) {
        Ok(r) => r,
        Err(code) => return ExecOutcome::Continue(code),
    };
```

Insert `fire_debug_trap` as the FIRST line of the body:

```rust
fn run_exec_single(cmd: &ExecCommand, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    crate::traps::fire_debug_trap(shell);
    let resolved = match resolve(cmd, shell) {
        Ok(r) => r,
        Err(code) => return ExecOutcome::Continue(code),
    };
```

- [ ] **Step 2: Update `call_function` to fire RETURN + set $? first**

In `src/executor.rs::call_function` (line 1344), the function body
currently is:

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
    let result = run_command(&body, shell, sink);
    // RETURN trap fires with $? set to the function's status AND the
    // function's positional args still in scope. After the action runs,
    // restore the caller's frame.
    let status_for_trap = match &result {
        ExecOutcome::FunctionReturn(n) => *n,
        ExecOutcome::Continue(c) => *c,
        // Exit/LoopBreak/LoopContinue propagate up; keep $? as-is.
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

- [ ] **Step 3: Remove the `#[allow(dead_code)]` on `Shell::firing_trap`**

In `src/shell_state.rs`, find the `#[allow(dead_code)]  // used by traps module + executor; remove in Task 3` annotation above `pub firing_trap` and delete that annotation line.

(Leave the `err_suppressed_depth` annotation in place — it's removed in Task 4 when the depth counter is actually pushed/popped.)

- [ ] **Step 4: Build to verify clean**

Run: `cargo build 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 5: Run the full lib suite**

Run: `cargo test --lib 2>&1 | tail -5`
Expected: 0 failures. (Unit tests for DEBUG/RETURN firing live at the integration level in Task 5 — no new unit tests in this task. The existing v35 trap tests must still pass.)

- [ ] **Step 6: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/executor.rs src/shell_state.rs
git commit -m "$(cat <<'EOF'
exec: DEBUG hook in run_exec_single + RETURN hook in call_function (v36 task 3)

DEBUG fires as the first line of run_exec_single — covers every simple
command (external, builtin, function call) regardless of context.
Compound commands don't fire DEBUG since they dispatch through other
paths.

RETURN fires in call_function BETWEEN the body run and the
positional-args restore, so the action sees the function's $0/$@.
$? is set to the function's return status (FunctionReturn(n) or
Continue(c)) before firing so the action's `$?` reflects the function
result. Removed firing_trap's dead_code annotation now that it's read.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: ERR hook in `execute_sequence_body` + err_suppressed_depth push/pop in run_if/run_while

**Files:**
- Modify: `src/executor.rs` (`execute_sequence_body` at line 55; `run_if` at line 348; `run_while` at line 195)
- Modify: `src/shell_state.rs` (remove `#[allow(dead_code)]` from `err_suppressed_depth`)

**Note for implementer:** This task wires the ERR trap. Three sub-changes:
(a) ERR fires from `execute_sequence_body` after each command's
non-zero `Continue(c)`, conditioned on `err_suppressed_depth == 0`
AND the next connector is NOT `Or`; (b) `run_if` pushes the depth
around each condition (initial `if` + each `elif`); (c) `run_while`
pushes the depth around the condition.

`run_while` handles both `while` and `until` via `clause.until: bool`
— a single push/pop covers both.

`!` negation push/pop is OMITTED per the spec — huck doesn't currently
support `!` as pipeline negation.

- [ ] **Step 1: Refactor `execute_sequence_body` loop to peek at the next connector**

In `src/executor.rs::execute_sequence_body` (line 55), the rest-of-sequence
loop currently iterates as `for (connector, command) in &seq.rest`. We
need to peek at the NEXT iteration's connector to know whether the
CURRENT command is the LHS of `||`. Convert to an indexed loop:

Find:

```rust
    for (connector, command) in &seq.rest {
        let should_run = match connector {
            Connector::Semi => true,
            Connector::And => matches!(status, ExecOutcome::Continue(0)),
            Connector::Or => matches!(status, ExecOutcome::Continue(c) if c != 0),
        };
        if should_run {
            status = run_command(command, shell, sink);
            if matches!(
                status,
                ExecOutcome::Exit(_) | ExecOutcome::LoopBreak | ExecOutcome::LoopContinue
                    | ExecOutcome::FunctionReturn(_)
            ) {
                return status;
            }
            if let ExecOutcome::Continue(c) = status {
                shell.set_last_status(c);
                if shell.pending_fatal_pe_error.is_some() {
                    return ExecOutcome::Continue(c);
                }
                crate::traps::dispatch_pending_traps(shell);
            }
        }
    }
```

Replace with:

```rust
    for i in 0..seq.rest.len() {
        let (connector, command) = &seq.rest[i];
        let should_run = match connector {
            Connector::Semi => true,
            Connector::And => matches!(status, ExecOutcome::Continue(0)),
            Connector::Or => matches!(status, ExecOutcome::Continue(c) if c != 0),
        };
        if should_run {
            status = run_command(command, shell, sink);
            if matches!(
                status,
                ExecOutcome::Exit(_) | ExecOutcome::LoopBreak | ExecOutcome::LoopContinue
                    | ExecOutcome::FunctionReturn(_)
            ) {
                return status;
            }
            if let ExecOutcome::Continue(c) = status {
                shell.set_last_status(c);
                if shell.pending_fatal_pe_error.is_some() {
                    return ExecOutcome::Continue(c);
                }
                crate::traps::dispatch_pending_traps(shell);
                // ERR fires if this command failed AND we're not in a
                // suppression context AND the NEXT connector is not Or
                // (i.e. the failure isn't "handled" by a following || clause).
                let next_is_or = matches!(seq.rest.get(i + 1), Some((Connector::Or, _)));
                if c != 0 && shell.err_suppressed_depth == 0 && !next_is_or {
                    crate::traps::fire_err_trap(shell);
                }
            }
        }
    }
```

- [ ] **Step 2: Add the ERR firing to the FIRST `set_last_status(c)` block**

In the same function, the FIRST `if let ExecOutcome::Continue(c) = status` block (around lines 68-74, before the loop) handles `seq.first`'s status. Add the ERR firing there with the same peek logic (the next connector is `seq.rest.first()`):

Find:

```rust
    if let ExecOutcome::Continue(c) = status {
        shell.set_last_status(c);
        if shell.pending_fatal_pe_error.is_some() {
            return ExecOutcome::Continue(c);
        }
        crate::traps::dispatch_pending_traps(shell);
    }
```

Replace with:

```rust
    if let ExecOutcome::Continue(c) = status {
        shell.set_last_status(c);
        if shell.pending_fatal_pe_error.is_some() {
            return ExecOutcome::Continue(c);
        }
        crate::traps::dispatch_pending_traps(shell);
        // ERR fires for seq.first's failure if not suppressed AND the
        // next connector (if any) is not Or.
        let next_is_or = matches!(seq.rest.first(), Some((Connector::Or, _)));
        if c != 0 && shell.err_suppressed_depth == 0 && !next_is_or {
            crate::traps::fire_err_trap(shell);
        }
    }
```

- [ ] **Step 3: Add push/pop in `run_if` around each condition**

In `src/executor.rs::run_if` (line 348), the function currently is:

```rust
fn run_if(clause: &IfClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    let cond = execute_sequence_body(&clause.condition, shell, sink);
    // ... exit/loop-break/return propagation ...
    if matches!(cond, ExecOutcome::Continue(0)) {
        return execute_sequence_body(&clause.then_body, shell, sink);
    }
    for elif in &clause.elif_branches {
        let elif_cond = execute_sequence_body(&elif.condition, shell, sink);
        // ... same propagation ...
        if matches!(elif_cond, ExecOutcome::Continue(0)) {
            return execute_sequence_body(&elif.body, shell, sink);
        }
    }
    if let Some(else_body) = &clause.else_body {
        return execute_sequence_body(else_body, shell, sink);
    }
    ExecOutcome::Continue(0)
}
```

Replace with a version that pushes the suppression depth around the
initial condition AND each `elif` condition. The bodies (then_body /
elif.body / else_body) are NOT inside suppression — only conditions
are.

```rust
fn run_if(clause: &IfClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    shell.err_suppressed_depth += 1;
    let cond = execute_sequence_body(&clause.condition, shell, sink);
    shell.err_suppressed_depth -= 1;
    if matches!(
        cond,
        ExecOutcome::Exit(_) | ExecOutcome::LoopBreak | ExecOutcome::LoopContinue
            | ExecOutcome::FunctionReturn(_)
    ) {
        return cond;
    }
    if matches!(cond, ExecOutcome::Continue(0)) {
        return execute_sequence_body(&clause.then_body, shell, sink);
    }
    for elif in &clause.elif_branches {
        shell.err_suppressed_depth += 1;
        let elif_cond = execute_sequence_body(&elif.condition, shell, sink);
        shell.err_suppressed_depth -= 1;
        if matches!(
            elif_cond,
            ExecOutcome::Exit(_) | ExecOutcome::LoopBreak | ExecOutcome::LoopContinue
                | ExecOutcome::FunctionReturn(_)
        ) {
            return elif_cond;
        }
        if matches!(elif_cond, ExecOutcome::Continue(0)) {
            return execute_sequence_body(&elif.body, shell, sink);
        }
    }
    if let Some(else_body) = &clause.else_body {
        return execute_sequence_body(else_body, shell, sink);
    }
    ExecOutcome::Continue(0)
}
```

(Note: this is panic-unsafe — if `execute_sequence_body` panics, the
depth counter is left incremented. Huck's executor doesn't use
unwind-based error propagation, and `execute_sequence_body` doesn't
panic in practice (errors come back as `ExecOutcome` values), so the
plain increment/decrement is acceptable. If the codebase later moves to
panic-on-error, switch to a `Drop`-based guard.)

- [ ] **Step 4: Add push/pop in `run_while` around the condition**

In `src/executor.rs::run_while` (line 195), find the condition-evaluation
line:

```rust
        let cond = execute_sequence_body(&clause.condition, shell, sink);
```

Replace with:

```rust
        shell.err_suppressed_depth += 1;
        let cond = execute_sequence_body(&clause.condition, shell, sink);
        shell.err_suppressed_depth -= 1;
```

(`run_while` handles both `while` and `until` via `clause.until` —
this single push/pop covers both.)

- [ ] **Step 5: Remove the `#[allow(dead_code)]` on `Shell::err_suppressed_depth`**

In `src/shell_state.rs`, find the annotation `#[allow(dead_code)]  // used by executor; remove in Task 4` above `pub err_suppressed_depth` and delete it.

- [ ] **Step 6: Build to verify clean**

Run: `cargo build 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 7: Run the full test suite**

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | grep -E "failed: [1-9]"`
Expected: no output. Existing v35 trap tests + all other tests must
still pass. New ERR/DEBUG/RETURN behavior tested at the integration
level in Task 5.

- [ ] **Step 8: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add src/executor.rs src/shell_state.rs
git commit -m "$(cat <<'EOF'
exec: ERR hook in execute_sequence_body + suppression-depth push/pop (v36 task 4)

ERR fires from both set_last_status sites in execute_sequence_body
(after seq.first + inside the rest loop), conditioned on:
non-zero status AND err_suppressed_depth == 0 AND next connector is not
Or. Rest-of-sequence loop refactored to indexed iteration for the
next-connector peek.

err_suppressed_depth pushed/popped around: initial if condition + each
elif condition (run_if), the loop condition (run_while, covers both
while and until via clause.until). `!` negation push/pop omitted per
spec (huck doesn't parse `!` as pipeline negation — pre-existing gap).
Removed err_suppressed_depth's dead_code annotation now that it's read.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Integration tests + docs (M-22 → fixed v36)

**Files:**
- Create: `tests/trap_pseudo_signals_integration.rs`
- Modify: `docs/bash-divergences.md` (M-22 → `[fixed v36]`; changelog row)
- Modify: `README.md` (v36 row)

**Note for implementer:** The integration tests spawn `huck` via
`Command::new(huck_binary())` with piped scripts. Each test gets a
fresh process — handler state never leaks. Same harness as v35's
`tests/trap_integration.rs`. ~13 new tests cover DEBUG, ERR exemption
behavior, and RETURN.

- [ ] **Step 1: Create `tests/trap_pseudo_signals_integration.rs`**

Create the file with:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run(script: &str) -> (String, String, std::process::ExitStatus) {
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
        out.status,
    )
}

// ──────────── DEBUG (4 tests) ────────────

#[test]
fn debug_fires_before_simple_command() {
    let (out, _err, _) = run("trap 'echo DBG' DEBUG\ntrue\nexit\n");
    assert!(out.lines().any(|l| l == "DBG"), "stdout: {out}");
}

#[test]
fn debug_fires_inside_function_body() {
    let (out, _err, _) = run("trap 'echo DBG' DEBUG\nf() { true; }\nf\nexit\n");
    // At least one DBG fires for the `true` inside f.
    let count = out.lines().filter(|l| **l == *"DBG").count();
    assert!(count >= 1, "expected ≥1 DBG, got {count}; stdout: {out}");
}

#[test]
fn debug_does_not_fire_for_compound_command_itself() {
    // `if true; then true; fi` has TWO simple commands (condition's
    // `true` + body's `true`). DEBUG fires for those, not for the
    // `if` itself.
    let (out, _err, _) = run("trap 'echo DBG' DEBUG\nif true; then true; fi\nexit\n");
    let count = out.lines().filter(|l| **l == *"DBG").count();
    // Exactly 2 DBG lines (one per simple command — the action's own
    // `echo DBG` is recursion-suppressed).
    assert_eq!(count, 2, "expected exactly 2 DBG lines, got {count}; stdout: {out}");
}

#[test]
fn debug_recursion_guard_prevents_infinite_fire() {
    // The trap action itself runs `echo DBG`, which IS a simple command,
    // but the recursion guard suppresses DEBUG re-firing inside the
    // action. So `true` produces exactly ONE DBG line, not infinite.
    let (out, _err, status) = run("trap 'echo DBG' DEBUG\ntrue\nexit\n");
    let count = out.lines().filter(|l| **l == *"DBG").count();
    assert_eq!(count, 1, "expected exactly 1 DBG, got {count}; stdout: {out}");
    assert_eq!(status.code(), Some(0));
}

// ──────────── ERR (7 tests) ────────────

#[test]
fn err_fires_on_simple_command_failure() {
    let (out, _err, _) = run("trap 'echo CAUGHT' ERR\nfalse\nexit\n");
    assert!(out.lines().any(|l| l == "CAUGHT"), "stdout: {out}");
}

#[test]
fn err_does_not_fire_in_if_condition() {
    let (out, _err, _) = run("trap 'echo CAUGHT' ERR\nif false; then :; fi\nexit\n");
    assert!(!out.lines().any(|l| l == "CAUGHT"), "stdout: {out}");
}

#[test]
fn err_does_not_fire_in_while_condition() {
    let (out, _err, _) = run("trap 'echo CAUGHT' ERR\nwhile false; do :; done\nexit\n");
    assert!(!out.lines().any(|l| l == "CAUGHT"), "stdout: {out}");
}

#[test]
fn err_does_not_fire_on_or_chain_lhs() {
    let (out, _err, _) = run("trap 'echo CAUGHT' ERR\nfalse || true\nexit\n");
    assert!(!out.lines().any(|l| l == "CAUGHT"), "stdout: {out}");
}

#[test]
fn err_fires_on_or_chain_when_all_fail() {
    let (out, _err, _) = run("trap 'echo CAUGHT' ERR\nfalse || false\nexit\n");
    assert!(out.lines().any(|l| l == "CAUGHT"), "stdout: {out}");
}

#[test]
fn err_fires_on_and_chain_lhs_failure() {
    let (out, _err, _) = run("trap 'echo CAUGHT' ERR\nfalse && true\nexit\n");
    assert!(out.lines().any(|l| l == "CAUGHT"), "stdout: {out}");
}

#[test]
fn err_fires_on_and_chain_rhs_failure() {
    let (out, _err, _) = run("trap 'echo CAUGHT' ERR\ntrue && false\nexit\n");
    assert!(out.lines().any(|l| l == "CAUGHT"), "stdout: {out}");
}

// ──────────── RETURN (2 tests) ────────────

#[test]
fn return_fires_after_function_return() {
    let (out, _err, _) = run("trap 'echo RET' RETURN\nf() { :; }\nf\nexit\n");
    let count = out.lines().filter(|l| **l == *"RET").count();
    assert_eq!(count, 1, "expected exactly 1 RET, got {count}; stdout: {out}");
}

#[test]
fn return_action_sees_function_status() {
    // The action runs with $? set to the function's return status.
    let (out, _err, _) = run("trap 'echo got=$?' RETURN\nf() { return 7; }\nf\necho done=$?\nexit\n");
    assert!(out.lines().any(|l| l == "got=7"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "done=7"), "stdout: {out}");
}
```

- [ ] **Step 2: Run the new integration tests**

Run: `cargo test --test trap_pseudo_signals_integration 2>&1 | tail -10`
Expected: all 13 tests pass.

If `debug_does_not_fire_for_compound_command_itself` fails with
"expected 2 DBG lines" but a different count, walk through the
script to verify your understanding: `if true; then true; fi` — the
condition is the simple command `true`, the body is the simple command
`true`. Both fire DEBUG. The action's own `echo DBG` is a simple
command but the recursion guard prevents re-fire. So exactly 2.

- [ ] **Step 3: Update `docs/bash-divergences.md` — mark M-22 as fixed v36**

Find the M-22 entry (search for `**M-22:`). The current text starts
with `[partial v35]`. Replace the entire M-22 entry with:

```markdown
- **M-22: `trap` builtin** — `[fixed v36]` high. All four bash pseudo-signals (EXIT, ERR, DEBUG, RETURN) + 13 trappable real signals (huck's 15-name table minus KILL/STOP) now supported via `trap ACTION SIGNAL...`, `trap -p`, `trap -l`, `trap - SIGNAL`, `trap "" SIGNAL`. Action body stored as raw text, re-parsed via `process_line` at fire time (late variable binding). Async-signal-safe `Arc<AtomicU32>` bitmask delivery for real signals; per-event firing for pseudo-signals with `Shell::firing_trap` recursion guard. EXIT self-removes before firing. ERR fires after any non-zero command exit except inside `if`/`elif`/`while`/`until` conditions or on LHS of `||` chain (matches bash 5.x `set -e` rules). DEBUG fires before each simple command. RETURN fires after a function returns with `$?` set to the return value and the function's positional args still in scope. Subshell trap-clear matches POSIX. **Known limitations**: M-41 limited signal set still applies (no SEGV/ABRT/etc.); `trap "" SIGNAL` registers an empty custom handler rather than true SIG_IGN, so child processes after `exec` do NOT inherit the "ignore" disposition (matters for `trap '' PIPE; cmd | head`-style scripts); `$BASH_COMMAND` variable inside DEBUG/ERR/RETURN actions is not set (the action runs but the variable expands to empty); `! cmd` pipeline negation is not parsed by huck, so the bash `!` ERR exemption is moot.
```

- [ ] **Step 4: Add a changelog row**

At the bottom of `docs/bash-divergences.md`, in the Change log section,
append:

```markdown
- **2026-05-27**: M-22 (`trap` builtin) closed as fixed v36. ERR, DEBUG, RETURN pseudo-signals added alongside v35's EXIT + 13 real signals. Per-event firing helpers (`fire_err_trap` / `fire_debug_trap` / `fire_return_trap`) share a `fire_pseudo_trap` body with a `Shell::firing_trap` recursion guard. ERR uses a `Shell::err_suppressed_depth` counter pushed/popped in `run_if` / `run_while` to implement bash 5.x exemptions. DEBUG hooks at `run_exec_single` entry; RETURN at `call_function` with $? set to the function's status. M-22 status: `[fixed v36]`.
```

- [ ] **Step 5: Update README.md version table**

Find the v35 row in `README.md`'s version table. Add a new row AFTER it:

```markdown
| v36       | `trap` pseudo-signals ERR/DEBUG/RETURN (closes M-22)            |
```

Match column alignment with the surrounding rows.

- [ ] **Step 6: Commit the docs**

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: M-22 fixed v36; v36 in README

ERR/DEBUG/RETURN pseudo-signals close M-22. Known limitations
preserved: M-41 (signal set), trap "" SIG SIG_IGN gap, $BASH_COMMAND
not set, ! pipeline negation not parsed.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 7: Commit the integration tests (or bundle into Step 6's commit)**

If steps 1-2 weren't already committed, commit them now:

```bash
git add tests/trap_pseudo_signals_integration.rs
git commit -m "$(cat <<'EOF'
test: trap pseudo-signals integration coverage (v36 task 5)

13 integration tests: 4 for DEBUG (fires-before-simple, fires-inside-
function, no-fire-on-compound, recursion-guard), 7 for ERR (fires-on-
failure + 6 exemption cases for if/while/||/&& combinations), 2 for
RETURN (fires-after-function + sees-function-status).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

(Or fold both into a single commit: `test+docs: trap pseudo-signals integration coverage + M-22 fixed v36 (v36 task 5)`. Match v35's pattern.)

- [ ] **Step 8: Run the entire test suite**

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | tail -30`
Expected: all suites pass. New baseline ~1397 (1372 from v35 + ~25
new tests across Tasks 1, 2, 5).

If PTY suite shows its v29-era flake (`pty_compound_stage_pipeline_stops_and_resumes`),
re-run it in isolation:
`cargo test --test pty_interactive pty_compound_stage_pipeline_stops_and_resumes 2>&1 | tail -5`.
Pre-existing — not a v36 regression.

- [ ] **Step 9: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: 0 warnings.

- [ ] **Step 10: Confirm working tree is clean**

Run: `git status`
Expected: `nothing to commit, working tree clean` on branch
`v36-trap-pseudo-signals`. No untracked files.

**No additional commit for steps 8-10** — they're verification only.
Hand back to the parent session for the final code-reviewer dispatch +
merge to main.
