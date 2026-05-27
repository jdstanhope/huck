//! Trap handler storage, signal-name parsing, and signal-delivery
//! plumbing for the `trap` builtin (huck v35).

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
