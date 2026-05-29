use std::collections::HashMap;
use std::io::IsTerminal;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::jobs::JobTable;

#[derive(Debug, Clone)]
pub struct Variable {
    pub value: String,
    pub exported: bool,
}

/// Per-session shell state: variables (each either exported or not) and the
/// last command's exit status. The initial set of variables is seeded from
/// the process environment huck inherited at startup, every one marked
/// exported.
#[derive(Debug, Clone)]
pub struct Shell {
    vars: HashMap<String, Variable>,
    last_status: i32,
    /// Current frame of positional parameters. Populated only by
    /// function calls (Task 5); empty at the top level.
    pub positional_args: Vec<String>,
    /// User-defined functions. Populated by `Command::FunctionDef`
    /// execution; looked up by `run_exec_single` when dispatching a
    /// simple command.
    pub functions: HashMap<String, Box<crate::command::Command>>,
    /// User-defined aliases. `name` → expansion text. Populated by
    /// the `alias` builtin; consumed by `expand_aliases_in_tokens`
    /// during interactive REPL input.
    pub aliases: std::collections::HashMap<String, String>,
    #[allow(dead_code)]
    pub jobs: JobTable,
    pub sigchld_flag: Arc<AtomicBool>,
    pub sigint_flag: Arc<AtomicBool>,
    pub shell_pgid: i32,
    pub history: crate::history::History,
    /// Shell PID, cached at startup via `getpid()`. Used for `$$`.
    pub shell_pid: i32,
    /// PID of the most-recently-backgrounded pipeline's last stage. Used for `$!`.
    pub last_bg_pid: Option<i32>,
    /// The shell's argv[0], cached at startup. Used for `$0` at the top level.
    pub shell_argv0: String,
    /// Stack of function names pushed/popped around each `call_function`.
    /// `$0` returns the top of this stack when inside a function.
    pub function_arg0: Vec<String>,
    /// `Some(status)` after a fatal parameter-expansion error fires
    /// inside an `expand_*` call. The executor peeks this to bail the
    /// current simple command; the REPL loop drains it via
    /// `take_pending_fatal_pe_error` to decide whether to exit (in
    /// non-interactive mode) or return to prompt (interactive).
    pub pending_fatal_pe_error: Option<i32>,
    /// True if stdin was a TTY at startup. Determines whether fatal PE
    /// errors exit the shell or just return to the prompt.
    pub is_interactive: bool,

    /// Registered trap handlers. `None` value = ignore that signal
    /// (corresponds to `trap "" SIGNAL`); `Some(text)` = action to
    /// re-parse and execute when the signal fires. Absent key =
    /// default disposition.
    pub traps: std::collections::HashMap<crate::traps::TrapSignal, Option<String>>,

    /// Per-signal bitmask of "trap pending" flags. Signal handlers set
    /// bits via `fetch_or`; the main loop drains via `swap` at the
    /// polling checkpoints. Bit N corresponds to libc signal number N.
    /// EXIT is NOT here — it fires at the exit-path boundary, not via
    /// a real signal.
    pub trap_pending: std::sync::Arc<std::sync::atomic::AtomicU32>,

    /// Map of signal number → signal-hook SigId for each currently-
    /// installed trap handler. Used by `traps::reset` to unregister.
    pub trap_sigids: std::collections::HashMap<i32, signal_hook::SigId>,

    /// Currently-firing pseudo-trap, if any. Set on entry to
    /// fire_err/fire_debug/fire_return; cleared on exit. Used to
    /// suppress re-firing of the SAME trap from within its own action.
    /// Different signals do NOT cross-suppress (a DEBUG action that
    /// triggers ERR still fires ERR).
    pub firing_trap: Option<crate::traps::TrapSignal>,

    /// Depth counter for ERR-suppression contexts (if/elif/while/until
    /// conditions). ERR trap only fires when this is 0.
    pub err_suppressed_depth: u32,

    /// Recursive `source`/`.` call depth. Capped at 64 in
    /// `builtin_source` to prevent runaway loops. Increment on
    /// enter, decrement on exit.
    pub source_depth: u32,

