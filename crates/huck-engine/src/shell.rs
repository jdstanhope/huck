use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use signal_hook::consts::{SIGCHLD, SIGINT};

use crate::builtins::{ExecOutcome, InterruptReason};
use crate::executor;
use crate::lexer::{self};
use crate::parser;
use crate::shell_state::Shell;

/// How the shell was invoked — resolved by `parse_cli` from argv.
#[derive(Debug, PartialEq, Eq)]
pub enum RunMode {
    /// REPL (tty) or piped-stdin command reading — current behavior.
    Interactive,
    /// `-c COMMAND [NAME [ARG...]]`: argv0 = NAME (None → keep the shell's
    /// default $0), args = the rest.
    Command { command: String, argv0: Option<String>, args: Vec<String> },
    /// `SCRIPT [ARG...]`: $0 = path, args = the rest.
    File { path: PathBuf, args: Vec<String> },
    /// `--version` / `-V`: print "huck {version}" and exit 0.
    PrintVersion,
}

pub struct CliOptions {
    pub rcfile_path: Option<PathBuf>,
    pub norc: bool,
    /// `-n`: read and parse commands without executing them (noexec / syntax
    /// check). Applied to `shell_options.noexec`; honored only non-interactively.
    pub noexec: bool,
    /// `--posix` CLI flag — start in POSIX mode (also set later for invocation
    /// as `sh` or with `POSIXLY_CORRECT`; see `startup_posix`).
    pub posix: bool,
    pub mode: RunMode,
}

impl Default for CliOptions {
    fn default() -> Self {
        CliOptions {
            rcfile_path: None,
            norc: false,
            noexec: false,
            posix: false,
            mode: RunMode::Interactive,
        }
    }
}

pub fn parse_cli(args: &[String]) -> Result<CliOptions, String> {
    let mut rcfile_path: Option<PathBuf> = None;
    let mut norc = false;
    let mut noexec = false;
    let mut posix = false;
    let mut command: Option<String> = None;
    let mut i = 0;

    // Scan leading options until the first operand, `--`, or `-c`.
    while i < args.len() {
        match args[i].as_str() {
            "--norc" => {
                norc = true;
                i += 1;
            }
            "-n" => {
                noexec = true;
                i += 1;
            }
            "--posix" => {
                posix = true;
                i += 1;
            }
            "--rcfile" => {
                i += 1;
                if i >= args.len() {
                    return Err("--rcfile: requires an argument".to_string());
                }
                rcfile_path = Some(PathBuf::from(&args[i]));
                i += 1;
            }
            s if s.starts_with("--rcfile=") => {
                rcfile_path = Some(PathBuf::from(&s["--rcfile=".len()..]));
                i += 1;
            }
            "-c" => {
                i += 1;
                if i >= args.len() {
                    return Err("-c: option requires an argument".to_string());
                }
                command = Some(args[i].clone());
                i += 1;
                break; // remaining args are operands, taken verbatim
            }
            "--" => {
                i += 1;
                break;
            }
            "--version" | "-V" => {
                return Ok(CliOptions {
                    rcfile_path: None,
                    norc: false,
                    noexec: false,
                    posix: false,
                    mode: RunMode::PrintVersion,
                });
            }
            s if s.starts_with('-') && s.len() > 1 => {
                return Err(format!("unrecognized option: {s}"));
            }
            _ => break, // first operand (script path)
        }
    }

    let rest = &args[i..];
    let mode = if let Some(command) = command {
        RunMode::Command {
            command,
            argv0: rest.first().cloned(),
            args: rest.get(1..).map(|s| s.to_vec()).unwrap_or_default(),
        }
    } else if let Some(path) = rest.first() {
        RunMode::File {
            path: PathBuf::from(path),
            args: rest[1..].to_vec(),
        }
    } else {
        RunMode::Interactive
    };

    Ok(CliOptions { rcfile_path, norc, noexec, posix, mode })
}

