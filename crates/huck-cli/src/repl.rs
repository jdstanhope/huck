//! huck's interactive REPL: the rustyline-driven read loop, the editor-apply
//! path for `bind` settings, and the logical-command reader. Runs over the
//! terminal-free `huck_engine`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use rustyline::config::Configurer;
use rustyline::error::ReadlineError;
use rustyline::history::FileHistory;
use rustyline::{CompletionType, Config, Editor};

use huck_engine::builtins::ExecOutcome;
use huck_engine::shell::{
    fire_prompt_command, install_job_control_signals, install_sigchld_handler,
    install_sigint_handler, maybe_source_rc_file, parse_cli, process_line, RunMode,
};
use huck_engine::shell_state::Shell;

use crate::completion_helper::HuckHelper;
use crate::readline_apply::{function_to_cmd, parse_keyseq};

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
            let mut engine = huck_engine::Engine::from_shell_cell(Rc::clone(&shell_cell));
            if let Some(a0) = argv0 {
                engine.set_arg0(&a0);
            }
            engine.set_args(args);
            return engine.run(&command);
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
            let mut engine = huck_engine::Engine::from_shell_cell(Rc::clone(&shell_cell));
            engine.set_args(args);
            return engine.run_script(&contents, &label);
        }
        RunMode::PrintVersion => return 0,
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
            huck_engine::traps::fire_exit_trap(&mut shell);
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
            huck_engine::jobs::reap_and_notify(&mut shell);
            huck_engine::traps::dispatch_pending_traps(&mut shell);
        }
        {
            let mut shell = shell_cell.borrow_mut();
            if let Some(exit_code) = fire_prompt_command(&mut shell) {
                huck_engine::traps::fire_exit_trap(&mut shell);
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
                        huck_engine::traps::fire_exit_trap(&mut shell);
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
                            huck_engine::traps::fire_exit_trap(&mut shell);
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
                    ExecOutcome::Interrupted(_) => {
                        let mut shell = shell_cell.borrow_mut();
                        shell.set_last_status(130);
                        eprintln!();
                    }
                }
            }
            ReadResult::Interrupted => continue,
            ReadResult::Eof => {
                let mut shell = shell_cell.borrow_mut();
                huck_engine::traps::fire_exit_trap(&mut shell);
                shell.hangup_jobs();
                shell.save_history();
                return shell.last_status();
            }
            ReadResult::EofMidCommand => {
                eprintln!("huck: syntax error: unexpected end of input");
                let mut shell = shell_cell.borrow_mut();
                huck_engine::traps::fire_exit_trap(&mut shell);
                shell.hangup_jobs();
                shell.save_history();
                return 2;
            }
            ReadResult::ReadError(msg) => {
                eprintln!("huck: input error: {msg}");
                let mut shell = shell_cell.borrow_mut();
                huck_engine::traps::fire_exit_trap(&mut shell);
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
            parse_keyseq(&seq),
            function_to_cmd(&func),
        ) {
            editor.bind_sequence(event, cmd);
            shell.readline_settings.active_binds.insert(seq, func);
        }
    }
    // 3. Pending unbinds.
    let unbinds = std::mem::take(&mut shell.readline_settings.pending_unbinds);
    for seq in unbinds {
        if let Some(event) = parse_keyseq(&seq) {
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
    use huck_engine::continuation::{classify, joiner_for, Completeness};

    let mut buffer = String::new();
    let mut history = String::new();
    // The reason the buffer-so-far is incomplete, and the line that
    // caused it — together they pick the joiner for the next line.
    let mut pending: Option<(huck_engine::continuation::ContinuationReason, String)> = None;

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
            let s = huck_engine::prompt::expand_prompt(&template, &mut shell);
            shell.shell_options.xtrace = saved_xtrace;
            shell.set_last_status(saved_status);
            shell.set_last_cmd_sub_status(saved_cmd_sub);
            s
        };

        // rustyline measures prompt width from the `raw` half of a (raw, styled)
        // Prompt and ignores `\x01`/`\x02` markers; pass the visible-only string for
        // measurement and the full styled string for display (B-01). For a marker-free
        // prompt the two are identical, so plain prompts are unaffected.
        let measured = huck_engine::prompt::prompt_raw(&expanded);
        match editor.readline(&(measured, expanded)) {
            Ok(raw) => {
                // History expansion runs per physical line, as before.
                let line = {
                    let mut shell = cell.borrow_mut();
                    match huck_engine::history::expand(&raw, &shell.history) {
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
                        if reason != huck_engine::continuation::ContinuationReason::Backslash {
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
                        if reason == huck_engine::continuation::ContinuationReason::Backslash {
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
