//! Trap handler storage, signal-name parsing, and signal-delivery
//! plumbing for the `trap` builtin (huck v35).

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
#[allow(dead_code)]  // called by dispatch_pending_traps + tests; wired to REPL in Task 5
pub fn drain_pending(shell: &mut Shell) -> Vec<i32> {
    let bits = shell.trap_pending.swap(0, Ordering::SeqCst);
    let mut out = Vec::new();
    for sig in 0u32..32u32 {
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
#[allow(dead_code)]  // called by REPL + executor in Task 5
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
#[allow(dead_code)]  // called at shell exit paths in Task 5
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(dead_code)]  // used by traps module + builtins; remove in Task 5
pub enum TrapSignal {
    Exit,
    Real(i32),
}

/// Trappable real signals — huck's existing 15-name table from `kill`,
/// minus KILL (9) and STOP (19) which POSIX says cannot be trapped.
/// Each entry: (name without SIG prefix, libc signal number).
#[allow(dead_code)]  // used by parse_trap_signal
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
#[allow(dead_code)]  // used by builtins; remove in Task 5
pub fn name_table() -> &'static [(&'static str, i32)] {
    TRAPPABLE
}

/// Parses `name` as a signal specification. Accepts:
/// - `"EXIT"` → `TrapSignal::Exit`
/// - `"INT"` / `"SIGINT"` / `"2"` → `TrapSignal::Real(2)`
/// - Same dual-form for every trappable signal.
///
/// Returns an error for `KILL`/`STOP`/unknown names/non-trappable numbers.
#[allow(dead_code)]  // used by builtins + tests; remove in Task 5
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