    /// Stack of `local`-snapshot frames. Pushed in `call_function`
    /// before the body runs; popped + restored after. Each frame
    /// maps `var_name` → the pre-`local` snapshot (None if the var
    /// was unset). Outside any function, this vec is empty —
    /// `builtin_local` checks for that.
    pub local_scopes: Vec<std::collections::HashMap<String, Option<Variable>>>,
}

impl Shell {
    pub fn new() -> Self {
        let mut vars = HashMap::new();
        for (key, value) in std::env::vars() {
            vars.insert(key, Variable { value, exported: true });
        }
        let shell_pid = unsafe { libc::getpid() };
        let shell_argv0 = std::env::args().next().unwrap_or_else(|| "huck".to_string());
        let shell = Self {
            vars,
            last_status: 0,
            positional_args: Vec::new(),
            functions: HashMap::new(),
            aliases: std::collections::HashMap::new(),
            jobs: JobTable::new(),
            sigchld_flag: Arc::new(AtomicBool::new(false)),
            sigint_flag: Arc::new(AtomicBool::new(false)),
            shell_pgid: unsafe { libc::getpgrp() },
            history: crate::history::History::new(),
            shell_pid,
            last_bg_pid: None,
            shell_argv0,
            function_arg0: Vec::new(),
            pending_fatal_pe_error: None,
            is_interactive: std::io::stdin().is_terminal(),
            traps: std::collections::HashMap::new(),
            trap_pending: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
            trap_sigids: std::collections::HashMap::new(),
            firing_trap: None,
            err_suppressed_depth: 0,
            source_depth: 0,
            local_scopes: Vec::new(),
        };
        // Make the trap_pending Arc visible to async-signal-safe
        // signal handlers installed by the traps module.
        crate::traps::init_pending_bitmask(std::sync::Arc::clone(&shell.trap_pending));
        shell
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(|v| v.value.as_str())
    }

    /// Variable lookup for expansion. Recognises positional names
    /// (`"1"`-`"9"`/`"10"`/..., and `"#"`) before falling back to the
    /// regular variable HashMap. Returns an owned `String` because
    /// positional/computed values are not stored as references.
    pub fn lookup_var(&self, name: &str) -> Option<String> {
        // Special parameters (v26).
        match name {
            "0" => return Some(
                self.function_arg0.last().cloned().unwrap_or_else(|| self.shell_argv0.clone())
            ),
            "$" => return Some(self.shell_pid.to_string()),
            "!" => return Some(
                // Returns "" not None when unset: bash expands $! to empty before
                // any background has happened (v26 spec §lookup_var changes).
                self.last_bg_pid.map(|p| p.to_string()).unwrap_or_default()
            ),
            _ => {}
        }
        if name == "#" {
            return Some(self.positional_args.len().to_string());
        }
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_digit()) {
            let n: usize = name.parse().ok()?;
            if n == 0 {
                // unreachable: "0" is matched by the special-params block above
                return None;
            }
            return self.positional_args.get(n - 1).cloned();
        }
        self.vars.get(name).map(|v| v.value.clone())
    }

    /// Sets a variable's value, preserving its existing `exported` flag (or
    /// creating it as unexported if it didn't exist).
    pub fn set(&mut self, name: &str, value: String) {
        match self.vars.get_mut(name) {
            Some(existing) => existing.value = value,
            None => {
                self.vars.insert(name.to_string(), Variable { value, exported: false });
            }
        }
    }

    /// Marks an existing variable as exported. If it doesn't exist, creates
    /// it with an empty value, already exported.
    pub fn export(&mut self, name: &str) {
        self.vars
            .entry(name.to_string())
            .and_modify(|v| v.exported = true)
            .or_insert_with(|| Variable {
                value: String::new(),
                exported: true,
            });
    }

    /// Sets a variable's value AND marks it exported.
    pub fn export_set(&mut self, name: &str, value: String) {
        self.vars.insert(
            name.to_string(),
            Variable { value, exported: true },
        );
    }

    pub fn unset(&mut self, name: &str) {
        self.vars.remove(name);
    }

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

    /// True if `name` is set and marked exported.
    pub fn is_exported(&self, name: &str) -> bool {
        self.vars.get(name).is_some_and(|v| v.exported)
    }

    pub fn last_status(&self) -> i32 {
        self.last_status
    }

    pub fn set_last_status(&mut self, status: i32) {
        self.last_status = status;
    }

    /// Returns and clears the pending fatal-PE-error flag.
    pub fn take_pending_fatal_pe_error(&mut self) -> Option<i32> {
        self.pending_fatal_pe_error.take()
    }

    /// Iterates only the exported variables, suitable for passing to a child
    /// process's `Command::envs`.
    pub fn exported_env(&self) -> impl Iterator<Item = (&str, &str)> {
        self.vars
            .iter()
            .filter(|(_, v)| v.exported)
            .map(|(k, v)| (k.as_str(), v.value.as_str()))
    }

    /// Iterates the names of all variables (exported or not).
    pub fn var_names(&self) -> impl Iterator<Item = &str> {
        self.vars.keys().map(|s| s.as_str())
    }

    /// Sends SIGHUP to every live job not marked for nohup. Called
    /// on each clean shell-exit path. Stopped jobs get SIGCONT first
    /// so they wake to die. Errors from `killpg` (e.g. ESRCH for an
    /// already-reaped pgid) are intentionally ignored; this is a
    /// best-effort cleanup.
    pub fn hangup_jobs(&mut self) {
        for job in self.jobs.iter() {
            if !should_hangup(job) {
                continue;
            }
            unsafe {
                libc::killpg(job.pgid, libc::SIGCONT);
                libc::killpg(job.pgid, libc::SIGHUP);
            }
        }
    }
}

