use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::jobs::JobTable;
use libc;

#[derive(Debug, Clone)]
struct Variable {
    value: String,
    exported: bool,
}

/// Per-session shell state: variables (each either exported or not) and the
/// last command's exit status. The initial set of variables is seeded from
/// the process environment huck inherited at startup, every one marked
/// exported.
#[derive(Debug, Clone)]
pub struct Shell {
    vars: HashMap<String, Variable>,
    last_status: i32,
    #[allow(dead_code)]
    pub jobs: JobTable,
    pub sigchld_flag: Arc<AtomicBool>,
    pub sigint_flag: Arc<AtomicBool>,
    pub shell_pgid: i32,
}

impl Shell {
    pub fn new() -> Self {
        let mut vars = HashMap::new();
        for (key, value) in std::env::vars() {
            vars.insert(key, Variable { value, exported: true });
        }
        Self {
            vars,
            last_status: 0,
            jobs: JobTable::new(),
            sigchld_flag: Arc::new(AtomicBool::new(false)),
            sigint_flag: Arc::new(AtomicBool::new(false)),
            shell_pgid: unsafe { libc::getpgrp() },
        }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(|v| v.value.as_str())
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

    pub fn last_status(&self) -> i32 {
        self.last_status
    }

    pub fn set_last_status(&mut self, status: i32) {
        self.last_status = status;
    }

    /// Iterates only the exported variables, suitable for passing to a child
    /// process's `Command::envs`.
    pub fn exported_env(&self) -> impl Iterator<Item = (&str, &str)> {
        self.vars
            .iter()
            .filter(|(_, v)| v.exported)
            .map(|(k, v)| (k.as_str(), v.value.as_str()))
    }
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
}
