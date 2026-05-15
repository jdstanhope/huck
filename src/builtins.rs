use std::env;
use std::io::Write;
use std::path::Path;

use crate::shell_state::Shell;

/// The result of running a command — either the shell continues (carrying the
/// command's exit status) or the shell should terminate with a code.
#[derive(Debug)]
pub enum ExecOutcome {
    Continue(i32),
    Exit(i32),
}

pub fn is_builtin(name: &str) -> bool {
    matches!(name, "cd" | "exit" | "pwd" | "echo" | "export" | "unset")
}

/// Runs a builtin. Caller must ensure `is_builtin(name)` is true. `out` is the
/// destination for any stdout the builtin produces (`echo`, `pwd`); `cd` and
/// `exit` produce no stdout and ignore it.
pub fn run_builtin(
    name: &str,
    args: &[String],
    out: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    match name {
        "cd" => builtin_cd(args, shell),
        "pwd" => builtin_pwd(out),
        "echo" => builtin_echo(args, out),
        "exit" => builtin_exit(args),
        "export" => builtin_export(args, out, shell),
        "unset" => builtin_unset(args, shell),
        _ => unreachable!("run_builtin called with non-builtin: {name}"),
    }
}

fn builtin_cd(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if args.len() > 1 {
        eprintln!("shuck: cd: too many arguments");
        return ExecOutcome::Continue(1);
    }
    let target = match args.first() {
        Some(dir) => dir.clone(),
        None => match shell.get("HOME") {
            Some(home) => home.to_string(),
            None => {
                eprintln!("shuck: cd: HOME not set");
                return ExecOutcome::Continue(1);
            }
        },
    };
    match env::set_current_dir(Path::new(&target)) {
        Ok(()) => ExecOutcome::Continue(0),
        Err(e) => {
            eprintln!("shuck: cd: {target}: {e}");
            ExecOutcome::Continue(1)
        }
    }
}

fn builtin_pwd(out: &mut dyn Write) -> ExecOutcome {
    match env::current_dir() {
        Ok(path) => {
            if let Err(e) = writeln!(out, "{}", path.display()) {
                eprintln!("shuck: pwd: {e}");
                return ExecOutcome::Continue(1);
            }
            ExecOutcome::Continue(0)
        }
        Err(e) => {
            eprintln!("shuck: pwd: {e}");
            ExecOutcome::Continue(1)
        }
    }
}

fn builtin_echo(args: &[String], out: &mut dyn Write) -> ExecOutcome {
    if let Err(e) = writeln!(out, "{}", args.join(" ")) {
        eprintln!("shuck: echo: {e}");
        return ExecOutcome::Continue(1);
    }
    ExecOutcome::Continue(0)
}

fn builtin_exit(args: &[String]) -> ExecOutcome {
    match args.first() {
        None => ExecOutcome::Exit(0),
        Some(code_str) => match code_str.parse::<i32>() {
            Ok(code) => ExecOutcome::Exit(code),
            Err(_) => {
                eprintln!("shuck: exit: {code_str}: numeric argument required");
                ExecOutcome::Continue(2)
            }
        },
    }
}

fn is_valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else { return false; };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn builtin_export(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if args.is_empty() {
        let mut entries: Vec<(String, String)> = shell
            .exported_env()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        entries.sort();
        for (name, value) in entries {
            if let Err(e) = writeln!(out, "export {name}={value}") {
                eprintln!("shuck: export: {e}");
                return ExecOutcome::Continue(1);
            }
        }
        return ExecOutcome::Continue(0);
    }
    let mut any_error = false;
    for arg in args {
        match arg.find('=') {
            Some(idx) => {
                let name = &arg[..idx];
                let value = &arg[idx + 1..];
                if !is_valid_name(name) {
                    eprintln!("shuck: export: '{arg}': not a valid identifier");
                    any_error = true;
                    continue;
                }
                shell.export_set(name, value.to_string());
            }
            None => {
                if !is_valid_name(arg) {
                    eprintln!("shuck: export: '{arg}': not a valid identifier");
                    any_error = true;
                    continue;
                }
                shell.export(arg);
            }
        }
    }
    if any_error {
        ExecOutcome::Continue(1)
    } else {
        ExecOutcome::Continue(0)
    }
}