/// POSIX mode is enabled at startup when `--posix` was passed, the shell was
/// invoked as `sh` (argv[0] basename), or `POSIXLY_CORRECT` is in the
/// environment. Mirrors bash's startup posix-mode triggers.
pub fn startup_posix(cli_posix: bool, argv0: &str, posixly_correct: bool) -> bool {
    cli_posix
        || posixly_correct
        || std::path::Path::new(argv0)
            .file_name()
            .is_some_and(|n| n == "sh")
}

fn default_rc_path(shell: &Shell) -> Option<std::path::PathBuf> {
    let home = shell
        .lookup_var("HOME")
        .or_else(|| std::env::var("HOME").ok())
        .filter(|s| !s.is_empty())?;
    Some(std::path::PathBuf::from(home).join(".huckrc"))
}

pub fn maybe_source_rc_file(shell: &mut Shell, opts: &CliOptions) -> Option<i32> {
    if opts.norc {
        return None;
    }
    if !shell.is_interactive {
        return None;
    }
    // Precedence: --rcfile > $HUCK_RC > ~/.huckrc.
    // Missing-file: explicit (--rcfile) → status 1 error;
    // implicit (env or default) → silent skip.
    let (path, explicit) = match &opts.rcfile_path {
        Some(p) => (p.clone(), true),
        None => {
            let from_env = shell
                .lookup_var("HUCK_RC")
                .or_else(|| std::env::var("HUCK_RC").ok())
                .filter(|s| !s.is_empty())
                .map(std::path::PathBuf::from);
            match from_env.or_else(|| default_rc_path(shell)) {
                Some(p) => (p, false),
                None => return None,
            }
        }
    };
    if !path.exists() {
        if explicit {
            crate::sh_error!(shell, None, "{}: No such file or directory", path.display());
            return Some(1);
        }
        return None;
    }
    let contents = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            crate::sh_error!(shell, None, "{}: {}", path.display(), e);
            return Some(1);
        }
    };
    let mut err = std::io::stderr();
    match crate::builtins::run_sourced_contents(&contents, &path, &mut err, shell) {
        crate::builtins::ExecOutcome::Exit(code) => Some(code),
        crate::builtins::ExecOutcome::Continue(status) => {
            shell.set_last_status(status);
            None
        }
        _ => None,
    }
}

/// Executes a non-interactive program (a `-c` string or a script file's
/// contents) and returns the process exit code, sending stdout to `sink`. Sets
/// $0 and the positional parameters, marks the shell non-interactive (so the rc
/// file is skipped), runs the program through the shared
/// `run_sourced_contents_in_sinks` engine, fires the EXIT trap, and hangs up
/// jobs. Does NOT touch interactive history or the line editor.
///
/// `run_program` is the `Terminal`-sink wrapper; the engine's `capture` passes a
/// `Capture` sink. Behavior with `Terminal` is identical to the old
/// `run_program`.
///
/// When `push_main_frame` is true (script-file mode), a base `FrameKind::Main`
/// frame is pushed before executing and popped after, so that BASH_SOURCE and
/// BASH_LINENO are populated at the top level and FUNCNAME gains the `main`
/// entry inside functions. For `-c` and other non-file modes pass `false`.
#[allow(clippy::too_many_arguments)]
pub fn run_program_in_sinks(
    contents: &str,
    argv0: Option<String>,
    args: Vec<String>,
    label: &str,
    push_main_frame: bool,
    sink: &mut crate::executor::StdoutSink,
    err_sink: &mut crate::executor::StderrSink,
    shell_cell: &Rc<RefCell<Shell>>,
) -> i32 {
    let mut shell = shell_cell.borrow_mut();
    shell.is_interactive = false;
    if let Some(a0) = argv0 {
        shell.shell_argv0 = a0;
    }
    shell.positional_args = args;

    if push_main_frame {
        shell.call_stack.push(crate::shell_state::Frame {
            funcname: "main".to_string(),
            source: label.to_string(),
            call_line: 0,
            kind: crate::shell_state::FrameKind::Main,
        });
        shell.sync_call_arrays();
    }

    let outcome = crate::builtins::run_sourced_contents_in_sinks(
        contents,
        std::path::Path::new(label),
        &mut shell,
        sink,
        err_sink,
    );

    if push_main_frame {
        shell.call_stack.pop();
        shell.sync_call_arrays();
    }

    let code = match outcome {
        ExecOutcome::Exit(n) => n,
        // run_sourced_contents normalizes FunctionReturn -> Continue, so this arm is
        // defensive; treat a stray top-level return code as the exit status.
        ExecOutcome::FunctionReturn(n) => n,
        ExecOutcome::Continue(s) => shell.take_pending_fatal_status().unwrap_or(s),
        ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_) => 0,
        ExecOutcome::Interrupted(InterruptReason::Sigint) => 130,
        ExecOutcome::Interrupted(InterruptReason::Timeout) => 124,
    };
    crate::traps::fire_exit_trap(&mut shell);
    shell.hangup_jobs();
    code
}

