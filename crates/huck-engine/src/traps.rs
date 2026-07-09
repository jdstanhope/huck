//! Trap handler storage, signal-name parsing, and signal-delivery
//! plumbing for the `trap` builtin (huck v35).

use std::collections::HashSet;
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

/// Set of signal numbers that were ignored when huck started. Per
/// POSIX, these cannot be trapped or reset. Populated lazily on
/// first `install` / `reset` call.
static IGNORED_AT_STARTUP: OnceLock<HashSet<i32>> = OnceLock::new();

fn ignored_at_startup_set() -> &'static HashSet<i32> {
    IGNORED_AT_STARTUP.get_or_init(|| {
        let mut set = HashSet::new();
        for (_, signum) in name_table() {
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

/// Returns the bits that were pending and atomically clears them.
/// Each returned value is a signal number (bit position).
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
pub fn dispatch_pending_traps(shell: &mut Shell) {
    for sig in drain_pending(shell) {
        let action = match shell.traps.get(&TrapSignal::Real(sig)) {
            Some(Some(text)) => text.clone(),
            Some(None) | None => continue,
        };
        let _ = crate::shell::process_line(&action, shell, false);
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
    let _ = crate::shell::process_line(&action, shell, false);
}

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
    let _ = crate::shell::process_line(&action, shell, false);
    shell.firing_trap = prev;
}

/// Resets all trap state in a freshly-forked subshell child. POSIX:
/// trapped signals reset to their original values in subshells; we
/// also clear EXIT so the parent's EXIT fires only when the parent
/// exits, not when the subshell does.
pub fn clear_for_subshell(shell: &mut Shell) {
    // Unregister every installed signal handler before clearing.
    for (_, sigid) in shell.trap_sigids.drain() {
        signal_hook::low_level::unregister(sigid);
    }
    shell.traps.clear();
    shell.trap_pending = Arc::new(AtomicU32::new(0));
    shell.firing_trap = None;
    shell.err_suppressed_depth = 0;
}

/// Installs a trap action for `sig`. `action = Some(text)` registers;
/// `action = None` ignores the signal (SIG_IGN). For pseudo-signals, no OS-level
/// handler is needed — just store the action and let the firing helpers
/// handle the firing.
///
/// Returns `Err(msg)` if `sig` was ignored at shell startup (POSIX
/// "Signals ignored upon entry to the shell cannot be trapped").
pub fn install(shell: &mut Shell, sig: TrapSignal, action: Option<String>) -> Result<(), String> {
    match sig {
        TrapSignal::Exit | TrapSignal::Err | TrapSignal::Debug | TrapSignal::Return => {
            shell.traps.insert(sig, action);
            Ok(())
        }
        TrapSignal::Real(signum) => {
            if ignored_at_startup_set().contains(&signum) {
                return Err(format!("signal {signum}: cannot reset ignored signal"));
            }
            // SIGKILL/SIGSTOP cannot be caught (sigaction returns EINVAL), so
            // like bash we only STORE the disposition (so `trap -p` lists it)
            // and never register an OS handler — the signal keeps its default
            // action.
            if signum == libc::SIGKILL || signum == libc::SIGSTOP {
                shell.traps.insert(TrapSignal::Real(signum), action);
                return Ok(());
            }
            // Remove any existing handler before installing a new one
            // so we don't accumulate multiple trap closures per signal.
            if let Some(sigid) = shell.trap_sigids.remove(&signum) {
                signal_hook::low_level::unregister(sigid);
            }
            let sigid = match &action {
                Some(_) => {
                    // Install closure that sets the bitmask bit.
                    let pending = TRAP_PENDING
                        .get()
                        .expect("TRAP_PENDING initialised by Shell::new")
                        .clone();
                    // SAFETY: the registered closure must be async-signal-safe.
                    // It performs a single lock-free `AtomicU32::fetch_or` — the
                    // only async-signal-safe operation it does — so it is sound.
                    // We use `register_unchecked` (not `register`) to bypass
                    // signal-hook's FORBIDDEN-list assert ([SIGILL, SIGFPE,
                    // SIGSEGV], plus KILL/STOP). That guard exists to stop
                    // arbitrary handlers that might call unsafe libc routines;
                    // ours doesn't, and bypassing it is required so huck can trap
                    // SEGV/FPE/ILL like bash (firing on e.g. `kill -SEGV`).
                    // `register_unchecked`'s action takes a `&siginfo_t` we ignore.
                    unsafe {
                        signal_hook_registry::register_unchecked(signum, move |_: &_| {
                            pending.fetch_or(1u32 << signum, Ordering::SeqCst);
                        })
                    }
                    .map_err(|e| format!("install signal handler: {e}"))?
                }
                None => {
                    // `trap "" SIGNAL` (ignore form): register an empty closure that
                    // does nothing when the signal fires. NOTE: this is NOT the same
                    // as POSIX SIG_IGN — custom handlers are reset to SIG_DFL at exec,
                    // so `trap '' PIPE; cmd | head` does NOT propagate SIG_IGN to the
                    // child `cmd` the way bash does. Tracked as a known gap in M-22's
                    // "Out of scope" note in docs/bash-divergences.md. True SIG_IGN
                    // would require libc::sigaction with SIG_IGN action + tracking
                    // trap-installed-ignores separately from startup-ignored signals.
                    // SAFETY: empty handler is trivially async-signal-safe;
                    // `register_unchecked` bypasses the FORBIDDEN-list assert (see
                    // the action-form comment above) so SEGV/FPE/ILL can be set to
                    // the ignore form without panicking. The action takes a
                    // `&siginfo_t` we ignore.
                    unsafe { signal_hook_registry::register_unchecked(signum, move |_: &_| {}) }
                        .map_err(|e| format!("install signal handler: {e}"))?
                }
            };
            shell.trap_sigids.insert(signum, sigid);
            shell.traps.insert(TrapSignal::Real(signum), action);
            Ok(())
        }
    }
}

/// Resets `sig` to default disposition. For pseudo-signals, just removes the
/// stored action. For real signals, unregisters any installed handler
/// — signal-hook's existing SIGINT/SIGCHLD handlers (installed by
/// `shell::install_sigint_handler` etc.) are unaffected because they
/// were registered separately and have their own SigIds.
pub fn reset(shell: &mut Shell, sig: TrapSignal) -> Result<(), String> {
    match sig {
        TrapSignal::Exit | TrapSignal::Err | TrapSignal::Debug | TrapSignal::Return => {
            shell.traps.remove(&sig);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrapSignal {
    Exit,
    Err,
    Debug,
    Return,
    Real(i32),
}

/// Every standard (non-real-time) signal this platform names, as
/// (name without SIG prefix, libc number). Built from libc constants so numbers
/// are correct per platform; platform-specific signals are cfg-gated so the
/// crate builds on macOS as well as Linux. Real-time signals (SIGRTMIN..) are
/// intentionally excluded — the trap pending bitmask is an AtomicU32 (bits
/// 1..=31 only).
fn standard_signals() -> Vec<(&'static str, i32)> {
    let mut v = vec![
        ("HUP", libc::SIGHUP),
        ("INT", libc::SIGINT),
        ("QUIT", libc::SIGQUIT),
        ("ILL", libc::SIGILL),
        ("TRAP", libc::SIGTRAP),
        ("ABRT", libc::SIGABRT),
        ("BUS", libc::SIGBUS),
        ("FPE", libc::SIGFPE),
        ("KILL", libc::SIGKILL),
        ("USR1", libc::SIGUSR1),
        ("SEGV", libc::SIGSEGV),
        ("USR2", libc::SIGUSR2),
        ("PIPE", libc::SIGPIPE),
        ("ALRM", libc::SIGALRM),
        ("TERM", libc::SIGTERM),
        ("CHLD", libc::SIGCHLD),
        ("CONT", libc::SIGCONT),
        ("STOP", libc::SIGSTOP),
        ("TSTP", libc::SIGTSTP),
        ("TTIN", libc::SIGTTIN),
        ("TTOU", libc::SIGTTOU),
        ("URG", libc::SIGURG),
        ("XCPU", libc::SIGXCPU),
        ("XFSZ", libc::SIGXFSZ),
        ("VTALRM", libc::SIGVTALRM),
        ("PROF", libc::SIGPROF),
        ("WINCH", libc::SIGWINCH),
        ("IO", libc::SIGIO),
        ("SYS", libc::SIGSYS),
    ];
    #[cfg(target_os = "linux")]
    {
        v.push(("STKFLT", libc::SIGSTKFLT));
        v.push(("PWR", libc::SIGPWR));
    }
    #[cfg(any(target_os = "macos", target_os = "freebsd", target_os = "openbsd"))]
    {
        v.push(("EMT", libc::SIGEMT));
        v.push(("INFO", libc::SIGINFO));
    }
    v
}

static FULL_TABLE: OnceLock<Vec<(&'static str, i32)>> = OnceLock::new();
static TRAPPABLE_VIEW: OnceLock<Vec<(&'static str, i32)>> = OnceLock::new();

/// Returns the trappable signal table (name → number) — every standard signal
/// except KILL and STOP (which POSIX says cannot be trapped).
pub fn name_table() -> &'static [(&'static str, i32)] {
    TRAPPABLE_VIEW
        .get_or_init(|| {
            killable_signals()
                .iter()
                .copied()
                .filter(|(_, n)| *n != libc::SIGKILL && *n != libc::SIGSTOP)
                .collect()
        })
        .as_slice()
}

/// Returns every signal huck can SEND via `kill` (the full standard set,
/// including KILL and STOP). Used by `kill` send + `kill -l` number↔name.
pub fn killable_signals() -> &'static [(&'static str, i32)] {
    FULL_TABLE.get_or_init(standard_signals).as_slice()
}

/// Parses `name` as a signal specification. Accepts:
/// - `"EXIT"` → `TrapSignal::Exit`
/// - `"ERR"` → `TrapSignal::Err`
/// - `"DEBUG"` → `TrapSignal::Debug`
/// - `"RETURN"` → `TrapSignal::Return`
/// - `"INT"` / `"SIGINT"` / `"2"` → `TrapSignal::Real(2)`
/// - Same dual-form for every trappable signal.
///
/// `KILL`/`STOP` parse OK (bash stores their disposition without registering an
/// OS handler — `install` skips registration for them). Returns an error only
/// for unknown names / out-of-table numbers.
pub fn parse_trap_signal(name: &str) -> Result<TrapSignal, String> {
    // EXIT pseudo-signal (case-sensitive to match bash).
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

    // Numeric form.
    if let Ok(n) = name.parse::<i32>() {
        // Numeric 0 is the EXIT pseudo-signal (bash: `trap … 0` ≡ `trap … EXIT`).
        if n == 0 {
            return Ok(TrapSignal::Exit);
        }
        // Accept any signal in the FULL table, including KILL/STOP. bash does
        // NOT reject `trap … KILL`/`STOP`: it stores the disposition (visible
        // via `trap -p`) but never registers an OS handler, so the signal still
        // performs its default action. `install`/`reset` skip OS registration
        // for these two.
        if killable_signals().iter().any(|(_, s)| *s == n) {
            return Ok(TrapSignal::Real(n));
        }
        return Err(format!("{name}: invalid signal specification"));
    }

    // Strip optional SIG prefix (case-sensitive — bash matches "SIGINT"
    // but not "Sigint").
    let stripped = name.strip_prefix("SIG").unwrap_or(name);

    // Look up in the full table (KILL/STOP included — see the numeric branch).
    for (n, sig) in killable_signals() {
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
        shell
            .trap_pending
            .fetch_or(1 << libc::SIGINT, Ordering::SeqCst);
        shell
            .trap_pending
            .fetch_or(1 << libc::SIGTERM, Ordering::SeqCst);
        shell
            .trap_pending
            .fetch_or(1 << libc::SIGHUP, Ordering::SeqCst);
        let drained = drain_pending(&mut shell);
        assert_eq!(drained, vec![libc::SIGHUP, libc::SIGINT, libc::SIGTERM]);
    }

    #[test]
    fn drain_pending_clears_the_bitmask() {
        let mut shell = Shell::new();
        shell
            .trap_pending
            .fetch_or(1 << libc::SIGINT, Ordering::SeqCst);
        let _ = drain_pending(&mut shell);
        assert_eq!(shell.trap_pending.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn dispatch_pending_traps_runs_registered_action() {
        let mut shell = Shell::new();
        shell
            .traps
            .insert(TrapSignal::Real(libc::SIGUSR1), Some("FOO=ran".to_string()));
        shell
            .trap_pending
            .fetch_or(1 << libc::SIGUSR1, Ordering::SeqCst);
        dispatch_pending_traps(&mut shell);
        assert_eq!(shell.get("FOO"), Some("ran"));
    }

    #[test]
    fn dispatch_pending_traps_skips_ignored_signal() {
        let mut shell = Shell::new();
        shell.traps.insert(TrapSignal::Real(libc::SIGUSR1), None); // ignore
        shell
            .trap_pending
            .fetch_or(1 << libc::SIGUSR1, Ordering::SeqCst);
        dispatch_pending_traps(&mut shell);
        // No action ran; no side effect to assert. The drain happened
        // (asserted by trap_pending now being 0).
        assert_eq!(shell.trap_pending.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn dispatch_pending_traps_skips_unregistered_signal() {
        let mut shell = Shell::new();
        // No entry in shell.traps for SIGUSR1.
        shell
            .trap_pending
            .fetch_or(1 << libc::SIGUSR1, Ordering::SeqCst);
        dispatch_pending_traps(&mut shell);
        assert_eq!(shell.trap_pending.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn fire_exit_trap_runs_action_then_removes_it() {
        let mut shell = Shell::new();
        shell
            .traps
            .insert(TrapSignal::Exit, Some("FOO=ran".to_string()));
        fire_exit_trap(&mut shell);
        assert_eq!(shell.get("FOO"), Some("ran"));
        // Trap is now absent: a second fire is a no-op.
        assert!(!shell.traps.contains_key(&TrapSignal::Exit));
    }

    #[test]
    fn fire_exit_trap_no_action_is_noop() {
        let mut shell = Shell::new();
        fire_exit_trap(&mut shell); // no panic, no side effect
        assert!(!shell.traps.contains_key(&TrapSignal::Exit));
    }

    #[test]
    fn clear_for_subshell_resets_traps_and_bitmask() {
        let mut shell = Shell::new();
        shell
            .traps
            .insert(TrapSignal::Exit, Some("nope".to_string()));
        shell
            .traps
            .insert(TrapSignal::Real(libc::SIGINT), Some("nope".to_string()));
        shell
            .trap_pending
            .fetch_or(1 << libc::SIGINT, Ordering::SeqCst);
        clear_for_subshell(&mut shell);
        assert!(shell.traps.is_empty());
        assert_eq!(shell.trap_pending.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn parse_trap_signal_exit() {
        assert_eq!(parse_trap_signal("EXIT"), Ok(TrapSignal::Exit));
    }

    #[test]
    fn parse_trap_signal_zero_is_exit() {
        assert_eq!(parse_trap_signal("0"), Ok(TrapSignal::Exit));
        // regression: the EXIT name still maps to the same thing.
        assert_eq!(parse_trap_signal("EXIT"), Ok(TrapSignal::Exit));
    }

    #[test]
    fn parse_trap_signal_name_no_prefix() {
        assert_eq!(parse_trap_signal("INT"), Ok(TrapSignal::Real(libc::SIGINT)));
        assert_eq!(
            parse_trap_signal("TERM"),
            Ok(TrapSignal::Real(libc::SIGTERM))
        );
        assert_eq!(parse_trap_signal("HUP"), Ok(TrapSignal::Real(libc::SIGHUP)));
    }

    #[test]
    fn parse_trap_signal_sig_prefix() {
        assert_eq!(
            parse_trap_signal("SIGINT"),
            Ok(TrapSignal::Real(libc::SIGINT))
        );
        assert_eq!(
            parse_trap_signal("SIGTERM"),
            Ok(TrapSignal::Real(libc::SIGTERM))
        );
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
    fn parse_trap_signal_kill_parses_ok() {
        // bash accepts `trap … KILL` (stores disposition, no OS handler).
        assert_eq!(
            parse_trap_signal("KILL"),
            Ok(TrapSignal::Real(libc::SIGKILL))
        );
        assert_eq!(
            parse_trap_signal("SIGKILL"),
            Ok(TrapSignal::Real(libc::SIGKILL))
        );
        let n = libc::SIGKILL.to_string();
        assert_eq!(parse_trap_signal(&n), Ok(TrapSignal::Real(libc::SIGKILL)));
    }

    #[test]
    fn parse_trap_signal_stop_parses_ok() {
        assert_eq!(
            parse_trap_signal("STOP"),
            Ok(TrapSignal::Real(libc::SIGSTOP))
        );
        assert_eq!(
            parse_trap_signal("SIGSTOP"),
            Ok(TrapSignal::Real(libc::SIGSTOP))
        );
        let n = libc::SIGSTOP.to_string();
        assert_eq!(parse_trap_signal(&n), Ok(TrapSignal::Real(libc::SIGSTOP)));
    }

    #[test]
    fn install_kill_stores_disposition_without_os_handler() {
        // install() must NOT attempt OS registration for KILL (sigaction would
        // EINVAL); it stores the disposition so `trap -p` can list it.
        let mut shell = Shell::new();
        assert!(install(&mut shell, TrapSignal::Real(libc::SIGKILL), None).is_ok());
        assert!(shell.traps.contains_key(&TrapSignal::Real(libc::SIGKILL)));
        assert!(
            !shell.trap_sigids.contains_key(&libc::SIGKILL),
            "KILL must not get an OS sigid"
        );
    }

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

    #[test]
    fn name_table_has_full_standard_set_minus_kill_stop() {
        let t = name_table();
        // newly-added standard signals are present (trappable)
        for name in [
            "ABRT", "SEGV", "BUS", "FPE", "ILL", "TRAP", "SYS", "URG", "XCPU",
        ] {
            assert!(
                t.iter().any(|(n, _)| *n == name),
                "trappable missing {name}"
            );
        }
        // KILL and STOP are NOT trappable
        assert!(
            !t.iter().any(|(n, _)| *n == "KILL"),
            "KILL must not be trappable"
        );
        assert!(
            !t.iter().any(|(n, _)| *n == "STOP"),
            "STOP must not be trappable"
        );
        // all numbers fit the AtomicU32 pending mask (1..=31)
        assert!(
            t.iter().all(|(_, num)| (1..=31).contains(num)),
            "signal out of 1..=31"
        );
    }

    #[test]
    fn killable_includes_kill_stop_and_new_signals() {
        let k = killable_signals();
        assert!(k.iter().any(|(n, _)| *n == "KILL"));
        assert!(k.iter().any(|(n, _)| *n == "STOP"));
        assert!(k.iter().any(|(n, _)| *n == "ABRT"));
        assert!(k.iter().any(|(n, _)| *n == "SEGV"));
        // number<->name agrees with libc
        assert_eq!(
            k.iter().find(|(n, _)| *n == "ABRT").map(|(_, x)| *x),
            Some(libc::SIGABRT)
        );
        assert_eq!(
            k.iter().find(|(n, _)| *n == "SEGV").map(|(_, x)| *x),
            Some(libc::SIGSEGV)
        );
    }

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
        install(
            &mut shell,
            TrapSignal::Real(libc::SIGUSR1),
            Some("echo usr1".to_string()),
        )
        .unwrap();
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
        install(
            &mut shell,
            TrapSignal::Real(libc::SIGUSR2),
            Some("echo usr2".to_string()),
        )
        .unwrap();
        reset(&mut shell, TrapSignal::Real(libc::SIGUSR2)).unwrap();
        assert!(!shell.trap_sigids.contains_key(&libc::SIGUSR2));
        assert!(!shell.traps.contains_key(&TrapSignal::Real(libc::SIGUSR2)));
    }

    #[test]
    fn fire_err_trap_runs_action_without_remove() {
        let mut shell = Shell::new();
        shell
            .traps
            .insert(TrapSignal::Err, Some("FOO=err_ran".to_string()));
        fire_err_trap(&mut shell);
        assert_eq!(shell.get("FOO"), Some("err_ran"));
        // Trap entry MUST still be present (unlike EXIT which self-removes).
        assert!(shell.traps.contains_key(&TrapSignal::Err));
    }

    #[test]
    fn fire_debug_trap_runs_action_without_remove() {
        let mut shell = Shell::new();
        shell
            .traps
            .insert(TrapSignal::Debug, Some("FOO=dbg_ran".to_string()));
        fire_debug_trap(&mut shell);
        assert_eq!(shell.get("FOO"), Some("dbg_ran"));
        assert!(shell.traps.contains_key(&TrapSignal::Debug));
    }

    #[test]
    fn fire_return_trap_runs_action_without_remove() {
        let mut shell = Shell::new();
        shell
            .traps
            .insert(TrapSignal::Return, Some("FOO=ret_ran".to_string()));
        fire_return_trap(&mut shell);
        assert_eq!(shell.get("FOO"), Some("ret_ran"));
        assert!(shell.traps.contains_key(&TrapSignal::Return));
    }

    #[test]
    fn fire_err_trap_recursion_guard_suppresses_reentry() {
        let mut shell = Shell::new();
        shell.firing_trap = Some(TrapSignal::Err);
        shell
            .traps
            .insert(TrapSignal::Err, Some("FOO=should_not_run".to_string()));
        fire_err_trap(&mut shell);
        // Action should NOT have run because firing_trap was already set.
        assert_eq!(shell.get("FOO"), None);
    }

    #[test]
    fn fire_debug_trap_recursion_guard_suppresses_reentry() {
        let mut shell = Shell::new();
        shell.firing_trap = Some(TrapSignal::Debug);
        shell
            .traps
            .insert(TrapSignal::Debug, Some("FOO=should_not_run".to_string()));
        fire_debug_trap(&mut shell);
        assert_eq!(shell.get("FOO"), None);
    }

    #[test]
    fn fire_return_trap_recursion_guard_suppresses_reentry() {
        let mut shell = Shell::new();
        shell.firing_trap = Some(TrapSignal::Return);
        shell
            .traps
            .insert(TrapSignal::Return, Some("FOO=should_not_run".to_string()));
        fire_return_trap(&mut shell);
        assert_eq!(shell.get("FOO"), None);
    }

    #[test]
    fn fire_err_trap_different_signal_in_flight_does_not_suppress() {
        // firing_trap is Some(Debug), but we're firing Err — should fire.
        let mut shell = Shell::new();
        shell.firing_trap = Some(TrapSignal::Debug);
        shell
            .traps
            .insert(TrapSignal::Err, Some("FOO=err_ran".to_string()));
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
}