fn builtin_unset(args: &[String], shell: &mut Shell) -> ExecOutcome {
    let mut any_error = false;
    for arg in args {
        if !is_valid_name(arg) {
            eprintln!("shuck: unset: '{arg}': not a valid identifier");
            any_error = true;
            continue;
        }
        shell.unset(arg);
    }
    if any_error {
        ExecOutcome::Continue(1)
    } else {
        ExecOutcome::Continue(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_builtin_recognizes_builtins() {
        assert!(is_builtin("cd"));
        assert!(is_builtin("exit"));
        assert!(is_builtin("pwd"));
        assert!(is_builtin("echo"));
        assert!(is_builtin("export"));
        assert!(is_builtin("unset"));
        assert!(!is_builtin("ls"));
    }

    #[test]
    fn exit_with_no_args() {
        assert!(matches!(builtin_exit(&[]), ExecOutcome::Exit(0)));
    }

    #[test]
    fn exit_with_code() {
        assert!(matches!(
            builtin_exit(&["3".to_string()]),
            ExecOutcome::Exit(3)
        ));
    }

    #[test]
    fn exit_with_bad_code_continues() {
        assert!(matches!(
            builtin_exit(&["abc".to_string()]),
            ExecOutcome::Continue(_)
        ));
    }

    #[test]
    fn echo_writes_args_joined_by_spaces() {
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_echo(&["hello".to_string(), "world".to_string()], &mut out);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(out, b"hello world\n");
    }

    #[test]
    fn echo_with_no_args_writes_a_blank_line() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&[], &mut out);
        assert_eq!(out, b"\n");
    }

    #[test]
    fn pwd_writes_the_current_directory() {
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_pwd(&mut out);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let written = String::from_utf8(out).unwrap();
        let expected = env::current_dir().unwrap();
        assert_eq!(written.trim_end(), expected.to_str().unwrap());
    }

    #[test]
    fn export_marks_existing() {
        let mut shell = Shell::new();
        shell.set("SHUCK_EXP", "v".to_string());
        let mut out = Vec::new();
        let outcome = builtin_export(&["SHUCK_EXP".to_string()], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let in_exported = shell.exported_env().any(|(k, _)| k == "SHUCK_EXP");
        assert!(in_exported);
    }

    #[test]
    fn export_name_only_creates_empty_exported() {
        let mut shell = Shell::new();
        let mut out = Vec::new();
        let outcome = builtin_export(&["SHUCK_NEW_VAR".to_string()], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.get("SHUCK_NEW_VAR"), Some(""));
        assert!(shell.exported_env().any(|(k, _)| k == "SHUCK_NEW_VAR"));
    }

    #[test]
    fn export_sets_and_exports() {
        let mut shell = Shell::new();
        let mut out = Vec::new();
        let outcome = builtin_export(&["SHUCK_EXP2=hello".to_string()], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.get("SHUCK_EXP2"), Some("hello"));
        let in_exported = shell.exported_env().any(|(k, _)| k == "SHUCK_EXP2");
        assert!(in_exported);
    }

    #[test]
    fn export_invalid_name_continues_with_error() {
        let mut shell = Shell::new();
        let mut out = Vec::new();
        let outcome = builtin_export(
            &["1BAD=x".to_string(), "GOOD=y".to_string()],
            &mut out,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        assert_eq!(shell.get("1BAD"), None);
        assert_eq!(shell.get("GOOD"), Some("y"));
    }

    #[test]
    fn unset_removes_variable() {
        let mut shell = Shell::new();
        shell.set("SHUCK_RM", "v".to_string());
        let outcome = builtin_unset(&["SHUCK_RM".to_string()], &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.get("SHUCK_RM"), None);
    }

    #[test]
    fn unset_invalid_name_is_error() {
        let mut shell = Shell::new();
        let outcome = builtin_unset(&["1BAD".to_string()], &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn unset_unknown_name_is_silent_ok() {
        let mut shell = Shell::new();
        let outcome = builtin_unset(&["NEVER_SET_SHUCK_XYZ".to_string()], &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }
}