/// Run a program/script with stdout going to the terminal (the default).
pub fn run_program(
    contents: &str,
    argv0: Option<String>,
    args: Vec<String>,
    label: &str,
    push_main_frame: bool,
    shell_cell: &Rc<RefCell<Shell>>,
) -> i32 {
    let mut sink = crate::executor::StdoutSink::Terminal;
    let mut err_sink = crate::executor::StderrSink::Terminal;
    run_program_in_sinks(contents, argv0, args, label, push_main_frame, &mut sink, &mut err_sink, shell_cell)
}

/// Installs a SIGINT handler that sets the supplied flag. Called once at
/// startup after `Shell::new()`; the flag lives on the `Shell` so the wait
/// builtin can poll it to break out of its loop when Ctrl-C is pressed.
/// `shell` is threaded in solely so a registration failure can emit through
/// the unified emitter (`sh_error!`) rather than a raw `eprintln!`.
pub fn install_sigint_handler(flag: Arc<AtomicBool>, shell: &Shell) {
    if let Err(e) = signal_hook::flag::register(SIGINT, flag) {
        crate::sh_error!(shell, None, "warning: could not install SIGINT handler: {}", crate::bash_io_error(&e));
    }
}

/// Installs a SIGCHLD handler that toggles the supplied flag. Called once
/// at startup; the flag lives on the `Shell` so the reap path can poll it.
pub fn install_sigchld_handler(flag: Arc<AtomicBool>, shell: &Shell) {
    if let Err(e) = signal_hook::flag::register(SIGCHLD, flag) {
        crate::sh_error!(shell, None, "warning: could not install SIGCHLD handler: {}", crate::bash_io_error(&e));
    }
}

/// Ignore SIGTSTP/SIGTTIN/SIGTTOU at the shell level so that:
///   - Ctrl-Z at the prompt does not suspend huck itself.
///   - `tcsetpgrp` from a non-foreground pgrp does not trigger SIGTTOU on us.
///   - Defensive: huck never reads `/dev/tty` directly today, but match bash.
///
/// NOTE: `SIG_IGN` is inherited across `execve`. Foreground children
/// spawned by the executor (Task 5) MUST reset these three signals to
/// `SIG_DFL` via a `CommandExt::pre_exec` hook — otherwise Ctrl-Z would
/// not stop `vim`/`less`/etc., and a backgrounded reader would never
/// get SIGTTIN.
///
/// Called before a `Shell` exists (pre-`Shell::new()`), so a failure here
/// emits via `emit_cli_error` (the pre-shell diagnostic path) rather than
/// `sh_error!` — `prog` is the CLI's own invocation basename.
pub fn install_job_control_signals(prog: &str) {
    for sig in [libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU] {
        let prev = unsafe { libc::signal(sig, libc::SIG_IGN) };
        if prev == libc::SIG_ERR {
            crate::emit_cli_error(prog, format_args!("warning: could not ignore signal {sig}"));
        }
    }
    // Rust's runtime sets SIGPIPE to SIG_IGN at startup; restore the OS default
    // so huck (and the stages it forks) die on a broken pipe like bash, instead
    // of getting EPIPE back from write(2) and looping. bash runs with SIGPIPE at
    // SIG_DFL everywhere; an interactive shell survives because its stdout is the
    // terminal, never a pipe. (v137)
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL); }
}

