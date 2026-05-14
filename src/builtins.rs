use std::env;
use std::path::Path;

/// The result of running a command — either the shell continues (carrying the
/// command's exit status) or the shell should terminate with a code.
#[derive(Debug)]
pub enum ExecOutcome {
    Continue(i32),
    Exit(i32),
}

pub fn is_builtin(name: &str) -> bool {
    matches!(name, "cd" | "exit" | "pwd" | "echo")
}

/// Runs a builtin. Caller must ensure `is_builtin(name)` is true.
pub fn run_builtin(name: &str, args: &[String]) -> ExecOutcome {
    match name {
        "cd" => builtin_cd(args),
        "pwd" => builtin_pwd(),
        "echo" => builtin_echo(args),
        "exit" => builtin_exit(args),
        _ => unreachable!("run_builtin called with non-builtin: {name}"),
    }
}

fn builtin_cd(args: &[String]) -> ExecOutcome {
    if args.len() > 1 {
        eprintln!("shuck: cd: too many arguments");
        return ExecOutcome::Continue(1);
    }
    let target = match args.first() {
        Some(dir) => dir.clone(),
        None => match env::var("HOME") {
            Ok(home) => home,
            Err(_) => {
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

fn builtin_pwd() -> ExecOutcome {
    match env::current_dir() {
        Ok(path) => {
            println!("{}", path.display());
            ExecOutcome::Continue(0)
        }
        Err(e) => {
            eprintln!("shuck: pwd: {e}");
            ExecOutcome::Continue(1)
        }
    }
}

fn builtin_echo(args: &[String]) -> ExecOutcome {
    println!("{}", args.join(" "));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_builtin_recognizes_builtins() {
        assert!(is_builtin("cd"));
        assert!(is_builtin("exit"));
        assert!(is_builtin("pwd"));
        assert!(is_builtin("echo"));
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
}
