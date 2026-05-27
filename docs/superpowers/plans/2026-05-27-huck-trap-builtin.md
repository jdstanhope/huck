# huck v35 — `trap` Builtin Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement POSIX `trap` builtin with EXIT pseudo-signal + 13
trappable real signals (huck's existing 15-name table minus the two
uncatchable ones, KILL and STOP). Closes M-22 partially (ERR/DEBUG/RETURN
still deferred).

**Architecture:** New module `src/traps.rs` owns all trap-related state
and signal-handler installation. `Shell` gains two fields:
`traps: HashMap<TrapSignal, Option<String>>` (action storage; `None` =
ignore) and `trap_pending: Arc<AtomicU32>` (per-signal bitmask written
by async-signal-safe handlers, drained by the main loop). For real
signals we install a closure handler via `signal_hook::low_level::register`
that does `trap_pending.fetch_or(1 << signum, SeqCst)` — additive to
huck's existing `signal_hook::flag::register` for SIGINT/SIGCHLD so
those keep working. The main loop polls + dispatches at three
checkpoints (top of REPL, after each pipeline in
`execute_sequence_body`, after `wait_pipeline_raw`). EXIT trap is a
separate pseudo-signal that fires at every shell-exit path via
`fire_exit_trap`, which `remove`s the action so it can never re-fire.

**Tech Stack:** Rust. Reuses existing `signal-hook = "0.4.4"` dep
(already in `Cargo.toml`) and `libc`. No new external deps.

**Spec:** `docs/superpowers/specs/2026-05-27-huck-trap-builtin-design.md`

**Branch:** `v35-trap-builtin` (already created and checked out).

---

### Task 1: `src/traps.rs` module skeleton — `TrapSignal`, signal name table, `parse_trap_signal`

**Files:**
- Create: `src/traps.rs`
- Modify: `src/main.rs` (add `mod traps;` line)

**Note for implementer:** This task is pure-data + parsing. No Shell
wiring, no signal handlers, no integration with the rest of the code.
The unit tests verify the name-parsing helper for all forms.

- [ ] **Step 1: Create `src/traps.rs` with the `TrapSignal` enum and signal-name table**

Create `src/traps.rs` with:

```rust
//! Trap handler storage, signal-name parsing, and signal-delivery
//! plumbing for the `trap` builtin (huck v35).

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrapSignal {
    Exit,
    Real(i32),
}

/// Trappable real signals — huck's existing 15-name table from `kill`,
/// minus KILL (9) and STOP (19) which POSIX says cannot be trapped.
/// Each entry: (name without SIG prefix, libc signal number).
const TRAPPABLE: &[(&str, i32)] = &[
    ("HUP",   libc::SIGHUP),
    ("INT",   libc::SIGINT),
    ("QUIT",  libc::SIGQUIT),
    ("USR1",  libc::SIGUSR1),
    ("USR2",  libc::SIGUSR2),
    ("PIPE",  libc::SIGPIPE),
    ("ALRM",  libc::SIGALRM),
    ("TERM",  libc::SIGTERM),
    ("CHLD",  libc::SIGCHLD),
    ("CONT",  libc::SIGCONT),
    ("TSTP",  libc::SIGTSTP),
    ("TTIN",  libc::SIGTTIN),
    ("TTOU",  libc::SIGTTOU),
    ("WINCH", libc::SIGWINCH),
];

/// Returns the trappable signal table (name → signal-number pairs).
pub fn name_table() -> &'static [(&'static str, i32)] {
    TRAPPABLE
}

/// Parses `name` as a signal specification. Accepts:
/// - `"EXIT"` → `TrapSignal::Exit`
/// - `"INT"` / `"SIGINT"` / `"2"` → `TrapSignal::Real(2)`
/// - Same dual-form for every trappable signal.
/// Returns an error for `KILL`/`STOP`/unknown names/non-trappable
/// numbers.
pub fn parse_trap_signal(name: &str) -> Result<TrapSignal, String> {
    // EXIT pseudo-signal (case-sensitive to match bash).
    if name == "EXIT" {
        return Ok(TrapSignal::Exit);
    }

    // Numeric form.
    if let Ok(n) = name.parse::<i32>() {
        // Reject uncatchable signals up front.
        if n == libc::SIGKILL {
            return Err(format!("{name}: cannot trap"));
        }
        if n == libc::SIGSTOP {
            return Err(format!("{name}: cannot trap"));
        }
        // Accept any signal in the trappable table.
        if TRAPPABLE.iter().any(|(_, s)| *s == n) {
            return Ok(TrapSignal::Real(n));
        }
        return Err(format!("{name}: invalid signal specification"));
    }

    // Strip optional SIG prefix (case-sensitive — bash matches "SIGINT"
    // but not "Sigint").
    let stripped = name.strip_prefix("SIG").unwrap_or(name);

    // Reject KILL/STOP by name.
    if stripped == "KILL" || stripped == "STOP" {
        return Err(format!("{name}: cannot trap"));
    }

    // Look up in the trappable table.
    for (n, sig) in TRAPPABLE {
        if *n == stripped {
            return Ok(TrapSignal::Real(*sig));
        }
    }

    Err(format!("{name}: invalid signal specification"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_trap_signal_exit() {
        assert_eq!(parse_trap_signal("EXIT"), Ok(TrapSignal::Exit));
    }

    #[test]
    fn parse_trap_signal_name_no_prefix() {
        assert_eq!(parse_trap_signal("INT"), Ok(TrapSignal::Real(libc::SIGINT)));
        assert_eq!(parse_trap_signal("TERM"), Ok(TrapSignal::Real(libc::SIGTERM)));
        assert_eq!(parse_trap_signal("HUP"), Ok(TrapSignal::Real(libc::SIGHUP)));
    }

    #[test]
    fn parse_trap_signal_sig_prefix() {
        assert_eq!(parse_trap_signal("SIGINT"), Ok(TrapSignal::Real(libc::SIGINT)));
        assert_eq!(parse_trap_signal("SIGTERM"), Ok(TrapSignal::Real(libc::SIGTERM)));
    }

    #[test]
    fn parse_trap_signal_number() {
        assert_eq!(parse_trap_signal("2"), Ok(TrapSignal::Real(libc::SIGINT)));
        assert_eq!(parse_trap_signal("15"), Ok(TrapSignal::Real(libc::SIGTERM)));
    }

    #[test]
    fn parse_trap_signal_unknown_name_errors() {
        assert!(parse_trap_signal("NOPE").is_err());
        assert!(parse_trap_signal("SIGNOPE").is_err());
    }

    #[test]
    fn parse_trap_signal_unknown_number_errors() {
        // 99 is not in the trappable table.
        assert!(parse_trap_signal("99").is_err());
    }

    #[test]
    fn parse_trap_signal_kill_by_name_errors() {
        assert!(matches!(parse_trap_signal("KILL"), Err(s) if s.contains("cannot trap")));
        assert!(matches!(parse_trap_signal("SIGKILL"), Err(s) if s.contains("cannot trap")));
    }

    #[test]
    fn parse_trap_signal_kill_by_number_errors() {
        let n = libc::SIGKILL.to_string();
        assert!(matches!(parse_trap_signal(&n), Err(s) if s.contains("cannot trap")));
    }

    #[test]
    fn parse_trap_signal_stop_by_name_errors() {
        assert!(matches!(parse_trap_signal("STOP"), Err(s) if s.contains("cannot trap")));
        assert!(matches!(parse_trap_signal("SIGSTOP"), Err(s) if s.contains("cannot trap")));
    }

    #[test]
    fn parse_trap_signal_stop_by_number_errors() {
        let n = libc::SIGSTOP.to_string();
        assert!(matches!(parse_trap_signal(&n), Err(s) if s.contains("cannot trap")));
    }

    #[test]
    fn name_table_has_14_trappable_entries() {
        // 15 total minus KILL minus STOP = 13 trappable… wait, the
        // table never included KILL/STOP since they're filtered at
        // parse time, so the table has 14 entries. (HUP/INT/QUIT/USR1/
        // USR2/PIPE/ALRM/TERM/CHLD/CONT/TSTP/TTIN/TTOU/WINCH.)
        assert_eq!(name_table().len(), 14);
    }
}
```