/// Fires `$PROMPT_COMMAND` if set, non-empty, and the shell is
/// interactive. Returns `Some(exit_code)` when PROMPT_COMMAND
/// returns `ExecOutcome::Exit` (e.g. `PROMPT_COMMAND='exit 7'`) —
/// the outer REPL handles the shell-exit cleanup. Returns `None`
/// otherwise; on `Continue`, updates `shell.last_status` so PS1's
/// `\?` and the next user command's `$?` both reflect
/// PROMPT_COMMAND's exit code (matches bash). Non-interactive
/// shells skip entirely.
///
/// An indexed-array `PROMPT_COMMAND` runs each NON-EMPTY element in
/// ascending index order (bash 5.1+); the first element that exits the
/// shell short-circuits the rest. A scalar runs as a single command.
pub fn fire_prompt_command(shell: &mut Shell) -> Option<i32> {
    if !shell.is_interactive {
        return None;
    }
    // Collect owned strings first so the immutable borrow ends before the
    // `&mut shell` process_line calls below.
    let commands: Vec<String> = if let Some(map) = shell.get_indexed("PROMPT_COMMAND") {
        map.values().filter(|s| !s.is_empty()).cloned().collect()
    } else {
        match shell.lookup_var("PROMPT_COMMAND") {
            Some(s) if !s.is_empty() => vec![s.to_string()],
            _ => return None,
        }
    };
    if commands.is_empty() {
        return None;
    }
    for cmd in commands {
        match process_line(&cmd, shell, true) {
            ExecOutcome::Exit(code) => return Some(code),
            ExecOutcome::Continue(status) => shell.set_last_status(status),
            _ => {}
        }
    }
    None
}

/// Tokenizes, parses, and executes a single input line.
pub fn process_line_in_sinks(
    line: &str,
    shell: &mut Shell,
    expand_aliases: bool,
    sink: &mut crate::executor::StdoutSink,
    err_sink: &mut crate::executor::StderrSink,
) -> ExecOutcome {
    // This is a top-level BATCH parse of a COMPLETE program string (piped-stdin
    // logical command, `eval`, a trap/PROMPT_COMMAND action). Like bash, an open
    // here-document is delimited by end-of-input rather than erroring. The
    // interactive REPL continuation check runs earlier in `classify` (its own
    // lexer keeps `eof_closes_heredoc=false`, so it still returns
    // `Incomplete(Heredoc)` and prompts) — by the time a buffer reaches here it is
    // a complete logical command, so closing at EOF matches bash.
    let opts = lexer::LexerOptions {
        extglob: shell.extglob(),
        eof_closes_heredoc: true,
        ..Default::default()
    };
    // Build a live lexer that expands aliases at command position as the parser
    // reads tokens. For non-interactive / non-alias paths, use an empty alias map
    // so the live lexer is alias-free (byte-identical to the old token pre-pass).
    let empty = std::collections::HashMap::new();
    let aliases = if expand_aliases { &shell.aliases } else { &empty };
    let mut lx = lexer::Lexer::new_live_atoms(line, aliases, opts);
    match parser::parse_sequence(&mut lx) {
        Ok(Some(sequence)) => executor::execute_with_sink(&sequence, shell, line, sink, err_sink),
        Ok(None) => ExecOutcome::Continue(0),
        Err(e) => {
            // No AST-carried position for an immediate (non-EOF) parse error on
            // a single logical command: derive the line from the live lexer's
            // cursor, counting newlines within THIS command's own text. Matches
            // bash for the common (first/only command) case; a longer piped-
            // stdin session with cumulative line tracking is a separate gap
            // (bash counts lines across the whole non-interactive input, which
            // huck's per-command REPL loop does not track today).
            let off = lx.cursor_pos().min(line.len());
            let ln = 1 + line.as_bytes()[..off].iter().filter(|&&b| b == b'\n').count() as u32;
            crate::err_thread_local::install_err_sinks(sink, err_sink, || {
                crate::emit_syntax_error(shell, ln, format_args!("syntax error: {}", crate::parse_error_message(&e)));
            });
            ExecOutcome::Continue(2)
        }
    }
}

