use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rustyline::config::Configurer;
use rustyline::error::ReadlineError;
use rustyline::history::FileHistory;
use rustyline::{CompletionType, Config, Editor};
use signal_hook::consts::{SIGCHLD, SIGINT};

use crate::builtins::ExecOutcome;
use crate::command::{self};
use crate::completion::HuckHelper;
use crate::executor;
use crate::lexer::{self};
use crate::shell_state::Shell;

const DEFAULT_PS1: &str = "huck> ";
const DEFAULT_PS2: &str = "> ";

/// The outcome of reading one logical (possibly multi-line) command.
enum ReadResult {
    /// A finished command: `buffer` is fed to the executor, `history`
    /// is its single-line form for the history list.
    Ready { buffer: String, history: String },
    /// Ctrl-C — any partial command is discarded; the REPL loops.
    Interrupted,
    /// Ctrl-D at an empty first-line prompt — exit the shell cleanly.
    Eof,
    /// EOF while a partial command was pending — a truncated command.
    EofMidCommand,
    /// A rustyline read error — exit the shell.
    ReadError(String),
}

/// How the shell was invoked — resolved by `parse_cli` from argv.
#[derive(Debug, PartialEq, Eq)]
enum RunMode {
    /// REPL (tty) or piped-stdin command reading — current behavior.
    Interactive,
    /// `-c COMMAND [NAME [ARG...]]`: argv0 = NAME (None → keep the shell's
    /// default $0), args = the rest.
    Command { command: String, argv0: Option<String>, args: Vec<String> },
    /// `SCRIPT [ARG...]`: $0 = path, args = the rest.
    File { path: PathBuf, args: Vec<String> },
}

struct CliOptions {
    rcfile_path: Option<PathBuf>,
    norc: bool,
    /// `-n`: read and parse commands without executing them (noexec / syntax
    /// check). Applied to `shell_options.noexec`; honored only non-interactively.
    noexec: bool,
    mode: RunMode,
}

impl Default for CliOptions {
    fn default() -> Self {
        CliOptions {
            rcfile_path: None,
            norc: false,
            noexec: false,
            mode: RunMode::Interactive,
        }
    }
}

fn parse_cli(args: &[String]) -> Result<CliOptions, String> {
    let mut rcfile_path: Option<PathBuf> = None;
    let mut norc = false;
    let mut noexec = false;
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

    Ok(CliOptions { rcfile_path, norc, noexec, mode })
}

fn default_rc_path(shell: &Shell) -> Option<std::path::PathBuf> {
    let home = shell
        .lookup_var("HOME")
        .or_else(|| std::env::var("HOME").ok())
        .filter(|s| !s.is_empty())?;
    Some(std::path::PathBuf::from(home).join(".huckrc"))
}

fn maybe_source_rc_file(shell: &mut Shell, opts: &CliOptions) -> Option<i32> {
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
            eprintln!("huck: {}: No such file or directory", path.display());
            return Some(1);
        }
        return None;
    }
    let contents = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("huck: {}: {}", path.display(), e);
            return Some(1);
        }
    };
    match crate::builtins::run_sourced_contents(&contents, &path, shell) {
        crate::builtins::ExecOutcome::Exit(code) => Some(code),
        crate::builtins::ExecOutcome::Continue(status) => {
            shell.set_last_status(status);
            None
        }
        _ => None,
    }
}

/// Executes a non-interactive program (a `-c` string or a script file's
/// contents) and returns the process exit code. Sets $0 and the positional
/// parameters, marks the shell non-interactive (so the rc file is skipped),
/// runs the program through the shared `run_sourced_contents` engine, fires the
/// EXIT trap, and hangs up jobs. Does NOT touch interactive history or the
/// line editor.
///
/// When `push_main_frame` is true (script-file mode), a base `FrameKind::Main`
/// frame is pushed before executing and popped after, so that BASH_SOURCE and
/// BASH_LINENO are populated at the top level and FUNCNAME gains the `main`
/// entry inside functions. For `-c` and other non-file modes pass `false`.
fn run_program(
    contents: &str,
    argv0: Option<String>,
    args: Vec<String>,
    label: &str,
    push_main_frame: bool,
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

    let outcome = crate::builtins::run_sourced_contents(
        contents,
        std::path::Path::new(label),
        &mut shell,
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
        ExecOutcome::Continue(s) => shell.take_pending_fatal_pe_error().unwrap_or(s),
        ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_) => 0,
        ExecOutcome::Interrupted => 130,
    };
    crate::traps::fire_exit_trap(&mut shell);
    shell.hangup_jobs();
    code
}