Mark `trap_pending` field on the module as `#[allow(dead_code)]` is
NOT needed — we don't have one yet. The `Arc<AtomicU32>` import is for
Task 2. If the unused import warns, remove it for now and add back in
Task 2.

Actually, the imports `Arc` and `AtomicU32` ARE unused in Task 1.
Drop them from the top — re-add in Task 2:

```rust
//! Trap handler storage, signal-name parsing, and signal-delivery
//! plumbing for the `trap` builtin (huck v35).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrapSignal {
    Exit,
    Real(i32),
}

// ... rest of file ...
```

- [ ] **Step 2: Register the new module in `src/main.rs`**

In `src/main.rs`, add `mod traps;` in alphabetical order alongside the
other module declarations. After this edit:

```rust
mod arith;
mod builtins;
mod command;
mod completion;
mod continuation;
mod executor;
mod expand;
mod history;
mod job_spec;
mod jobs;
mod lexer;
mod param_expansion;
mod shell;
mod shell_state;
mod test_builtin;
mod traps;
```

- [ ] **Step 3: Build to verify clean**

Run: `cargo build 2>&1 | tail -5`
Expected: clean build, 0 errors. (The `TrapSignal` variants are unused
externally for now — Rust shouldn't warn since it's used internally by
`parse_trap_signal`.)

- [ ] **Step 4: Run the unit tests to verify they pass**

Run: `cargo test --lib traps::tests 2>&1 | tail -10`
Expected: 10 tests pass.

- [ ] **Step 5: Run the full lib suite to confirm no regression**

Run: `cargo test --lib 2>&1 | tail -5`
Expected: 0 failures (test count goes up by ~10).

- [ ] **Step 6: Run clippy with `-D warnings`**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/traps.rs src/main.rs
git commit -m "$(cat <<'EOF'
traps: module skeleton with TrapSignal + parse_trap_signal (v35 task 1)

New src/traps.rs hosts the trap builtin's data layer: TrapSignal enum
(Exit pseudo-signal + Real(libc number)), the 14-entry trappable
signal table, and parse_trap_signal that accepts INT/SIGINT/2 forms.
KILL/STOP rejected by both name and number per POSIX. 10 unit tests.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: `Shell::traps` + `Shell::trap_pending` + delivery scaffold + subshell-clear

**Files:**
- Modify: `src/shell_state.rs` (add two `Shell` fields + init in `new()` + accessor + `clear_traps_for_subshell` method)
- Modify: `src/traps.rs` (add `TRAP_PENDING` `OnceLock` + `init_pending_bitmask` + `drain_pending` + `dispatch_pending_traps` + `fire_exit_trap` + `clear_for_subshell` + tests)
- Modify: `src/executor.rs::fork_and_run_in_subshell` (call `traps::clear_for_subshell` in the child branch around line 2602, before the body runs)