/// Terminal-sink wrapper around [`process_line_in_sinks`] — the entry point for
/// callers (REPL, traps, helpers) that run at top level (stdout → terminal).
pub fn process_line(line: &str, shell: &mut Shell, expand_aliases: bool) -> ExecOutcome {
    let mut sink = crate::executor::StdoutSink::Terminal;
    let mut err_sink = crate::executor::StderrSink::Terminal;
    process_line_in_sinks(line, shell, expand_aliases, &mut sink, &mut err_sink)
}

#[cfg(test)]
mod prompt_command_tests {
    use super::*;
    use crate::shell_state::Shell;

    fn interactive_shell() -> Shell {
        let mut shell = Shell::new();
        shell.is_interactive = true;
        shell
    }

    #[test]
    fn fires_when_set() {
        let mut shell = interactive_shell();
        shell.set("PROMPT_COMMAND", "true".to_string());
        assert_eq!(fire_prompt_command(&mut shell), None);
        assert_eq!(shell.last_status(), 0);
    }

    #[test]
    fn last_status_reflects_pc() {
        let mut shell = interactive_shell();
        shell.set("PROMPT_COMMAND", "false".to_string());
        assert_eq!(fire_prompt_command(&mut shell), None);
        assert_eq!(shell.last_status(), 1);
    }

    #[test]
    fn no_op_when_unset() {
        let mut shell = interactive_shell();
        shell.set_last_status(42);
        assert_eq!(fire_prompt_command(&mut shell), None);
        assert_eq!(shell.last_status(), 42);
    }

    #[test]
    fn no_op_when_empty() {
        let mut shell = interactive_shell();
        shell.set("PROMPT_COMMAND", String::new());
        shell.set_last_status(42);
        assert_eq!(fire_prompt_command(&mut shell), None);
        assert_eq!(shell.last_status(), 42);
    }

    #[test]
    fn propagates_exit() {
        let mut shell = interactive_shell();
        shell.set("PROMPT_COMMAND", "exit 7".to_string());
        assert_eq!(fire_prompt_command(&mut shell), Some(7));
    }

    #[test]
    fn silent_when_non_interactive() {
        let mut shell = Shell::new();
        shell.is_interactive = false;
        shell.set("PROMPT_COMMAND", "false".to_string());
        shell.set_last_status(42);
        assert_eq!(fire_prompt_command(&mut shell), None);
        // last_status unchanged since PC didn't run.
        assert_eq!(shell.last_status(), 42);
    }

    fn arr(elems: &[&str]) -> std::collections::BTreeMap<usize, String> {
        elems.iter().enumerate().map(|(i, s)| (i, s.to_string())).collect()
    }

    #[test]
    fn array_runs_all_elements_in_order() {
        let mut shell = interactive_shell();
        // literal element strings — NOT pre-expanded; each runs as a command later.
        shell
            .replace_indexed("PROMPT_COMMAND", arr(&["ORDER=${ORDER}a", "ORDER=${ORDER}b"]))
            .unwrap();
        assert_eq!(fire_prompt_command(&mut shell), None);
        assert_eq!(shell.get("ORDER"), Some("ab"), "both elements ran, in order");
    }

    #[test]
    fn array_skips_empty_elements() {
        let mut shell = interactive_shell();
        shell.replace_indexed("PROMPT_COMMAND", arr(&["MA=1", "", "MB=1"])).unwrap();
        assert_eq!(fire_prompt_command(&mut shell), None);
        assert_eq!(shell.get("MA"), Some("1"));
        assert_eq!(shell.get("MB"), Some("1"));
    }