/// Runs the interactive shell loop. Returns the process exit code.
pub fn run(args: &[String]) -> i32 {
    let opts = match parse_cli(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("huck: {e}");
            return 2;
        }
    };

    install_job_control_signals();

    let shell_cell = Rc::new(RefCell::new(Shell::new()));

    {
        let shell = shell_cell.borrow();
        install_sigint_handler(Arc::clone(&shell.sigint_flag));
        install_sigchld_handler(Arc::clone(&shell.sigchld_flag));
    }

    // `-n`: parse-only (noexec). Honored only in non-interactive modes; the
    // run_command guard checks `is_interactive`, and `is_interactive` is set
    // false for Command/File modes (an interactive REPL keeps executing).
    shell_cell.borrow_mut().shell_options.noexec = opts.noexec;

    // Non-interactive program modes bypass the REPL entirely.
    match opts.mode {
        RunMode::Command { command, argv0, args } => {
            let label = argv0
                .clone()
                .unwrap_or_else(|| shell_cell.borrow().shell_argv0.clone());
            return run_program(&command, argv0, args, &label, false, &shell_cell);
        }
        RunMode::File { path, args } => {
            let contents = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    let (msg, code) = if e.kind() == std::io::ErrorKind::NotFound {
                        ("No such file or directory".to_string(), 127)
                    } else {
                        (e.to_string(), 126)
                    };
                    eprintln!("huck: {}: {msg}", path.display());
                    return code;
                }
            };
            let label = path.display().to_string();
            return run_program(&contents, Some(label.clone()), args, &label, true, &shell_cell);
        }
        RunMode::Interactive => {}
    }

    // ----- interactive / piped-stdin REPL (unchanged below this line) -----
    let config = Config::builder()
        .completion_type(CompletionType::List)
        .build();
    let mut editor: Editor<HuckHelper, FileHistory> = match Editor::with_config(config) {
        Ok(editor) => editor,
        Err(e) => {
            eprintln!("huck: failed to initialize line editor: {e}");
            return 1;
        }
    };

    {
        let mut shell = shell_cell.borrow_mut();
        Rc::make_mut(&mut shell.history).load();
        for (_, command) in shell.history.entries() {
            let _ = editor.add_history_entry(command);
        }
    }

    editor.set_helper(Some(HuckHelper::new(Rc::clone(&shell_cell))));

    {
        let mut shell = shell_cell.borrow_mut();
        if let Some(exit_code) = maybe_source_rc_file(&mut shell, &opts) {
            crate::traps::fire_exit_trap(&mut shell);
            shell.hangup_jobs();
            shell.save_history();
            return exit_code;
        }
        // v139: re-apply the in-memory cap now that ~/.huckrc may have set HISTSIZE
        // (history was loaded before rc). Nets out to bash's rc-then-history effect.
        let cap = shell.resolve_histsize();
        Rc::make_mut(&mut shell.history).set_max(cap);
    }

    loop {
        apply_readline_settings(&mut editor, &shell_cell);
        {
            let mut shell = shell_cell.borrow_mut();
            crate::jobs::reap_and_notify(&mut shell);
            crate::traps::dispatch_pending_traps(&mut shell);
        }
        {
            let mut shell = shell_cell.borrow_mut();
            if let Some(exit_code) = fire_prompt_command(&mut shell) {
                crate::traps::fire_exit_trap(&mut shell);
                shell.hangup_jobs();
                shell.save_history();
                return exit_code;
            }
        }
        match read_logical_command(&mut editor, &shell_cell) {
            ReadResult::Ready { buffer, history } => {
                {
                    let mut shell = shell_cell.borrow_mut();
                    if !history.trim().is_empty() {
                        shell.record_history(history.clone());
                        let _ = editor.add_history_entry(history.as_str());
                    }
                }
                let do_alias = {
                    let shell = shell_cell.borrow();
                    shell.is_interactive
                        || std::env::var("HUCK_EXPAND_ALIASES").is_ok()
                };
                let outcome = {
                    let mut shell = shell_cell.borrow_mut();
                    process_line(&buffer, &mut shell, do_alias)
                };
                match outcome {
                    ExecOutcome::Exit(code) => {
                        let mut shell = shell_cell.borrow_mut();
                        crate::traps::fire_exit_trap(&mut shell);
                        shell.hangup_jobs();
                        shell.save_history();
                        return code;
                    }
                    ExecOutcome::Continue(status) => {
                        let mut shell = shell_cell.borrow_mut();
                        shell.set_last_status(status);
                        // Drain any fatal PE error. In non-interactive mode
                        // (stdin not a TTY), exit immediately with the fatal
                        // status. Interactive: $? already set; fall through
                        // to the next prompt iteration.
                        if let Some(fatal_status) = shell.take_pending_fatal_pe_error()
                            && !shell.is_interactive
                        {
                            crate::traps::fire_exit_trap(&mut shell);
                            shell.hangup_jobs();
                            shell.save_history();
                            return fatal_status;
                        }
                    }
                    ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_)
                    | ExecOutcome::FunctionReturn(_) => {
                        let mut shell = shell_cell.borrow_mut();
                        shell.set_last_status(0)
                    }
                    ExecOutcome::Interrupted => {
                        let mut shell = shell_cell.borrow_mut();
                        shell.set_last_status(130);
                        eprintln!();
                    }
                }
            }
            ReadResult::Interrupted => continue,
            ReadResult::Eof => {
                let mut shell = shell_cell.borrow_mut();
                crate::traps::fire_exit_trap(&mut shell);
                shell.hangup_jobs();
                shell.save_history();
                return shell.last_status();
            }
            ReadResult::EofMidCommand => {
                eprintln!("huck: syntax error: unexpected end of input");
                let mut shell = shell_cell.borrow_mut();
                crate::traps::fire_exit_trap(&mut shell);
                shell.hangup_jobs();
                shell.save_history();
                return 2;
            }
            ReadResult::ReadError(msg) => {
                eprintln!("huck: input error: {msg}");
                let mut shell = shell_cell.borrow_mut();
                crate::traps::fire_exit_trap(&mut shell);
                shell.hangup_jobs();
                return 1;
            }
        }
    }
}

