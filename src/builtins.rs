use std::env;
use std::io::Write;
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

/// Runs a builtin. Caller must ensure `is_builtin(name)` is true. `out` is the
/// destination for any stdout the builtin produces (`echo`, `pwd`); `cd` and
/// `exit` produce no stdout and ignore it.
pub fn run_builtin(name: &str, args: &[String], out: &mut dyn Write) -> ExecOutcome {
    match name {
        "cd" => builtin_cd(args),
        "pwd" => builtin_pwd(out),
        "echo" => builtin_echo(args, out),
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
}