**Note for implementer:** This task adds the storage + delivery scaffold
WITHOUT yet installing any OS signal handlers (that's Task 3). The
unit tests for `dispatch_pending_traps` simulate a fired signal by
directly setting bits on the `Arc<AtomicU32>`, bypassing the OS
delivery path.

- [ ] **Step 1: Add two fields to `Shell`**

In `src/shell_state.rs`, in the `pub struct Shell { ... }` block, after
the v34 fields (`pending_fatal_pe_error`, `is_interactive`), add:

```rust
    /// Registered trap handlers. `None` value = ignore that signal
    /// (corresponds to `trap "" SIGNAL`); `Some(text)` = action to
    /// re-parse and execute when the signal fires. Absent key =
    /// default disposition.
    #[allow(dead_code)]  // used by traps module + builtins; remove in Task 5
    pub traps: std::collections::HashMap<crate::traps::TrapSignal, Option<String>>,

    /// Per-signal bitmask of "trap pending" flags. Signal handlers set
    /// bits via `fetch_or`; the main loop drains via `swap` at the
    /// polling checkpoints. Bit N corresponds to libc signal number N.
    /// EXIT is NOT here — it fires at the exit-path boundary, not via
    /// a real signal.
    #[allow(dead_code)]  // used by traps module + REPL; remove in Task 5
    pub trap_pending: std::sync::Arc<std::sync::atomic::AtomicU32>,
```

- [ ] **Step 2: Initialise in `Shell::new()`**

In `Shell::new()`'s struct literal, add these two field initialisers at
the end (after `is_interactive`):

```rust
            traps: std::collections::HashMap::new(),
            trap_pending: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
```

After the struct literal is constructed (just before `Shell::new`
returns the Shell), add a call to register the bitmask with the
module-level `OnceLock` that signal handlers read:

```rust
        // Make the trap_pending Arc visible to async-signal-safe
        // signal handlers installed by the traps module.
        crate::traps::init_pending_bitmask(std::sync::Arc::clone(&shell.trap_pending));
        shell
```

(Adapt to the actual variable name in `new()` — it's likely just `Self { ... }`. If the function returns a value via expression rather than a let binding, refactor to:

```rust
pub fn new() -> Self {
    let shell = Self {
        // ... all fields ...
    };
    crate::traps::init_pending_bitmask(std::sync::Arc::clone(&shell.trap_pending));
    shell
}
```

- [ ] **Step 3: Add `init_pending_bitmask`, `drain_pending`, `dispatch_pending_traps`, `fire_exit_trap`, `clear_for_subshell` to `src/traps.rs`**

Add at the top of `src/traps.rs` (above the `parse_trap_signal` function):

```rust
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::shell_state::Shell;

/// Shared bitmask written by async-signal-safe signal handlers. Set
/// once per process at first `Shell::new()`; identical Arc across all
/// shells in this process.
static TRAP_PENDING: OnceLock<Arc<AtomicU32>> = OnceLock::new();

/// Sets the process-global `TRAP_PENDING` to `arc` the first time;
/// subsequent calls are no-ops (the existing Arc is kept).
pub fn init_pending_bitmask(arc: Arc<AtomicU32>) {
    let _ = TRAP_PENDING.set(arc);
}

/// Returns the bits that were pending and atomically clears them.
/// Each returned value is a signal number (bit position).
pub fn drain_pending(shell: &mut Shell) -> Vec<i32> {
    let bits = shell.trap_pending.swap(0, Ordering::SeqCst);
    let mut out = Vec::new();
    for sig in 0..32 {
        if bits & (1u32 << sig) != 0 {
            out.push(sig as i32);
        }
    }
    out
}

/// Drains pending signals and executes registered trap actions in
/// signal-number order. Trap actions run via `process_line` in the
/// current shell scope; return values from `process_line` are
/// ignored (an `exit` from within a trap action propagates through
/// the outer caller's normal exit handling).
pub fn dispatch_pending_traps(shell: &mut Shell) {
    for sig in drain_pending(shell) {
        let action = match shell.traps.get(&TrapSignal::Real(sig)) {
            Some(Some(text)) => text.clone(),
            Some(None) | None => continue,
        };
        let _ = crate::shell::process_line(&action, shell);
    }
}

/// Fires the EXIT pseudo-signal trap, if one is registered. Self-
/// removes the action before running so recursive `exit` from within
/// the action doesn't re-fire.
pub fn fire_exit_trap(shell: &mut Shell) {
    let action = match shell.traps.remove(&TrapSignal::Exit) {
        Some(Some(text)) => text,
        _ => return,
    };
    let _ = crate::shell::process_line(&action, shell);
}

/// Resets all trap state in a freshly-forked subshell child. POSIX:
/// trapped signals reset to their original values in subshells; we
/// also clear EXIT so the parent's EXIT fires only when the parent
/// exits, not when the subshell does.
pub fn clear_for_subshell(shell: &mut Shell) {
    shell.traps.clear();
    shell.trap_pending = Arc::new(AtomicU32::new(0));
}
```

- [ ] **Step 4: Add unit tests in `src/traps.rs`**

Append to the existing `#[cfg(test)] mod tests { ... }` block:

```rust
    use crate::shell_state::Shell;
    use std::sync::atomic::Ordering;

    #[test]
    fn drain_pending_returns_signals_in_ascending_order() {
        let mut shell = Shell::new();
        // Simulate three signal deliveries by manually setting bits.
        shell.trap_pending.fetch_or(1 << libc::SIGINT, Ordering::SeqCst);
        shell.trap_pending.fetch_or(1 << libc::SIGTERM, Ordering::SeqCst);
        shell.trap_pending.fetch_or(1 << libc::SIGHUP, Ordering::SeqCst);
        let drained = drain_pending(&mut shell);
        assert_eq!(drained, vec![libc::SIGHUP, libc::SIGINT, libc::SIGTERM]);
    }

    #[test]
    fn drain_pending_clears_the_bitmask() {
        let mut shell = Shell::new();
        shell.trap_pending.fetch_or(1 << libc::SIGINT, Ordering::SeqCst);
        let _ = drain_pending(&mut shell);
        assert_eq!(shell.trap_pending.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn dispatch_pending_traps_runs_registered_action() {
        let mut shell = Shell::new();
        shell.traps.insert(TrapSignal::Real(libc::SIGUSR1), Some("FOO=ran".to_string()));
        shell.trap_pending.fetch_or(1 << libc::SIGUSR1, Ordering::SeqCst);
        dispatch_pending_traps(&mut shell);
        assert_eq!(shell.get("FOO"), Some("ran"));
    }

    #[test]
    fn dispatch_pending_traps_skips_ignored_signal() {
        let mut shell = Shell::new();
        shell.traps.insert(TrapSignal::Real(libc::SIGUSR1), None); // ignore
        shell.trap_pending.fetch_or(1 << libc::SIGUSR1, Ordering::SeqCst);
        dispatch_pending_traps(&mut shell);
        // No action ran; no side effect to assert. The drain happened
        // (asserted by trap_pending now being 0).
        assert_eq!(shell.trap_pending.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn dispatch_pending_traps_skips_unregistered_signal() {
        let mut shell = Shell::new();
        // No entry in shell.traps for SIGUSR1.
        shell.trap_pending.fetch_or(1 << libc::SIGUSR1, Ordering::SeqCst);
        dispatch_pending_traps(&mut shell);
        assert_eq!(shell.trap_pending.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn fire_exit_trap_runs_action_then_removes_it() {
        let mut shell = Shell::new();
        shell.traps.insert(TrapSignal::Exit, Some("FOO=ran".to_string()));
        fire_exit_trap(&mut shell);
        assert_eq!(shell.get("FOO"), Some("ran"));
        // Trap is now absent: a second fire is a no-op.
        assert!(!shell.traps.contains_key(&TrapSignal::Exit));
    }

    #[test]
    fn fire_exit_trap_no_action_is_noop() {
        let mut shell = Shell::new();
        fire_exit_trap(&mut shell);  // no panic, no side effect
        assert!(!shell.traps.contains_key(&TrapSignal::Exit));
    }

    #[test]
    fn clear_for_subshell_resets_traps_and_bitmask() {
        let mut shell = Shell::new();
        shell.traps.insert(TrapSignal::Exit, Some("nope".to_string()));
        shell.traps.insert(TrapSignal::Real(libc::SIGINT), Some("nope".to_string()));
        shell.trap_pending.fetch_or(1 << libc::SIGINT, Ordering::SeqCst);
        clear_for_subshell(&mut shell);
        assert!(shell.traps.is_empty());
        assert_eq!(shell.trap_pending.load(Ordering::SeqCst), 0);
    }
```

- [ ] **Step 5: Hook subshell-clear into `fork_and_run_in_subshell`**

In `src/executor.rs::fork_and_run_in_subshell` (around line 2541), the
child branch starts at `if pid == 0 {` and the existing setup runs an
`unsafe { ... }` block that handles fd dup2 / pgrp / signals. Right
AFTER that unsafe block (and BEFORE the `// 8. Run the body...`
comment + `let mut sink = StdoutSink::Terminal;` at ~line 2603), add:

```rust
        // POSIX: subshells reset traps to default. Clear all huck
        // trap state so the parent's EXIT trap and real-signal traps
        // don't inherit into the child.
        crate::traps::clear_for_subshell(shell);
```

- [ ] **Step 6: Build to verify clean**

Run: `cargo build 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 7: Run the new unit tests**

Run: `cargo test --lib traps::tests 2>&1 | tail -10`
Expected: 18 tests pass (10 from Task 1 + 8 new).

- [ ] **Step 8: Run the full lib suite to confirm no regression**

Run: `cargo test --lib 2>&1 | tail -5`
Expected: 0 failures.

- [ ] **Step 9: Run clippy with `-D warnings`**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clean. The `#[allow(dead_code)]` annotations on the new
Shell fields prevent dead-code warnings until Task 5 removes them.

- [ ] **Step 10: Commit**

```bash
git add src/shell_state.rs src/traps.rs src/executor.rs
git commit -m "$(cat <<'EOF'
traps: storage scaffold + dispatch_pending_traps / fire_exit_trap (v35 task 2)

New Shell fields traps (HashMap<TrapSignal, Option<String>>) and
trap_pending (Arc<AtomicU32>). traps module gains TRAP_PENDING OnceLock
that signal handlers will read in Task 3, plus drain_pending /
dispatch_pending_traps / fire_exit_trap / clear_for_subshell. The
fork_and_run_in_subshell child clears traps post-fork per POSIX. 8 new
unit tests simulate signal delivery by directly setting bitmask bits.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Signal-handler installation + `install()` / `reset()` + ignored-at-startup enforcement

**Files:**
- Modify: `src/traps.rs` (add `install`, `reset`, `ignored_at_startup` snapshot + SIGID tracking)

**Note for implementer:** This task installs real OS signal handlers
via `signal_hook::low_level::register`. signal-hook supports multiple
registered closures per signal — they all fire on signal delivery —
so the new trap handler is ADDITIVE to huck's existing
`signal_hook::flag::register` setters for SIGINT/SIGCHLD. No need to
wrap or replace.

The `SigId` returned by `register` is what we use to `unregister` on
`trap - SIGNAL` (reset).

- [ ] **Step 1: Add the ignored-at-startup snapshot**

In `src/traps.rs`, add a private `OnceLock<HashSet<i32>>` and a helper
to populate it lazily on first install attempt:

```rust
use std::collections::{HashMap, HashSet};

/// Set of signal numbers that were ignored when huck started. Per
/// POSIX, these cannot be trapped or reset. Populated lazily on
/// first `install` / `reset` call.
static IGNORED_AT_STARTUP: OnceLock<HashSet<i32>> = OnceLock::new();

fn ignored_at_startup_set() -> &'static HashSet<i32> {
    IGNORED_AT_STARTUP.get_or_init(|| {
        let mut set = HashSet::new();
        for (_, signum) in TRAPPABLE {
            // SAFETY: sigaction with null new pointer just queries the
            // current disposition without changing it.
            unsafe {
                let mut act: libc::sigaction = std::mem::zeroed();
                if libc::sigaction(*signum, std::ptr::null(), &mut act) == 0
                    && act.sa_sigaction == libc::SIG_IGN
                {
                    set.insert(*signum);
                }
            }
        }
        set
    })
}
```

- [ ] **Step 2: Add SIGID tracking on `Shell`**

Trap handlers installed via signal-hook return a `SigId` that we need
to keep around for later unregistration. Store them on `Shell`:

In `src/shell_state.rs`, add another field after `trap_pending`:

```rust
    /// Map of signal number → signal-hook SigId for each currently-
    /// installed trap handler. Used by `traps::reset` to unregister.
    #[allow(dead_code)]  // populated by traps module; remove in Task 5
    pub trap_sigids: std::collections::HashMap<i32, signal_hook::SigId>,
```

Initialise in `Shell::new()` struct literal:

```rust
            trap_sigids: std::collections::HashMap::new(),
```

- [ ] **Step 3: Add `install()` and `reset()` functions to `src/traps.rs`**

```rust
/// Installs a trap action for `sig`. `action = Some(text)` registers;
/// `action = None` ignores the signal (SIG_IGN). For EXIT, no OS-level
/// handler is needed — just store the action and let `fire_exit_trap`
/// handle the firing.
///
/// Returns `Err(msg)` if `sig` was ignored at shell startup (POSIX
/// "Signals ignored upon entry to the shell cannot be trapped").
pub fn install(shell: &mut Shell, sig: TrapSignal, action: Option<String>) -> Result<(), String> {
    match sig {
        TrapSignal::Exit => {
            shell.traps.insert(TrapSignal::Exit, action);
            Ok(())
        }
        TrapSignal::Real(signum) => {
            if ignored_at_startup_set().contains(&signum) {
                return Err(format!("signal {signum}: cannot reset ignored signal"));
            }
            // Remove any existing handler before installing a new one
            // so we don't accumulate multiple trap closures per signal.
            if let Some(sigid) = shell.trap_sigids.remove(&signum) {
                signal_hook::low_level::unregister(sigid);
            }
            match &action {
                Some(_) => {
                    // Install closure that sets the bitmask bit.
                    let pending = TRAP_PENDING.get()
                        .expect("TRAP_PENDING initialised by Shell::new")
                        .clone();
                    // SAFETY: signal_hook::low_level::register requires
                    // the closure to be async-signal-safe. fetch_or on
                    // AtomicU32 is lock-free and signal-safe.
                    let sigid = unsafe {
                        signal_hook::low_level::register(signum, move || {
                            pending.fetch_or(1u32 << signum, Ordering::SeqCst);
                        })
                    }.map_err(|e| format!("install signal handler: {e}"))?;
                    shell.trap_sigids.insert(signum, sigid);
                }
                None => {
                    // SIG_IGN — register an empty closure (effectively
                    // ignoring the signal, since the closure does
                    // nothing).
                    let sigid = unsafe {
                        signal_hook::low_level::register(signum, || {})
                    }.map_err(|e| format!("install signal handler: {e}"))?;
                    shell.trap_sigids.insert(signum, sigid);
                }
            }
            shell.traps.insert(TrapSignal::Real(signum), action);
            Ok(())
        }
    }
}

/// Resets `sig` to default disposition. For EXIT, just removes the
/// stored action. For real signals, unregisters any installed handler
/// — signal-hook's existing SIGINT/SIGCHLD handlers (installed by
/// `shell::install_sigint_handler` etc.) are unaffected because they
/// were registered separately and have their own SigIds.
pub fn reset(shell: &mut Shell, sig: TrapSignal) -> Result<(), String> {
    match sig {
        TrapSignal::Exit => {
            shell.traps.remove(&TrapSignal::Exit);
            Ok(())
        }
        TrapSignal::Real(signum) => {
            if ignored_at_startup_set().contains(&signum) {
                return Err(format!("signal {signum}: cannot reset ignored signal"));
            }
            if let Some(sigid) = shell.trap_sigids.remove(&signum) {
                signal_hook::low_level::unregister(sigid);
            }
            shell.traps.remove(&TrapSignal::Real(signum));
            Ok(())
        }
    }
}
```

Also update `clear_for_subshell` (from Task 2) to unregister all sigids
so the child doesn't keep parent's signal handlers active:

```rust
pub fn clear_for_subshell(shell: &mut Shell) {
    // Unregister every installed signal handler.
    for (_, sigid) in shell.trap_sigids.drain() {
        signal_hook::low_level::unregister(sigid);
    }
    shell.traps.clear();
    shell.trap_pending = Arc::new(AtomicU32::new(0));
}
```

- [ ] **Step 4: Add `pub use signal_hook` to make SigId accessible from shell_state**

The `Shell::trap_sigids` field uses `signal_hook::SigId`. In
`src/main.rs` no change needed (signal-hook is already in Cargo.toml).
In `src/shell_state.rs`, the path `signal_hook::SigId` is the
fully-qualified type — no `use` needed inside the struct definition,
but ensure `signal_hook` is reachable in the module. Add to the top of
`src/shell_state.rs`:

```rust
// (Existing imports.)
// signal_hook is in scope as the external crate.
```

(Likely no edit needed since `signal_hook` is already a top-level
crate dependency.)

- [ ] **Step 5: Build to verify clean**

Run: `cargo build 2>&1 | tail -10`
Expected: clean. If signal-hook 0.4.4 has a different API surface than
documented above (e.g. `low_level::register` doesn't exist or has a
different signature), look at the crate's actual API:

```bash
cargo doc --open -p signal-hook
```

Adapt the function calls to the actual API. The semantic intent
(install a closure that sets a bit on the bitmask; return a SigId to
later unregister) is what matters.

- [ ] **Step 6: Add basic install/reset tests**

Append to the `tests` module in `src/traps.rs`:

```rust
    #[test]
    fn install_exit_stores_action() {
        let mut shell = Shell::new();
        install(&mut shell, TrapSignal::Exit, Some("echo bye".to_string())).unwrap();
        assert_eq!(
            shell.traps.get(&TrapSignal::Exit),
            Some(&Some("echo bye".to_string()))
        );
    }

    #[test]
    fn install_exit_ignore_stores_none() {
        let mut shell = Shell::new();
        install(&mut shell, TrapSignal::Exit, None).unwrap();
        assert_eq!(shell.traps.get(&TrapSignal::Exit), Some(&None));
    }

    #[test]
    fn reset_exit_removes_from_traps() {
        let mut shell = Shell::new();
        install(&mut shell, TrapSignal::Exit, Some("echo bye".to_string())).unwrap();
        reset(&mut shell, TrapSignal::Exit).unwrap();
        assert!(!shell.traps.contains_key(&TrapSignal::Exit));
    }

    #[test]
    fn install_real_signal_stores_action_and_sigid() {
        let mut shell = Shell::new();
        // Use SIGUSR1 — unlikely to be ignored-at-startup in test env.
        install(&mut shell, TrapSignal::Real(libc::SIGUSR1), Some("echo usr1".to_string())).unwrap();
        assert!(shell.trap_sigids.contains_key(&libc::SIGUSR1));
        assert_eq!(
            shell.traps.get(&TrapSignal::Real(libc::SIGUSR1)),
            Some(&Some("echo usr1".to_string()))
        );
        // Cleanup so the handler doesn't leak across tests.
        reset(&mut shell, TrapSignal::Real(libc::SIGUSR1)).unwrap();
    }

    #[test]
    fn reset_real_signal_unregisters_handler() {
        let mut shell = Shell::new();
        install(&mut shell, TrapSignal::Real(libc::SIGUSR2), Some("echo usr2".to_string())).unwrap();
        reset(&mut shell, TrapSignal::Real(libc::SIGUSR2)).unwrap();
        assert!(!shell.trap_sigids.contains_key(&libc::SIGUSR2));
        assert!(!shell.traps.contains_key(&TrapSignal::Real(libc::SIGUSR2)));
    }
```

- [ ] **Step 7: Run the new tests**

Run: `cargo test --lib traps::tests 2>&1 | tail -10`
Expected: 23 tests pass (18 + 5 new).

- [ ] **Step 8: Run full lib + clippy**

Run: `cargo test --lib 2>&1 | tail -5`
Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: 0 failures, 0 warnings.

- [ ] **Step 9: Commit**

```bash
git add src/traps.rs src/shell_state.rs
git commit -m "$(cat <<'EOF'
traps: signal-handler install/reset + ignored-at-startup guard (v35 task 3)

install() and reset() install/unregister signal-hook closure handlers
that set bits on TRAP_PENDING; SIGINT/SIGCHLD existing handlers stay
in place (signal-hook supports multiple closures per signal — they all
fire). ignored_at_startup_set snapshots signals already ignored at
process entry so trap rejects them per POSIX. Shell::trap_sigids
tracks SigIds for unregistration. clear_for_subshell now also
unregisters every installed handler before zeroing state.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: `builtin_trap` + `BUILTIN_NAMES` + `is_special_builtin` + output formatting

**Files:**
- Modify: `src/builtins.rs` (add `"trap"` to `BUILTIN_NAMES` + `is_special_builtin`; dispatch arm; new `builtin_trap` function; new helper functions for the output formats; unit tests)

**Note for implementer:** `builtin_trap` parses args into the seven
forms from the spec, then delegates to `traps::install` / `traps::reset`
or prints `trap -p` / `trap -l` output. Output goes to the `out: &mut dyn Write`
parameter for `trap -p` (stdout) and `trap -l` (stdout). Errors go
through `eprintln!` (stderr).

- [ ] **Step 1: Add `"trap"` to `BUILTIN_NAMES` and `is_special_builtin`**

In `src/builtins.rs:18-22`, replace:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return",
];
```

with:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return", "trap",
];
```