/// Applies any pending `bind` settings (editor-mapped vars + key (un)binds)
/// from the shell to the live rustyline editor, then clears the dirty flag.
/// No-op when nothing is dirty.
fn apply_readline_settings(
    editor: &mut Editor<HuckHelper, FileHistory>,
    shell_cell: &Rc<RefCell<Shell>>,
) {
    let mut shell = shell_cell.borrow_mut();
    if !shell.readline_settings.dirty {
        return;
    }

    // 1. Editor-mapped variables.
    if let Some(v) = shell.readline_settings.vars.get("editing-mode") {
        let mode = if v == "vi" { rustyline::EditMode::Vi } else { rustyline::EditMode::Emacs };
        editor.set_edit_mode(mode);
    }
    if let Some(v) = shell.readline_settings.vars.get("bell-style") {
        let style = match v.as_str() {
            "none" => rustyline::config::BellStyle::None,
            "visible" => rustyline::config::BellStyle::Visible,
            _ => rustyline::config::BellStyle::Audible,
        };
        editor.set_bell_style(style);
    }
    if let Some(v) = shell.readline_settings.vars.get("show-all-if-ambiguous") {
        editor.set_completion_show_all_if_ambiguous(v == "on");
    }
    // Numeric vars: parse as i64 and CLAMP to the setter's range so an
    // out-of-range value (the validator accepts any integer, like bash) still
    // applies clamped rather than being silently dropped.
    if let Some(n) = shell.readline_settings.vars.get("completion-query-items").and_then(|s| s.parse::<i64>().ok()) {
        editor.set_completion_prompt_limit(n.max(0) as usize);
    }
    if let Some(n) = shell.readline_settings.vars.get("keyseq-timeout").and_then(|s| s.parse::<i64>().ok()) {
        editor.set_keyseq_timeout(Some(n.clamp(0, u16::MAX as i64) as u16));
    }

    // 2. Pending key binds.
    let binds = std::mem::take(&mut shell.readline_settings.pending_binds);
    for (seq, func) in binds {
        if let (Some(event), Some(cmd)) = (
            crate::readline_bind::parse_keyseq(&seq),
            crate::readline_bind::function_to_cmd(&func),
        ) {
            editor.bind_sequence(event, cmd);
            shell.readline_settings.active_binds.insert(seq, func);
        }
    }
    // 3. Pending unbinds.
    let unbinds = std::mem::take(&mut shell.readline_settings.pending_unbinds);
    for seq in unbinds {
        if let Some(event) = crate::readline_bind::parse_keyseq(&seq) {
            editor.unbind_sequence(event);
            shell.readline_settings.active_binds.remove(&seq);
        }
    }
    shell.readline_settings.dirty = false;
}