    #[test]
    fn array_propagates_exit_and_stops() {
        let mut shell = interactive_shell();
        shell.replace_indexed("PROMPT_COMMAND", arr(&["MA=1", "exit 7", "MB=1"])).unwrap();
        assert_eq!(fire_prompt_command(&mut shell), Some(7));
        assert_eq!(shell.get("MA"), Some("1"), "element before exit ran");
        assert_eq!(shell.get("MB"), None, "element after exit did NOT run");
    }

    #[test]
    fn array_last_status_reflects_last_element() {
        let mut shell = interactive_shell();
        shell.replace_indexed("PROMPT_COMMAND", arr(&["true", "false"])).unwrap();
        assert_eq!(fire_prompt_command(&mut shell), None);
        assert_eq!(shell.last_status(), 1, "last element (false) sets $?");
    }
}

#[cfg(test)]
mod rc_tests {
    use super::*;
    use crate::shell_state::Shell;

    // ── CLI parser ─────────────────────────────────────────────

    #[test]
    fn parse_cli_empty() {
        let opts = parse_cli(&[]).unwrap();
        assert!(!opts.norc);
        assert!(opts.rcfile_path.is_none());
        assert_eq!(opts.mode, RunMode::Interactive);
    }

    #[test]
    fn parse_cli_norc() {
        let opts = parse_cli(&["--norc".to_string()]).unwrap();
        assert!(opts.norc);
        assert!(opts.rcfile_path.is_none());
    }

    #[test]
    fn parse_cli_noexec_flag() {
        // `-n` alone (no script) → noexec set, Interactive mode.
        let o = parse_cli(&["-n".to_string()]).unwrap();
        assert!(o.noexec);
        assert_eq!(o.mode, RunMode::Interactive);
        // `-n script.sh args` → noexec + File mode.
        let o = parse_cli(&["-n".to_string(), "s.sh".to_string(), "a".to_string()]).unwrap();
        assert!(o.noexec);
        assert_eq!(o.mode, RunMode::File { path: "s.sh".into(), args: vec!["a".into()] });
        // `-n -c 'echo'` → noexec + Command mode.
        let o = parse_cli(&["-n".to_string(), "-c".to_string(), "echo hi".to_string()]).unwrap();
        assert!(o.noexec);
        assert_eq!(o.mode, RunMode::Command { command: "echo hi".into(), argv0: None, args: vec![] });
        // default: no -n → noexec false.
        let o = parse_cli(&["-c".to_string(), "echo".to_string()]).unwrap();
        assert!(!o.noexec);
    }

    #[test]
    fn parse_cli_posix_flag() {
        let o = parse_cli(&["--posix".into(), "script.sh".into()]).unwrap();
        assert!(o.posix);
        assert_eq!(o.mode, RunMode::File { path: PathBuf::from("script.sh"), args: vec![] });
    }

    #[test]
    fn parse_cli_posix_default_off() {
        let o = parse_cli(&["script.sh".into()]).unwrap();
        assert!(!o.posix);
    }

    #[test]
    fn startup_posix_sources() {
        assert!(startup_posix(true, "/usr/bin/huck", false), "--posix");
        assert!(startup_posix(false, "/bin/sh", false), "invoked as sh");
        assert!(startup_posix(false, "sh", false), "argv0 bare sh");
        assert!(startup_posix(false, "/usr/bin/huck", true), "POSIXLY_CORRECT");
        assert!(!startup_posix(false, "/usr/bin/huck", false), "none → off");
        assert!(!startup_posix(false, "/usr/bin/bash", false), "bash basename → off");
    }

    #[test]
    fn parse_cli_rcfile_separate() {
        let opts = parse_cli(&[
            "--rcfile".to_string(),
            "/x".to_string(),
        ]).unwrap();
        assert_eq!(opts.rcfile_path, Some(std::path::PathBuf::from("/x")));
        assert!(!opts.norc);
    }

    #[test]
    fn parse_cli_rcfile_joined() {
        let opts = parse_cli(&["--rcfile=/x".to_string()]).unwrap();
        assert_eq!(opts.rcfile_path, Some(std::path::PathBuf::from("/x")));
    }