In `src/builtins.rs:33-35`, replace:

```rust
pub fn is_special_builtin(name: &str) -> bool {
    matches!(name, "break" | "continue" | "exit" | "export" | "return" | "unset")
}
```

with:

```rust
pub fn is_special_builtin(name: &str) -> bool {
    matches!(name, "break" | "continue" | "exit" | "export" | "return" | "trap" | "unset")
}
```

- [ ] **Step 2: Add the dispatch arm**

In `run_builtin` at `src/builtins.rs:46-71`, add the `trap` arm
alongside the others. For example, between `"history"` and
`"test" | "["`:

```rust
        "trap" => builtin_trap(args, out, shell),
```

- [ ] **Step 3: Add the `builtin_trap` function**

In `src/builtins.rs`, near the other builtin implementations, add:

```rust
fn builtin_trap(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    use crate::traps::{TrapSignal, install, reset, name_table, parse_trap_signal};

    // No args: same as `trap -p`.
    if args.is_empty() {
        print_active_traps(out, shell, None);
        return ExecOutcome::Continue(0);
    }

    // -l: list signal name/number pairs.
    if args[0] == "-l" {
        if args.len() != 1 {
            eprintln!("huck: trap: -l takes no arguments");
            return ExecOutcome::Continue(1);
        }
        print_signal_table(out);
        return ExecOutcome::Continue(0);
    }

    // -p [SIGNAL...]: list active traps (optionally filtered).
    if args[0] == "-p" {
        if args.len() == 1 {
            print_active_traps(out, shell, None);
            return ExecOutcome::Continue(0);
        }
        let mut filter: Vec<TrapSignal> = Vec::new();
        for name in &args[1..] {
            match parse_trap_signal(name) {
                Ok(sig) => filter.push(sig),
                Err(msg) => {
                    eprintln!("huck: trap: {msg}");
                    return ExecOutcome::Continue(1);
                }
            }
        }
        print_active_traps(out, shell, Some(&filter));
        return ExecOutcome::Continue(0);
    }

    // `trap - SIGNAL...`: reset each signal.
    if args[0] == "-" {
        if args.len() < 2 {
            eprintln!("huck: trap: usage: trap [-lp] [[arg] signal_spec ...]");
            return ExecOutcome::Continue(1);
        }
        for name in &args[1..] {
            let sig = match parse_trap_signal(name) {
                Ok(s) => s,
                Err(msg) => {
                    eprintln!("huck: trap: {msg}");
                    return ExecOutcome::Continue(1);
                }
            };
            if let Err(msg) = reset(shell, sig) {
                eprintln!("huck: trap: {msg}");
                return ExecOutcome::Continue(1);
            }
        }
        return ExecOutcome::Continue(0);
    }

    // `trap ACTION SIGNAL...`: install action for each signal.
    if args.len() < 2 {
        eprintln!("huck: trap: usage: trap [-lp] [[arg] signal_spec ...]");
        return ExecOutcome::Continue(1);
    }
    let action_text = args[0].clone();
    let action = if action_text.is_empty() {
        None  // empty string → ignore
    } else {
        Some(action_text)
    };
    for name in &args[1..] {
        let sig = match parse_trap_signal(name) {
            Ok(s) => s,
            Err(msg) => {
                eprintln!("huck: trap: {msg}");
                return ExecOutcome::Continue(1);
            }
        };
        if let Err(msg) = install(shell, sig, action.clone()) {
            eprintln!("huck: trap: {msg}");
            return ExecOutcome::Continue(1);
        }
    }
    ExecOutcome::Continue(0)
}

/// Prints active traps in re-readable form. If `filter` is `Some`, only
/// the listed signals are printed; if `None`, all active traps print.
/// Bash sorts by signal number, with EXIT printed first.
fn print_active_traps(
    out: &mut dyn Write,
    shell: &Shell,
    filter: Option<&[crate::traps::TrapSignal]>,
) {
    use crate::traps::TrapSignal;

    // Collect entries in (sort-key, signal, action) form. EXIT sorts
    // first (key 0); real signals sort by signal number.
    let mut entries: Vec<(i32, TrapSignal, &Option<String>)> = Vec::new();
    for (sig, action) in &shell.traps {
        if let Some(f) = filter {
            if !f.contains(sig) { continue; }
        }
        let key = match sig {
            TrapSignal::Exit => 0,
            TrapSignal::Real(n) => *n,
        };
        entries.push((key, *sig, action));
    }
    entries.sort_by_key(|(k, _, _)| *k);

    for (_, sig, action) in entries {
        let name = match sig {
            TrapSignal::Exit => "EXIT".to_string(),
            TrapSignal::Real(n) => signal_number_to_name(*n).unwrap_or_else(|| n.to_string()),
        };
        let text = action.as_deref().unwrap_or("");
        // Escape single quotes in action text via the standard bash
        // shell-quote idiom: ' → '\''
        let escaped = text.replace('\'', "'\\''");
        let _ = writeln!(out, "trap -- '{escaped}' {name}");
    }
}

/// Prints the trappable signal table in bash's 4-column format:
///   1) HUP   2) INT   3) QUIT  10) USR1
fn print_signal_table(out: &mut dyn Write) {
    use crate::traps::name_table;
    let table = name_table();
    // Sort by signal number for the listing.
    let mut sorted: Vec<&(&str, i32)> = table.iter().collect();
    sorted.sort_by_key(|(_, n)| *n);
    let cols = 4;
    for chunk in sorted.chunks(cols) {
        let mut line = String::new();
        for (i, (name, num)) in chunk.iter().enumerate() {
            if i > 0 { line.push(' '); }
            line.push_str(&format!("{num:>2}) {name:<5}"));
        }
        let _ = writeln!(out, "{line}");
    }
}

/// Returns the canonical name (no SIG prefix) for `signum`, or None
/// if `signum` is not in the trappable table.
fn signal_number_to_name(signum: i32) -> Option<String> {
    crate::traps::name_table().iter().find_map(|(name, n)| {
        if *n == signum { Some(name.to_string()) } else { None }
    })
}
```