/// Reads one logical command, gathering continuation lines until the
/// accumulated buffer classifies as `Complete` or a genuine `Error`.
fn read_logical_command(
    editor: &mut Editor<HuckHelper, FileHistory>,
    cell: &RefCell<Shell>,
) -> ReadResult {
    use crate::continuation::{classify, joiner_for, Completeness};

    let mut buffer = String::new();
    let mut history = String::new();
    // The reason the buffer-so-far is incomplete, and the line that
    // caused it — together they pick the joiner for the next line.
    let mut pending: Option<(crate::continuation::ContinuationReason, String)> = None;

    loop {
        let expanded = {
            let mut shell = cell.borrow_mut();
            let (var_name, default) = if pending.is_none() {
                ("PS1", DEFAULT_PS1)
            } else {
                ("PS2", DEFAULT_PS2)
            };
            let template = shell
                .lookup_var(var_name)
                .unwrap_or_else(|| default.to_string());
            // Rendering a prompt must be transparent to $? (bash saves/restores it).
            let saved_status = shell.last_status();
            let saved_cmd_sub = shell.last_cmd_sub_status();
            let saved_xtrace = shell.shell_options.xtrace;
            shell.shell_options.xtrace = false;
            let s = crate::prompt::expand_prompt(&template, &mut shell);
            shell.shell_options.xtrace = saved_xtrace;
            shell.set_last_status(saved_status);
            shell.set_last_cmd_sub_status(saved_cmd_sub);
            s
        };

        // rustyline measures prompt width from the `raw` half of a (raw, styled)
        // Prompt and ignores `\x01`/`\x02` markers; pass the visible-only string for
        // measurement and the full styled string for display (B-01). For a marker-free
        // prompt the two are identical, so plain prompts are unaffected.
        let measured = crate::prompt::prompt_raw(&expanded);
        match editor.readline(&(measured, expanded)) {
            Ok(raw) => {
                // History expansion runs per physical line, as before.
                let line = {
                    let mut shell = cell.borrow_mut();
                    match crate::history::expand(&raw, &shell.history) {
                        Ok(None) => raw,
                        Ok(Some(expanded)) => {
                            println!("{expanded}");
                            expanded
                        }
                        Err(e) => {
                            eprintln!("huck: {e}");
                            shell.set_last_status(1);
                            return ReadResult::Interrupted;
                        }
                    }
                };

                // `set -v` verbose: echo each physical input line to stderr as
                // it is read, before it is parsed/executed.
                if cell.borrow().shell_options.verbose {
                    eprintln!("{line}");
                }

                match pending.take() {
                    None => {
                        // First physical line.
                        buffer.push_str(&line);
                        history.push_str(&line);
                    }
                    Some((reason, prev_line)) => {
                        // `buffer` joins with a real newline, except a
                        // backslash continuation which joins with nothing.
                        if reason != crate::continuation::ContinuationReason::Backslash {
                            buffer.push('\n');
                        }
                        buffer.push_str(&line);
                        history.push_str(joiner_for(reason, &prev_line));
                        history.push_str(&line);
                    }
                }

                match classify(&buffer, cell.borrow().shopt_options.get("extglob").unwrap_or(false)) {
                    Completeness::Complete | Completeness::Error => {
                        return ReadResult::Ready { buffer, history };
                    }
                    Completeness::Incomplete(reason) => {
                        if reason == crate::continuation::ContinuationReason::Backslash {
                            // Drop the unescaped trailing backslash from
                            // both accumulators before the next line.
                            buffer.pop();
                            history.pop();
                        }
                        pending = Some((reason, line));
                    }
                }
            }
            Err(ReadlineError::Interrupted) => return ReadResult::Interrupted,
            Err(ReadlineError::Eof) => {
                return if buffer.is_empty() {
                    ReadResult::Eof
                } else {
                    ReadResult::EofMidCommand
                };
            }
            Err(e) => return ReadResult::ReadError(e.to_string()),
        }
    }
}