/// Pure predicate: should this job receive SIGHUP at shell exit?
/// True iff the job is still alive (Running or Stopped) AND has
/// not been marked for nohup by `disown -h`.
fn should_hangup(job: &crate::jobs::Job) -> bool {
    let live = matches!(
        job.state,
        crate::jobs::JobState::Running | crate::jobs::JobState::Stopped(_)
    );
    live && !job.marked_for_nohup
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_captures_inherited_env_as_exported() {
        let shell = Shell::new();
        // PATH is reliably present in test environments.
        assert!(shell.get("PATH").is_some(), "PATH should be inherited");
        let path_exported = shell.exported_env().any(|(k, _)| k == "PATH");
        assert!(path_exported);
    }

    #[test]
    fn set_creates_unexported_var() {
        let mut shell = Shell::new();
        shell.set("HUCK_TEST_SET", "value".to_string());
        assert_eq!(shell.get("HUCK_TEST_SET"), Some("value"));
        let in_exported = shell.exported_env().any(|(k, _)| k == "HUCK_TEST_SET");
        assert!(!in_exported);
    }

    #[test]
    fn set_preserves_existing_exported_flag() {
        let mut shell = Shell::new();
        shell.export_set("HUCK_TEST_KEEP", "v1".to_string());
        shell.set("HUCK_TEST_KEEP", "v2".to_string());
        assert_eq!(shell.get("HUCK_TEST_KEEP"), Some("v2"));
        let in_exported = shell.exported_env().any(|(k, _)| k == "HUCK_TEST_KEEP");
        assert!(in_exported);
    }

    #[test]
    fn export_marks_existing_exported() {
        let mut shell = Shell::new();
        shell.set("HUCK_TEST_EX", "value".to_string());
        shell.export("HUCK_TEST_EX");
        let in_exported = shell.exported_env().any(|(k, _)| k == "HUCK_TEST_EX");
        assert!(in_exported);
    }

    #[test]
    fn export_creates_empty_when_missing() {
        let mut shell = Shell::new();
        shell.export("HUCK_TEST_EMPTY");
        assert_eq!(shell.get("HUCK_TEST_EMPTY"), Some(""));
        let in_exported = shell.exported_env().any(|(k, _)| k == "HUCK_TEST_EMPTY");
        assert!(in_exported);
    }

    #[test]
    fn unset_removes_variable() {
        let mut shell = Shell::new();
        shell.set("HUCK_TEST_REMOVE", "v".to_string());
        shell.unset("HUCK_TEST_REMOVE");
        assert_eq!(shell.get("HUCK_TEST_REMOVE"), None);
        let in_exported = shell.exported_env().any(|(k, _)| k == "HUCK_TEST_REMOVE");
        assert!(!in_exported);
    }

    #[test]
    fn last_status_round_trip() {
        let mut shell = Shell::new();
        assert_eq!(shell.last_status(), 0);
        shell.set_last_status(42);
        assert_eq!(shell.last_status(), 42);
    }

    #[test]
    fn exported_env_excludes_unexported() {
        let mut shell = Shell::new();
        shell.set("HUCK_TEST_HIDDEN", "v".to_string());
        let in_exported = shell.exported_env().any(|(k, _)| k == "HUCK_TEST_HIDDEN");
        assert!(!in_exported);
    }

    #[test]
    fn new_captures_shell_pgid_from_getpgrp() {
        let s = Shell::new();
        let expected = unsafe { libc::getpgrp() };
        assert_eq!(s.shell_pgid, expected);
        assert!(s.shell_pgid > 0, "pgrp should be positive");
    }

    #[test]
    fn new_initializes_sigint_flag_to_false() {
        let s = Shell::new();
        assert!(!s.sigint_flag.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn var_names_lists_all_variables() {
        let mut shell = Shell::new();
        shell.set("HUCK_TEST_VN", "value".to_string());
        let names: Vec<&str> = shell.var_names().collect();
        assert!(names.contains(&"HUCK_TEST_VN"));
    }

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

    #[test]
    fn shell_new_caches_pid_and_argv0() {
        let shell = Shell::new();
        assert!(shell.shell_pid > 0, "shell_pid should be positive");
        assert!(!shell.shell_argv0.is_empty(), "shell_argv0 should be non-empty");
        assert_eq!(shell.last_bg_pid, None);
        assert!(shell.function_arg0.is_empty());
    }

    #[test]
    fn lookup_var_dollar_returns_cached_pid_as_string() {
        let mut shell = Shell::new();
        shell.shell_pid = 12345;
        assert_eq!(shell.lookup_var("$"), Some("12345".to_string()));
    }

    #[test]
    fn lookup_var_bang_unset_returns_empty_string() {
        let shell = Shell::new();
        assert_eq!(shell.lookup_var("!"), Some(String::new()));
    }

    #[test]
    fn lookup_var_bang_after_set_returns_pid_string() {
        let mut shell = Shell::new();
        shell.last_bg_pid = Some(54321);
        assert_eq!(shell.lookup_var("!"), Some("54321".to_string()));
    }

    #[test]
    fn lookup_var_zero_top_level_returns_shell_argv0() {
        let mut shell = Shell::new();
        shell.shell_argv0 = "my-shell".to_string();
        assert_eq!(shell.lookup_var("0"), Some("my-shell".to_string()));
    }

    #[test]
    fn lookup_var_zero_in_function_returns_function_name() {
        let mut shell = Shell::new();
        shell.shell_argv0 = "my-shell".to_string();
        shell.function_arg0.push("myfunc".to_string());
        assert_eq!(shell.lookup_var("0"), Some("myfunc".to_string()));
    }

    #[test]
    fn lookup_var_zero_nested_returns_innermost() {
        let mut shell = Shell::new();
        shell.function_arg0.push("outer".to_string());
        shell.function_arg0.push("inner".to_string());
        assert_eq!(shell.lookup_var("0"), Some("inner".to_string()));
        shell.function_arg0.pop();
        assert_eq!(shell.lookup_var("0"), Some("outer".to_string()));
        shell.function_arg0.pop();
        assert!(shell.lookup_var("0").is_some());  // falls through to shell_argv0
    }

    #[test]
    fn should_hangup_skips_marked_and_done_jobs() {
        use crate::jobs::{JobState, JobTable};
        let mut t = JobTable::new();
        let id = t.add(0, vec![1234], "sleep 30".to_string());

        // Running + not marked → hangup
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(super::should_hangup(job));

        // Running + marked → skip
        t.mark_for_nohup(id);
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(!super::should_hangup(job));

        // Done + not marked → skip
        t.jobs_mut()[0].marked_for_nohup = false;
        t.jobs_mut()[0].state = JobState::Done(0);
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(!super::should_hangup(job));

        // Stopped + not marked → hangup (Stopped is "live" for SIGHUP purposes)
        t.jobs_mut()[0].marked_for_nohup = false;
        t.jobs_mut()[0].state = JobState::Stopped(::libc::SIGTSTP);
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(super::should_hangup(job));
    }
}