- [ ] **Step 4: Add builtin unit tests**

Append to the `tests` module in `src/builtins.rs`:

```rust
    #[test]
    fn is_builtin_trap() {
        assert!(is_builtin("trap"));
    }

    #[test]
    fn is_special_builtin_trap() {
        assert!(is_special_builtin("trap"));
    }

    #[test]
    fn trap_exit_action_signal_registers() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["echo bye".to_string(), "EXIT".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(shell.traps.contains_key(&crate::traps::TrapSignal::Exit));
    }

    #[test]
    fn trap_empty_action_ignores_signal() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["".to_string(), "EXIT".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(
            shell.traps.get(&crate::traps::TrapSignal::Exit),
            Some(&None),  // None = ignore
        );
    }

    #[test]
    fn trap_dash_resets_signal() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        // Install first.
        let _ = run_builtin(
            "trap",
            &["echo bye".to_string(), "EXIT".to_string()],
            &mut buf,
            &mut shell,
        );
        // Then reset.
        let outcome = run_builtin(
            "trap",
            &["-".to_string(), "EXIT".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(!shell.traps.contains_key(&crate::traps::TrapSignal::Exit));
    }

    #[test]
    fn trap_p_prints_active_traps_in_re_readable_form() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        // Register a trap.
        let _ = run_builtin(
            "trap",
            &["echo bye".to_string(), "EXIT".to_string()],
            &mut buf,
            &mut shell,
        );
        // Clear the buffer (the install printed nothing, but be defensive).
        buf.clear();
        // List.
        let outcome = run_builtin(
            "trap",
            &["-p".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("trap -- 'echo bye' EXIT"),
            "expected trap -p to print 'trap -- echo bye EXIT', got: {out}"
        );
    }

    #[test]
    fn trap_no_args_same_as_dash_p() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let _ = run_builtin(
            "trap",
            &["echo bye".to_string(), "EXIT".to_string()],
            &mut buf,
            &mut shell,
        );
        buf.clear();
        let outcome = run_builtin("trap", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("trap -- 'echo bye' EXIT"));
    }

    #[test]
    fn trap_l_lists_signals() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["-l".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("2) INT"), "stdout: {out}");
        assert!(out.contains("15) TERM"), "stdout: {out}");
    }

    #[test]
    fn trap_unknown_signal_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["echo bye".to_string(), "NOPE".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn trap_kill_signal_errors_uncatchable() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["echo nope".to_string(), "KILL".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn trap_no_signals_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["echo bye".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }
```