/// Installs a SIGINT handler that sets the supplied flag. Called once at
/// startup after `Shell::new()`; the flag lives on the `Shell` so the wait
/// builtin can poll it to break out of its loop when Ctrl-C is pressed.
fn install_sigint_handler(flag: Arc<AtomicBool>) {
    if let Err(e) = signal_hook::flag::register(SIGINT, flag) {
        eprintln!("huck: warning: could not install SIGINT handler: {e}");
    }
}

/// Installs a SIGCHLD handler that toggles the supplied flag. Called once
/// at startup; the flag lives on the `Shell` so the reap path can poll it.
fn install_sigchld_handler(flag: Arc<AtomicBool>) {
    if let Err(e) = signal_hook::flag::register(SIGCHLD, flag) {
        eprintln!("huck: warning: could not install SIGCHLD handler: {e}");
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
fn install_job_control_signals() {
    for sig in [libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU] {
        let prev = unsafe { libc::signal(sig, libc::SIG_IGN) };
        if prev == libc::SIG_ERR {
            eprintln!("huck: warning: could not ignore signal {sig}");
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
pub fn process_line_in_sink(
    line: &str,
    shell: &mut Shell,
    expand_aliases: bool,
    sink: &mut crate::executor::StdoutSink,
) -> ExecOutcome {
    let opts = lexer::LexerOptions { extglob: shell.shopt_options.get("extglob").unwrap_or(false) };
    let (tokens, _offsets, lex_lines) = match lexer::tokenize_with_offsets(line, opts) {
        Ok((tokens, offsets, lines)) => (tokens, offsets, lines),
        Err((e, _off)) => {
            eprintln!("huck: syntax error{}", crate::lex_error_message(e));
            return ExecOutcome::Continue(2);
        }
    };
    // Per-token source lines stamped directly by the lexer (true O(n), no second pass).
    // lex_lines.len() == tokens.len() + 1 (includes sentinel); slice to token count.
    let lines: Vec<u32> = lex_lines[..tokens.len()].to_vec();
    let (tokens, lines) = if expand_aliases {
        match crate::alias_expand::expand_aliases_in_tokens(tokens, &shell.aliases) {
            Ok(t) => {
                // If alias expansion changed the token count, fall back to zeros
                // rather than corrupting the line index.
                let l = if t.len() == lines.len() { lines } else { vec![0; t.len()] };
                (t, l)
            }
            Err(e) => {
                eprintln!("huck: syntax error{}", crate::lex_error_message(e));
                return ExecOutcome::Continue(2);
            }
        }
    } else {
        (tokens, lines)
    };

    match command::parse_with_lines(tokens, lines) {
        Ok(Some(sequence)) => executor::execute_with_sink(&sequence, shell, line, sink),
        Ok(None) => ExecOutcome::Continue(0),
        Err(e) => {
            eprintln!("huck: syntax error: {}", crate::parse_error_message(e));
            ExecOutcome::Continue(2)
        }
    }
}

/// Terminal-sink wrapper around [`process_line_in_sink`] — the entry point for
/// callers (REPL, traps, helpers) that run at top level (stdout → terminal).
pub fn process_line(line: &str, shell: &mut Shell, expand_aliases: bool) -> ExecOutcome {
    let mut sink = crate::executor::StdoutSink::Terminal;
    process_line_in_sink(line, shell, expand_aliases, &mut sink)
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