    #[test]
    fn parse_cli_unknown_errors() {
        assert!(parse_cli(&["--bogus".to_string()]).is_err());
    }

    #[test]
    fn parse_cli_rcfile_no_arg_errors() {
        assert!(parse_cli(&["--rcfile".to_string()]).is_err());
    }

    // ── RunMode resolution (new in v82) ────────────────────────

    #[test]
    fn cli_bare_double_dash_is_interactive() {
        // `--` with no following operand ends options and leaves Interactive.
        let o = parse_cli(&["--".into()]).unwrap();
        assert_eq!(o.mode, RunMode::Interactive);
    }

    #[test]
    fn cli_file_mode_sets_path_and_args() {
        let o = parse_cli(&["s.sh".into(), "a".into(), "b".into()]).unwrap();
        assert_eq!(o.mode, RunMode::File { path: "s.sh".into(), args: vec!["a".into(), "b".into()] });
    }

    #[test]
    fn cli_dash_c_first_operand_is_argv0() {
        let o = parse_cli(&["-c".into(), "echo hi".into(), "name".into(), "x".into()]).unwrap();
        assert_eq!(o.mode, RunMode::Command {
            command: "echo hi".into(),
            argv0: Some("name".into()),
            args: vec!["x".into()],
        });
    }

    #[test]
    fn cli_dash_c_no_operands_argv0_none() {
        let o = parse_cli(&["-c".into(), "echo hi".into()]).unwrap();
        assert_eq!(o.mode, RunMode::Command { command: "echo hi".into(), argv0: None, args: vec![] });
    }

    #[test]
    fn cli_dash_c_requires_argument() {
        assert!(parse_cli(&["-c".into()]).is_err());
    }

    #[test]
    fn cli_double_dash_ends_options_for_file() {
        // `--` lets a dash-leading name be the script path.
        let o = parse_cli(&["--".into(), "-weird".into(), "a".into()]).unwrap();
        assert_eq!(o.mode, RunMode::File { path: "-weird".into(), args: vec!["a".into()] });
    }

    #[test]
    fn cli_operands_after_c_are_verbatim_including_dashdash() {
        // After `-c CMD`, operands are taken verbatim: `--` becomes $0, `-x` becomes $1.
        let o = parse_cli(&["-c".into(), "cmd".into(), "--".into(), "-x".into()]).unwrap();
        assert_eq!(o.mode, RunMode::Command {
            command: "cmd".into(), argv0: Some("--".into()), args: vec!["-x".into()],
        });
    }

    #[test]
    fn cli_unknown_leading_flag_errors() {
        assert!(parse_cli(&["-x".into()]).is_err());
    }

    #[test]
    fn cli_dash_c_precedence_over_file() {
        // `-c` wins; the operand is $0, not a script path.
        let o = parse_cli(&["-c".into(), "cmd".into(), "file.sh".into()]).unwrap();
        assert_eq!(o.mode, RunMode::Command { command: "cmd".into(), argv0: Some("file.sh".into()), args: vec![] });
    }

    #[test]
    fn cli_norc_then_file_still_parses() {
        let o = parse_cli(&["--norc".into(), "s.sh".into()]).unwrap();
        assert!(o.norc);
        assert_eq!(o.mode, RunMode::File { path: "s.sh".into(), args: vec![] });
    }

    #[test]
    fn parse_cli_version_long() {
        let opts = parse_cli(&["--version".to_string()]).expect("parse");
        assert!(matches!(opts.mode, RunMode::PrintVersion));
    }

    #[test]
    fn parse_cli_version_short() {
        let opts = parse_cli(&["-V".to_string()]).expect("parse");
        assert!(matches!(opts.mode, RunMode::PrintVersion));
    }

    // ── rc loader ──────────────────────────────────────────────

    fn write_tempfile(contents: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        let nonce = format!(
            "huck-rc-test-{}-{}",
            std::process::id(),
            // Use the test's address as a pseudo-random discriminator
            // without relying on rand/time.
            contents.as_ptr() as usize,
        );
        path.push(nonce);
        std::fs::write(&path, contents).expect("write tempfile");
        path
    }