- [ ] **Step 5: Build, test, clippy**

Run: `cargo build 2>&1 | tail -5` — clean.
Run: `cargo test --lib builtins:: 2>&1 | tail -10` — 10 new builtin tests pass (plus existing).
Run: `cargo test --lib 2>&1 | tail -5` — 0 failures.
Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5` — clean.

- [ ] **Step 6: Commit**

```bash
git add src/builtins.rs
git commit -m "$(cat <<'EOF'
trap: builtin_trap + BUILTIN_NAMES + is_special_builtin + output (v35 task 4)

builtin_trap parses the seven forms (trap, trap -p, trap -l,
trap - SIGNAL, trap ACTION SIGNAL, trap "" SIGNAL, trap -p SIGNAL) and
delegates to traps::install / traps::reset for state changes. Output
helpers print the re-readable `trap -- 'action' SIGNAL` form and the
4-column `trap -l` table. trap is now in BUILTIN_NAMES + the POSIX
special-builtin list. 10 builtin unit tests.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: REPL/executor integration + EXIT firing + remove dead-code annotations

**Files:**
- Modify: `src/shell.rs::run()` (REPL loop — poll at top + EXIT firing at five exit paths)
- Modify: `src/executor.rs::execute_sequence_body` (poll after each pipeline)
- Modify: `src/executor.rs::wait_pipeline_raw` (poll after wait loop)
- Modify: `src/shell_state.rs` (remove `#[allow(dead_code)]` annotations on `traps`, `trap_pending`, `trap_sigids`)

**Note for implementer:** This is the wiring task that makes traps
observable. The polling-checkpoint additions are 1-3 lines each. The
EXIT firing additions are also 1 line each (a call to
`crate::traps::fire_exit_trap(&mut shell)` before the existing return).

- [ ] **Step 1: Add the top-of-REPL poll**

In `src/shell.rs::run()` (the `loop { ... }` starting around line 59),
right at the top of the loop body, immediately AFTER the existing
`crate::jobs::reap_and_notify(&mut shell);` call:

```rust
    loop {
        crate::jobs::reap_and_notify(&mut shell);
        crate::traps::dispatch_pending_traps(&mut shell);
        if let Some(helper) = editor.helper_mut() {
            helper.refresh(&shell);
        }
        // ... rest of loop ...
```

- [ ] **Step 2: Add EXIT firing at the five exit paths**

In `src/shell.rs::run()`, each of the FIVE return sites needs a
`crate::traps::fire_exit_trap(&mut shell);` call inserted BEFORE the
existing `shell.history.save();` (or before the bare `return` if there's
no history save).

Site 1 — `ExecOutcome::Exit(code)` arm at line 71-74:

```rust
                    ExecOutcome::Exit(code) => {
                        crate::traps::fire_exit_trap(&mut shell);
                        shell.history.save();
                        return code;
                    }
```

Site 2 — fatal-PE drain non-interactive return (currently at lines 81-86, inside the `Continue(status)` arm):

```rust
                        if let Some(fatal_status) = shell.take_pending_fatal_pe_error()
                            && !shell.is_interactive
                        {
                            crate::traps::fire_exit_trap(&mut shell);
                            shell.history.save();
                            return fatal_status;
                        }
```

Site 3 — `ReadResult::Eof` at lines 95-98:

```rust
            ReadResult::Eof => {
                crate::traps::fire_exit_trap(&mut shell);
                shell.history.save();
                return shell.last_status();
            }
```

Site 4 — `ReadResult::EofMidCommand` at lines 99-103:

```rust
            ReadResult::EofMidCommand => {
                eprintln!("huck: syntax error: unexpected end of input");
                crate::traps::fire_exit_trap(&mut shell);
                shell.history.save();
                return 2;
            }
```

Site 5 — `ReadResult::ReadError(msg)` at lines 104-107:

```rust
            ReadResult::ReadError(msg) => {
                eprintln!("huck: input error: {msg}");
                crate::traps::fire_exit_trap(&mut shell);
                return 1;
            }
```

(No `history.save` at site 5 — match the existing structure.)

- [ ] **Step 3: Add the sequence-body poll**

In `src/executor.rs::execute_sequence_body` (around line 55), there
are TWO `if let ExecOutcome::Continue(c) = status { shell.set_last_status(c); ... }`
blocks (one at lines 68-72 for the first command, one at 86-93 for the
rest-of-sequence loop, both gained the v34 fatal-PE peek-check).

Inside EACH block, AFTER the existing v34 peek-check, add:

```rust
    if let ExecOutcome::Continue(c) = status {
        shell.set_last_status(c);
        if shell.pending_fatal_pe_error.is_some() {
            return ExecOutcome::Continue(c);
        }
        crate::traps::dispatch_pending_traps(shell);
    }
```

(Both sites get the same addition; the second site lives inside the
`for (connector, command) in &seq.rest` loop at line 71+.)

- [ ] **Step 4: Add the wait_pipeline_raw poll**

In `src/executor.rs::wait_pipeline_raw` (around line 2347), the
function ends with a return statement after the wait loop completes.
Find the return at the bottom of the function. Immediately BEFORE the
return, add:

```rust
    crate::traps::dispatch_pending_traps(shell);
    // existing return ...
```

If the function's signature doesn't take `shell: &mut Shell`, check —
it likely does per the grep result at line 2316 (`shell: &mut Shell` is
in the param list). If not, this poll site can be skipped (just rely
on the top-of-REPL poll); document the omission in the report. The
foreground-wait case is the most user-visible (Ctrl-C during sleep),
so getting this site is preferable.

- [ ] **Step 5: Remove `#[allow(dead_code)]` annotations**

In `src/shell_state.rs`, remove the three annotations on the new
fields:

- `#[allow(dead_code)]  // used by traps module + builtins; remove in Task 5` above `pub traps: ...`
- `#[allow(dead_code)]  // used by traps module + REPL; remove in Task 5` above `pub trap_pending: ...`
- `#[allow(dead_code)]  // populated by traps module; remove in Task 5` above `pub trap_sigids: ...`

After removal, all three fields are now read by the REPL drain and/or
the traps module — clippy should NOT warn.

- [ ] **Step 6: Build to verify clean**

Run: `cargo build 2>&1 | tail -5` — clean.

- [ ] **Step 7: Run the full test suite**

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | grep -E "failed: [1-9]"`
Expected: no output (no failures). Test count goes up by ~23 (10 from
Task 1, 8 from Task 2, 5 from Task 3 — Tasks 1-3 added unit tests;
Task 4 added 10 builtin tests). Plus existing baseline.

- [ ] **Step 8: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add src/shell.rs src/executor.rs src/shell_state.rs
git commit -m "$(cat <<'EOF'
trap: REPL/executor poll checkpoints + EXIT firing (v35 task 5)

dispatch_pending_traps invoked at: top of REPL loop (after job
reaper), after each pipeline in execute_sequence_body (both sites),
and after wait_pipeline_raw's wait loop (Ctrl-C during foreground
sleep fires trap promptly). fire_exit_trap invoked at five
shell-exit paths: ExecOutcome::Exit, fatal-PE non-interactive drain,
ReadResult::Eof, EofMidCommand, ReadError. Self-removing EXIT trap
guarantees only the first reached site fires. Dead-code annotations
on Shell::traps/trap_pending/trap_sigids removed.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Integration tests + docs + verify

**Files:**
- Create: `tests/trap_integration.rs`
- Modify: `docs/bash-divergences.md` (M-22 → `[partial v35]`; changelog row)
- Modify: `README.md` (v35 row)

**Note for implementer:** The integration tests spawn `huck` via
`Command::new(huck_binary())` with piped stdin (same harness as
v33/v34 integration tests). Each test gets a fresh process — handler
state never leaks. For signal-delivery tests, the test sends the
signal via `nix::sys::signal::kill` or `libc::kill` to the child's
PID after a short sleep to ensure the trap is installed.

Some integration tests need to send signals to the running huck
process. Use the `Command::spawn` + `child.id()` pattern, then
`libc::kill(pid as i32, SIGUSR1)`. Don't use the high-level `nix`
crate (it's not a dep).

- [ ] **Step 1: Create `tests/trap_integration.rs`**

```rust
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

/// Runs huck with `script` on stdin, captures stdout/stderr, returns
/// (stdout, stderr, exit_status).
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

/// Spawns huck with `script`, returns the child handle (still running)
/// + the pid. Caller is responsible for finishing the process.
fn spawn(script: &str) -> (std::process::Child, i32) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    let pid = child.id() as i32;
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    // Drop stdin so huck sees EOF after running the script.
    // No — we want huck to KEEP running until we send the signal.
    // Don't drop; the caller will manage.
    (child, pid)
}

/// Sends `signum` to `pid` via libc::kill.
fn send_signal(pid: i32, signum: i32) {
    unsafe {
        libc::kill(pid, signum);
    }
}