    #[test]
    fn rc_skips_when_norc() {
        let mut shell = Shell::new();
        shell.is_interactive = true;
        let p = write_tempfile("export HUCK_RC_TEST_ABC=hello\n");
        let opts = CliOptions {
            rcfile_path: Some(p.clone()),
            norc: true,
            noexec: false,
            posix: false,
            mode: RunMode::Interactive,
        };
        assert_eq!(maybe_source_rc_file(&mut shell, &opts), None);
        assert!(shell.lookup_var("HUCK_RC_TEST_ABC").is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn rc_skips_when_non_interactive() {
        let mut shell = Shell::new();
        shell.is_interactive = false;
        let p = write_tempfile("export HUCK_RC_TEST_DEF=hello\n");
        let opts = CliOptions {
            rcfile_path: Some(p.clone()),
            norc: false,
            noexec: false,
            posix: false,
            mode: RunMode::Interactive,
        };
        assert_eq!(maybe_source_rc_file(&mut shell, &opts), None);
        assert!(shell.lookup_var("HUCK_RC_TEST_DEF").is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn rc_sources_explicit_path() {
        let mut shell = Shell::new();
        shell.is_interactive = true;
        let p = write_tempfile("export HUCK_RC_TEST_GHI=hello\n");
        let opts = CliOptions {
            rcfile_path: Some(p.clone()),
            norc: false,
            noexec: false,
            posix: false,
            mode: RunMode::Interactive,
        };
        assert_eq!(maybe_source_rc_file(&mut shell, &opts), None);
        assert_eq!(
            shell.lookup_var("HUCK_RC_TEST_GHI").as_deref(),
            Some("hello"),
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn rc_explicit_missing_errors() {
        let mut shell = Shell::new();
        shell.is_interactive = true;
        let opts = CliOptions {
            rcfile_path: Some(std::path::PathBuf::from(
                "/no/such/file/huck_rc_does_not_exist",
            )),
            norc: false,
            noexec: false,
            posix: false,
            mode: RunMode::Interactive,
        };
        assert_eq!(maybe_source_rc_file(&mut shell, &opts), Some(1));
    }

    #[test]
    fn rc_explicit_exit_propagates() {
        let mut shell = Shell::new();
        shell.is_interactive = true;
        let p = write_tempfile("exit 42\n");
        let opts = CliOptions {
            rcfile_path: Some(p.clone()),
            norc: false,
            noexec: false,
            posix: false,
            mode: RunMode::Interactive,
        };
        assert_eq!(maybe_source_rc_file(&mut shell, &opts), Some(42));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn rc_default_missing_silent() {
        // When --rcfile is unset, $HUCK_RC is unset/empty, and the
        // default ~/.huckrc doesn't exist (here: HOME unset so
        // default_rc_path returns None), the loader must silently
        // return None — no error message, no non-zero status.
        let mut shell = Shell::new();
        shell.is_interactive = true;
        // Empty HOME → default_rc_path's filter(|s| !s.is_empty())
        // drops it and the chain returns None. Also clear HUCK_RC
        // in case the test environment exports one.
        shell.set("HOME", String::new());
        shell.set("HUCK_RC", String::new());
        let opts = CliOptions::default();
        // Process env may still have HOME set; the shell's local
        // empty HOME wins per lookup_var precedence, so default_rc_path
        // gets the empty string and returns None. But std::env::var
        // is consulted as fallback — guard by also clearing it
        // for the duration of this test.
        let saved_home = std::env::var("HOME").ok();
        let saved_huck_rc = std::env::var("HUCK_RC").ok();
        unsafe {
            std::env::remove_var("HOME");
            std::env::remove_var("HUCK_RC");
        }
        let result = maybe_source_rc_file(&mut shell, &opts);
        unsafe {
            if let Some(h) = saved_home { std::env::set_var("HOME", h); }
            if let Some(r) = saved_huck_rc { std::env::set_var("HUCK_RC", r); }
        }
        assert_eq!(result, None);
    }
}