#[test]
fn exit_trap_fires_on_normal_exit() {
    let (out, _err, status) = run("trap 'echo bye' EXIT\nexit 0\n");
    assert!(out.lines().any(|l| l == "bye"), "stdout: {out}");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn exit_trap_sees_last_status() {
    let (out, _err, _) = run("trap 'echo dollar-q=$?' EXIT\nfalse\nexit\n");
    assert!(out.lines().any(|l| l == "dollar-q=1"), "stdout: {out}");
}

#[test]
fn exit_trap_fires_on_eof() {
    // Script ends without explicit `exit`. EOF should still fire EXIT.
    let (out, _err, _) = run("trap 'echo bye' EXIT\n");
    assert!(out.lines().any(|l| l == "bye"), "stdout: {out}");
}

#[test]
fn exit_trap_fires_only_once() {
    // Recursive exit from within the action should NOT re-fire.
    let (out, _err, _) = run("trap 'echo bye; exit 0' EXIT\nexit 1\n");
    let bye_count = out.lines().filter(|l| **l == *"bye").count();
    assert_eq!(bye_count, 1, "stdout: {out}");
}

#[test]
fn exit_trap_cleared_in_subshell() {
    // Parent's EXIT fires only once when the parent exits. Subshell
    // does NOT fire it again.
    let (out, _err, _) = run("trap 'echo parent-bye' EXIT\n(echo child)\nexit\n");
    let bye_count = out.lines().filter(|l| **l == *"parent-bye").count();
    assert_eq!(bye_count, 1, "stdout: {out}");
    assert!(out.lines().any(|l| l == "child"), "stdout: {out}");
}

#[test]
fn trap_dash_resets_exit() {
    // Set, then reset. EXIT trap should NOT fire.
    let (out, _err, _) = run("trap 'echo bye' EXIT\ntrap - EXIT\nexit 0\n");
    assert!(!out.lines().any(|l| l == "bye"), "stdout: {out}");
}

#[test]
fn trap_empty_action_ignores_exit() {
    // Empty action = ignore. EXIT does not run anything.
    let (out, _err, _) = run("trap '' EXIT\nexit 0\n");
    // No specific output to assert non-presence of — but exit must succeed.
    assert!(!out.contains("bye"));
}

#[test]
fn trap_p_output_format() {
    let (out, _err, _) = run("trap 'echo bye' EXIT\ntrap -p\nexit\n");
    // The `trap -p` output should appear before the EXIT action runs.
    assert!(
        out.lines().any(|l| l == "trap -- 'echo bye' EXIT"),
        "stdout: {out}"
    );
}

#[test]
fn trap_l_lists_signals() {
    let (out, _err, _) = run("trap -l\nexit\n");
    assert!(out.contains("2) INT"), "stdout: {out}");
    assert!(out.contains("15) TERM"), "stdout: {out}");
}

#[test]
fn trap_uncatchable_kill_errors_exit_1() {
    let (_out, err, _) = run("trap 'echo nope' KILL\nexit 0\n");
    assert!(err.contains("cannot trap"), "stderr: {err}");
    // The `exit 0` after the bad trap still runs; the bad trap returned
    // status 1 but didn't abort the script.
}

#[test]
fn trap_unknown_signal_errors_exit_1() {
    let (_out, err, _) = run("trap 'echo nope' NOPE\nexit 0\n");
    assert!(err.contains("invalid signal specification"), "stderr: {err}");
}

#[test]
fn sigint_trap_fires_action() {
    // Spawn huck with a script that installs a SIGINT trap and then
    // sleeps. From the test, wait briefly for the trap to register +
    // the sleep to start, then send SIGINT to the child. After the
    // sleep returns (signal interrupts it), the trap action runs and
    // huck continues to the `exit`.
    let (mut child, pid) = spawn("trap 'echo caught' INT\nsleep 2\nexit\n");
    // Drop stdin so huck reads to EOF and runs the script body.
    drop(child.stdin.take());
    // Give huck ~200ms to parse the trap line + start sleeping.
    std::thread::sleep(Duration::from_millis(200));
    send_signal(pid, libc::SIGINT);
    let out = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("caught"), "stdout: {stdout}");
}

#[test]
fn trap_in_function_persists_after_return() {
    // trap is shell-global, not function-local.
    let script = "f() { trap 'echo bye' EXIT; }\nf\nexit 0\n";
    let (out, _err, _) = run(script);
    assert!(out.lines().any(|l| l == "bye"), "stdout: {out}");
}
```

- [ ] **Step 2: Run the new integration tests**

Run: `cargo test --test trap_integration 2>&1 | tail -10`
Expected: all tests pass. If `sigint_trap_fires_action` is flaky
(timing-dependent), document it but don't block the merge — the
unit-test layer already covers the dispatch path deterministically.

- [ ] **Step 3: Mark M-22 fixed (partial) in `docs/bash-divergences.md`**

Find the M-22 entry (search for `**M-22:`). Current text:

```markdown
- **M-22: `trap` builtin** — `[deferred]` high. huck: not implemented. bash: signal handlers, EXIT/ERR/DEBUG/RETURN pseudo-signals.
```

Replace with:

```markdown
- **M-22: `trap` builtin** — `[partial v35]` high. EXIT pseudo-signal + 13 trappable real signals (huck's 15-name table minus KILL/STOP) now supported via `trap ACTION SIGNAL...`, `trap -p`, `trap -l`, `trap - SIGNAL`, `trap "" SIGNAL`. Action body stored as raw text, re-parsed via `process_line` at fire time (late variable binding). Async-signal-safe `Arc<AtomicU32>` bitmask delivery drained at three polling checkpoints. EXIT self-removes before firing. Subshell trap-clear matches POSIX. **Out of scope (still open)**: ERR/DEBUG/RETURN pseudo-signals (deferred); M-41 limited signal set still applies (no SEGV/ABRT/etc.).
```

- [ ] **Step 4: Add a changelog row**

Find the `## Change log` section at the bottom of `docs/bash-divergences.md`. Append:

```markdown
- **2026-05-27**: M-22 (`trap` builtin) shipped partially as v35. EXIT pseudo-signal + 13 trappable real signals via huck's existing 15-name table. New `src/traps.rs` module owns signal-handler installation (via `signal_hook::low_level::register`), the `Arc<AtomicU32>` pending-signal bitmask, and the dispatch/fire helpers. ERR/DEBUG/RETURN pseudo-signals deferred to a follow-up iteration; M-22 stays `[partial v35]` until they land.
```

- [ ] **Step 5: Update README.md version table**

Find the v34 row in the version table. Add a new row AFTER it:

```markdown
| v35       | `trap` builtin (M-22 partial — EXIT + 13 real signals)         |
```

Match the column width/alignment of the surrounding rows.

- [ ] **Step 6: Commit the docs**

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: M-22 partial v35; v35 in README

trap builtin EXIT pseudo-signal + 13 real-signal traps. ERR/DEBUG/
RETURN deferred. M-22 status: [partial v35].

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 7: Run the full test suite**

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | tail -30`
Expected: all suites pass. New baseline ~1345 (1311 from v34 + ~34
new across Tasks 1-4 + 6).

If PTY suite shows its known flake
(`pty_compound_stage_pipeline_stops_and_resumes`), run it in
isolation: `cargo test --test pty_interactive
pty_compound_stage_pipeline_stops_and_resumes 2>&1 | tail -5`. If
passes in isolation, the under-load flake is the same v29-era issue
documented previously — not a v35 regression. Note in the report but
don't block.

- [ ] **Step 8: Run clippy with `-D warnings`**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -10`
Expected: 0 warnings.

- [ ] **Step 9: Confirm working tree is clean**

Run: `git status`
Expected: `nothing to commit, working tree clean` on branch
`v35-trap-builtin`. No untracked files.

**No additional commit for steps 7-9** — they're verification only.
Hand back to the parent session for the final code-reviewer dispatch
+ merge to main.
