use std::env;
use std::io::Write;
use std::path::Path;
use std::rc::Rc;

use crate::command::DeclArg;
use crate::shell_state::{SHOPT_TABLE, Shell};

/// Why an executor run was interrupted. Used to discriminate the top-level
/// exit code mapping (SIGINT -> 130, ExecBuilder::timeout -> 124).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptReason {
    Sigint,
    Timeout,
    /// v312/v313: a fatal error that DISCARDS the current top-level command
    /// (bash `jump_to_top_level(DISCARD)`) — unwind out of loops/functions,
    /// status 1, but the shell is NOT exited. Raised by a fatal `$(( ))`
    /// expansion error (#3) and a readonly-variable assignment error (#31);
    /// contained at execution boundaries; the driver loop continues on it.
    DiscardCommand,
}

/// The result of running a command — either the shell continues (carrying the
/// command's exit status) or the shell should terminate with a code.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ExecOutcome {
    Continue(i32),
    Exit(i32),
    LoopBreak(u32, i32), // (level: 1-based capped to loop_depth, terminal $?: 0 normal / 1 malformed-arg)
    LoopContinue(u32),
    FunctionReturn(i32),
    /// v138: an untrapped SIGINT was observed — abort the running command list.
    /// Propagates like `Exit` until a top-level consumer (REPL reprompts with
    /// `$?`=130 and does NOT exit; `-c`/script exits 130).
    /// v206: carries an `InterruptReason` so the top-level reducer can
    /// distinguish SIGINT (130) from `ExecBuilder::timeout` (124).
    Interrupted(InterruptReason),
}

pub const BUILTIN_NAMES: &[&str] = &[
    "cd",
    "exit",
    "pwd",
    "echo",
    "export",
    "unset",
    "jobs",
    "wait",
    "fg",
    "bg",
    "kill",
    "disown",
    "history",
    "test",
    "[",
    "break",
    "continue",
    "return",
    "trap",
    "alias",
    "unalias",
    "set",
    "shopt",
    "shift",
    "getopts",
    ".",
    "source",
    "local",
    ":",
    "true",
    "false",
    "command",
    "builtin",
    "exec",
    "readonly",
    "read",
    "mapfile",
    "readarray",
    "printf",
    "type",
    "hash",
    "pushd",
    "popd",
    "dirs",
    "declare",
    "typeset",
    "eval",
    "let",
    "help",
    "complete",
    "compgen",
    "compopt",
    "bind",
    "umask",
    "ulimit",
    "times",
    "enable",
];

pub fn is_builtin(name: &str) -> bool {
    BUILTIN_NAMES.contains(&name)
}

/// True if `name` is a known builtin that is currently ENABLED (not turned off
/// via `enable -n`). Command dispatch and `type`/`command -v` use this so a
/// disabled builtin falls through to the external command. `enable`'s validity
/// check and the `builtin` forcing builtin use `is_builtin` (name known) instead.
pub fn builtin_active(name: &str, shell: &Shell) -> bool {
    is_builtin(name) && !shell.disabled_builtins.contains(name)
}

/// True for "declaration commands" (bash terminology). Their
/// assignment-shaped args (`a=(x y)`, `a[i]+=v`) are parsed as
/// `Assignment`s and routed through `apply_one_assignment`, NOT
/// expanded as ordinary Words. Non-assignment args (flags like
/// `-a`, bare names) flow through normal expansion. See `resolve()`
/// in src/executor.rs for the split logic.
pub fn is_declaration_command(name: &str) -> bool {
    matches!(
        name,
        "declare" | "typeset" | "local" | "readonly" | "export"
    )
}

/// True for POSIX "special builtins" (2.14). Inline assignments preceding a
/// special builtin persist in the shell; assignments preceding a regular
/// builtin or external command are scoped to the command. The set is huck's
/// existing builtins intersected with the POSIX special list; expand here as
/// huck adds `exec`.
pub fn is_special_builtin(name: &str) -> bool {
    matches!(
        name,
        ":" | "."
            | "break"
            | "continue"
            | "eval"
            | "exec"
            | "exit"
            | "export"
            | "readonly"
            | "return"
            | "set"
            | "shift"
            | "source"
            | "times"
            | "trap"
            | "unset"
    )
}

/// Runs a builtin. Caller must ensure `is_builtin(name)` is true. `out` is the
/// destination for any stdout the builtin produces (`echo`, `pwd`); `cd` and
/// `exit` produce no stdout and ignore it.
pub fn run_builtin(
    name: &str,
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    // Declaration commands (`declare`, `typeset`, `local`, `readonly`,
    // `export`) must flow through `run_declaration_builtin` so that
    // compound-RHS assignments (`a=(x y z)`, `a[i]+=v`) reach
    // `apply_one_assignment`. The executor's `is_declaration_command`
    // predicate routes them there; this debug_assert is a tripwire so a
    // future refactor that bypasses the predicate doesn't silently end
    // up here, where the legacy paths are array-unaware.
    debug_assert!(
        !is_declaration_command(name),
        "declaration command `{name}` reached run_builtin; should have been routed to run_declaration_builtin",
    );
    match name {
        "cd" => builtin_cd(args, out, err, shell),
        "pwd" => builtin_pwd(args, out, err, shell),
        "echo" => builtin_echo(args, out, err, shell),
        "exit" => {
            let outcome = builtin_exit(args, err, shell);
            // POSIX case #1: `exit <non-numeric>` is a usage error (the only
            // Continue(2) exit produces; a valid `exit N` is ExecOutcome::Exit).
            if matches!(outcome, ExecOutcome::Continue(2)) {
                shell.builtin_usage_error = Some(2);
            }
            outcome
        }
        "unset" => builtin_unset(args, err, shell),
        "jobs" => builtin_jobs(args, out, err, shell),
        "wait" => builtin_wait(args, out, err, shell),
        "fg" => builtin_fg(args, err, shell),
        "bg" => builtin_bg(args, out, err, shell),
        "kill" => builtin_kill(args, out, err, shell),
        "disown" => builtin_disown(args, err, shell),
        "history" => builtin_history(args, out, err, shell),
        "trap" => builtin_trap(args, out, err, shell),
        "set" => builtin_set(args, out, err, shell),
        "shopt" => builtin_shopt(args, out, err, shell),
        "shift" => builtin_shift(args, err, shell),
        "getopts" => builtin_getopts(args, err, shell),
        "." | "source" => builtin_source(args, err, shell),
        "eval" => builtin_eval(args, shell),
        "let" => builtin_let(args, err, shell),
        "help" => builtin_help(args, out, err, shell),
        "complete" => crate::completion_builtins::builtin_complete(args, out, err, shell),
        "compgen" => crate::completion_builtins::builtin_compgen(args, out, err, shell),
        "compopt" => crate::completion_builtins::builtin_compopt(args, out, err, shell),
        "alias" => builtin_alias(args, out, err, shell),
        "unalias" => builtin_unalias(args, err, shell),
        ":" => builtin_colon(args, shell),
        "true" => builtin_true(args, shell),
        "false" => builtin_false(args, shell),
        "command" => builtin_command(args, out, err, shell),
        // `builtin` is normally consumed by the executor's strip loop before
        // dispatch; this guards a bare `builtin` that reaches run_builtin.
        "builtin" => ExecOutcome::Continue(0),
        // `exec` is intercepted by the executor (run_exec_single) before dispatch
        // — it replaces the process image / applies permanent redirects, which
        // this (name, args, out, shell) signature can't express. Guard against a
        // future refactor routing it here so it degrades instead of panicking.
        "exec" => {
            crate::sh_error_to!(shell, err, None, "exec: not supported in this context");
            ExecOutcome::Continue(1)
        }
        "type" => builtin_type(args, out, err, shell),
        "hash" => builtin_hash(args, out, err, shell),
        "pushd" => builtin_pushd(args, out, err, shell),
        "popd" => builtin_popd(args, out, err, shell),
        "dirs" => builtin_dirs(args, out, err, shell),
        "read" => builtin_read(args, out, err, shell),
        "mapfile" | "readarray" => builtin_mapfile(args, err, shell),
        "printf" => builtin_printf(args, out, err, shell),
        "test" | "[" => builtin_test(name, args, err, shell),
        "break" => builtin_break(args, err, shell),
        "continue" => builtin_continue(args, err, shell),
        "return" => {
            // POSIX case #1: `return` outside a function or sourced script is a
            // usage error (bash: "can only `return' from a function or sourced
            // script"). A legitimate `return N` (inside a Function/Source frame)
            // leaves the signal unset. Detected here (builtin_return takes &Shell).
            let in_fn_or_source = shell.call_stack.iter().any(|f| {
                matches!(
                    f.kind,
                    crate::shell_state::FrameKind::Function | crate::shell_state::FrameKind::Source
                )
            });
            if !in_fn_or_source {
                shell.builtin_usage_error = Some(2);
            }
            builtin_return(args, shell)
        }
        "bind" => builtin_bind(args, out, err, shell),
        "umask" => builtin_umask(args, out, err, shell),
        "ulimit" => builtin_ulimit(args, out, err, shell),
        "times" => builtin_times(args, out, err, shell),
        "enable" => builtin_enable(args, out, err, shell),
        _ => unreachable!("run_builtin called with non-builtin: {name}"),
    }
}

/// Parses the loop-level argument for `break` / `continue`.
/// `Ok(N)` is the validated positive level (defaults to 1 with no args).
/// `Err(outcome)` is the `ExecOutcome` to return immediately, after the
/// diagnostic has already been printed.
///
/// Bash 5.2 semantics for the (already-in-a-loop) argument:
/// - Too many args (`break 1 2 3`): prints "too many arguments", breaks ALL
///   enclosing loops with terminal $?=1; script continues (`BreakAll`).
/// - Non-numeric arg (e.g. `break abc`): prints "numeric argument required",
///   aborts the whole script with status 128 (`Fatal`).
/// - Numeric but out-of-range (e.g. `break 0`, `break -1`): prints "loop count
///   out of range", breaks ALL enclosing loops with terminal $?=1; script
///   continues (`BreakAll`).
/// - Valid N>=1: `Level(N)` (not yet capped to loop_depth).
enum LoopArg {
    Level(u32),
    BreakAll,
    Fatal,
}

/// Classifies break/continue args per bash 5.2, printing the matching
/// diagnostic. Caller has already verified loop_depth > 0.
fn classify_loop_arg(args: &[String], cmd: &str, err: &mut dyn Write, shell: &Shell) -> LoopArg {
    if args.len() > 1 {
        crate::sh_error_to!(shell, err, None, "{cmd}: too many arguments");
        return LoopArg::BreakAll;
    }
    let Some(arg) = args.first() else {
        return LoopArg::Level(1);
    };
    match arg.parse::<i64>() {
        Ok(n) if n >= 1 => LoopArg::Level(n.min(u32::MAX as i64) as u32),
        Ok(_) => {
            crate::sh_error_to!(shell, err, None, "{cmd}: {arg}: loop count out of range");
            LoopArg::BreakAll
        }
        Err(_) => {
            crate::sh_error_to!(shell, err, None, "{cmd}: {arg}: numeric argument required");
            LoopArg::Fatal
        }
    }
}

fn builtin_break(args: &[String], err: &mut dyn Write, shell: &Shell) -> ExecOutcome {
    if shell.loop_depth == 0 {
        crate::sh_error_to!(
            shell,
            err,
            None,
            "break: only meaningful in a `for', `while', or `until' loop"
        );
        return ExecOutcome::Continue(0);
    }
    match classify_loop_arg(args, "break", err, shell) {
        LoopArg::Level(n) => ExecOutcome::LoopBreak(n.min(shell.loop_depth), 0),
        LoopArg::BreakAll => ExecOutcome::LoopBreak(shell.loop_depth, 1),
        LoopArg::Fatal => ExecOutcome::Exit(128),
    }
}

fn builtin_continue(args: &[String], err: &mut dyn Write, shell: &Shell) -> ExecOutcome {
    if shell.loop_depth == 0 {
        crate::sh_error_to!(
            shell,
            err,
            None,
            "continue: only meaningful in a `for', `while', or `until' loop"
        );
        return ExecOutcome::Continue(0);
    }
    match classify_loop_arg(args, "continue", err, shell) {
        LoopArg::Level(n) => ExecOutcome::LoopContinue(n.min(shell.loop_depth)),
        // out-of-range/too-many continue breaks all loops, like bash
        LoopArg::BreakAll => ExecOutcome::LoopBreak(shell.loop_depth, 1),
        LoopArg::Fatal => ExecOutcome::Exit(128),
    }
}

/// `return [N]` builtin. Sets the exit status to N (or `$?` if N is
/// omitted or unparseable) and returns `FunctionReturn(code)` so the
/// enclosing function unwinds. Behavior preserved from the v0 inline
/// implementation — extracted to a named helper for symmetry with
/// builtin_break and builtin_continue.
fn builtin_return(args: &[String], shell: &Shell) -> ExecOutcome {
    let code = match args.first() {
        Some(s) => s.parse::<i32>().unwrap_or_else(|_| shell.last_status()),
        None => shell.last_status(),
    };
    ExecOutcome::FunctionReturn(code)
}

/// Test-only convenience: call `run_declaration_builtin` from string
/// args. Strings shaped like `NAME=value` (valid identifier on the
/// left) are wrapped as `DeclArg::Assign` with a single-Literal value
/// — mirroring what the executor produces from a parsed assignment
/// word. Everything else (flags, bare names, invalid identifiers)
/// becomes `DeclArg::Plain`. Compound-RHS coverage (`a=(x y)`,
/// `a[i]+=v`) lives in integration tests where the lexer can build
/// the actual `ArrayLiteral` / `AssignPrefix` parts.
#[cfg(test)]
pub(crate) fn run_declaration_builtin_strs(
    name: &str,
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    use crate::command::{AssignTarget, Assignment};
    use crate::lexer::{Word, WordPart};

    fn is_valid_ident(s: &str) -> bool {
        let mut chars = s.chars();
        match chars.next() {
            Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
            _ => return false,
        }
        chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
    }

    let decl_args: Vec<DeclArg> = args
        .iter()
        .map(|s| match s.find('=') {
            Some(eq) if is_valid_ident(&s[..eq]) => {
                let name = s[..eq].to_string();
                let val = s[eq + 1..].to_string();
                DeclArg::Assign(Assignment {
                    target: AssignTarget::Bare(name),
                    value: Word(vec![WordPart::Literal {
                        text: val,
                        quoted: false,
                    }]),
                    append: false,
                })
            }
            _ => DeclArg::Plain(s.clone()),
        })
        .collect();
    run_declaration_builtin(name, &decl_args, out, err, shell)
}

/// Entry point for declaration commands (`declare` / `typeset` / `local` /
/// `readonly` / `export`). Differs from `run_builtin` by passing `DeclArg`s
/// instead of pre-expanded `String`s: assignment-shaped args arrive as
/// parsed `Assignment` records so compound-RHS (`a=(x y z)`) flows through
/// `apply_one_assignment`, mirroring the path used by ordinary assignment
/// commands. Caller must ensure `is_declaration_command(name)` is true.
pub fn run_declaration_builtin(
    name: &str,
    decl_args: &[DeclArg],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    match name {
        "declare" | "typeset" => builtin_declare_decl(decl_args, out, err, shell),
        "local" => builtin_local_decl(decl_args, err, shell),
        "readonly" => builtin_readonly_decl(decl_args, out, err, shell),
        "export" => builtin_export_decl(decl_args, out, err, shell),
        _ => unreachable!("run_declaration_builtin called with non-declaration: {name}"),
    }
}

/// Lexically normalizes an ABSOLUTE path for logical `cd`: collapses `.`,
/// empty components (from `//`), and `..` (removing the preceding component
/// WITHOUT resolving symlinks). A `..` at the root is dropped (bash behavior).
/// Always returns an absolute path; `/` for an empty result.
fn normalize_logical(path: &str) -> String {
    let mut components: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                // cd always passes an absolute path here, so `..` is never on
                // the stack — a non-empty stack means a real parent to pop.
                if !components.is_empty() {
                    components.pop();
                }
            }
            other => components.push(other),
        }
    }
    if components.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", components.join("/"))
    }
}

pub(crate) fn builtin_cd(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    builtin_cd_as("cd", args, out, err, shell)
}

/// The `cd` implementation, parameterized on the reporting name. `pushd`/
/// `popd` delegate their actual directory-change step here (bash's own
/// `pushd`/`popd` are NOT thin `cd` wrappers — they have entirely separate
/// option grammars for `-n`/`+N`/`-N` — but the successful-parse chdir
/// failure path (`<dir>: No such file or directory`, etc.) is the same
/// underlying operation bash reports under the CALLER's name, not `cd:`).
fn builtin_cd_as(
    caller: &str,
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    if crate::restricted::is_restricted(shell)
        && let Err(msg) = crate::restricted::check_cd()
    {
        crate::sh_error_to!(shell, err, None, "{msg}");
        return ExecOutcome::Continue(1);
    }
    // 1. Parse leading -L/-P flags (last wins) and `--`. `-` is NOT a flag (it
    //    is the OLDPWD shortcut / target).
    let mut physical_flag: Option<bool> = None;
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "-L" => {
                physical_flag = Some(false);
                idx += 1;
            }
            "-P" => {
                physical_flag = Some(true);
                idx += 1;
            }
            "--" => {
                idx += 1;
                break;
            }
            "-" => break, // OLDPWD shortcut, handled as the target below
            s if s.starts_with('-') && s.len() > 1 => {
                crate::sh_error_to!(shell, err, None, "cd: {s}: invalid option");
                e!(err, "cd: usage: cd [-L|[-P [-e]] [-@]] [dir]");
                return ExecOutcome::Continue(2);
            }
            _ => break, // a target
        }
    }
    let rest = &args[idx..];
    if rest.len() > 1 {
        crate::sh_error_to!(shell, err, None, "cd: too many arguments");
        return ExecOutcome::Continue(1);
    }

    // 2. Effective mode: explicit flag, else the `physical` set-option.
    let physical = physical_flag.unwrap_or_else(|| option_get(shell, "physical").unwrap_or(false));

    // 3. Compute the target directory.
    let mut print_new_pwd = false;
    let target = match rest.first() {
        Some(dir) if dir == "-" => match shell.get("OLDPWD") {
            Some(oldpwd) if !oldpwd.is_empty() => {
                print_new_pwd = true;
                oldpwd.to_string()
            }
            _ => {
                crate::sh_error_to!(shell, err, None, "cd: OLDPWD not set");
                return ExecOutcome::Continue(1);
            }
        },
        Some(dir) => dir.clone(),
        None => match shell.get("HOME") {
            Some(home) => home.to_string(),
            None => {
                crate::sh_error_to!(shell, err, None, "cd: HOME not set");
                return ExecOutcome::Continue(1);
            }
        },
    };

    let prev_pwd = shell.get("PWD").map(str::to_string);

    let new_pwd: String = if physical {
        // Physical: chdir to the target, store the canonical cwd.
        if let Err(e) = env::set_current_dir(Path::new(&target)) {
            crate::sh_error_to!(
                shell,
                err,
                Some(caller),
                "{target}: {}",
                crate::bash_io_error(&e)
            );
            return ExecOutcome::Continue(1);
        }
        match env::current_dir() {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(e) => {
                crate::sh_error_to!(
                    shell,
                    err,
                    None,
                    "cd: warning: could not read current dir: {}",
                    crate::bash_io_error(&e)
                );
                prev_pwd.clone().unwrap_or_default()
            }
        }
    } else {
        // Logical: build curpath from $PWD (for relative targets), lexically
        // normalize, chdir to the normalized path, store it.
        let curpath = if target.starts_with('/') {
            target.clone()
        } else {
            let base = prev_pwd
                .clone()
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| {
                    env::current_dir()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default()
                });
            format!("{base}/{target}")
        };
        let normalized = normalize_logical(&curpath);
        if let Err(e) = env::set_current_dir(Path::new(&normalized)) {
            crate::sh_error_to!(
                shell,
                err,
                Some(caller),
                "{target}: {}",
                crate::bash_io_error(&e)
            );
            return ExecOutcome::Continue(1);
        }
        normalized
    };

    // 4. Maintain OLDPWD / PWD.
    if let Some(prev) = prev_pwd {
        shell.export_set("OLDPWD", prev);
    }
    shell.export_set("PWD", new_pwd.clone());

    // 5. `cd -` prints the new directory.
    if print_new_pwd && writeln!(out, "{new_pwd}").is_err() {
        // v308: reported once by the run_builtin_with_redirects epilogue.
        return ExecOutcome::Continue(1);
    }
    ExecOutcome::Continue(0)
}

fn builtin_pwd(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    // Parse -L/-P (last wins); `--` ends flags; non-flag args are ignored
    // (bash prints pwd anyway). Unknown flag → invalid option, rc 2.
    let mut physical_flag: Option<bool> = None;
    for a in args {
        match a.as_str() {
            "-L" => physical_flag = Some(false),
            "-P" => physical_flag = Some(true),
            "--" => break,
            s if s.starts_with('-') && s.len() > 1 => {
                crate::sh_error_to!(shell, err, None, "pwd: {s}: invalid option");
                e!(err, "pwd: usage: pwd [-LP]");
                return ExecOutcome::Continue(2);
            }
            _ => {} // ignore non-flag args
        }
    }
    let physical = physical_flag.unwrap_or_else(|| option_get(shell, "physical").unwrap_or(false));

    let path: String = if physical {
        // Resolved physical path.
        match env::current_dir() {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(_) => shell
                .get("PWD")
                .and_then(|p| std::fs::canonicalize(p).ok())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    } else {
        // Logical: use $PWD only if it is valid (canonicalises to the real
        // cwd) — mirrors bash's pwd -L validation.  An inherited $PWD that
        // doesn't match the process cwd (e.g. because the shell was spawned
        // with current_dir() but without updating $PWD) is silently
        // discarded and we fall back to getcwd().
        let real_cwd = env::current_dir().ok();
        let logical = shell.get("PWD").filter(|p| !p.is_empty()).and_then(|p| {
            let canon = std::fs::canonicalize(p).ok()?;
            if real_cwd.as_deref() == Some(canon.as_path()) {
                Some(p.to_string())
            } else {
                None
            }
        });
        logical.unwrap_or_else(|| {
            real_cwd
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
    };

    if writeln!(out, "{path}").is_err() {
        // v308: the write error is reported once, by the run_builtin_with_redirects
        // epilogue (it holds the recorded errno). Stop writing; stay silent.
        return ExecOutcome::Continue(1);
    }
    ExecOutcome::Continue(0)
}

fn builtin_echo(
    args: &[String],
    out: &mut dyn Write,
    _err: &mut dyn Write,
    _shell: &Shell,
) -> ExecOutcome {
    let (mut suppress_newline, process_escapes, consumed) = parse_echo_flags(args);
    let joined = args[consumed..].join(" ");
    let mut bytes = if process_escapes {
        let (b, hit_c) = process_echo_escapes(&joined);
        if hit_c {
            suppress_newline = true;
        }
        b
    } else {
        joined.into_bytes()
    };

    // #208: the whole line (content + newline) must reach the fd in ONE
    // write(2) call, or two concurrent backgrounded `echo`s can interleave
    // between the content write and the newline write. Append the newline
    // to the buffer instead of writing it separately.
    if !suppress_newline {
        bytes.push(b'\n');
    }
    if out.write_all(&bytes).is_err() {
        // v308: reported once by the epilogue (see pwd above).
        return ExecOutcome::Continue(1);
    }
    ExecOutcome::Continue(0)
}

fn parse_echo_flags(args: &[String]) -> (bool, bool, usize) {
    let mut suppress_newline = false;
    let mut process_escapes = false;
    let mut idx = 0;
    while idx < args.len() {
        let arg = &args[idx];
        if arg.len() < 2 || !arg.starts_with('-') {
            break;
        }
        let rest = &arg[1..];
        if !rest.chars().all(|c| matches!(c, 'n' | 'e' | 'E')) {
            break;
        }
        for c in rest.chars() {
            match c {
                'n' => suppress_newline = true,
                'e' => process_escapes = true,
                'E' => process_escapes = false,
                _ => unreachable!(),
            }
        }
        idx += 1;
    }
    (suppress_newline, process_escapes, idx)
}

fn process_echo_escapes(s: &str) -> (Vec<u8>, bool) {
    let mut out = Vec::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut buf = [0u8; 4];
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next() {
            None => out.push(b'\\'),
            Some('a') => out.push(0x07),
            Some('b') => out.push(0x08),
            Some('c') => return (out, true),
            Some('e') => out.push(0x1B),
            Some('f') => out.push(0x0C),
            Some('n') => out.push(0x0A),
            Some('r') => out.push(0x0D),
            Some('t') => out.push(0x09),
            Some('v') => out.push(0x0B),
            Some('\\') => out.push(b'\\'),
            Some('0') => {
                let mut value: u32 = 0;
                for _ in 0..3 {
                    let Some(&d) = chars.peek() else { break };
                    let Some(n) = d.to_digit(8) else { break };
                    value = value * 8 + n;
                    chars.next();
                }
                out.push((value & 0xFF) as u8);
            }
            Some('x') => {
                let mut value: u32 = 0;
                let mut consumed = 0;
                for _ in 0..2 {
                    let Some(&d) = chars.peek() else { break };
                    let Some(n) = d.to_digit(16) else { break };
                    value = value * 16 + n;
                    chars.next();
                    consumed += 1;
                }
                if consumed == 0 {
                    out.extend_from_slice(b"\\x");
                } else {
                    out.push(value as u8);
                }
            }
            Some(other) => {
                out.push(b'\\');
                out.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    (out, false)
}

#[cfg(test)]
mod echo_atomic_tests {
    use super::*;

    struct RecordingWriter {
        calls: Vec<Vec<u8>>,
    }
    impl std::io::Write for RecordingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.calls.push(buf.to_vec());
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn echo_calls(args: &[&str]) -> Vec<Vec<u8>> {
        let shell = Shell::new();
        // `args[0]` is the pseudo-argv0 ("echo"), for readability at call
        // sites; `builtin_echo` itself takes only the arguments after the
        // command name (see `run_builtin`'s `"echo" => builtin_echo(args,
        // ...)` dispatch, where `args` is already name-stripped).
        let owned: Vec<String> = args[1..].iter().map(|s| s.to_string()).collect();
        let mut rec = RecordingWriter { calls: Vec::new() };
        let mut sink: Vec<u8> = Vec::new();
        let _ = builtin_echo(&owned, &mut rec, &mut sink, &shell);
        rec.calls
    }

    #[test]
    fn echo_writes_line_in_one_call() {
        // The whole line (content + newline) must arrive in ONE write() call, so
        // concurrent backgrounded echoes can't interleave between them (#208).
        assert_eq!(echo_calls(&["echo", "hi"]), vec![b"hi\n".to_vec()]);
    }

    #[test]
    fn echo_n_writes_content_only_one_call() {
        assert_eq!(echo_calls(&["echo", "-n", "hi"]), vec![b"hi".to_vec()]);
    }

    #[test]
    fn echo_no_args_writes_just_newline_one_call() {
        assert_eq!(echo_calls(&["echo"]), vec![b"\n".to_vec()]);
    }

    #[test]
    fn echo_n_empty_issues_no_write() {
        // v308 zero-byte rule: empty output must not issue a write() at all.
        assert_eq!(echo_calls(&["echo", "-n", ""]), Vec::<Vec<u8>>::new());
    }
}

fn builtin_exit(args: &[String], err: &mut dyn Write, shell: &Shell) -> ExecOutcome {
    match args.first() {
        None => ExecOutcome::Exit(shell.last_status()),
        Some(code_str) => match code_str.parse::<i32>() {
            Ok(code) => ExecOutcome::Exit(code.rem_euclid(256)),
            Err(_) => {
                crate::sh_error_to!(
                    shell,
                    err,
                    None,
                    "exit: {code_str}: numeric argument required"
                );
                ExecOutcome::Continue(2)
            }
        },
    }
}

pub(crate) fn is_valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn builtin_unset(args: &[String], err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    // Leading flags select the namespace and apply to all following names:
    // `-f` => function namespace, `-v` (or no flag) => variable namespace.
    // `-n` => variable namespace but unset the nameref variable ITSELF (no deref).
    let mut mode_fn = false;
    let mut unset_nameref = false;
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "-f" => {
                mode_fn = true;
                idx += 1;
            }
            "-v" => {
                mode_fn = false;
                idx += 1;
            }
            "-n" => {
                unset_nameref = true;
                idx += 1;
            }
            "--" => {
                idx += 1;
                break;
            }
            s if s.len() > 1 && s.starts_with('-') => {
                crate::sh_error_to!(shell, err, None, "unset: {s}: invalid option");
                // POSIX case #1: bad option is a usage error (the "cannot unset
                // readonly" path below is runtime and stays unmarked).
                shell.builtin_usage_error = Some(2);
                return ExecOutcome::Continue(2);
            }
            _ => break,
        }
    }
    let names = &args[idx..];
    let mut any_error = false;
    for arg in names {
        if mode_fn {
            // Function namespace: remove if present. Identifier validity is
            // still enforced (bash rejects e.g. `unset -f 1bad`), but an
            // absent function name is success (no error), matching bash. No
            // readonly/array-subscript handling applies here.
            if !is_valid_name(arg) {
                crate::sh_error_to!(shell, err, None, "unset: '{arg}': not a valid identifier");
                any_error = true;
                continue;
            }
            shell.remove_function(arg);
            continue;
        }
        // `unset -n NAME`: remove the nameref variable ITSELF, without dereffing.
        // On a non-nameref, bash silently does nothing (the var survives). Matches bash.
        if unset_nameref {
            if !shell.is_nameref(arg) {
                // Not a nameref: bash no-ops silently. Skip.
                continue;
            }
            if !is_valid_name(arg) {
                crate::sh_error_to!(shell, err, None, "unset: '{arg}': not a valid identifier");
                any_error = true;
                continue;
            }
            if shell.is_readonly(arg) {
                crate::sh_error_to!(shell, err, None, "unset: {arg}: readonly variable");
                any_error = true;
                continue;
            }
            shell.unset_var(arg);
            continue;
        }
        // `unset NAME` where NAME is a nameref: resolve to the target and unset that.
        // For a chain (a→b→c), resolve_nameref follows to the end, so we unset c.
        let resolved_owned: String;
        let effective_arg: &str = if shell.is_nameref(arg) {
            match shell.resolve_nameref(arg) {
                crate::shell_state::ResolvedName::Name(n) => {
                    resolved_owned = n;
                    &resolved_owned
                }
                crate::shell_state::ResolvedName::Element {
                    name: base,
                    subscript,
                } => {
                    resolved_owned = format!("{base}[{subscript}]");
                    &resolved_owned
                }
                // Unbound or cycle: nothing to unset, skip silently (matches bash).
                crate::shell_state::ResolvedName::Unbound(_)
                | crate::shell_state::ResolvedName::Cycle => continue,
            }
        } else {
            arg
        };
        match parse_subscripted_arg(effective_arg) {
            Ok(Some((name, sub_text))) => {
                // `unset a[i]`: remove a single element. The subscript is
                // parsed as a synthetic literal `Word` so subscript
                // evaluation matches a real expansion. When `a` is
                // associative, the subscript is the string key directly;
                // otherwise it's arith-evaluated as an index.
                let sub_word = crate::lexer::Word(vec![crate::lexer::WordPart::Literal {
                    text: sub_text.to_string(),
                    quoted: false,
                }]);
                if shell.get_associative(name).is_some() {
                    let key = crate::expand::eval_subscript_key(&sub_word, shell);
                    if shell.unset_associative_element(name, &key).is_err() {
                        any_error = true;
                    }
                } else {
                    match crate::expand::eval_subscript(&sub_word, shell, name) {
                        Ok(idx) => {
                            if shell.unset_indexed_element(name, idx).is_err() {
                                any_error = true;
                            }
                        }
                        Err(e) => {
                            crate::sh_error_to!(shell, err, None, "unset: {e}");
                            any_error = true;
                        }
                    }
                }
                continue;
            }
            Ok(None) => {}
            Err(e) => {
                crate::sh_error_to!(shell, err, None, "unset: {e}");
                any_error = true;
                continue;
            }
        }
        if !is_valid_name(effective_arg) {
            crate::sh_error_to!(
                shell,
                err,
                None,
                "unset: '{effective_arg}': not a valid identifier"
            );
            any_error = true;
            continue;
        }
        if shell.is_readonly(effective_arg) {
            crate::sh_error_to!(
                shell,
                err,
                None,
                "unset: {effective_arg}: readonly variable"
            );
            any_error = true;
            continue;
        }
        shell.unset_var(effective_arg);
    }
    if any_error {
        ExecOutcome::Continue(1)
    } else {
        ExecOutcome::Continue(0)
    }
}

/// If `s` has the form `NAME[SUB]` where NAME is a valid identifier
/// and `SUB` is non-empty, returns `Ok(Some((NAME, SUB)))`. If `s` has
/// no `[` at all, returns `Ok(None)` so the caller falls through to the
/// whole-variable unset path. Otherwise returns `Err(diagnostic)` —
/// e.g. `a[`, `a[]`, or `1foo[i]` — matching bash's "bad array subscript"
/// / "not a valid identifier" diagnostics for `unset`.
pub(crate) fn parse_subscripted_arg(s: &str) -> Result<Option<(&str, &str)>, String> {
    let Some(bracket) = s.find('[') else {
        return Ok(None);
    };
    if !s.ends_with(']') {
        return Err(format!("`{s}': bad array subscript"));
    }
    let name = &s[..bracket];
    if !is_valid_name(name) {
        return Err(format!("`{s}': not a valid identifier"));
    }
    let sub = &s[bracket + 1..s.len() - 1];
    if sub.is_empty() {
        return Err(format!("`{s}': bad array subscript"));
    }
    Ok(Some((name, sub)))
}

// ─────────────────────────────────────────────────────────────
// declare / typeset (v64) — see spec
// `docs/superpowers/specs/2026-05-31-huck-declare-design.md`.
// ─────────────────────────────────────────────────────────────

/// Backslash-escape `"`, `\`, `$`, and backtick for safe embedding
/// inside a double-quoted value (used by `format_declare_line`).
/// bash's variable-listing quoting (the bare `declare` / `set` / `set -x`
/// style): bare unless the value needs quoting; a value with a shell
/// metacharacter is single-quoted (with `'` rewritten `'\''`); a value with a
/// control char uses ANSI-C `$'…'`; the EMPTY value is bare (`name=`). This is
/// NOT `${v@Q}` (which always quotes); it mirrors bash's `sh_contains_shell_metas`
/// + `sh_single_quote`.
pub(crate) fn declare_scalar_quote(v: &str) -> String {
    if v.is_empty() {
        return String::new();
    }
    if v.chars().any(|c| c.is_control()) {
        return crate::param_expansion::ansi_c_quote(v);
    }
    if crate::param_expansion::contains_shell_metas(v) {
        // bash's `sh_single_quote` special-cases a value that is exactly one
        // single-quote character: it backslash-escapes it (`\'`) instead of
        // emitting the degenerate `''\'''` wrap. Only the lone `'` — two or
        // more quotes still use the normal `'\''` wrapping.
        if v == "'" {
            return r"\'".to_string();
        }
        return format!("'{}'", escape_alias_value(v));
    }
    v.to_string()
}

/// Renders a `declare ATTR NAME="value"` line. Empty attrs print as
/// `declare --`; otherwise the attribute order is `airx` to match
/// bash's display (e.g. `-a`, `-ai`, `-i`, `-ir`, `-irx`, `-rx`).
/// For indexed-array variables, the value is rendered as
/// `([0]="v0" [1]="v1" ...)` over the keys in ascending order.
pub(crate) fn format_declare_line(name: &str, var: &crate::shell_state::Variable) -> String {
    use crate::shell_state::VarValue;

    let mut attrs = String::new();
    // Order matches bash's `declare -p` output: n, a/A, i, r, x, l/u.
    if var.nameref {
        attrs.push('n');
    }
    if matches!(var.value, VarValue::Indexed(_)) {
        attrs.push('a');
    }
    if matches!(var.value, VarValue::Associative(_)) {
        attrs.push('A');
    }
    if var.integer {
        attrs.push('i');
    }
    if var.readonly {
        attrs.push('r');
    }
    if var.exported {
        attrs.push('x');
    }
    match var.case_fold {
        Some(crate::shell_state::CaseFold::Lower) => attrs.push('l'),
        Some(crate::shell_state::CaseFold::Upper) => attrs.push('u'),
        None => {}
    }
    let flag_str = if attrs.is_empty() {
        "--".to_string()
    } else {
        let mut s = String::with_capacity(1 + attrs.len());
        s.push('-');
        s.push_str(&attrs);
        s
    };
    let value_part = render_declare_value_part(var);
    format!("declare {flag_str} {name}{value_part}")
}

/// Renders an associative-array subscript key for `declare`-style
/// output. Bash uses bareword when the key matches `[A-Za-z0-9_-]+`
/// (covers identifiers, integers including negative, dashed words);
/// otherwise double-quoted with `\$`/`\\`/`\"`/`` \` `` escapes
/// (same policy as values inside `(…)`). Resolves L-44(a).
fn quote_subscript_key(k: &str) -> String {
    if !k.is_empty()
        && k.bytes()
            .all(|b| matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
    {
        k.to_string()
    } else {
        format!("\"{}\"", crate::escape_double_quote_value(k))
    }
}

/// Quote a value for `declare -p` display. bash double-quotes normally but
/// switches the whole value to ANSI-C `$'…'` when it contains a control
/// character (newline, tab, etc.) — the same `is_control()` trigger as
/// `declare_scalar_quote`, so the `-p` and bare forms agree. Returns the full
/// quoted token (`"…"` or `$'…'`), with no leading `=`.
fn declare_p_value_quote(s: &str) -> String {
    if s.chars().any(|c| c.is_control()) {
        crate::param_expansion::ansi_c_quote(s)
    } else {
        format!("\"{}\"", crate::escape_double_quote_value(s))
    }
}

/// Renders the `=<value>` suffix of a declare line: `="v"` for a scalar,
/// `=([k]="v" …)` for arrays. Shared by `format_declare_line` (the `-p` form)
/// and `format_declare_bare_line` (arrays only).
fn render_declare_value_part(var: &crate::shell_state::Variable) -> String {
    use crate::shell_state::VarValue;
    match &var.value {
        VarValue::Scalar(s) => {
            // Unbound namerefs (empty value) omit the `=""` part — matches bash.
            if var.nameref && s.is_empty() {
                String::new()
            } else {
                format!("={}", declare_p_value_quote(s))
            }
        }
        VarValue::Indexed(m) => {
            let parts: Vec<String> = m
                .iter()
                .map(|(k, v)| format!("[{k}]={}", declare_p_value_quote(v)))
                .collect();
            format!("=({})", parts.join(" "))
        }
        VarValue::Associative(pairs) => {
            let parts: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("[{}]={}", quote_subscript_key(k), declare_p_value_quote(v)))
                .collect();
            if parts.is_empty() {
                "=()".to_string()
            } else {
                // Bash assoc body has a trailing space before `)`.
                // Indexed body does NOT (mirrors bash's inconsistency).
                format!("=({} )", parts.join(" "))
            }
        }
    }
}

/// Formats one variable in bash's bare-`declare` (no-args) form: `name=value`
/// with NO `declare -X` prefix and NO attribute flags. Scalars use the minimal
/// `declare_scalar_quote`; arrays reuse the `-p` value renderer (their element
/// format is identical to `declare -p` minus the `declare -a/-A ` prefix).
fn format_declare_bare_line(name: &str, var: &crate::shell_state::Variable) -> String {
    use crate::shell_state::VarValue;
    match &var.value {
        VarValue::Scalar(s) => {
            if var.nameref && s.is_empty() {
                name.to_string()
            } else {
                format!("{name}={}", declare_scalar_quote(s))
            }
        }
        VarValue::Indexed(_) | VarValue::Associative(_) => {
            format!("{name}{}", render_declare_value_part(var))
        }
    }
}

/// Lists every EXPORTED variable, sorted by name, as bash's
/// `declare -x NAME="value"` (reuses `format_declare_line` for attr order +
/// value quoting). Used by bare `export` / `export -p`.
fn list_exported(out: &mut dyn Write, shell: &Shell) -> ExecOutcome {
    let mut entries: Vec<(&String, &crate::shell_state::Variable)> =
        shell.iter_vars().filter(|(_, v)| v.exported).collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    for (name, var) in entries {
        if writeln!(out, "{}", format_declare_line(name, var)).is_err() {
            // v308: reported once by the epilogue.
            return ExecOutcome::Continue(1);
        }
    }
    ExecOutcome::Continue(0)
}

/// Lists exported functions (sorted) as `generate` body + `declare -fx NAME`.
fn list_exported_functions(out: &mut dyn Write, shell: &Shell) -> ExecOutcome {
    for name in shell.exported_function_names() {
        if let Some(body) = shell.functions.get(&name)
            && (writeln!(out, "{}", crate::generate::function_to_source(&name, body)).is_err()
                || writeln!(out, "declare -fx {name}").is_err())
        {
            // v308: reported once by the epilogue.
            return ExecOutcome::Continue(1);
        }
    }
    ExecOutcome::Continue(0)
}

/// If we're inside a function call AND `name` hasn't been snapshotted
/// in the current local frame yet, snapshot the current Variable (or
/// None if unset). The unwinding in `call_function` will restore it on
/// function exit. No-op when the local-scopes stack is empty (outside
/// any function). Mirrors the per-frame idempotency pattern used by
/// `builtin_local` (v52).
fn snapshot_for_local_scope(shell: &mut Shell, name: &str) {
    if shell.local_scopes.is_empty() {
        return;
    }
    let already_saved = shell
        .local_scopes
        .last()
        .map(|f| f.contains_key(name))
        .unwrap_or(false);
    if already_saved {
        return;
    }
    let snap = shell.snapshot_var(name);
    shell
        .local_scopes
        .last_mut()
        .unwrap()
        .insert(name.to_string(), snap);
}

/// Emit every variable in `shell` (sorted by name) as a
/// `declare ATTR NAME="value"` line.
fn declare_list_all_vars(out: &mut dyn std::io::Write, shell: &Shell, bare: bool) -> ExecOutcome {
    let mut entries: Vec<(&String, &crate::shell_state::Variable)> = shell.iter_vars().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    for (name, var) in entries {
        let line = if bare {
            format_declare_bare_line(name, var)
        } else {
            format_declare_line(name, var)
        };
        let _ = writeln!(out, "{line}");
    }
    // bare `declare` also lists all functions (sorted), in the `f () {…}` form.
    if bare {
        let mut fnames: Vec<String> = shell.functions.keys().cloned().collect();
        fnames.sort();
        for n in &fnames {
            emit_function(n, false, false, out, shell);
        }
    }
    ExecOutcome::Continue(0)
}

/// Emit function definitions for each named function (or every
/// function, sorted, when `names` is empty).
///
/// When `names_only` (the `-F` form) is set, print just the
/// `declare -f NAME` header line. Otherwise (the `-f` form) print the
/// full function body, serialized from the AST by `generate` in a
/// NORMALIZED, re-parseable format (not byte-identical to bash's
/// pretty-printer, but semantically equivalent — see M-121).
fn declare_list_functions(
    names: &[String],
    names_only: bool,
    want_export: bool,
    out: &mut dyn std::io::Write,
    shell: &mut Shell,
) -> ExecOutcome {
    if names.is_empty() {
        let mut fnames: Vec<String> = shell.functions.keys().cloned().collect();
        fnames.sort();
        for n in &fnames {
            // bash applies the `-x` export filter only to the bulk listing.
            if want_export && !shell.is_function_exported(n) {
                continue;
            }
            emit_function(n, names_only, false, out, shell); // listing: not explicit
        }
        return ExecOutcome::Continue(0);
    }
    let mut exit: i32 = 0;
    for name in names {
        if shell.functions.contains_key(name) {
            emit_function(name, names_only, true, out, shell); // explicit name
        } else {
            // bash: `declare -f`/`-F` on a missing function is silent (rc 1).
            exit = 1;
        }
    }
    ExecOutcome::Continue(exit)
}

/// Emit a single existing function: the `-F` header for `names_only`,
/// otherwise the full normalized body via `generate::function_to_source`.
///
/// `explicit` is true when the caller named this function explicitly
/// (e.g. `declare -F foo`).  When `names_only && explicit`, bash prints
/// just the bare name; when `names_only && !explicit` (listing all
/// functions), bash prints the `declare -f NAME` header form.
fn emit_function(
    name: &str,
    names_only: bool,
    explicit: bool,
    out: &mut dyn std::io::Write,
    shell: &Shell,
) {
    if names_only {
        if explicit {
            // bash `declare -F NAME` (specific name) → bare name.
            let _ = writeln!(out, "{name}");
        } else {
            // bash `declare -F` (listing) → re-declarable header form;
            // the listing reflects the export attribute.
            if shell.is_function_exported(name) {
                let _ = writeln!(out, "declare -fx {name}");
            } else {
                let _ = writeln!(out, "declare -f {name}");
            }
        }
    } else if let Some(body) = shell.functions.get(name) {
        let _ = writeln!(out, "{}", crate::generate::function_to_source(name, body));
    }
}

// ─────────────────────────────────────────────────────────────
// Declaration-builtin entry points (DeclArg-aware) — v71 Task 5
// ─────────────────────────────────────────────────────────────
//
// These accept `&[DeclArg]` from `run_declaration_builtin`. Plain args
// (flags, bare names, scalar `name=val` produced by string expansion) come
// through as `Plain`. Compound-RHS or subscripted assignments (`a=(x y)`,
// `a[i]+=v`) come through as parsed `Assignment` records and are applied
// via `executor::apply_one_assignment` — the same path used by ordinary
// assignment commands.

/// `export` entry point with DeclArg input. Mirrors the legacy `builtin_export`
/// behavior: scalar `=` assigns + exports; array compound-RHS (`name=(x y)`)
/// assigns the array via `apply_one_assignment` and sets the export attribute
/// (bash `declare -ax`); bare `NAME` flips the export bit without checking
/// readonly.
fn builtin_export_decl(
    args: &[DeclArg],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    // Parse leading flags. `-a` is a huck-specific no-op (mise emits
    // `export -a chpwd_functions`); `-p` lists (only when no operands);
    // `-n` unexports; `-f` is function export (DEFERRED).
    let mut unexport = false;
    let mut func = false;
    let mut saw_p = false;
    let mut saw_a = false;
    let mut idx = 0;
    while idx < args.len() {
        match &args[idx] {
            DeclArg::Plain(s) => {
                if s == "--" {
                    idx += 1;
                    break;
                }
                if s.starts_with('-') && s.len() > 1 {
                    for c in s[1..].chars() {
                        match c {
                            'p' => saw_p = true,
                            'a' => saw_a = true, // huck-specific no-op (mise `export -a chpwd_functions`)
                            'n' => unexport = true,
                            'f' => func = true,
                            _ => {
                                crate::sh_error_to!(
                                    shell,
                                    err,
                                    None,
                                    "export: -{c}: invalid option"
                                );
                                e!(
                                    err,
                                    "export: usage: export [-fn] [name[=value] ...] or export -p"
                                );
                                // POSIX case #1: bad option is a usage error.
                                shell.builtin_usage_error = Some(2);
                                return ExecOutcome::Continue(2);
                            }
                        }
                    }
                    idx += 1;
                    continue;
                }
                break;
            }
            DeclArg::Assign(_) => break,
        }
    }
    let operands = &args[idx..];

    if operands.is_empty() {
        if unexport {
            return ExecOutcome::Continue(0);
        }
        // `-f` with no operands lists exported functions. `-a` (mise
        // accommodation) suppresses the var listing: rc 0, no output.
        // Otherwise list exported variables (bare `export` or `-p`).
        if func && !saw_p {
            return list_exported_functions(out, shell);
        }
        if saw_a && !saw_p {
            return ExecOutcome::Continue(0);
        }
        return list_exported(out, shell);
    }

    let mut any_error = false;
    for arg in operands {
        if func {
            let name: &str = match arg {
                DeclArg::Plain(s) => s.as_str(),
                DeclArg::Assign(a) => a.target.name(),
            };
            if unexport {
                // export -nf NAME: remove the export mark (lenient — no-op if not
                // exported, matching bash's -n).
                shell.unmark_function_exported(name);
            } else if shell.functions.contains_key(name) {
                shell.mark_function_exported(name);
            } else {
                crate::sh_error_to!(shell, err, None, "export: {name}: not a function");
                any_error = true;
            }
            continue;
        }
        match arg {
            DeclArg::Plain(s) => match s.find('=') {
                Some(eq) => {
                    let name = &s[..eq];
                    let value = &s[eq + 1..];
                    if !is_valid_name(name) {
                        crate::sh_error_to!(
                            shell,
                            err,
                            None,
                            "export: '{s}': not a valid identifier"
                        );
                        any_error = true;
                        continue;
                    }
                    if shell.is_readonly(name) {
                        crate::sh_error_to!(shell, err, None, "export: {name}: readonly variable");
                        any_error = true;
                        continue;
                    }
                    if unexport {
                        shell.set(name, value.to_string());
                        shell.unexport(name);
                    } else {
                        shell.export_set(name, value.to_string());
                    }
                }
                None => {
                    if !is_valid_name(s) {
                        crate::sh_error_to!(
                            shell,
                            err,
                            None,
                            "export: '{s}': not a valid identifier"
                        );
                        any_error = true;
                        continue;
                    }
                    if unexport {
                        shell.unexport(s);
                    } else {
                        shell.export(s);
                    }
                }
            },
            DeclArg::Assign(a) => {
                if matches!(&a.target, crate::command::AssignTarget::Indexed { .. }) {
                    let name = a.target.name();
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "export: `{name}': not a valid identifier"
                    );
                    // POSIX case #1: an invalid-identifier ASSIGNMENT (`AA[4]=1`)
                    // is a bad-assignment usage error → exit status 1. A bad name
                    // WITHOUT `=` (the Plain branches above) stays unmarked.
                    shell.builtin_usage_error = Some(1);
                    any_error = true;
                    continue;
                }
                let name = a.target.name().to_string();
                if shell.is_readonly(&name) {
                    crate::sh_error_to!(shell, err, None, "export: {name}: readonly variable");
                    any_error = true;
                    continue;
                }
                if crate::executor::apply_one_assignment(a, shell, err).is_err() {
                    any_error = true;
                    continue;
                }
                if unexport {
                    shell.unexport(&name);
                } else {
                    shell.export(&name);
                }
            }
        }
    }
    if any_error {
        ExecOutcome::Continue(1)
    } else {
        ExecOutcome::Continue(0)
    }
}

/// `local` entry point with DeclArg input. Supports `-a` flag for array
/// declaration; routes compound-RHS through `apply_one_assignment` while
/// re-using the existing per-frame snapshot machinery for unwind on
/// function return.
fn builtin_local_decl(args: &[DeclArg], err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if shell.local_scopes.is_empty() {
        crate::sh_error_to!(shell, err, None, "local: can only be used in a function");
        return ExecOutcome::Continue(1);
    }
    let mut want_array = false;
    let mut want_associative = false;
    let mut want_integer = false;
    let mut want_readonly = false;
    let mut saw_minus_l = false;
    let mut saw_minus_u = false;
    let mut saw_minus_n = false;
    let mut idx = 0;
    // Parse leading flags from Plain args. Letters cluster (`-ri`, `-ir`)
    // exactly as in `declare`.
    while idx < args.len() {
        let DeclArg::Plain(s) = &args[idx] else { break };
        if s == "--" {
            idx += 1;
            break;
        }
        if !(s.starts_with('-') && s.len() > 1) {
            break;
        }
        for &c in &s.as_bytes()[1..] {
            match c {
                b'a' => want_array = true,
                b'A' => want_associative = true,
                b'i' => want_integer = true,
                b'r' => want_readonly = true,
                b'l' => saw_minus_l = true,
                b'u' => saw_minus_u = true,
                b'n' => saw_minus_n = true,
                other => {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "local: -{}: invalid option",
                        other as char
                    );
                    return ExecOutcome::Continue(1);
                }
            }
        }
        idx += 1;
    }
    if want_array && want_associative {
        crate::sh_error_to!(shell, err, None, "local: cannot specify both -a and -A");
        return ExecOutcome::Continue(1);
    }

    // Net case-fold attribute from this command's flags.
    let minus_case_fold: Option<Option<crate::shell_state::CaseFold>> =
        if saw_minus_l && saw_minus_u {
            Some(None) // both cancel → clear
        } else if saw_minus_l {
            Some(Some(crate::shell_state::CaseFold::Lower))
        } else if saw_minus_u {
            Some(Some(crate::shell_state::CaseFold::Upper))
        } else {
            None // no minus case-fold flag this command
        };
    let mut exit: i32 = 0;
    for arg in &args[idx..] {
        match arg {
            DeclArg::Plain(s) => {
                // Bare NAME (no value). The lexer would have given us an
                // Assign for "NAME=VAL", so a Plain here that contains `=`
                // came from expansion (e.g. `local "$x"`); bash treats that
                // as an invalid identifier.
                let name = s.as_str();
                if !is_valid_name(name) {
                    crate::sh_error_to!(shell, err, None, "local: `{s}': not a valid identifier");
                    exit = 1;
                    continue;
                }
                if shell.is_readonly(name) {
                    crate::sh_error_to!(shell, err, None, "local: {name}: readonly variable");
                    exit = 1;
                    continue;
                }
                // Whether NAME is already local in the current frame (a prior
                // `local NAME` in this function). A bare re-`local` of an
                // already-local name must NOT unset it (bash preserves the
                // value: `local x=v; local x` keeps v); capture this before the
                // snapshot no-ops on an already-saved name.
                let already_local = shell
                    .local_scopes
                    .last()
                    .map(|f| f.contains_key(name))
                    .unwrap_or(false);
                snapshot_for_local_scope(shell, name);
                if saw_minus_n {
                    // `local -n NAME` (bare, no value): declare as nameref,
                    // leave value empty (unbound nameref).
                    shell.set_nameref(name, true);
                } else if want_array {
                    // Promote existing scalar to element 0 (bash semantics)
                    // or create an empty indexed array.
                    if shell.get_indexed(name).is_none() {
                        let mut empty = std::collections::BTreeMap::new();
                        if let Some(scalar) = shell.get(name) {
                            empty.insert(0, scalar.to_string());
                        }
                        if shell.replace_indexed(name, empty).is_err() {
                            exit = 1;
                            // Shape creation FAILED — skip the post-chain
                            // mark_integer (consistent with the associative
                            // branch / builtin_declare_decl).
                            continue;
                        }
                    }
                } else if want_associative {
                    // local -A NAME: ensure name is an associative array.
                    // declare_associative errors if name is already indexed
                    // or scalar; the snapshot above lets call_function
                    // restore the prior value on function exit.
                    if shell.get_associative(name).is_none()
                        && let Err(e) = shell.declare_associative(name)
                    {
                        crate::sh_error_to!(
                            shell,
                            err,
                            None,
                            "{}",
                            crate::shell_state::declare_err_message("local", name, &e)
                        );
                        exit = 1;
                        // Shape creation FAILED — skip the post-chain
                        // mark_integer so the integer attribute is not
                        // applied to a var whose associative shape never
                        // materialized (matches builtin_declare_decl).
                        continue;
                    }
                } else if want_integer && !(want_array || want_associative) {
                    // Bare `local -i NAME`: create the local as a set-but-empty
                    // integer scalar (matches bash + `declare -i NAME`) so a
                    // later `NAME=2+3` arithmetic-coerces. mark_integer creates
                    // the empty scalar when absent; the snapshot above records
                    // the outer value for restore on return.
                    shell.mark_integer(name);
                } else if !already_local {
                    // Bare `local NAME` with no value (fresh local): declare it
                    // function-local but UNSET (matches bash + `declare NAME`).
                    // The snapshot above records the outer value so it is
                    // restored on return; unsetting makes `[[ -v NAME ]]` /
                    // `${NAME-d}` see it as unset until assigned. A bare
                    // re-`local` of an already-local name preserves its value
                    // (bash), so only unset when NOT already_local. (M-111)
                    shell.unset(name);
                }
                // `local -ai`/`-Ai` NAME (bare): apply the integer flag AFTER
                // the array shape was created above (mark_integer sets the flag
                // on the existing var without clobbering shape). A later
                // `NAME[i]=expr` then arith-coerces (L-49).
                if want_integer && (want_array || want_associative) {
                    shell.mark_integer(name);
                }
                // Apply case-fold attribute AFTER shape setup (so that -lA
                // finds the associative var and only updates case_fold) but
                // BEFORE value is set — for bare names there is no value,
                // so ordering is a no-op here beyond attribute stamping.
                if let Some(fold) = minus_case_fold {
                    shell.set_case_fold(name, fold);
                }
                // Apply the readonly attribute last so `local -r NAME` (no
                // value) marks the freshly-declared local readonly. (For an
                // -i bare local, mark_integer above created the scalar; for a
                // plain bare local it was unset — mark_readonly then creates an
                // empty readonly scalar, matching `declare -r NAME`.)
                if want_readonly {
                    shell.mark_readonly(name);
                }
            }
            DeclArg::Assign(a) => {
                let name = a.target.name().to_string();
                if !is_valid_name(&name) {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "local: `{name}': not a valid identifier"
                    );
                    exit = 1;
                    continue;
                }
                if shell.is_readonly(&name) {
                    crate::sh_error_to!(shell, err, None, "local: {name}: readonly variable");
                    exit = 1;
                    continue;
                }
                snapshot_for_local_scope(shell, &name);

                // `local -n NAME=target`: nameref bind — validate and store raw.
                if saw_minus_n {
                    // Expand the RHS word to obtain the target name string.
                    let target = crate::expand::expand_assignment(&a.value, shell);
                    if target == name {
                        crate::sh_error_to!(
                            shell,
                            err,
                            None,
                            "local: {name}: nameref variable self references not allowed"
                        );
                        exit = 1;
                        continue;
                    }
                    let valid = is_valid_name(&target)
                        || matches!(parse_subscripted_arg(&target), Ok(Some((b, _))) if is_valid_name(b));
                    if !valid {
                        crate::sh_error_to!(
                            shell,
                            err,
                            None,
                            "local: `{target}': invalid variable name for name reference"
                        );
                        exit = 1;
                        continue;
                    }
                    shell.set_nameref(&name, true);
                    shell.set(&name, target);
                    // Apply co-requested -r (local does not support -x,
                    // but mirror the same pattern for safety).
                    if want_readonly {
                        shell.mark_readonly(&name);
                    }
                    continue;
                }

                // For `local -A NAME=([k]=v)`: ensure NAME is associative
                // BEFORE apply_one_assignment so the executor routes the
                // compound RHS through the associative path. Without this,
                // apply_one_assignment would see an absent (or indexed)
                // variable and dispatch to the indexed-array path.
                if want_associative
                    && shell.get_associative(&name).is_none()
                    && let Err(e) = shell.declare_associative(&name)
                {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "{}",
                        crate::shell_state::declare_err_message("local", &name, &e)
                    );
                    exit = 1;
                    continue;
                }
                // `local -i NAME=expr`: flip the integer flag BEFORE the
                // assignment so apply_one_assignment routes the RHS through
                // the arithmetic coerce (mirrors declare's ordering).
                if want_integer {
                    shell.mark_integer(&name);
                }
                // `local -l/-u NAME=val`: set case-fold attribute BEFORE the
                // assignment so the value is folded on write.
                if let Some(fold) = minus_case_fold {
                    shell.set_case_fold(&name, fold);
                }
                if crate::executor::apply_one_assignment(a, shell, err).is_err() {
                    exit = 1;
                    continue;
                }
                // `local -r NAME=val`: mark readonly AFTER the value is set
                // (mirrors declare's `-r NAME=VALUE` ordering).
                if want_readonly {
                    shell.mark_readonly(&name);
                }
            }
        }
    }
    ExecOutcome::Continue(exit)
}

/// `readonly` entry point with DeclArg input. Routes compound-RHS through
/// `apply_one_assignment`; rejects subscripted-target assignments.
fn builtin_readonly_decl(
    args: &[DeclArg],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    // Parse leading flags (-p, -A). `--` terminates option processing.
    let mut want_list = false;
    let mut want_associative = false;
    let mut idx = 0;
    while idx < args.len() {
        let DeclArg::Plain(s) = &args[idx] else { break };
        match s.as_str() {
            "-p" => {
                want_list = true;
                idx += 1;
            }
            "-A" => {
                want_associative = true;
                idx += 1;
            }
            "--" => {
                idx += 1;
                break;
            }
            o if o.starts_with('-') && o.len() > 1 => {
                crate::sh_error_to!(shell, err, None, "readonly: {o}: invalid option");
                // POSIX case #1: bad option is a usage error.
                shell.builtin_usage_error = Some(2);
                return ExecOutcome::Continue(2);
            }
            _ => break,
        }
    }
    let rest = &args[idx..];

    if rest.is_empty() || want_list {
        for name in shell.readonly_names() {
            // Route through snapshot_var/format_declare_line so arrays
            // render as `declare -ar a=([0]="x" [1]="y")` instead of
            // collapsing to element 0 via scalar_view().
            let line = match shell.snapshot_var(&name) {
                Some(var) => format_declare_line(&name, &var),
                None => {
                    // Marked readonly but never assigned: emit just the
                    // bare attribute form, mirroring `declare -p` for
                    // attribute-only variables.
                    format!("declare -r {name}")
                }
            };
            if writeln!(out, "{line}").is_err() {
                // v308: reported once by the epilogue.
                return ExecOutcome::Continue(1);
            }
        }
        return ExecOutcome::Continue(0);
    }

    let mut exit = 0;
    for arg in rest {
        match arg {
            DeclArg::Plain(s) => {
                let name = s.as_str();
                if !is_valid_name(name) {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "readonly: `{s}': not a valid identifier"
                    );
                    exit = 1;
                    continue;
                }
                // `readonly -A NAME` (no value): ensure name is associative
                // before marking readonly.
                if want_associative
                    && shell.get_associative(name).is_none()
                    && let Err(e) = shell.declare_associative(name)
                {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "{}",
                        crate::shell_state::declare_err_message("readonly", name, &e)
                    );
                    exit = 1;
                    continue;
                }
                shell.mark_readonly(name);
            }
            DeclArg::Assign(a) => match &a.target {
                crate::command::AssignTarget::Bare(name) => {
                    if shell.is_readonly(name) {
                        crate::sh_error_to!(
                            shell,
                            err,
                            None,
                            "readonly: {name}: readonly variable"
                        );
                        exit = 1;
                        continue;
                    }
                    // `readonly -A NAME=([k]=v)`: ensure NAME is associative
                    // BEFORE apply_one_assignment so the compound RHS routes
                    // through the associative executor path.
                    if want_associative
                        && shell.get_associative(name).is_none()
                        && let Err(e) = shell.declare_associative(name)
                    {
                        crate::sh_error_to!(
                            shell,
                            err,
                            None,
                            "{}",
                            crate::shell_state::declare_err_message("readonly", name, &e)
                        );
                        exit = 1;
                        continue;
                    }
                    if crate::executor::apply_one_assignment(a, shell, err).is_err() {
                        exit = 1;
                        continue;
                    }
                    shell.mark_readonly(name);
                }
                crate::command::AssignTarget::Indexed { name, .. } => {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "readonly: `{name}': cannot make subscripted-assignment target readonly"
                    );
                    // POSIX case #1: invalid-identifier ASSIGNMENT (`AA[4]=1`) →
                    // bad-assignment usage error, exit status 1. A bad name without
                    // `=` (the Plain branch above) stays unmarked.
                    shell.builtin_usage_error = Some(1);
                    exit = 1;
                }
            },
        }
    }
    ExecOutcome::Continue(exit)
}

/// `declare`/`typeset` entry point with DeclArg input.
fn builtin_declare_decl(
    args: &[DeclArg],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let mut want_readonly = false;
    let mut want_export = false;
    let mut want_remove_export = false;
    let mut want_integer = false;
    let mut want_remove_integer = false;
    let mut want_array = false;
    let mut want_associative = false;
    let mut function_mode = false;
    let mut function_names_only = false;
    let mut print_mode = false;
    let mut global = false;
    let mut saw_minus_l = false;
    let mut saw_minus_u = false;
    let mut saw_plus_l = false;
    let mut saw_plus_u = false;
    let mut saw_minus_n = false;
    let mut saw_plus_n = false;

    // Parse leading flags from Plain args. As soon as we hit a non-flag
    // Plain or any Assign, switch into the per-name phase.
    let mut idx = 0;
    while idx < args.len() {
        let DeclArg::Plain(arg) = &args[idx] else {
            break;
        };
        if arg == "--" {
            idx += 1;
            break;
        }
        let plus = arg.starts_with('+');
        let minus = arg.starts_with('-');
        if !(plus || minus) || arg.len() < 2 {
            break;
        }
        for &c in &arg.as_bytes()[1..] {
            match c {
                b'r' if minus => want_readonly = true,
                b'r' if plus => {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "declare: +r: readonly attribute cannot be removed"
                    );
                    return ExecOutcome::Continue(1);
                }
                b'x' if minus => want_export = true,
                b'x' if plus => want_remove_export = true,
                b'i' if minus => want_integer = true,
                b'i' if plus => want_remove_integer = true,
                b'a' if minus => want_array = true,
                b'a' if plus => {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "declare: +a: array attribute cannot be removed"
                    );
                    return ExecOutcome::Continue(1);
                }
                b'A' if minus => want_associative = true,
                b'A' if plus => {
                    // TODO: bash compat — bash silently ignores `+A` on
                    // existing associatives (the attribute can't be
                    // removed once set). We mirror `+a`'s conservative
                    // rejection for now; revisit if real scripts need
                    // silent-ignore behavior.
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "declare: +A: associative attribute cannot be removed"
                    );
                    return ExecOutcome::Continue(1);
                }
                b'l' if minus => saw_minus_l = true,
                b'l' if plus => saw_plus_l = true,
                b'u' if minus => saw_minus_u = true,
                b'u' if plus => saw_plus_u = true,
                b'n' if minus => saw_minus_n = true,
                b'n' if plus => saw_plus_n = true,
                b'f' if minus => function_mode = true,
                b'F' if minus => {
                    function_mode = true;
                    function_names_only = true;
                }
                b'p' if minus => print_mode = true,
                b'g' if minus => global = true,
                other => {
                    let sign = if plus { '+' } else { '-' };
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "declare: {sign}{}: invalid option",
                        other as char
                    );
                    return ExecOutcome::Continue(2);
                }
            }
        }
        idx += 1;
    }
    let names = &args[idx..];

    // Net case-fold attribute from this command's flags.
    let minus_case_fold: Option<Option<crate::shell_state::CaseFold>> =
        if saw_minus_l && saw_minus_u {
            Some(None) // both cancel → clear
        } else if saw_minus_l {
            Some(Some(crate::shell_state::CaseFold::Lower))
        } else if saw_minus_u {
            Some(Some(crate::shell_state::CaseFold::Upper))
        } else {
            None // no minus case-fold flag this command
        };

    // Reject the combinations we haven't implemented yet.
    if want_array && want_associative {
        crate::sh_error_to!(shell, err, None, "declare: cannot specify both -a and -A");
        return ExecOutcome::Continue(1);
    }

    // Function export: `declare -fx [NAME...]`. With no names, list exported
    // functions; with names, mark each existing function exported (mirrors
    // `export -f`). A missing function is silent with rc 1.
    if function_mode && want_export && !function_names_only {
        let plain_names: Vec<String> = names
            .iter()
            .filter_map(|a| match a {
                DeclArg::Plain(s) => Some(s.clone()),
                DeclArg::Assign(_) => None,
            })
            .collect();
        if plain_names.is_empty() {
            return list_exported_functions(out, shell);
        }
        let mut any_error = false;
        for name in &plain_names {
            if shell.functions.contains_key(name.as_str()) {
                shell.mark_function_exported(name);
            } else {
                // bash: declare -f on a missing function is silent, rc 1.
                any_error = true;
            }
        }
        return if any_error {
            ExecOutcome::Continue(1)
        } else {
            ExecOutcome::Continue(0)
        };
    }

    // Function-mode listing: only Plain names accepted.
    if function_mode {
        let plain_names: Vec<String> = names
            .iter()
            .filter_map(|a| match a {
                DeclArg::Plain(s) => Some(s.clone()),
                DeclArg::Assign(_) => None,
            })
            .collect();
        return declare_list_functions(&plain_names, function_names_only, want_export, out, shell);
    }

    // Bare `declare` (or `declare -p`) with no names: list everything.
    // `declare -a` with no names: list indexed arrays only.
    // `declare -A` with no names: list associative arrays only.
    if names.is_empty() {
        if want_array {
            use crate::shell_state::VarValue;
            let mut entries: Vec<(&String, &crate::shell_state::Variable)> = shell
                .iter_vars()
                .filter(|(_, v)| matches!(v.value, VarValue::Indexed(_)))
                .collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            for (name, var) in entries {
                let _ = writeln!(out, "{}", format_declare_line(name, var));
            }
            return ExecOutcome::Continue(0);
        }
        if want_associative {
            use crate::shell_state::VarValue;
            let mut entries: Vec<(&String, &crate::shell_state::Variable)> = shell
                .iter_vars()
                .filter(|(_, v)| matches!(v.value, VarValue::Associative(_)))
                .collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            for (name, var) in entries {
                let _ = writeln!(out, "{}", format_declare_line(name, var));
            }
            return ExecOutcome::Continue(0);
        }
        return declare_list_all_vars(out, shell, !print_mode);
    }

    let mut exit: i32 = 0;
    for arg in names {
        // Validate name. For Plain, treat the whole string as the
        // candidate; for Assign, use the target's name.
        let (name, assign_opt): (&str, Option<&crate::command::Assignment>) = match arg {
            DeclArg::Plain(s) => (s.as_str(), None),
            DeclArg::Assign(a) => (a.target.name(), Some(a)),
        };
        if !is_valid_name(name) {
            crate::sh_error_to!(
                shell,
                err,
                None,
                "declare: `{name}': not a valid identifier"
            );
            exit = 1;
            continue;
        }

        if print_mode {
            match shell.snapshot_var(name) {
                Some(var) => {
                    let _ = writeln!(out, "{}", format_declare_line(name, &var));
                }
                None => {
                    // v269 T3b: now uses sh_error_to! (writer-based emitter),
                    // which writes directly to the `err` writer this arm
                    // already holds — the same writer the executor's
                    // in-memory route_err_to_out/route_out_to_err builtin
                    // redirect fixup (bare-builtin `2>&1`/`>&2` under a
                    // Capture sink) swaps, so the diagnostic lands in the
                    // redirect target regardless of the ambient thread-local
                    // sink. (The prior `sh_error!` conversion broke this by
                    // going through the thread-local sink instead.)
                    crate::sh_error_to!(shell, err, None, "declare: {name}: not found");
                    exit = 1;
                }
            }
            continue;
        }

        // Snapshot for local-scope unwind BEFORE mutating. With -g, write to
        // the global map and drop any outer snapshot so it survives function exit.
        if global {
            if let Some(frame) = shell.local_scopes.last_mut() {
                frame.remove(name);
            }
        } else {
            snapshot_for_local_scope(shell, name);
        }

        // Integer-attribute changes on readonly variable are rejected.
        if (want_integer || want_remove_integer) && shell.is_readonly(name) {
            crate::sh_error_to!(shell, err, None, "declare: {name}: readonly variable");
            exit = 1;
            continue;
        }

        // Apply integer-flag flips before any value-set path. For ARRAY/assoc
        // declarations the integer flag is applied AFTER shape creation
        // (mark_integer creates an empty Scalar when the name is unset, which
        // would otherwise make declare_associative/replace_indexed see a scalar);
        // see the deferred `mark_integer` below. For a plain (scalar) integer
        // declaration it must run BEFORE the `=value` path so the value coerces.
        if want_integer && !(want_array || want_associative) {
            shell.mark_integer(name);
        }
        if want_remove_integer {
            shell.unmark_integer(name);
        }

        // Array-attribute handling. `-a NAME` with no value: promote
        // scalar to element 0 (or create empty array). With a value,
        // fall through into the assignment path below — it always
        // routes compound RHS through apply_one_assignment.
        if want_array && assign_opt.is_none() && shell.get_indexed(name).is_none() {
            let mut empty = std::collections::BTreeMap::new();
            if let Some(scalar) = shell.get(name) {
                empty.insert(0, scalar.to_string());
            }
            if shell.replace_indexed(name, empty).is_err() {
                crate::sh_error_to!(shell, err, None, "declare: {name}: readonly variable");
                exit = 1;
                continue;
            }
        }

        // Associative-attribute handling. `declare -A NAME` ensures an
        // empty associative; `declare -A NAME=([k]=v)` ensures associative
        // BEFORE apply_one_assignment so the executor routes the compound
        // RHS through the associative path (not the indexed-array path).
        if want_associative
            && shell.get_associative(name).is_none()
            && let Err(e) = shell.declare_associative(name)
        {
            crate::sh_error_to!(
                shell,
                err,
                None,
                "{}",
                crate::shell_state::declare_err_message("declare", name, &e)
            );
            exit = 1;
            continue;
        }

        // Integer flag for array/associative declarations (`declare -ai`/`-Ai`):
        // applied AFTER the array shape exists (set on the existing var without
        // clobbering shape) and BEFORE any `=value` assignment below, so the
        // funnel arith-coerces the literal's element values on store (L-49).
        if want_integer && (want_array || want_associative) {
            shell.mark_integer(name);
        }

        // Apply case-fold attribute AFTER shape setup (so -lA finds the
        // associative var and only flips case_fold) but BEFORE any value
        // assignment (so the fold is in effect when the value is written).
        if let Some(fold) = minus_case_fold {
            shell.set_case_fold(name, fold);
        }
        if saw_plus_l && shell.case_fold_of(name) == Some(crate::shell_state::CaseFold::Lower) {
            shell.set_case_fold(name, None);
        }
        if saw_plus_u && shell.case_fold_of(name) == Some(crate::shell_state::CaseFold::Upper) {
            shell.set_case_fold(name, None);
        }

        // Nameref (-n / +n) handling. Must come BEFORE the compound-assignment
        // path so that the target is stored raw (not through apply_one_assignment).
        if saw_minus_n {
            let target_opt: Option<String> =
                assign_opt.map(|a| crate::expand::expand_assignment(&a.value, shell));
            if let Some(ref target) = target_opt {
                // Direct self-reference is a hard error.
                if target == name {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "declare: {name}: nameref variable self references not allowed"
                    );
                    exit = 1;
                    continue;
                }
                // Target must be a valid variable name OR name[subscript].
                let valid = is_valid_name(target)
                    || matches!(parse_subscripted_arg(target), Ok(Some((b, _))) if is_valid_name(b));
                if !valid {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "declare: `{target}': invalid variable name for name reference"
                    );
                    exit = 1;
                    continue;
                }
            }
            shell.set_nameref(name, true);
            // BIND: store the target name as the RAW value (not through
            // apply_one_assignment which post-Task-4 will deref namerefs).
            if let Some(target) = target_opt {
                shell.set(name, target);
            }
            // Apply co-requested attributes (-r, -x) that the normal
            // path would handle below — must not skip them on the
            // nameref fast-path.
            if want_readonly {
                shell.mark_readonly(name);
            }
            if want_export {
                shell.export(name);
            } else if want_remove_export {
                shell.unexport(name);
            }
            continue;
        }
        if saw_plus_n && shell.is_nameref(name) {
            shell.set_nameref(name, false);
            // Other attribute changes (export etc.) can still apply.
            // Fall through to the no-value path below.
        }

        // Compound assignment path: a parsed Assignment (scalar or array).
        if let Some(a) = assign_opt {
            // Skip if +n was requested (nameref removal only, no value).
            if saw_plus_n {
                if want_readonly {
                    shell.mark_readonly(name);
                }
                if want_export {
                    shell.export(name);
                } else if want_remove_export {
                    shell.unexport(name);
                }
                continue;
            }
            // -r combined with =VALUE: must not clobber an existing
            // readonly. Other =VALUE assignments rely on
            // apply_one_assignment's internal readonly check.
            if want_readonly && shell.is_readonly(name) {
                crate::sh_error_to!(shell, err, None, "declare: {name}: readonly variable");
                exit = 1;
                continue;
            }
            if shell.is_readonly(name) {
                crate::sh_error_to!(shell, err, None, "{name}: readonly variable");
                exit = 1;
                continue;
            }
            if crate::executor::apply_one_assignment(a, shell, err).is_err() {
                exit = 1;
                continue;
            }
            if want_readonly {
                shell.mark_readonly(name);
            }
            if want_export {
                shell.export(name);
            } else if want_remove_export {
                shell.unexport(name);
            }
            continue;
        }

        // No value supplied. Apply attribute-only changes.
        if want_readonly {
            shell.mark_readonly(name);
        }
        if want_export {
            shell.export(name);
        }
        if want_remove_export {
            shell.unexport(name);
        }
        // Bare `declare NAME` (no flag, no value): inside a function,
        // the snapshot is enough. Outside, no-op. Match the legacy
        // builtin_declare behavior.
    }
    ExecOutcome::Continue(exit)
}

/// Reads one logical line from `r` honoring the terminator byte `delim`
/// and POSIX/bash escape handling.
///
/// - `raw = true`: no escape processing; backslash is literal.
/// - `raw = false`: `\X` (X ≠ newline) → X (escape removal);
///   `\<LF>` (backslash followed by newline) is line continuation —
///   both bytes are dropped and reading continues onto the next line.
///
/// Returns `Ok(None)` when EOF hits BEFORE any byte was read (the
/// caller treats this as `read` exit status 1). Returns
/// `Ok(Some(partial))` when EOF hits AFTER at least one byte but
/// before the delim (caller still assigns and returns status 0).
/// Reads one record up to (not including) `delim`. Returns `(content, had_delim)`;
/// `had_delim` is false for a final unterminated record at EOF. `None` only when
/// nothing remains. Raw bytes — no backslash processing (mapfile reads raw lines).
fn read_one_record<R: std::io::Read>(
    r: &mut R,
    delim: u8,
) -> std::io::Result<Option<(String, bool)>> {
    let mut out: Vec<u8> = Vec::new();
    let mut any = false;
    loop {
        let mut byte = [0u8; 1];
        let n = r.read(&mut byte)?;
        if n == 0 {
            if !any {
                return Ok(None);
            }
            return Ok(Some((String::from_utf8_lossy(&out).into_owned(), false)));
        }
        any = true;
        if byte[0] == delim {
            return Ok(Some((String::from_utf8_lossy(&out).into_owned(), true)));
        }
        out.push(byte[0]);
    }
}

#[derive(Clone)]
struct ReadCfg {
    raw: bool,
    delim: u8,
    delim_active: bool,
    max_chars: Option<usize>,
    deadline: Option<std::time::Instant>,
}

enum ReadStop {
    Delim,
    Count,
    Eof,
    Timeout,
}

/// Reads one `read`-record byte-at-a-time (the shared-fd-0 reason still applies —
/// see RawFdReader). Honors `-r` backslash processing, a custom `delim`, an
/// optional character-count cap (`-n`/`-N`), and an optional `-t` deadline
/// (polled via `poll_fd`). Returns the decoded string, why it stopped, and
/// whether any byte was read at all.
fn read_record<R: std::io::Read>(
    r: &mut R,
    cfg: &ReadCfg,
    poll_fd: Option<std::os::unix::io::RawFd>,
) -> std::io::Result<(String, ReadStop, bool)> {
    let mut out: Vec<u8> = Vec::new();
    let mut any = false;
    let mut chars: usize = 0;
    // A count cap of 0 (`read -n 0`) reads nothing and succeeds via Count.
    if cfg.max_chars == Some(0) {
        return Ok((String::new(), ReadStop::Count, false));
    }
    loop {
        // -t timeout: poll before each byte. On expiry stop with what we have.
        #[cfg(unix)]
        if let (Some(deadline), Some(fd)) = (cfg.deadline, poll_fd) {
            let now = std::time::Instant::now();
            if now >= deadline {
                return Ok((
                    String::from_utf8_lossy(&out).into_owned(),
                    ReadStop::Timeout,
                    any,
                ));
            }
            let ms = (deadline - now).as_millis().min(i32::MAX as u128) as i32;
            let mut pfd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            let pr = unsafe { libc::poll(&mut pfd, 1, ms) };
            if pr == 0 {
                return Ok((
                    String::from_utf8_lossy(&out).into_owned(),
                    ReadStop::Timeout,
                    any,
                ));
            }
            if pr < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue; // EINTR: re-check the deadline and re-poll
                }
                // Other poll errors: fall through and attempt the read, as before.
            }
            // pr > 0: fall through and attempt the read.
        }
        let mut byte = [0u8; 1];
        let n = r.read(&mut byte)?;
        if n == 0 {
            return Ok((
                String::from_utf8_lossy(&out).into_owned(),
                ReadStop::Eof,
                any,
            ));
        }
        any = true;
        let b = byte[0];
        if cfg.delim_active && b == cfg.delim {
            return Ok((
                String::from_utf8_lossy(&out).into_owned(),
                ReadStop::Delim,
                any,
            ));
        }
        if !cfg.raw && b == b'\\' {
            let mut nxt = [0u8; 1];
            let m = r.read(&mut nxt)?;
            if m == 0 {
                out.push(b'\\'); // trailing backslash at EOF
                return Ok((
                    String::from_utf8_lossy(&out).into_owned(),
                    ReadStop::Eof,
                    any,
                ));
            }
            if nxt[0] == b'\n' {
                continue; // line continuation — no char committed
            }
            out.push(nxt[0]); // \X -> X, may complete (or continue) a UTF-8 scalar
            if is_char_boundary_complete(&out) {
                chars += 1;
                if cfg.max_chars == Some(chars) {
                    return Ok((
                        String::from_utf8_lossy(&out).into_owned(),
                        ReadStop::Count,
                        any,
                    ));
                }
            }
            continue;
        }
        out.push(b);
        // Count a character only when this byte COMPLETES a UTF-8 scalar (or is a
        // lone/invalid byte). A continuation byte (0b10xx_xxxx) mid-sequence does
        // not bump the count.
        if is_char_boundary_complete(&out) {
            chars += 1;
            if cfg.max_chars == Some(chars) {
                return Ok((
                    String::from_utf8_lossy(&out).into_owned(),
                    ReadStop::Count,
                    any,
                ));
            }
        }
    }
}

/// True if `out` ends on a complete UTF-8 scalar boundary (so the last pushed
/// byte finished a character). Uses the fact that a valid trailing sequence ends
/// exactly when `from_utf8` succeeds on the final 1–4 bytes; a lone invalid byte
/// also counts as one character (huck is lossy elsewhere).
fn is_char_boundary_complete(out: &[u8]) -> bool {
    let last = out[out.len() - 1];
    if last < 0x80 {
        return true;
    } // ASCII
    if last & 0b1100_0000 == 0b1000_0000 {
        // continuation byte
        // Complete iff it finishes the expected sequence length.
        let mut i = out.len();
        let mut cont = 0;
        while i > 0 && out[i - 1] & 0b1100_0000 == 0b1000_0000 {
            i -= 1;
            cont += 1;
        }
        if i == 0 {
            return true;
        } // dangling continuations: count each
        let lead = out[i - 1];
        let need = if lead >= 0xF0 {
            3
        } else if lead >= 0xE0 {
            2
        } else if lead >= 0xC0 {
            1
        } else {
            return true;
        };
        cont == need
    } else {
        // A lead byte just pushed: a 1-byte "character" only if it's a lone
        // invalid lead (0xC0.. with a multibyte need) — treat as incomplete so
        // the following continuation completes it. But a stray >=0x80 non-cont
        // non-lead is its own char.
        last < 0xC0
    }
}

/// POSIX/bash `read`-style field splitting. Assigns fields to
/// `names` left-to-right; the LAST name gets the remainder of the
/// line (no further splitting). Trailing IFS-whitespace is stripped
/// from the last assigned field. For a single name, the line is
/// assigned whole with leading + trailing IFS-whitespace stripped.
///
/// `ifs` is the current value of the IFS variable (caller looks it
/// up). Empty IFS means "no splitting" — assign whole line to first
/// name, rest empty.
fn split_into_names(line: &str, names: &[String], ifs: &str) -> Vec<(String, String)> {
    if names.is_empty() {
        return Vec::new();
    }

    // Classify IFS bytes.
    let ifs_bytes: Vec<u8> = ifs.bytes().collect();
    let is_ws = |b: u8| ifs_bytes.contains(&b) && matches!(b, b' ' | b'\t' | b'\n');
    let is_nonws = |b: u8| ifs_bytes.contains(&b) && !matches!(b, b' ' | b'\t' | b'\n');
    let is_any_ifs = |b: u8| ifs_bytes.contains(&b);

    let bytes = line.as_bytes();

    // Empty IFS: no splitting at all.
    if ifs_bytes.is_empty() {
        let mut out: Vec<(String, String)> = Vec::with_capacity(names.len());
        out.push((names[0].clone(), line.to_string()));
        for n in &names[1..] {
            out.push((n.clone(), String::new()));
        }
        return out;
    }

    // Single-name: strip leading + trailing IFS-whitespace, assign whole.
    if names.len() == 1 {
        let mut start = 0;
        while start < bytes.len() && is_ws(bytes[start]) {
            start += 1;
        }
        let mut end = bytes.len();
        while end > start && is_ws(bytes[end - 1]) {
            end -= 1;
        }
        let value = String::from_utf8_lossy(&bytes[start..end]).into_owned();
        return vec![(names[0].clone(), value)];
    }

    // Multi-name walk.
    let mut fields: Vec<String> = Vec::new();
    let mut i = 0;

    // Skip leading IFS-whitespace.
    while i < bytes.len() && is_ws(bytes[i]) {
        i += 1;
    }

    while fields.len() < names.len() - 1 && i < bytes.len() {
        // Consume one field.
        let start = i;
        while i < bytes.len() && !is_any_ifs(bytes[i]) {
            i += 1;
        }
        let field = String::from_utf8_lossy(&bytes[start..i]).into_owned();
        fields.push(field);

        if i >= bytes.len() {
            break;
        }

        // Consume the separator run.
        // If the separator is a non-ws IFS char, consume EXACTLY one,
        // then optionally trailing ws-IFS. If it's ws-IFS, consume
        // all consecutive ws-IFS, then optionally a single non-ws-IFS.
        if is_nonws(bytes[i]) {
            i += 1;
            while i < bytes.len() && is_ws(bytes[i]) {
                i += 1;
            }
        } else {
            while i < bytes.len() && is_ws(bytes[i]) {
                i += 1;
            }
            if i < bytes.len() && is_nonws(bytes[i]) {
                i += 1;
                while i < bytes.len() && is_ws(bytes[i]) {
                    i += 1;
                }
            }
        }
    }

    // Pad missing fields.
    while fields.len() < names.len() - 1 {
        fields.push(String::new());
    }

    // Last field: rest of line from position i, with trailing ws-IFS stripped.
    // (B-03 — additionally stripping a trailing NON-ws IFS delimiter here — was
    // reverted in v276: no simple heuristic matches bash's read.def last-field
    // splitter across the ifs-posix suite's multi-char-IFS cases. Deferred to
    // its own iteration that ports bash's algorithm faithfully.)
    let mut end = bytes.len();
    while end > i && is_ws(bytes[end - 1]) {
        end -= 1;
    }
    let last = String::from_utf8_lossy(&bytes[i..end]).into_owned();
    fields.push(last);

    names
        .iter()
        .zip(fields)
        .map(|(n, v)| (n.clone(), v))
        .collect()
}

/// Splits `line` into ALL IFS fields (the unbounded form used by `read -a` /
/// mapfile element splitting). Mirrors bash word-splitting: leading IFS-ws is
/// stripped; a non-ws IFS char delimits (a leading one yields a leading empty
/// field, an adjacent pair yields an empty field between, but a TRAILING one
/// yields no trailing empty field); ws-IFS runs collapse. Empty IFS -> the whole
/// line as one field (none for an empty line).
fn split_read_fields(line: &str, ifs: &str) -> Vec<String> {
    let ifs_bytes: Vec<u8> = ifs.bytes().collect();
    if ifs_bytes.is_empty() {
        return if line.is_empty() {
            Vec::new()
        } else {
            vec![line.to_string()]
        };
    }
    let is_ws = |b: u8| ifs_bytes.contains(&b) && matches!(b, b' ' | b'\t' | b'\n');
    let is_nonws = |b: u8| ifs_bytes.contains(&b) && !matches!(b, b' ' | b'\t' | b'\n');
    let is_any = |b: u8| ifs_bytes.contains(&b);
    let bytes = line.as_bytes();
    let mut fields: Vec<String> = Vec::new();
    let mut i = 0;
    while i < bytes.len() && is_ws(bytes[i]) {
        i += 1;
    }
    while i < bytes.len() {
        let start = i;
        while i < bytes.len() && !is_any(bytes[i]) {
            i += 1;
        }
        fields.push(String::from_utf8_lossy(&bytes[start..i]).into_owned());
        if i >= bytes.len() {
            break;
        }
        // Consume one separator. Non-ws IFS: exactly one + trailing ws-IFS.
        // ws-IFS: collapse the run, then optionally one non-ws IFS + trailing ws.
        if is_nonws(bytes[i]) {
            i += 1;
            while i < bytes.len() && is_ws(bytes[i]) {
                i += 1;
            }
        } else {
            while i < bytes.len() && is_ws(bytes[i]) {
                i += 1;
            }
            if i < bytes.len() && is_nonws(bytes[i]) {
                i += 1;
                while i < bytes.len() && is_ws(bytes[i]) {
                    i += 1;
                }
            }
        }
    }
    fields
}

#[cfg(unix)]
unsafe fn silent_disable_echo(fd: std::os::unix::io::RawFd) -> Option<libc::termios> {
    if unsafe { libc::isatty(fd) } == 0 {
        return None;
    }
    let mut t: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut t) } != 0 {
        return None;
    }
    let saved = t;
    t.c_lflag &= !libc::ECHO;
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &t) };
    Some(saved)
}

#[cfg(unix)]
unsafe fn silent_restore_echo(fd: std::os::unix::io::RawFd, saved: libc::termios) {
    let _ = unsafe { libc::tcsetattr(fd, libc::TCSANOW, &saved) };
}

/// Reads one byte at a time from a raw OS file descriptor via `libc::read`,
/// bypassing Rust's shared `std::io::stdin()` BufReader. For fd 0 this is
/// necessary because rustyline's non-tty `readline_direct` path fills that same
/// BufReader with script-ahead bytes; using it here would return
/// cached script bytes instead of the redirected fd 0. For `read -u FD` it
/// reads directly from the caller-chosen fd.
struct RawFdReader {
    fd: std::os::unix::io::RawFd,
}

impl RawFdReader {
    /// Default reader over fd 0 (stdin).
    fn new() -> Self {
        RawFdReader {
            fd: libc::STDIN_FILENO,
        }
    }

    /// Reader over an arbitrary already-open fd (`read -u FD`).
    fn from_fd(fd: std::os::unix::io::RawFd) -> Self {
        RawFdReader { fd }
    }

    fn raw_fd(&self) -> std::os::unix::io::RawFd {
        self.fd
    }
}

impl std::io::Read for RawFdReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let n =
                unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n >= 0 {
                return Ok(n as usize);
            }
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
    }
}

/// Extract an option's value from the rest of a flag cluster (`-dVALUE`) or
/// consume the next argument (`-d VALUE`). Advances `*i` only in the
/// separate-arg case. Returns `Err(2)` (the exit code) if there is no next
/// arg, after printing the standard diagnostic.
fn take_opt_value(
    args: &[String],
    i: &mut usize,
    bytes: &[u8],
    j: usize,
    cmd: &str,
    opt: char,
    err: &mut dyn Write,
    shell: &Shell,
) -> Result<String, i32> {
    if j + 1 < bytes.len() {
        Ok(String::from_utf8_lossy(&bytes[j + 1..]).into_owned())
    } else {
        *i += 1;
        if *i >= args.len() {
            crate::sh_error_to!(
                shell,
                err,
                None,
                "{cmd}: -{opt}: option requires an argument"
            );
            return Err(2);
        }
        Ok(args[*i].clone())
    }
}

/// `mapfile [-d DELIM] [-n COUNT] [-O ORIGIN] [-s SKIP] [-t] [ARRAY]`
/// (alias `readarray`). Reads delimiter-separated records from stdin into the
/// indexed array ARRAY (default MAPFILE). Core option set (v140); `-u`/`-C`/`-c`
/// are not implemented.
fn builtin_mapfile(args: &[String], err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let mut delim: u8 = b'\n';
    let mut strip_t = false;
    let mut count: usize = 0; // 0 = unlimited
    let mut skip: usize = 0;
    let mut origin: Option<usize> = None;
    let mut i = 0;

    // Parse a numeric option value (rest-of-arg or next arg).
    fn num_val(
        args: &[String],
        i: &mut usize,
        j: usize,
        bytes: &[u8],
        opt: char,
        err: &mut dyn Write,
        shell: &Shell,
    ) -> Result<usize, ()> {
        let s = if j + 1 < bytes.len() {
            String::from_utf8_lossy(&bytes[j + 1..]).into_owned()
        } else {
            *i += 1;
            if *i >= args.len() {
                crate::sh_error_to!(
                    shell,
                    err,
                    None,
                    "mapfile: -{opt}: option requires an argument"
                );
                return Err(());
            }
            args[*i].clone()
        };
        match s.trim().parse::<usize>() {
            Ok(n) => Ok(n),
            Err(_) => {
                crate::sh_error_to!(shell, err, None, "mapfile: {s}: invalid number");
                Err(())
            }
        }
    }

    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        if !arg.starts_with('-') || arg.len() < 2 {
            break;
        }
        let bytes = arg.as_bytes();
        let mut j = 1;
        let mut consumed_rest = false;
        while j < bytes.len() {
            match bytes[j] {
                b't' => strip_t = true,
                b'd' => {
                    let s = match take_opt_value(args, &mut i, bytes, j, "mapfile", 'd', err, shell)
                    {
                        Ok(v) => v,
                        Err(rc) => return ExecOutcome::Continue(rc),
                    };
                    delim = s.bytes().next().unwrap_or(0u8); // empty -> NUL
                    consumed_rest = true;
                }
                b'n' => match num_val(args, &mut i, j, bytes, 'n', err, shell) {
                    Ok(n) => {
                        count = n;
                        consumed_rest = true;
                    }
                    Err(()) => return ExecOutcome::Continue(2),
                },
                b's' => match num_val(args, &mut i, j, bytes, 's', err, shell) {
                    Ok(n) => {
                        skip = n;
                        consumed_rest = true;
                    }
                    Err(()) => return ExecOutcome::Continue(2),
                },
                b'O' => match num_val(args, &mut i, j, bytes, 'O', err, shell) {
                    Ok(n) => {
                        origin = Some(n);
                        consumed_rest = true;
                    }
                    Err(()) => return ExecOutcome::Continue(2),
                },
                c => {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "mapfile: -{}: invalid option",
                        c as char
                    );
                    return ExecOutcome::Continue(2);
                }
            }
            if consumed_rest {
                break;
            }
            j += 1;
        }
        i += 1;
    }

    let array_name = args
        .get(i)
        .cloned()
        .unwrap_or_else(|| "MAPFILE".to_string());
    if !is_valid_name(&array_name) {
        crate::sh_error_to!(
            shell,
            err,
            None,
            "mapfile: `{array_name}': not a valid array name"
        );
        return ExecOutcome::Continue(1);
    }

    let mut handle = RawFdReader::new();
    // Skip the first `skip` records.
    for _ in 0..skip {
        match read_one_record(&mut handle, delim) {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => {
                crate::sh_error_to!(shell, err, None, "mapfile: {}", crate::bash_io_error(&e));
                return ExecOutcome::Continue(1);
            }
        }
    }
    // Collect up to `count` (0 = unlimited) records.
    let mut elements: Vec<String> = Vec::new();
    loop {
        if count != 0 && elements.len() >= count {
            break;
        }
        match read_one_record(&mut handle, delim) {
            Ok(Some((content, had_delim))) => {
                let mut val = content;
                if had_delim && !strip_t {
                    val.push(delim as char);
                }
                elements.push(val);
            }
            Ok(None) => break,
            Err(e) => {
                crate::sh_error_to!(shell, err, None, "mapfile: {}", crate::bash_io_error(&e));
                return ExecOutcome::Continue(1);
            }
        }
    }

    match origin {
        None => {
            let map: std::collections::BTreeMap<usize, String> =
                elements.into_iter().enumerate().collect();
            if shell.replace_indexed(&array_name, map).is_err() {
                return ExecOutcome::Continue(1);
            }
        }
        Some(o) => {
            for (k, val) in elements.into_iter().enumerate() {
                if shell.set_indexed_element(&array_name, o + k, val).is_err() {
                    return ExecOutcome::Continue(1);
                }
            }
        }
    }
    ExecOutcome::Continue(0)
}

/// `read [-r] [-p PROMPT] [-s] [-d DELIM] [-a ARRAY] [NAME ...]`. Regular
/// builtin. Reads one logical line from stdin and assigns fields to
/// NAME(s) per IFS field-splitting. With no NAME, assigns the whole
/// line to `REPLY`. `-r` disables backslash processing. `-p` writes
/// PROMPT to stderr (only when stdin is a tty, matching bash). `-s`
/// disables ECHO via termios for the duration of the read (when
/// stdin is a tty). `-d` sets the line-terminator byte (empty DELIM
/// → NUL). Exit 0 on success; 1 on EOF-before-any-byte or readonly
/// assignment failure; 2 on bad flag.
fn builtin_read(
    args: &[String],
    _out: &mut dyn std::io::Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let mut raw = false;
    let mut silent = false;
    let mut prompt: Option<String> = None;
    let mut delim: u8 = b'\n';
    let mut array_name: Option<String> = None;
    // `-u FD`: read from this file descriptor instead of stdin. `None` = stdin.
    let mut read_fd: Option<std::os::unix::io::RawFd> = None;
    let mut max_chars: Option<usize> = None;
    let mut nchars_active_delim = true;
    let mut timeout: Option<f64> = None;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        if !arg.starts_with('-') || arg.len() < 2 {
            break;
        }
        let bytes = arg.as_bytes();
        let mut j = 1;
        while j < bytes.len() {
            match bytes[j] {
                b'r' => raw = true,
                b's' => silent = true,
                b'p' => {
                    // -p PROMPT — value is rest-of-arg OR next arg.
                    if j + 1 < bytes.len() {
                        prompt = Some(String::from_utf8_lossy(&bytes[j + 1..]).into_owned());
                    } else {
                        i += 1;
                        if i >= args.len() {
                            crate::sh_error_to!(
                                shell,
                                err,
                                None,
                                "read: -p: option requires an argument"
                            );
                            return ExecOutcome::Continue(2);
                        }
                        prompt = Some(args[i].clone());
                    }
                    break;
                }
                b'd' => {
                    let d_val =
                        match take_opt_value(args, &mut i, bytes, j, "read", 'd', err, shell) {
                            Ok(v) => v,
                            Err(rc) => return ExecOutcome::Continue(rc),
                        };
                    // Empty DELIM means NUL byte.
                    delim = d_val.bytes().next().unwrap_or(0u8);
                    break;
                }
                b'a' => {
                    let v = match take_opt_value(args, &mut i, bytes, j, "read", 'a', err, shell) {
                        Ok(v) => v,
                        Err(rc) => return ExecOutcome::Continue(rc),
                    };
                    array_name = Some(v);
                    break;
                }
                b'u' => {
                    let v = match take_opt_value(args, &mut i, bytes, j, "read", 'u', err, shell) {
                        Ok(v) => v,
                        Err(rc) => return ExecOutcome::Continue(rc),
                    };
                    // A non-numeric fd spec is rejected up front (bash:
                    // "read: <val>: invalid file descriptor specification").
                    match v.trim().parse::<std::os::unix::io::RawFd>() {
                        Ok(fd) if fd >= 0 => read_fd = Some(fd),
                        _ => {
                            crate::sh_error_to!(
                                shell,
                                err,
                                None,
                                "read: {v}: invalid file descriptor specification"
                            );
                            return ExecOutcome::Continue(1);
                        }
                    }
                    break;
                }
                b'n' | b'N' => {
                    let upper = bytes[j] == b'N';
                    let v = match take_opt_value(
                        args,
                        &mut i,
                        bytes,
                        j,
                        "read",
                        bytes[j] as char,
                        err,
                        shell,
                    ) {
                        Ok(v) => v,
                        Err(rc) => return ExecOutcome::Continue(rc),
                    };
                    match v.trim().parse::<usize>() {
                        Ok(k) => {
                            max_chars = Some(k);
                            nchars_active_delim = !upper;
                        }
                        Err(_) => {
                            crate::sh_error_to!(shell, err, None, "read: {v}: invalid number");
                            return ExecOutcome::Continue(1);
                        }
                    }
                    break;
                }
                b't' => {
                    let v = match take_opt_value(args, &mut i, bytes, j, "read", 't', err, shell) {
                        Ok(v) => v,
                        Err(rc) => return ExecOutcome::Continue(rc),
                    };
                    match v.trim().parse::<f64>() {
                        Ok(t) if t >= 0.0 && t.is_finite() => timeout = Some(t),
                        _ => {
                            crate::sh_error_to!(
                                shell,
                                err,
                                None,
                                "read: {v}: invalid timeout specification"
                            );
                            return ExecOutcome::Continue(1);
                        }
                    }
                    break;
                }
                c => {
                    crate::sh_error_to!(shell, err, None, "read: -{}: invalid option", c as char);
                    return ExecOutcome::Continue(2);
                }
            }
            j += 1;
        }
        i += 1;
    }
    let names: Vec<String> = args[i..].to_vec();

    // Validate names BEFORE reading (POSIX ordering).
    for name in &names {
        if !is_valid_name(name) {
            crate::sh_error_to!(shell, err, None, "read: `{name}': not a valid identifier");
            return ExecOutcome::Continue(1);
        }
    }
    if let Some(arr) = &array_name
        && !is_valid_name(arr)
    {
        crate::sh_error_to!(shell, err, None, "read: `{arr}': not a valid identifier");
        return ExecOutcome::Continue(1);
    }

    // `-u FD`: validate the fd is actually open BEFORE reading (bash checks
    // immediately via fcntl(fd, F_GETFD) == -1 && errno == EBADF), so an
    // unopened fd errors without consuming any input.
    if let Some(fd) = read_fd
        && unsafe { libc::fcntl(fd, libc::F_GETFD) } == -1
    {
        crate::sh_error_to!(
            shell,
            err,
            None,
            "read: {fd}: invalid file descriptor: Bad file descriptor"
        );
        return ExecOutcome::Continue(1);
    }

    // Prompt — only when stdin is a tty (matches bash).
    if let Some(p) = &prompt {
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() {
            let _ = write!(err, "{p}");
            let _ = err.flush();
        }
    }

    // -s silent: toggle ECHO off on the read fd's tty (stdin unless `-u FD`)
    // for the duration of the read, then restore.
    #[cfg(unix)]
    let tty_fd = read_fd.unwrap_or(libc::STDIN_FILENO);
    #[cfg(unix)]
    let saved_term = if silent {
        unsafe { silent_disable_echo(tty_fd) }
    } else {
        None
    };

    // Read directly from STDIN_FILENO via libc::read, bypassing Rust's
    // BufReader-backed std::io::stdin(). The static BufReader is shared
    // with rustyline's non-tty `readline_direct` path, which fills it
    // with subsequent script lines on a single underlying read; using
    // BufReader here would return cached script bytes instead of the
    // redirected fd 0 (e.g. our `<<<` here-string pipe).
    let mut handle = match read_fd {
        Some(fd) => RawFdReader::from_fd(fd),
        None => RawFdReader::new(),
    };
    // `-t 0`: availability probe — poll once with 0 timeout, read nothing.
    #[cfg(unix)]
    if timeout == Some(0.0) {
        let fd = handle.raw_fd();
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let pr = unsafe { libc::poll(&mut pfd, 1, 0) };
        if let Some(s) = saved_term {
            unsafe {
                silent_restore_echo(tty_fd, s);
            }
        }
        return ExecOutcome::Continue(if pr > 0 { 0 } else { 1 });
    }
    let deadline = timeout.and_then(|t| {
        if t > 0.0 {
            Some(std::time::Instant::now() + std::time::Duration::from_secs_f64(t))
        } else {
            None
        }
    });
    let poll_fd = Some(handle.raw_fd());
    let cfg = ReadCfg {
        raw,
        delim,
        delim_active: nchars_active_delim,
        max_chars,
        deadline,
    };
    let (line, stop, _any_read) = match read_record(&mut handle, &cfg, poll_fd) {
        Ok(t) => t,
        Err(e) => {
            crate::sh_error_to!(shell, err, None, "read: {}", crate::bash_io_error(&e));
            #[cfg(unix)]
            if let Some(s) = saved_term {
                unsafe {
                    silent_restore_echo(tty_fd, s);
                }
            }
            return ExecOutcome::Continue(1);
        }
    };

    // Restore echo. Only emit the trailing newline when we ACTUALLY
    // suppressed echo (tty AND tcsetattr succeeded), so that
    // `read -s X < pipe` doesn't print a stray blank line. EOF
    // doesn't change that — if echo was off on a tty, the user's
    // Enter (or Ctrl-D) still didn't show, so the newline belongs.
    #[cfg(unix)]
    let was_silenced = saved_term.is_some();
    #[cfg(not(unix))]
    let was_silenced = false;
    #[cfg(unix)]
    if let Some(s) = saved_term {
        unsafe {
            silent_restore_echo(tty_fd, s);
        }
    }
    if was_silenced {
        e!(err, "");
    }

    // Base exit status from the stop reason (bash): 0 iff a delimiter or the
    // -n/-N count was reached; 1 on EOF (even with partial data); 128+SIGALRM
    // on -t timeout.
    let base_exit = match stop {
        ReadStop::Delim | ReadStop::Count => 0,
        ReadStop::Eof => 1,
        ReadStop::Timeout => 128 + libc::SIGALRM,
    };

    // Assignment ALWAYS runs (even on EOF/empty) so named vars are cleared to
    // empty — bash sets them, it does not leave stale values. `line` is "" on a
    // pure EOF.
    // `-N` (uppercase count) assigns the RAW read string — no IFS splitting,
    // no leading/trailing trim — to the first named var (or as a single `-a`
    // array element, or to REPLY). `-n` (lowercase) and the no-count case
    // still split normally. `nchars_active_delim` is `false` only for `-N`.
    let raw_count_mode = max_chars.is_some() && !nchars_active_delim;

    let ifs = shell.ifs();
    if let Some(arr) = array_name {
        let map: std::collections::BTreeMap<usize, String> = if raw_count_mode {
            std::iter::once((0usize, line.clone())).collect()
        } else {
            split_read_fields(&line, &ifs)
                .into_iter()
                .enumerate()
                .collect()
        };
        if shell.replace_indexed(&arr, map).is_err() {
            return ExecOutcome::Continue(1); // replace_indexed printed the readonly message
        }
        return ExecOutcome::Continue(base_exit);
    }
    let assignments: Vec<(String, String)> = if names.is_empty() {
        vec![("REPLY".to_string(), line)]
    } else if raw_count_mode {
        let mut out = Vec::with_capacity(names.len());
        out.push((names[0].clone(), line));
        for n in &names[1..] {
            out.push((n.clone(), String::new()));
        }
        out
    } else {
        split_into_names(&line, &names, &ifs)
    };

    let mut exit = base_exit;
    for (name, value) in assignments {
        if shell.try_set(&name, value).is_err() {
            crate::sh_error_to!(shell, err, None, "read: {name}: readonly variable");
            exit = 1;
        }
    }
    ExecOutcome::Continue(exit)
}

// ════════════════════════════════════════════════════════════════════
// printf builtin (M-73, v56)
// ════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq)]
enum FormatPart {
    Literal(Vec<u8>),
    Conv(ConvSpec),
}

#[derive(Debug, Clone, PartialEq, Default)]
struct ConvFlags {
    left_align: bool,
    sign: bool,
    space_sign: bool,
    alt: bool,
    zero_pad: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct ConvSpec {
    flags: ConvFlags,
    width: Option<usize>,
    precision: Option<usize>,
    /// Width came from a `*` (dynamic): take it from the next arg.
    width_star: bool,
    /// Precision came from a `.*` (dynamic): take it from the next arg.
    prec_star: bool,
    conv: ConvChar,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ConvChar {
    S,
    D,
    I,
    U,
    O,
    X,
    BigX,
    C,
    B,
    Q,
    Percent,
    /// Floating-point: `f F e E g G` (rendered via libc::snprintf).
    Float(u8),
}

/// Decodes a backslash-escape starting at the byte AFTER the `\`.
/// Returns `(decoded_bytes, advance)` where `advance` is the number
/// of bytes consumed past the backslash. Unknown escapes are emitted
/// as the literal backslash + the next char (printf's bash-compatible
/// behavior); a trailing backslash (empty `rest`) becomes a literal
/// `\`.
fn decode_printf_escape(rest: &[u8]) -> (Vec<u8>, usize) {
    if rest.is_empty() {
        return (b"\\".to_vec(), 0);
    }
    match rest[0] {
        b'\\' => (b"\\".to_vec(), 1),
        b'a' => (b"\x07".to_vec(), 1),
        b'b' => (b"\x08".to_vec(), 1),
        b'f' => (b"\x0C".to_vec(), 1),
        b'n' => (b"\n".to_vec(), 1),
        b'r' => (b"\r".to_vec(), 1),
        b't' => (b"\t".to_vec(), 1),
        b'v' => (b"\x0B".to_vec(), 1),
        b'/' => (b"/".to_vec(), 1),
        b'"' => (b"\"".to_vec(), 1),
        b'\'' => (b"'".to_vec(), 1),
        // \NNN (1-3 octal digits). When the first digit is '0', accept
        // up to 4 digits (the leading '0' counts toward the budget),
        // matching bash printf's `\0NNN` form.
        c if (b'0'..=b'7').contains(&c) => {
            let max = if c == b'0' { 4 } else { 3 };
            let mut n = 0usize;
            let mut v: u32 = 0;
            while n < max && n < rest.len() && (b'0'..=b'7').contains(&rest[n]) {
                v = v * 8 + (rest[n] - b'0') as u32;
                n += 1;
            }
            (vec![(v & 0xFF) as u8], n)
        }
        b'x' => {
            // 1-2 hex digits after \x.
            let mut n = 1;
            let mut hex = 0u32;
            let mut count = 0;
            while count < 2 && n < rest.len() && (rest[n] as char).is_ascii_hexdigit() {
                hex = hex * 16 + (rest[n] as char).to_digit(16).unwrap();
                n += 1;
                count += 1;
            }
            if count == 0 {
                // \x with no hex digit: emit literally.
                (vec![b'\\', b'x'], 1)
            } else {
                (vec![hex as u8], n)
            }
        }
        // \c at format-string level is literal; %b's caller handles
        // it separately.
        b'c' => (vec![b'\\', b'c'], 1),
        // Unknown — emit backslash + the char literally.
        c => (vec![b'\\', c], 1),
    }
}

/// Decodes escape sequences in a `%b` argument. Returns the decoded
/// bytes and a bool: true if a `\c` was encountered (caller halts
/// output).
fn decode_printf_b_arg(arg: &str) -> (Vec<u8>, bool) {
    let bytes = arg.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            // \c halts.
            if bytes[i + 1] == b'c' {
                return (out, true);
            }
            let (dec, used) = decode_printf_escape(&bytes[i + 1..]);
            out.extend_from_slice(&dec);
            i += 1 + used;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    (out, false)
}

/// Parses a printf format string into a sequence of `FormatPart`s.
/// Literals have backslash escapes already decoded; conv specs
/// capture flags + width + precision + conv-char.
fn parse_format(fmt: &str) -> Result<Vec<FormatPart>, String> {
    let bytes = fmt.as_bytes();
    let mut parts: Vec<FormatPart> = Vec::new();
    let mut lit: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' {
            let (dec, used) = decode_printf_escape(&bytes[i + 1..]);
            lit.extend_from_slice(&dec);
            i += 1 + used;
            continue;
        }
        if b != b'%' {
            lit.push(b);
            i += 1;
            continue;
        }
        // Flush literal.
        if !lit.is_empty() {
            parts.push(FormatPart::Literal(std::mem::take(&mut lit)));
        }
        i += 1; // past '%'

        // Parse spec: [flags][width][.precision][conv]
        let mut flags = ConvFlags::default();
        loop {
            if i >= bytes.len() {
                return Err("missing conversion character".into());
            }
            match bytes[i] {
                b'-' => flags.left_align = true,
                b'+' => flags.sign = true,
                b' ' => flags.space_sign = true,
                b'#' => flags.alt = true,
                b'0' => flags.zero_pad = true,
                _ => break,
            }
            i += 1;
        }
        // Width: `*` (dynamic, from next arg) or decimal digits.
        let mut width: Option<usize> = None;
        let mut width_star = false;
        if i < bytes.len() && bytes[i] == b'*' {
            width_star = true;
            i += 1;
        } else {
            let mut wstr = String::new();
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                wstr.push(bytes[i] as char);
                i += 1;
            }
            if !wstr.is_empty() {
                width = Some(wstr.parse().unwrap_or(0));
            }
        }
        // Precision: `.` then `*` (dynamic) or decimal digits.
        let mut precision: Option<usize> = None;
        let mut prec_star = false;
        if i < bytes.len() && bytes[i] == b'.' {
            i += 1;
            if i < bytes.len() && bytes[i] == b'*' {
                prec_star = true;
                i += 1;
            } else {
                let mut pstr = String::new();
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    pstr.push(bytes[i] as char);
                    i += 1;
                }
                precision = Some(if pstr.is_empty() {
                    0
                } else {
                    pstr.parse().unwrap_or(0)
                });
            }
        }
        // Conversion char.
        if i >= bytes.len() {
            return Err("missing conversion character".into());
        }
        let conv = match bytes[i] {
            b's' => ConvChar::S,
            b'd' => ConvChar::D,
            b'i' => ConvChar::I,
            b'u' => ConvChar::U,
            b'o' => ConvChar::O,
            b'x' => ConvChar::X,
            b'X' => ConvChar::BigX,
            b'c' => ConvChar::C,
            b'b' => ConvChar::B,
            b'q' => ConvChar::Q,
            b'%' => ConvChar::Percent,
            c @ (b'f' | b'F' | b'e' | b'E' | b'g' | b'G') => ConvChar::Float(c),
            c => return Err(format!("`%{}': invalid directive", c as char)),
        };
        i += 1;
        parts.push(FormatPart::Conv(ConvSpec {
            flags,
            width,
            precision,
            width_star,
            prec_star,
            conv,
        }));
    }
    if !lit.is_empty() {
        parts.push(FormatPart::Literal(lit));
    }
    Ok(parts)
}

/// Parses a printf integer argument per POSIX / bash rules.
/// Returns (value, optional error message). On trailing garbage, the
/// parsed prefix is returned along with an error string; on empty,
/// returns 0 with no error.
fn parse_printf_int(s: &str) -> (i64, Option<String>) {
    let trimmed = s.trim_start();
    if trimmed.is_empty() {
        return (0, None);
    }
    let bytes = trimmed.as_bytes();
    // Char-literal form: leading ' or ".
    if bytes[0] == b'\'' || bytes[0] == b'"' {
        if bytes.len() == 1 {
            return (0, None);
        }
        let v = bytes[1] as i64;
        let extra = if bytes.len() > 2 {
            Some(format!(
                "warning: `{s}': character(s) following character constant have been ignored"
            ))
        } else {
            None
        };
        return (v, extra);
    }
    // Signed prefix.
    let (sign, rest) = match bytes[0] {
        b'+' => (1i64, &trimmed[1..]),
        b'-' => (-1i64, &trimmed[1..]),
        _ => (1i64, trimmed),
    };
    // Hex / octal / decimal.
    let (radix, digits) = if rest.starts_with("0x") || rest.starts_with("0X") {
        (16u32, &rest[2..])
    } else if rest.starts_with('0') && rest.len() > 1 {
        (8u32, &rest[1..])
    } else {
        (10u32, rest)
    };
    if digits.is_empty() {
        return (0, None);
    }
    // Consume all valid digits; report trailing garbage.
    let mut end = 0;
    for (j, c) in digits.char_indices() {
        if c.is_digit(radix) {
            end = j + c.len_utf8();
        } else {
            break;
        }
    }
    if end == 0 {
        // No valid digits at all.
        return (0, Some(format!("`{s}': invalid number")));
    }
    let parsed = i64::from_str_radix(&digits[..end], radix).unwrap_or(0);
    let err = if end < digits.len() {
        Some(format!("`{s}': invalid number"))
    } else {
        None
    };
    (sign.saturating_mul(parsed), err)
}

/// Parses a printf float argument. Returns (value, optional error).
/// Mirrors `parse_printf_int`'s contract: empty → 0 (no error);
/// a leading `'`/`"` char-literal yields that char's code; otherwise
/// a leading numeric prefix is parsed as f64 and trailing garbage is
/// reported (value = parsed prefix, or 0 if none).
fn parse_printf_float(s: &str) -> (f64, Option<String>) {
    let trimmed = s.trim_start();
    if trimmed.is_empty() {
        return (0.0, None);
    }
    let bytes = trimmed.as_bytes();
    // Char-literal form: leading ' or " (same as the integer path).
    if bytes[0] == b'\'' || bytes[0] == b'"' {
        if bytes.len() == 1 {
            return (0.0, None);
        }
        let v = bytes[1] as f64;
        let extra = if bytes.len() > 2 {
            Some(format!(
                "warning: `{s}': character(s) following character constant have been ignored"
            ))
        } else {
            None
        };
        return (v, extra);
    }
    // Whole string parses cleanly (covers integers, decimals, exponents,
    // nan/inf): no error.
    if let Ok(v) = trimmed.parse::<f64>() {
        return (v, None);
    }
    // Otherwise find the longest leading prefix that parses as f64; the
    // remaining bytes are trailing garbage (matches bash's `invalid number`
    // warning while still using the parsed prefix).
    let mut best: Option<f64> = None;
    for (idx, _) in trimmed.char_indices().skip(1) {
        if let Ok(v) = trimmed[..idx].parse::<f64>() {
            best = Some(v);
        }
    }
    match best {
        Some(v) => (v, Some(format!("`{s}': invalid number"))),
        None => (0.0, Some(format!("`{s}': invalid number"))),
    }
}

/// Renders one resolved float directive via `libc::snprintf`, matching
/// C/bash float formatting byte-for-byte. `width`/`precision` are already
/// resolved to concrete values (dynamic `*` handled by the caller).
fn snprintf_float(spec: &ConvSpec, conv: u8, value: f64) -> Vec<u8> {
    // Reconstruct the C conversion spec: %[flags][width][.precision]<conv>.
    let mut cfmt = String::from("%");
    if spec.flags.left_align {
        cfmt.push('-');
    }
    if spec.flags.sign {
        cfmt.push('+');
    }
    if spec.flags.space_sign {
        cfmt.push(' ');
    }
    if spec.flags.alt {
        cfmt.push('#');
    }
    if spec.flags.zero_pad {
        cfmt.push('0');
    }
    if let Some(w) = spec.width {
        cfmt.push_str(&w.to_string());
    }
    if let Some(p) = spec.precision {
        cfmt.push('.');
        cfmt.push_str(&p.to_string());
    }
    cfmt.push(conv as char);

    let cfmt_c = match std::ffi::CString::new(cfmt) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    // 512 is plenty for normal use; grow once if truncated.
    let mut cap = 512usize;
    loop {
        let mut buf = vec![0u8; cap];
        // SAFETY: `cfmt_c` is a single, well-formed float conversion spec
        // (one directive, no `%n`, no `*` — those were resolved away). The
        // matching variadic argument is the `f64` `value`, which is the
        // correct type for `f`/`e`/`g` conversions on all targets. The
        // buffer is `cap` bytes and `snprintf` never writes past it.
        let n = unsafe {
            libc::snprintf(
                buf.as_mut_ptr() as *mut libc::c_char,
                cap,
                cfmt_c.as_ptr(),
                value,
            )
        };
        if n < 0 {
            return Vec::new();
        }
        let n = n as usize;
        if n < cap {
            buf.truncate(n);
            return buf;
        }
        // Truncated: grow to fit and retry.
        cap = n + 1;
    }
}

/// bash `printf %q`: quote `arg` so it re-reads as the same word. Empty → `''`;
/// a control char → the `$'…'` ANSI-C form; otherwise backslash-escape each
/// shell-special char. `~` and `#` are special ONLY as the leading char
/// (tilde-expansion / comment); everything else in the set is special at any
/// position. Letters, digits, `%+-./:=@_`, and printable UTF-8 are emitted
/// as-is.
fn printf_q(arg: &str) -> String {
    if arg.is_empty() {
        return "''".to_string();
    }
    if arg.chars().any(|c| c.is_control()) {
        return crate::param_expansion::ansi_c_quote(arg);
    }
    const ALWAYS: &str = " !\"$&'()*,;<>?[\\]^`{|}";
    let mut out = String::with_capacity(arg.len());
    for (i, c) in arg.chars().enumerate() {
        if ALWAYS.contains(c) || (i == 0 && (c == '#' || c == '~')) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Formats a single conv-spec + arg into `out`. Returns Ok(true) for
/// normal completion, Ok(false) if `\c` halted output (only possible
/// for `%b`), Err for an invalid integer arg (caller logs + sets
/// status 1 but does NOT halt).
fn format_one(spec: &ConvSpec, arg: &str, out: &mut Vec<u8>) -> Result<bool, String> {
    let pad_string = |s: &[u8], spec: &ConvSpec| -> Vec<u8> {
        let truncated: &[u8] = if let Some(p) = spec.precision {
            &s[..s.len().min(p)]
        } else {
            s
        };
        let width = spec.width.unwrap_or(0);
        if truncated.len() >= width {
            return truncated.to_vec();
        }
        let pad_len = width - truncated.len();
        let mut v = Vec::with_capacity(width);
        if spec.flags.left_align {
            v.extend_from_slice(truncated);
            v.extend(std::iter::repeat_n(b' ', pad_len));
        } else {
            v.extend(std::iter::repeat_n(b' ', pad_len));
            v.extend_from_slice(truncated);
        }
        v
    };

    let pad_number = |digits: &[u8], spec: &ConvSpec, prefix: &[u8]| -> Vec<u8> {
        // Precision = min digit count (zero-pad to precision).
        // POSIX: when precision is explicitly 0 and the value is 0,
        // no digits are produced. (`printf '%.0d' 0` → empty string.)
        let prec = spec.precision.unwrap_or(1);
        let digit_part: Vec<u8> = if spec.precision == Some(0) && digits.iter().all(|&b| b == b'0')
        {
            Vec::new()
        } else if digits.len() >= prec {
            digits.to_vec()
        } else {
            let mut v = Vec::with_capacity(prec);
            v.extend(std::iter::repeat_n(b'0', prec - digits.len()));
            v.extend_from_slice(digits);
            v
        };
        let body_len = prefix.len() + digit_part.len();
        let width = spec.width.unwrap_or(0);
        if body_len >= width {
            let mut v = Vec::with_capacity(body_len);
            v.extend_from_slice(prefix);
            v.extend_from_slice(&digit_part);
            return v;
        }
        let pad_len = width - body_len;
        // Zero-pad only when no precision AND not left-aligned.
        let use_zero = spec.flags.zero_pad && !spec.flags.left_align && spec.precision.is_none();
        let pad_char = if use_zero { b'0' } else { b' ' };
        let mut v = Vec::with_capacity(width);
        if spec.flags.left_align {
            v.extend_from_slice(prefix);
            v.extend_from_slice(&digit_part);
            v.extend(std::iter::repeat_n(b' ', pad_len));
        } else if use_zero {
            // Sign/0x prefix before zeros: prefix then zeros then digits.
            v.extend_from_slice(prefix);
            v.extend(std::iter::repeat_n(pad_char, pad_len));
            v.extend_from_slice(&digit_part);
        } else {
            v.extend(std::iter::repeat_n(pad_char, pad_len));
            v.extend_from_slice(prefix);
            v.extend_from_slice(&digit_part);
        }
        v
    };

    match spec.conv {
        ConvChar::S => {
            out.extend_from_slice(&pad_string(arg.as_bytes(), spec));
            Ok(true)
        }
        ConvChar::Q => {
            out.extend_from_slice(&pad_string(printf_q(arg).as_bytes(), spec));
            Ok(true)
        }
        ConvChar::C => {
            // First byte (or empty).
            let bytes = arg.as_bytes();
            let body: &[u8] = if bytes.is_empty() { &[] } else { &bytes[..1] };
            out.extend_from_slice(&pad_string(body, spec));
            Ok(true)
        }
        ConvChar::D | ConvChar::I => {
            let (val, err) = parse_printf_int(arg);
            let abs = val.unsigned_abs();
            let digits = abs.to_string().into_bytes();
            let mut prefix: Vec<u8> = Vec::new();
            if val < 0 {
                prefix.push(b'-');
            } else if spec.flags.sign {
                prefix.push(b'+');
            } else if spec.flags.space_sign {
                prefix.push(b' ');
            }
            out.extend_from_slice(&pad_number(&digits, spec, &prefix));
            err.map_or(Ok(true), Err)
        }
        ConvChar::U => {
            let (val, err) = parse_printf_int(arg);
            let unsigned = val as u64;
            let digits = unsigned.to_string().into_bytes();
            out.extend_from_slice(&pad_number(&digits, spec, &[]));
            err.map_or(Ok(true), Err)
        }
        ConvChar::O => {
            let (val, err) = parse_printf_int(arg);
            let unsigned = val as u64;
            let s = format!("{unsigned:o}");
            let prefix: &[u8] = if spec.flags.alt && !s.starts_with('0') {
                b"0"
            } else {
                b""
            };
            out.extend_from_slice(&pad_number(s.as_bytes(), spec, prefix));
            err.map_or(Ok(true), Err)
        }
        ConvChar::X => {
            let (val, err) = parse_printf_int(arg);
            let unsigned = val as u64;
            let s = format!("{unsigned:x}");
            let prefix: &[u8] = if spec.flags.alt && unsigned != 0 {
                b"0x"
            } else {
                b""
            };
            out.extend_from_slice(&pad_number(s.as_bytes(), spec, prefix));
            err.map_or(Ok(true), Err)
        }
        ConvChar::BigX => {
            let (val, err) = parse_printf_int(arg);
            let unsigned = val as u64;
            let s = format!("{unsigned:X}");
            let prefix: &[u8] = if spec.flags.alt && unsigned != 0 {
                b"0X"
            } else {
                b""
            };
            out.extend_from_slice(&pad_number(s.as_bytes(), spec, prefix));
            err.map_or(Ok(true), Err)
        }
        ConvChar::B => {
            let (decoded, halted) = decode_printf_b_arg(arg);
            out.extend_from_slice(&pad_string(&decoded, spec));
            Ok(!halted)
        }
        ConvChar::Float(conv) => {
            let (val, err) = parse_printf_float(arg);
            out.extend_from_slice(&snprintf_float(spec, conv, val));
            err.map_or(Ok(true), Err)
        }
        ConvChar::Percent => {
            // Caller treats `%%` specially (no arg consumed); shouldn't
            // reach here, but emit a `%` defensively.
            out.push(b'%');
            Ok(true)
        }
    }
}

fn builtin_printf(
    args: &[String],
    out: &mut dyn std::io::Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    // Parse leading flags: -v VAR, -- end-of-flags.
    let mut v_var: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-v" => {
                i += 1;
                if i >= args.len() {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "printf: -v: option requires an argument"
                    );
                    return ExecOutcome::Continue(2);
                }
                let target = &args[i];
                let valid = is_valid_name(target)
                    || crate::expand::split_name_subscript(target)
                        .map(|(name, sub)| is_valid_name(&name) && !sub.is_empty())
                        .unwrap_or(false);
                if !valid {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "printf: `{target}': not a valid identifier"
                    );
                    return ExecOutcome::Continue(1);
                }
                v_var = Some(target.clone());
                i += 1;
            }
            "--" => {
                i += 1;
                break;
            }
            s if s.starts_with('-') && s.len() > 1 && s != "-" => {
                // Bash's printf rejects unknown flags but accepts a
                // lone "-" as a format. We do the same.
                crate::sh_error_to!(shell, err, None, "printf: {s}: invalid option");
                return ExecOutcome::Continue(2);
            }
            _ => break,
        }
    }

    if i >= args.len() {
        e!(err, "printf: usage: printf [-v var] format [arguments]");
        return ExecOutcome::Continue(2);
    }

    let format = args[i].clone();
    let rest_args: &[String] = &args[i + 1..];

    let parts = match parse_format(&format) {
        Ok(p) => p,
        Err(e) => {
            crate::sh_error_to!(shell, err, None, "printf: {e}");
            return ExecOutcome::Continue(1);
        }
    };

    // Determine whether the format has any consuming conv (anything
    // that pops an arg from `rest_args`). %% does NOT consume.
    let has_consuming_conv = parts.iter().any(|p| match p {
        FormatPart::Conv(c) => !matches!(c.conv, ConvChar::Percent),
        _ => false,
    });

    let mut buf: Vec<u8> = Vec::new();
    let mut exit: i32 = 0;
    let mut arg_idx = 0;
    let mut halted = false;

    loop {
        for part in &parts {
            if halted {
                break;
            }
            match part {
                FormatPart::Literal(s) => buf.extend_from_slice(s),
                FormatPart::Conv(c) if matches!(c.conv, ConvChar::Percent) => {
                    buf.push(b'%');
                }
                FormatPart::Conv(c) => {
                    // Resolve dynamic `*` width/precision: each `*` consumes
                    // the next arg as an integer before the conversion's own
                    // arg. A negative width means left-justify (C semantics);
                    // a negative precision is treated as if omitted.
                    let mut spec = c.clone();
                    let next_arg = |arg_idx: &mut usize| -> &str {
                        let a = if *arg_idx < rest_args.len() {
                            rest_args[*arg_idx].as_str()
                        } else {
                            ""
                        };
                        *arg_idx += 1;
                        a
                    };
                    if spec.width_star {
                        let (n, perr) = parse_printf_int(next_arg(&mut arg_idx));
                        if let Some(msg) = perr {
                            crate::sh_error_to!(shell, err, None, "printf: {msg}");
                            exit = 1;
                        }
                        if n < 0 {
                            spec.flags.left_align = true;
                            spec.width = Some(n.unsigned_abs() as usize);
                        } else {
                            spec.width = Some(n as usize);
                        }
                    }
                    if spec.prec_star {
                        let (n, perr) = parse_printf_int(next_arg(&mut arg_idx));
                        if let Some(msg) = perr {
                            crate::sh_error_to!(shell, err, None, "printf: {msg}");
                            exit = 1;
                        }
                        spec.precision = if n < 0 { None } else { Some(n as usize) };
                    }
                    let arg = next_arg(&mut arg_idx);
                    match format_one(&spec, arg, &mut buf) {
                        Ok(true) => {}
                        Ok(false) => halted = true,
                        Err(msg) => {
                            crate::sh_error_to!(shell, err, None, "printf: {msg}");
                            exit = 1;
                        }
                    }
                }
            }
        }
        if halted {
            break;
        }
        // Cycle iff there's at least one consuming conv AND args remain.
        if !has_consuming_conv {
            break;
        }
        if arg_idx >= rest_args.len() {
            break;
        }
    }

    // Output.
    if let Some(var) = v_var {
        let s = String::from_utf8_lossy(&buf).into_owned();
        if let Some((name, sub)) = crate::expand::split_name_subscript(&var) {
            // Array-element target: write via the same path as `name[sub]=value`,
            // so the subscript is arith-evaluated (indexed) / string-keyed
            // (associative), the array is created/promoted, and readonly is
            // enforced — all by reuse. (M-109)
            let assignment = crate::command::Assignment {
                target: crate::command::AssignTarget::Indexed {
                    name,
                    subscript: crate::lexer::Word(vec![crate::lexer::WordPart::Literal {
                        text: sub,
                        quoted: false,
                    }]),
                },
                value: crate::lexer::Word(vec![crate::lexer::WordPart::Literal {
                    text: s,
                    quoted: true,
                }]),
                append: false,
            };
            if crate::executor::apply_one_assignment(&assignment, shell, err).is_err() {
                // apply_one_assignment already printed the specific diagnostic
                // (readonly / type mismatch / bad subscript).
                return ExecOutcome::Continue(1);
            }
        } else if shell.try_set(&var, s).is_err() {
            crate::sh_error_to!(shell, err, None, "printf: {var}: readonly variable");
            return ExecOutcome::Continue(1);
        }
    } else if out.write_all(&buf).is_err() {
        // v308: reported once by the epilogue, with bash's wording. This site
        // used the raw io::Error Display, which appended Rust's "(os error N)".
        return ExecOutcome::Continue(1);
    }
    ExecOutcome::Continue(exit)
}

/// Parsed form of the `jobs` argv after flag and positional separation.
struct JobsArgs {
    long: bool,
    pids_only: bool,
    only_new: bool,
    only_running: bool,
    only_stopped: bool,
    targets: Vec<u32>,
}

/// Parses `jobs`'s argv into flags + target ids. Returns
/// `Err(ExecOutcome)` on any usage / lookup failure with the error
/// already printed.
fn parse_jobs_args(
    args: &[String],
    err: &mut dyn Write,
    shell: &Shell,
) -> Result<JobsArgs, ExecOutcome> {
    let mut long = false;
    let mut pids_only = false;
    let mut only_new = false;
    let mut only_running = false;
    let mut only_stopped = false;
    let mut idx = 0;

    while idx < args.len() {
        let a = &args[idx];
        if a == "--" {
            idx += 1;
            break;
        }
        if let Some(rest) = a.strip_prefix('-') {
            if rest.is_empty() {
                break;
            }
            for c in rest.chars() {
                match c {
                    'l' => long = true,
                    'p' => pids_only = true,
                    'n' => only_new = true,
                    'r' => only_running = true,
                    's' => only_stopped = true,
                    _ => {
                        crate::sh_error_to!(shell, err, None, "jobs: -{c}: invalid option");
                        e!(err, "jobs: usage: jobs [-lpnrs] [%spec ...]");
                        return Err(ExecOutcome::Continue(2));
                    }
                }
            }
            idx += 1;
        } else {
            break;
        }
    }

    let mut targets = Vec::new();
    for arg in &args[idx..] {
        if !arg.starts_with('%') {
            crate::sh_error_to!(shell, err, None, "jobs: {arg}: no such job");
            return Err(ExecOutcome::Continue(1));
        }
        let id = resolve_spec_or_error(arg, "jobs", err, shell)?;
        targets.push(id);
    }

    Ok(JobsArgs {
        long,
        pids_only,
        only_new,
        only_running,
        only_stopped,
        targets,
    })
}

/// Returns true if `job` passes the filters in `parsed`.
fn matches_jobs_filter(parsed: &JobsArgs, job: &crate::jobs::Job) -> bool {
    if !parsed.targets.is_empty() && !parsed.targets.contains(&job.id) {
        return false;
    }
    if parsed.only_running && !matches!(job.state, crate::jobs::JobState::Running) {
        return false;
    }
    if parsed.only_stopped && !matches!(job.state, crate::jobs::JobState::Stopped(_)) {
        return false;
    }
    if parsed.only_new && job.notified {
        return false;
    }
    true
}

fn builtin_jobs(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    // #158: observe any pending STOP/CONT reports before reading job state, so
    // non-interactive `jobs` reflects Stopped/Running like the interactive REPL
    // (which reaps pre-prompt). Non-blocking + idempotent.
    crate::jobs::reap_completed(shell);
    let parsed = match parse_jobs_args(args, err, shell) {
        Ok(p) => p,
        Err(outcome) => return outcome,
    };
    let (current, previous) = shell.jobs.current_and_previous();
    let mut printed_ids: Vec<u32> = Vec::new();
    for job in shell.jobs.iter() {
        if !matches_jobs_filter(&parsed, job) {
            continue;
        }
        let flag = if Some(job.id) == current {
            '+'
        } else if Some(job.id) == previous {
            '-'
        } else {
            ' '
        };
        let write_result: std::io::Result<()> = if parsed.pids_only {
            writeln!(out, "{}", job.pgid)
        } else if parsed.long {
            let mut r = Ok(());
            for line in crate::jobs::notification_line_long(job, flag) {
                if let Err(e) = writeln!(out, "{}", line) {
                    r = Err(e);
                    break;
                }
            }
            r
        } else {
            writeln!(out, "{}", crate::jobs::notification_line(job, flag))
        };
        if write_result.is_err() {
            // v308: reported once by the epilogue.
            return ExecOutcome::Continue(1);
        }
        printed_ids.push(job.id);
    }
    if parsed.only_new {
        shell.jobs.mark_notified(&printed_ids);
    }
    ExecOutcome::Continue(0)
}

/// A single positional `wait` target. Built by `parse_wait_args` from a
/// `%spec` (resolved to a job id) or a positive integer PID.
enum WaitTarget {
    Job(u32),
    Pid(i32),
}

/// Parsed form of the `wait` argv after flag and positional separation.
struct WaitArgs {
    wait_any: bool,
    pid_var: Option<String>,
    targets: Vec<WaitTarget>,
}

/// Parses `wait`'s argv into flags + targets. Returns `Err(ExecOutcome)`
/// on any usage / parse failure, with the appropriate stderr message
/// already printed.
fn parse_wait_args(
    args: &[String],
    err: &mut dyn Write,
    shell: &Shell,
) -> Result<WaitArgs, ExecOutcome> {
    let mut wait_any = false;
    let mut pid_var: Option<String> = None;
    let mut idx = 0;

    while idx < args.len() {
        let a = &args[idx];
        match a.as_str() {
            "-n" => {
                wait_any = true;
                idx += 1;
            }
            "-f" => {
                // #160: "wait for full termination rather than a status change".
                // huck's wait has no return-on-stop path (it already blocks to
                // termination), so accept-and-conform: no state to record.
                idx += 1;
            }
            "-p" => {
                if idx + 1 >= args.len() {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "wait: -p: option requires a variable name"
                    );
                    return Err(ExecOutcome::Continue(2));
                }
                pid_var = Some(args[idx + 1].clone());
                idx += 2;
            }
            "--" => {
                idx += 1;
                break;
            }
            s if s.starts_with('-') && s.len() > 1 => {
                crate::sh_error_to!(shell, err, None, "wait: {s}: invalid option");
                e!(err, "wait: usage: wait [-fn] [-p var] [id ...]");
                return Err(ExecOutcome::Continue(2));
            }
            _ => break,
        }
    }

    if pid_var.is_some() && !wait_any {
        crate::sh_error_to!(shell, err, None, "wait: -p: option requires -n");
        return Err(ExecOutcome::Continue(2));
    }

    let mut targets = Vec::with_capacity(args.len() - idx);
    while idx < args.len() {
        let arg = &args[idx];
        if arg.starts_with('%') {
            let id = resolve_spec_or_error(arg, "wait", err, shell)?;
            targets.push(WaitTarget::Job(id));
        } else {
            match arg.parse::<i32>() {
                Ok(pid) if pid > 0 => targets.push(WaitTarget::Pid(pid)),
                _ => {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "wait: `{arg}': not a pid or valid job spec"
                    );
                    // bash returns 1 for a malformed spec (not 2/usage).
                    return Err(ExecOutcome::Continue(1));
                }
            }
        }
        idx += 1;
    }

    Ok(WaitArgs {
        wait_any,
        pid_var,
        targets,
    })
}

fn builtin_wait(
    args: &[String],
    _out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let parsed = match parse_wait_args(args, err, shell) {
        Ok(p) => p,
        Err(outcome) => return outcome,
    };

    match (parsed.wait_any, parsed.targets.len()) {
        (false, 0) => wait_all(shell),
        (false, 1) => match &parsed.targets[0] {
            WaitTarget::Job(id) => wait_for_job(*id, shell),
            WaitTarget::Pid(pid) => wait_for_pid(*pid, err, shell),
        },
        (false, _) => wait_for_all(parsed.targets, err, shell),
        (true, 0) => wait_any_pending(parsed.pid_var, shell),
        (true, _) => wait_any_of(parsed.targets, parsed.pid_var, shell),
    }
}

fn wait_all(shell: &mut Shell) -> ExecOutcome {
    while shell.jobs.has_pending() {
        if let Some(o) = crate::executor::check_interrupt(shell) {
            return o;
        }
        // #183: reap only children we OWN. This used to be `waitpid(-1)`, which
        // reaps ANY child of the process — right for a standalone shell, wrong for
        // huck-engine as a library (it steals the embedder's children) and fatal in
        // the multithreaded test binary, where it drained other tests' children and
        // wedged them. `reap_owned_once` is the single bounded implementation.
        if !crate::jobs::reap_owned_once(shell) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    // Print Done lines for anything that just transitioned during the wait.
    crate::jobs::reap_and_notify(shell);
    ExecOutcome::Continue(0)
}

fn wait_for_job(id: u32, shell: &mut Shell) -> ExecOutcome {
    loop {
        // Check terminal state first — handles already-Done jobs.
        let terminal = shell
            .jobs
            .iter()
            .find(|j| j.id == id)
            .and_then(|j| match j.state {
                crate::jobs::JobState::Done(c) => Some(c),
                crate::jobs::JobState::Signaled(s) => Some(128 + s),
                _ => None,
            });
        if let Some(code) = terminal {
            // #175: bash removes a waited job immediately, so a following
            // `jobs` does not show it — but it retains the terminal status so a
            // later `wait $pid` on the same job still resolves.
            shell.jobs.remove_job_recording_status(id);
            return ExecOutcome::Continue(code);
        }
        if let Some(o) = crate::executor::check_interrupt(shell) {
            return o;
        }
        // #183: reap only children we OWN. This used to be `waitpid(-1)`, which
        // reaps ANY child of the process — right for a standalone shell, wrong for
        // huck-engine as a library (it steals the embedder's children) and fatal in
        // the multithreaded test binary, where it drained other tests' children and
        // wedged them. `reap_owned_once` is the single bounded implementation.
        if !crate::jobs::reap_owned_once(shell) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

fn wait_for_pid(pid: i32, err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let mut first = true;
    loop {
        if let Some(o) = crate::executor::check_interrupt(shell) {
            return o;
        }
        let mut status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if r > 0 {
            shell.jobs.reap(r, status);
            if libc::WIFSTOPPED(status) {
                // Still alive; keep polling. Do NOT reap_coproc (would close a
                // live coproc's fds + unset NAME while it's merely stopped).
                first = false;
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            shell.reap_coproc(r);
            let code = if libc::WIFEXITED(status) {
                libc::WEXITSTATUS(status)
            } else if libc::WIFSIGNALED(status) {
                128 + libc::WTERMSIG(status)
            } else {
                1
            };
            // #175: retain the terminal status so a second `wait $pid` on the
            // same (now-reaped) pid resolves to the same code instead of
            // ECHILD-ing, matching bash. Independent of whether a between-command
            // prune has recorded it yet.
            shell.jobs.record_terminal_status(r, code);
            return ExecOutcome::Continue(code);
        }
        if r < 0 {
            // ECHILD: not a (live) child. #175: the job may have already
            // completed and been auto-pruned from the visible `jobs` list; bash
            // retains its terminal status so `wait $pid` still resolves (even
            // repeatedly). Consult the saved-status ring before erroring.
            if let Some(code) = shell.jobs.saved_status(pid) {
                return ExecOutcome::Continue(code);
            }
            // Genuinely not a child (or already reaped without a saved status).
            // On the first call, surface as "not a child." On a subsequent call,
            // treat as a race we can't recover from.
            if first {
                crate::sh_error_to!(
                    shell,
                    err,
                    None,
                    "wait: pid {pid} is not a child of this shell"
                );
                return ExecOutcome::Continue(127);
            }
            return ExecOutcome::Continue(1);
        }
        first = false;
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Multi-arg `wait` (M-38): wait sequentially for each target. Return
/// the status of the LAST target waited.
fn wait_for_all(targets: Vec<WaitTarget>, err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let mut last = 0;
    for t in targets {
        let outcome = match t {
            WaitTarget::Job(id) => wait_for_job(id, shell),
            WaitTarget::Pid(pid) => wait_for_pid(pid, err, shell),
        };
        match outcome {
            ExecOutcome::Continue(c) => last = c,
            other => return other,
        }
    }
    ExecOutcome::Continue(last)
}

/// `wait -n` with no positional args (M-37 bare). Snapshot the set of
/// currently-Running job ids at entry, then poll until one of them
/// transitions to `Done(c)` or `Signaled(s)`. Returns 127 immediately
/// if no Running jobs at entry, or if all snapshotted jobs vanish from
/// the table mid-wait. Captures the finished job's pgid into `$pid_var`
/// when provided; on the 127 path sets `$pid_var = ""`.
fn wait_any_pending(pid_var: Option<String>, shell: &mut Shell) -> ExecOutcome {
    let snapshot: Vec<u32> = shell
        .jobs
        .iter()
        .filter(|j| matches!(j.state, crate::jobs::JobState::Running))
        .map(|j| j.id)
        .collect();

    if snapshot.is_empty() {
        if let Some(name) = &pid_var {
            shell.set(name, String::new());
        }
        return ExecOutcome::Continue(127);
    }

    loop {
        let found = shell.jobs.iter().find_map(|j| {
            if !snapshot.contains(&j.id) {
                return None;
            }
            match j.state {
                crate::jobs::JobState::Done(c) => Some((j.pgid, c)),
                crate::jobs::JobState::Signaled(s) => Some((j.pgid, 128 + s)),
                _ => None,
            }
        });
        if let Some((pgid, status)) = found {
            if let Some(name) = &pid_var {
                shell.set(name, pgid.to_string());
            }
            return ExecOutcome::Continue(status);
        }

        let still_present = shell.jobs.iter().any(|j| snapshot.contains(&j.id));
        if !still_present {
            if let Some(name) = &pid_var {
                shell.set(name, String::new());
            }
            return ExecOutcome::Continue(127);
        }

        if let Some(o) = crate::executor::check_interrupt(shell) {
            return o;
        }
        // #183: reap only children we OWN. This used to be `waitpid(-1)`, which
        // reaps ANY child of the process — right for a standalone shell, wrong for
        // huck-engine as a library (it steals the embedder's children) and fatal in
        // the multithreaded test binary, where it drained other tests' children and
        // wedged them. `reap_owned_once` is the single bounded implementation.
        if !crate::jobs::reap_owned_once(shell) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

/// `wait -n` with explicit target list (M-37 with subset). Returns the
/// status of the first listed target to finish. Captures the finished
/// PID into `$pid_var` when provided — for `WaitTarget::Job(id)` that's
/// the job's pgid; for `WaitTarget::Pid(pid)` that's the literal PID.
/// If at entry no target can ever finish (all unknown / not children),
/// returns 127 with `$pid_var = ""`.
fn wait_any_of(
    targets: Vec<WaitTarget>,
    pid_var: Option<String>,
    shell: &mut Shell,
) -> ExecOutcome {
    if let Some((pid, status)) = check_targets_terminal(&targets, shell) {
        if let Some(name) = &pid_var {
            shell.set(name, pid.to_string());
        }
        return ExecOutcome::Continue(status);
    }

    // Probe each target once; collect any pid that was reaped inline here so
    // we can call reap_coproc after the closure (can't hold two &mut borrows).
    // Only record the pid for coproc reaping when it actually exited (not a
    // mere WIFSTOPPED stop, which leaves the coproc alive).
    let mut inlined_reaped_pid: Option<i32> = None;
    let any_active = targets.iter().any(|t| match t {
        WaitTarget::Job(id) => shell.jobs.iter().any(|j| j.id == *id),
        WaitTarget::Pid(pid) => {
            let mut s: libc::c_int = 0;
            let r = unsafe { libc::waitpid(*pid, &mut s, libc::WNOHANG | libc::WUNTRACED) };
            if r > 0 {
                shell.jobs.reap(r, s);
                if !libc::WIFSTOPPED(s) {
                    inlined_reaped_pid = Some(r);
                }
                true
            } else {
                r == 0
            }
        }
    });
    if let Some(r) = inlined_reaped_pid {
        shell.reap_coproc(r);
    }
    if !any_active {
        if let Some(name) = &pid_var {
            shell.set(name, String::new());
        }
        return ExecOutcome::Continue(127);
    }

    if let Some((pid, status)) = check_targets_terminal(&targets, shell) {
        if let Some(name) = &pid_var {
            shell.set(name, pid.to_string());
        }
        return ExecOutcome::Continue(status);
    }

    loop {
        if let Some(o) = crate::executor::check_interrupt(shell) {
            return o;
        }
        // #183: reap only children we OWN. This used to be `waitpid(-1)`, which
        // reaps ANY child of the process — right for a standalone shell, wrong for
        // huck-engine as a library (it steals the embedder's children) and fatal in
        // the multithreaded test binary, where it drained other tests' children and
        // wedged them. `reap_owned_once` is the single bounded implementation.
        if !crate::jobs::reap_owned_once(shell) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        if let Some((pid, st)) = check_targets_terminal(&targets, shell) {
            if let Some(name) = &pid_var {
                shell.set(name, pid.to_string());
            }
            return ExecOutcome::Continue(st);
        }
    }
}

/// Returns `(captured_pid, exit_status)` for the first target that is
/// currently terminal, or `None`.
///
/// For `WaitTarget::Job(id)` the captured pid is the job's `pgid`. For
/// `WaitTarget::Pid(pid)` the captured pid is the literal PID arg.
fn check_targets_terminal(targets: &[WaitTarget], shell: &Shell) -> Option<(i32, i32)> {
    for t in targets {
        match t {
            WaitTarget::Job(id) => {
                if let Some(job) = shell.jobs.iter().find(|j| j.id == *id) {
                    match job.state {
                        crate::jobs::JobState::Done(c) => return Some((job.pgid, c)),
                        crate::jobs::JobState::Signaled(s) => return Some((job.pgid, 128 + s)),
                        _ => {}
                    }
                }
            }
            WaitTarget::Pid(pid) => {
                if let Some(job) = shell.jobs.iter().find(|j| j.pids.contains(pid)) {
                    match job.state {
                        crate::jobs::JobState::Done(c) => return Some((*pid, c)),
                        crate::jobs::JobState::Signaled(s) => return Some((*pid, 128 + s)),
                        _ => {}
                    }
                }
            }
        }
    }
    None
}

fn print_killable_table(out: &mut dyn Write) {
    print_sig_listing(out, crate::traps::killable_signals());
}

/// Prints a signal listing in bash's `kill -l` format: signals sorted by number,
/// `SIG`-prefixed names, 5 columns per row, tab-separated, number right-aligned
/// to width 2. (huck lists the standard signals 1–31; bash additionally appends
/// the real-time tail 34–64, deferred.)
fn print_sig_listing(out: &mut dyn Write, table: &[(&str, i32)]) {
    let mut sigs: Vec<&(&str, i32)> = table.iter().collect();
    sigs.sort_by_key(|(_, n)| *n);
    let last = sigs.len().saturating_sub(1);
    for (i, (name, num)) in sigs.iter().enumerate() {
        let sep = if i % 5 == 4 || i == last { "\n" } else { "\t" };
        let _ = write!(out, "{num:>2}) SIG{name}{sep}");
    }
}

fn handle_kill_l(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &Shell,
) -> ExecOutcome {
    if args.is_empty() {
        print_killable_table(out);
        return ExecOutcome::Continue(0);
    }

    for arg in args {
        if let Ok(n) = arg.parse::<i32>() {
            let lookup = if n >= 128 { n - 128 } else { n };
            match crate::traps::killable_signals()
                .iter()
                .find(|(_, num)| *num == lookup)
            {
                Some((name, _)) => {
                    let _ = writeln!(out, "{name}");
                }
                None => {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "kill: {arg}: invalid signal specification"
                    );
                    return ExecOutcome::Continue(1);
                }
            }
        } else {
            let upper = arg.to_ascii_uppercase();
            let name = upper.strip_prefix("SIG").unwrap_or(&upper);
            match crate::traps::killable_signals()
                .iter()
                .find(|(table_name, _)| *table_name == name)
            {
                Some((_, num)) => {
                    let _ = writeln!(out, "{num}");
                }
                None => {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "kill: {arg}: invalid signal specification"
                    );
                    return ExecOutcome::Continue(1);
                }
            }
        }
    }
    ExecOutcome::Continue(0)
}

fn signal_by_name(s: &str) -> Option<i32> {
    let upper = s.to_ascii_uppercase();
    let name = upper.strip_prefix("SIG").unwrap_or(&upper);
    crate::traps::killable_signals()
        .iter()
        .find_map(|(table_name, num)| {
            if *table_name == name {
                Some(*num)
            } else {
                None
            }
        })
}

/// Parses `arg` as a job spec and resolves it to a job id. On parse or
/// resolution failure, prints a `huck: <builtin>: ...` error to stderr
/// and returns `Err(ExecOutcome::Continue(1))` so the caller can `?` it.
fn resolve_spec_or_error(
    arg: &str,
    builtin: &str,
    err: &mut dyn Write,
    shell: &Shell,
) -> Result<u32, ExecOutcome> {
    let spec = crate::job_spec::parse_job_spec(arg).map_err(|_| {
        crate::sh_error_to!(shell, err, None, "{builtin}: {arg}: bad job spec");
        ExecOutcome::Continue(1)
    })?;
    match shell.jobs.resolve(&spec) {
        Ok(id) => Ok(id),
        Err(crate::jobs::JobSpecResolveError::NotFound) => {
            crate::sh_error_to!(shell, err, None, "{builtin}: {arg}: no such job");
            Err(ExecOutcome::Continue(1))
        }
        Err(crate::jobs::JobSpecResolveError::Ambiguous) => {
            crate::sh_error_to!(shell, err, None, "{builtin}: {arg}: ambiguous job spec");
            Err(ExecOutcome::Continue(1))
        }
    }
}

fn builtin_kill(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    if matches!(args.first().map(|s| s.as_str()), Some("-l")) {
        return handle_kill_l(&args[1..], out, err, shell);
    }
    match args.first().map(|s| s.as_str()) {
        Some("-s") => return kill_with_s_flag(&args[1..], err, shell),
        Some("-n") => return kill_with_n_flag(&args[1..], err, shell),
        _ => {}
    }
    let (sig, targets) = if let Some(first) = args.first() {
        if let Some(rest) = first.strip_prefix('-') {
            // -<sig> form
            let sig = match rest.parse::<i32>() {
                Ok(n) if (0..=64).contains(&n) => n,
                Ok(_) => {
                    crate::sh_error_to!(shell, err, None, "kill: {rest}: invalid signal number");
                    return ExecOutcome::Continue(1);
                }
                Err(_) => match signal_by_name(rest) {
                    Some(n) => n,
                    None => {
                        crate::sh_error_to!(shell, err, None, "kill: {rest}: invalid signal");
                        return ExecOutcome::Continue(1);
                    }
                },
            };
            if args.len() < 2 {
                e!(
                    err,
                    "kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | %job ..."
                );
                return ExecOutcome::Continue(2);
            }
            (sig, &args[1..])
        } else {
            (libc::SIGTERM, args)
        }
    } else {
        e!(
            err,
            "kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | %job ..."
        );
        return ExecOutcome::Continue(2);
    };

    send_signal_to_targets(sig, targets, err, shell)
}

/// Handles `kill -s SIGNAME [targets...]`. The `-s` token has already
/// been consumed by the dispatcher; `args` is everything after it.
fn kill_with_s_flag(args: &[String], err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let name = match args.first() {
        Some(n) => n,
        None => {
            crate::sh_error_to!(shell, err, None, "kill: -s: option requires an argument");
            return ExecOutcome::Continue(2);
        }
    };
    let sig = match signal_by_name(name) {
        Some(n) => n,
        None => {
            crate::sh_error_to!(
                shell,
                err,
                None,
                "kill: {name}: invalid signal specification"
            );
            return ExecOutcome::Continue(1);
        }
    };
    let targets = &args[1..];
    if targets.is_empty() {
        e!(
            err,
            "kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | %job ..."
        );
        return ExecOutcome::Continue(2);
    }
    send_signal_to_targets(sig, targets, err, shell)
}

/// Handles `kill -n SIGNUM [targets...]`. The `-n` token has already
/// been consumed by the dispatcher; `args` is everything after it.
/// Number must be in `killable_signals()` (matching `kill -l`'s set).
fn kill_with_n_flag(args: &[String], err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let num_arg = match args.first() {
        Some(s) => s,
        None => {
            crate::sh_error_to!(shell, err, None, "kill: -n: option requires an argument");
            return ExecOutcome::Continue(2);
        }
    };
    let n = match num_arg.parse::<i32>() {
        Ok(n) if (1..=64).contains(&n) => n,
        _ => {
            crate::sh_error_to!(
                shell,
                err,
                None,
                "kill: {num_arg}: invalid signal specification"
            );
            return ExecOutcome::Continue(1);
        }
    };
    if !crate::traps::killable_signals()
        .iter()
        .any(|(_, num)| *num == n)
    {
        crate::sh_error_to!(
            shell,
            err,
            None,
            "kill: {num_arg}: invalid signal specification"
        );
        return ExecOutcome::Continue(1);
    }
    let targets = &args[1..];
    if targets.is_empty() {
        e!(
            err,
            "kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | %job ..."
        );
        return ExecOutcome::Continue(2);
    }
    send_signal_to_targets(n, targets, err, shell)
}

/// Sends `sig` to each target (`%spec` or PID). Returns `Continue(1)`
/// if any send failed (with errors already on stderr), `Continue(0)`
/// otherwise. Shared between every kill dispatch arm.
fn send_signal_to_targets(
    sig: i32,
    targets: &[String],
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let mut any_failed = false;
    for target in targets {
        if let Some(_rest) = target.strip_prefix('%') {
            let id = match resolve_spec_or_error(target, "kill", err, shell) {
                Ok(id) => id,
                Err(_) => {
                    any_failed = true;
                    continue;
                }
            };
            let (own_pgroup, pgid, pids) = match shell.jobs.iter().find(|j| j.id == id) {
                Some(j) => (j.own_pgroup, j.pgid, j.pids.clone()),
                None => {
                    crate::sh_error_to!(shell, err, None, "kill: {target}: no such job");
                    any_failed = true;
                    continue;
                }
            };
            // A job that owns its group is signalled via the group (catches
            // grandchildren); a group-less job (non-interactive background, v173)
            // is signalled per-pid, matching bash's J_JOBCONTROL-unset path.
            let rc = if own_pgroup {
                unsafe { libc::killpg(pgid, sig) }
            } else {
                let mut r = 0;
                for pid in &pids {
                    if unsafe { libc::kill(*pid, sig) } != 0 {
                        r = -1;
                    }
                }
                r
            };
            if rc != 0 {
                let errno = std::io::Error::last_os_error();
                crate::sh_error_to!(shell, err, None, "kill: ({target}) - {errno}");
                any_failed = true;
            }
        } else {
            match target.parse::<i32>() {
                Ok(pid) if pid > 0 => {
                    let rc = unsafe { libc::kill(pid, sig) };
                    if rc != 0 {
                        let errno = std::io::Error::last_os_error();
                        crate::sh_error_to!(shell, err, None, "kill: ({pid}) - {errno}");
                        any_failed = true;
                    }
                }
                _ => {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "kill: {target}: arguments must be process or job IDs"
                    );
                    any_failed = true;
                }
            }
        }
    }
    if any_failed {
        ExecOutcome::Continue(1)
    } else {
        ExecOutcome::Continue(0)
    }
}

fn builtin_disown(args: &[String], err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let mut all = false;
    let mut running_only = false;
    let mut mark_nohup = false;
    let mut idx = 0;
    while idx < args.len() {
        let a = &args[idx];
        if a == "--" {
            idx += 1;
            break;
        }
        if let Some(rest) = a.strip_prefix('-') {
            if rest.is_empty() {
                break;
            }
            for c in rest.chars() {
                match c {
                    'a' => all = true,
                    'r' => running_only = true,
                    'h' => mark_nohup = true,
                    _ => {
                        crate::sh_error_to!(shell, err, None, "disown: -{c}: invalid option");
                        e!(
                            err,
                            "disown: usage: disown [-h] [-ar] [jobspec ... | pid ...]"
                        );
                        return ExecOutcome::Continue(2);
                    }
                }
            }
            idx += 1;
        } else {
            break;
        }
    }

    let positional = &args[idx..];

    let mut target_ids: Vec<u32> = if all {
        shell.jobs.iter().map(|j| j.id).collect()
    } else if !positional.is_empty() {
        let mut ids = Vec::new();
        for arg in positional {
            if arg.starts_with('%') {
                match resolve_spec_or_error(arg, "disown", err, shell) {
                    Ok(id) => ids.push(id),
                    Err(outcome) => return outcome,
                }
            } else {
                match arg.parse::<i32>() {
                    Ok(pid) if pid > 0 => match shell.jobs.iter().find(|j| j.pids.contains(&pid)) {
                        Some(job) => ids.push(job.id),
                        None => {
                            crate::sh_error_to!(shell, err, None, "disown: {arg}: no such job");
                            return ExecOutcome::Continue(1);
                        }
                    },
                    _ => {
                        crate::sh_error_to!(
                            shell,
                            err,
                            None,
                            "disown: {arg}: not a valid job spec"
                        );
                        return ExecOutcome::Continue(1);
                    }
                }
            }
        }
        ids
    } else if running_only {
        // bash-faithful: `disown -r` alone operates on ALL running jobs.
        shell.jobs.iter().map(|j| j.id).collect()
    } else {
        match shell.jobs.current_id() {
            Some(id) => vec![id],
            None => {
                crate::sh_error_to!(shell, err, None, "disown: no current job");
                return ExecOutcome::Continue(1);
            }
        }
    };

    if running_only {
        target_ids.retain(|id| {
            shell
                .jobs
                .iter()
                .find(|j| j.id == *id)
                .map(|j| matches!(j.state, crate::jobs::JobState::Running))
                .unwrap_or(false)
        });
    }

    if mark_nohup {
        for id in &target_ids {
            shell.jobs.mark_for_nohup(*id);
        }
    } else {
        shell
            .jobs
            .jobs_mut()
            .retain(|j| !target_ids.contains(&j.id));
    }

    ExecOutcome::Continue(0)
}

/// For builtins that accept no options (`fg`/`bg`), return the first invalid
/// option character if the first argument is a `-`-prefixed token other than
/// `-` or `--`. bash's getopt reports the first such character (`fg -sx` →
/// `-s`), so callers format it as `-{c}: invalid option`.
fn leading_invalid_option(args: &[String]) -> Option<char> {
    let first = args.first()?;
    if first == "--" {
        return None;
    }
    let rest = first.strip_prefix('-')?;
    rest.chars().next()
}

/// #162: true if the resolved job has already completed (Done/Signaled) — the
/// entry-reap consumed its terminal status. bash reaps and removes such a job
/// before `fg`/`bg` look it up, so both builtins must treat it as gone rather
/// than acting on a phantom entry with a dead process group.
fn job_already_terminal(shell: &Shell, id: u32) -> bool {
    shell.jobs.iter().find(|j| j.id == id).is_some_and(|j| {
        matches!(
            j.state,
            crate::jobs::JobState::Done(_) | crate::jobs::JobState::Signaled(_)
        )
    })
}

fn builtin_fg(args: &[String], err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    // #158: drain pending STOP/CONT before resolving/acting on the job.
    crate::jobs::reap_completed(shell);
    // #161: fg takes no options; a leading-dash argument (other than `--`) is
    // reported as an invalid option before the usage line, matching bash.
    if let Some(c) = leading_invalid_option(args) {
        crate::sh_error_to!(shell, err, None, "fg: -{c}: invalid option");
        e!(err, "fg: usage: fg [job_spec]");
        return ExecOutcome::Continue(2);
    }
    let id = match args.len() {
        0 => match shell.jobs.current_id() {
            Some(id) => id,
            None => {
                crate::sh_error_to!(shell, err, None, "fg: no current job");
                return ExecOutcome::Continue(1);
            }
        },
        1 if args[0].starts_with('%') => match resolve_spec_or_error(&args[0], "fg", err, shell) {
            Ok(id) => id,
            Err(outcome) => return outcome,
        },
        _ => {
            e!(err, "fg: usage: fg [job_spec]");
            return ExecOutcome::Continue(2);
        }
    };
    // #162: if the entry-reap already completed this job, it is gone as far as
    // fg is concerned — match bash: report "no such job", drop the phantom
    // entry, and return 1 (rather than clobbering it back to Running and racing
    // waitpid(-pgid) into ECHILD, which leaked a Running entry with a dead pgid).
    if job_already_terminal(shell, id) {
        let spec = args.first().map(String::as_str).unwrap_or("current");
        crate::sh_error_to!(shell, err, None, "fg: {spec}: no such job");
        shell.jobs.jobs_mut().retain(|j| j.id != id);
        return ExecOutcome::Continue(1);
    }
    let (pgid, pids, command) = {
        if let Some(job) = shell.jobs.jobs_mut().iter_mut().find(|j| j.id == id) {
            job.state = crate::jobs::JobState::Running;
            job.notified = true;
            (job.pgid, job.pids.clone(), job.command.clone())
        } else {
            crate::sh_error_to!(shell, err, None, "fg: no current job");
            return ExecOutcome::Continue(1);
        }
    };

    e!(err, "{command}");

    // #167: hand the terminal to the job's group only when stdin is a
    // controlling tty. Under `set -m` in a script/pipe there is no tty, but the
    // SIGCONT + waitpid(-pgid) below still resume and wait on the job's group.
    unsafe {
        if libc::isatty(libc::STDIN_FILENO) == 1 {
            libc::tcsetpgrp(libc::STDIN_FILENO, pgid);
        }
        libc::killpg(pgid, libc::SIGCONT);
    }

    let mut last_status = 0;
    let mut stopped_sig: Option<i32> = None;
    let mut completed = 0;
    let total = pids.len();
    loop {
        if completed == total {
            break;
        }
        let mut status: libc::c_int = 0;
        // Wait for any child in this pgrp. -pgid means "any pid whose pgid == pgid".
        let r = unsafe { libc::waitpid(-pgid, &mut status, libc::WUNTRACED) };
        if r < 0 {
            // ECHILD — SIGCHLD reaper got ahead of us. Stop the loop; the
            // job will be cleaned up by the next prompt's notify cycle.
            last_status = 1;
            break;
        }
        if libc::WIFSTOPPED(status) {
            stopped_sig = Some(libc::WSTOPSIG(status));
            break;
        }
        if libc::WIFEXITED(status) {
            last_status = libc::WEXITSTATUS(status);
        } else if libc::WIFSIGNALED(status) {
            last_status = 128 + libc::WTERMSIG(status);
        } else {
            last_status = 1;
        }
        completed += 1;
    }

    unsafe {
        if libc::isatty(libc::STDIN_FILENO) == 1 {
            libc::tcsetpgrp(libc::STDIN_FILENO, shell.shell_pgid);
        }
    }

    if let Some(sig) = stopped_sig {
        if let Some(job) = shell.jobs.jobs_mut().iter_mut().find(|j| j.id == id) {
            job.state = crate::jobs::JobState::Stopped(sig);
            job.notified = true;
        }
        let line = shell
            .jobs
            .iter()
            .find(|j| j.id == id)
            .map(|j| crate::jobs::notification_line(j, '+'))
            .unwrap_or_default();
        e!(err, "\n{line}");
        return ExecOutcome::Continue(128 + sig);
    }

    // Only remove from the job table if all pids completed successfully.
    // If the wait loop exited early (ECHILD race), leave the job for the
    // prompt-time reaper to handle.
    if completed == total {
        shell.jobs.jobs_mut().retain(|j| j.id != id);
    }
    ExecOutcome::Continue(last_status)
}

fn builtin_bg(
    args: &[String],
    _out: &mut dyn std::io::Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    // #158: drain pending STOP/CONT so `bg` finds a newly-stopped job.
    crate::jobs::reap_completed(shell);
    // #161: bg takes no options; a leading-dash argument (other than `--`) is
    // reported as an invalid option before the usage line, matching bash.
    if let Some(c) = leading_invalid_option(args) {
        crate::sh_error_to!(shell, err, None, "bg: -{c}: invalid option");
        e!(err, "bg: usage: bg [job_spec ...]");
        return ExecOutcome::Continue(2);
    }
    let id = match args.len() {
        0 => match shell.jobs.current_stopped_id() {
            Some(id) => id,
            None => {
                crate::sh_error_to!(shell, err, None, "bg: no current job");
                return ExecOutcome::Continue(1);
            }
        },
        1 if args[0].starts_with('%') => {
            let id = match resolve_spec_or_error(&args[0], "bg", err, shell) {
                Ok(id) => id,
                Err(outcome) => return outcome,
            };
            // #162: a job the entry-reap already completed is gone — match bash's
            // "no such job" + drop the phantom entry, before the not-stopped
            // check below would misreport it as "already running".
            if job_already_terminal(shell, id) {
                let spec = &args[0];
                crate::sh_error_to!(shell, err, None, "bg: {spec}: no such job");
                shell.jobs.jobs_mut().retain(|j| j.id != id);
                return ExecOutcome::Continue(1);
            }
            // Verify the resolved job is actually Stopped.
            let is_stopped = shell
                .jobs
                .iter()
                .find(|j| j.id == id)
                .map(|j| matches!(j.state, crate::jobs::JobState::Stopped(_)))
                .unwrap_or(false);
            if !is_stopped {
                crate::sh_error_to!(shell, err, None, "bg: job %{id} already running");
                return ExecOutcome::Continue(1);
            }
            id
        }
        _ => {
            e!(err, "bg: usage: bg [job_spec ...]");
            return ExecOutcome::Continue(2);
        }
    };
    let (pgid, command) = {
        if let Some(job) = shell.jobs.jobs_mut().iter_mut().find(|j| j.id == id) {
            job.state = crate::jobs::JobState::Running;
            job.notified = true;
            (job.pgid, job.command.clone())
        } else {
            crate::sh_error_to!(shell, err, None, "bg: no current job");
            return ExecOutcome::Continue(1);
        }
    };

    unsafe {
        libc::killpg(pgid, libc::SIGCONT);
    }

    e!(err, "[{id}]+ {command} &");
    ExecOutcome::Continue(0)
}

fn builtin_history(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    // Resolve a -d operand (single offset, negative -K, or the two bounds of a
    // range A-B) to an absolute history number. Negative K counts from the end.
    fn resolve_offset(shell: &Shell, s: &str) -> Option<usize> {
        let last = shell.history.last_number()?;
        if let Some(k) = s.strip_prefix('-') {
            let k: usize = k.parse().ok().filter(|&k| k >= 1)?;
            last.checked_sub(k - 1)
        } else {
            s.parse::<usize>().ok()
        }
    }

    let mut idx = 0;
    // Set true only when -c/-d/-w/-r/-a actually ran (NOT for `--` or an
    // unknown option), so the trailing "list all" block can distinguish
    // "no operand, no action" (list all) from "no operand, action already
    // performed" (nothing more to do).
    let mut did_action = false;
    // ---- options ----
    while idx < args.len() {
        let a = &args[idx];
        if a == "--" {
            idx += 1;
            break;
        }
        match a.as_str() {
            "-c" => {
                Rc::make_mut(&mut shell.history).clear();
                did_action = true;
                idx += 1;
            }
            "-d" => {
                let Some(operand) = args.get(idx + 1) else {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "history: -d: option requires an argument"
                    );
                    return ExecOutcome::Continue(1);
                };
                // Range iff a '-' appears AFTER the first char (so a leading
                // negative sign on a single offset isn't mistaken for a
                // range). `operand.get(1..)` (rather than `operand[1..]`)
                // avoids panicking when `operand` is empty or the byte at
                // index 1 isn't a char boundary; an empty operand simply
                // falls through to the single-offset path below, where
                // `resolve_offset("")` fails and yields the standard
                // out-of-range error.
                let split = operand.get(1..).and_then(|s| s.find('-')).map(|i| i + 1);
                let range = match split {
                    Some(i) => Some((&operand[..i], &operand[i + 1..])),
                    None => None,
                };
                if let Some((sa, sb)) = range {
                    match (resolve_offset(shell, sa), resolve_offset(shell, sb)) {
                        (Some(a), Some(b)) => {
                            Rc::make_mut(&mut shell.history).delete_range(a, b);
                            did_action = true;
                        }
                        _ => {
                            crate::sh_error_to!(
                                shell,
                                err,
                                None,
                                "history: {operand}: history position out of range"
                            );
                            return ExecOutcome::Continue(1);
                        }
                    }
                } else {
                    match resolve_offset(shell, operand) {
                        Some(n) if Rc::make_mut(&mut shell.history).delete(n) => {
                            did_action = true;
                        }
                        _ => {
                            crate::sh_error_to!(
                                shell,
                                err,
                                None,
                                "history: {operand}: history position out of range"
                            );
                            return ExecOutcome::Continue(1);
                        }
                    }
                }
                idx += 2;
            }
            "-w" | "-r" | "-a" => {
                let flag = a.clone();
                // Optional filename operand; else the default histfile.
                let file: std::path::PathBuf = match args.get(idx + 1) {
                    Some(f) if !f.starts_with('-') => {
                        idx += 1;
                        std::path::PathBuf::from(f)
                    }
                    _ => match shell.history.file_path() {
                        Some(p) => p.to_path_buf(),
                        None => {
                            crate::sh_error_to!(
                                shell,
                                err,
                                None,
                                "history: cannot use the history file"
                            );
                            return ExecOutcome::Continue(1);
                        }
                    },
                };
                let h = Rc::make_mut(&mut shell.history);
                let res = match flag.as_str() {
                    "-w" => h.write_all_to(&file),
                    "-a" => h.append_new_to(&file),
                    _ => h.read_append_from(&file),
                };
                if let Err(e) = res {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "history: {}: {}",
                        file.display(),
                        crate::bash_io_error(&e)
                    );
                    return ExecOutcome::Continue(1);
                }
                did_action = true;
                idx += 1;
            }
            "-p" | "-s" | "-n" => {
                crate::sh_error_to!(shell, err, None, "history: {a}: not yet implemented");
                return ExecOutcome::Continue(1);
            }
            other if other.starts_with('-') && other.len() > 1 => {
                crate::sh_error_to!(shell, err, None, "history: {other}: invalid option");
                e!(
                    err,
                    "history: usage: history [-c] [-d offset] [n] or history -anrw [filename] or history -ps arg [arg...]"
                );
                shell.builtin_usage_error = Some(2);
                return ExecOutcome::Continue(2);
            }
            _ => break, // a non-option operand (the N count)
        }
    }

    // ---- trailing operand: the listing count N (only when no option consumed it) ----
    let rest = &args[idx..];
    if did_action {
        // Bash only validates/uses trailing operands on the pure "list"
        // path. Once an action (-c/-d/-w/-r/-a) has actually run, any
        // leftover operands (numeric or not, one or many) are silently
        // discarded — confirmed against bash 5.2: `history -d 2 3 4` and
        // `history -c 3` neither error nor print a listing.
        return ExecOutcome::Continue(0);
    }
    match rest.first().map(|s| s.as_str()) {
        None => {
            // No numeric operand and no action ran: list all (this also
            // covers a bare `--` with nothing after it).
            for (number, command) in shell.history.entries() {
                if writeln!(out, "{number:>5}  {command}").is_err() {
                    return ExecOutcome::Continue(1);
                }
            }
            ExecOutcome::Continue(0)
        }
        // Bash validates the FIRST operand numerically BEFORE counting operands:
        // `history abc def` → "abc: numeric argument required", not "too many".
        Some(n_str) => match n_str.parse::<usize>() {
            Err(_) => {
                crate::sh_error_to!(
                    shell,
                    err,
                    None,
                    "history: {n_str}: numeric argument required"
                );
                ExecOutcome::Continue(1)
            }
            Ok(_) if rest.len() > 1 => {
                crate::sh_error_to!(shell, err, None, "history: too many arguments");
                ExecOutcome::Continue(1)
            }
            Ok(n) => {
                for (number, command) in shell.history.tail(n) {
                    if writeln!(out, "{number:>5}  {command}").is_err() {
                        return ExecOutcome::Continue(1);
                    }
                }
                ExecOutcome::Continue(0)
            }
        },
    }
}

fn builtin_trap(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    use crate::traps::{TrapSignal, install, parse_trap_signal, reset};

    // No args: same as `trap -p`.
    if args.is_empty() {
        print_active_traps(out, shell, None);
        return ExecOutcome::Continue(0);
    }

    // -l: list signal name/number pairs.
    if args[0] == "-l" {
        if args.len() != 1 {
            crate::sh_error_to!(shell, err, None, "trap: -l takes no arguments");
            return ExecOutcome::Continue(1);
        }
        print_signal_table(out);
        return ExecOutcome::Continue(0);
    }

    // -p [SIGNAL...]: list active traps (optionally filtered).
    if args[0] == "-p" {
        if args.len() == 1 {
            print_active_traps(out, shell, None);
            return ExecOutcome::Continue(0);
        }
        let mut filter: Vec<TrapSignal> = Vec::new();
        for name in &args[1..] {
            match parse_trap_signal(name) {
                Ok(sig) => filter.push(sig),
                Err(msg) => {
                    crate::sh_error_to!(shell, err, None, "trap: {msg}");
                    return ExecOutcome::Continue(1);
                }
            }
        }
        print_active_traps(out, shell, Some(&filter));
        return ExecOutcome::Continue(0);
    }

    // `trap - SIGNAL...`: reset each signal.
    if args[0] == "-" {
        if args.len() < 2 {
            e!(err, "trap: usage: trap [-lp] [[arg] signal_spec ...]");
            return ExecOutcome::Continue(1);
        }
        for name in &args[1..] {
            let sig = match parse_trap_signal(name) {
                Ok(s) => s,
                Err(msg) => {
                    crate::sh_error_to!(shell, err, None, "trap: {msg}");
                    return ExecOutcome::Continue(1);
                }
            };
            if let Err(msg) = reset(shell, sig) {
                crate::sh_error_to!(shell, err, None, "trap: {msg}");
                return ExecOutcome::Continue(1);
            }
        }
        return ExecOutcome::Continue(0);
    }

    // `trap ACTION SIGNAL...`: install action for each signal.
    if args.len() < 2 {
        e!(err, "trap: usage: trap [-lp] [[arg] signal_spec ...]");
        return ExecOutcome::Continue(1);
    }
    let action_text = args[0].clone();
    let action = if action_text.is_empty() {
        None // empty string → ignore
    } else {
        Some(action_text)
    };
    for name in &args[1..] {
        let sig = match parse_trap_signal(name) {
            Ok(s) => s,
            Err(msg) => {
                crate::sh_error_to!(shell, err, None, "trap: {msg}");
                return ExecOutcome::Continue(1);
            }
        };
        if let Err(msg) = install(shell, sig, action.clone()) {
            crate::sh_error_to!(shell, err, None, "trap: {msg}");
            return ExecOutcome::Continue(1);
        }
    }
    ExecOutcome::Continue(0)
}

/// Prints active traps in re-readable form. If `filter` is `Some`, only
/// the listed signals are printed; if `None`, all active traps print.
/// Bash sorts by signal number, with EXIT printed first.
fn print_active_traps(
    out: &mut dyn Write,
    shell: &Shell,
    filter: Option<&[crate::traps::TrapSignal]>,
) {
    use crate::traps::TrapSignal;

    // Collect entries in (sort-key, signal, action) form. Pseudo-signals
    // (EXIT=0, ERR=1, DEBUG=2, RETURN=3) sort first; real signals (100+n)
    // sort after pseudo-signals.
    let mut entries: Vec<(i32, TrapSignal, &Option<String>)> = Vec::new();
    for (sig, action) in &shell.traps {
        if let Some(f) = filter
            && !f.contains(sig)
        {
            continue;
        }
        let key = match sig {
            TrapSignal::Exit => 0,
            TrapSignal::Err => 1,
            TrapSignal::Debug => 2,
            TrapSignal::Return => 3,
            TrapSignal::Real(n) => 100 + *n,
        };
        entries.push((key, *sig, action));
    }
    entries.sort_by_key(|(k, _, _)| *k);

    for (_, sig, action) in entries {
        let name = match sig {
            TrapSignal::Exit => "EXIT".to_string(),
            TrapSignal::Err => "ERR".to_string(),
            TrapSignal::Debug => "DEBUG".to_string(),
            TrapSignal::Return => "RETURN".to_string(),
            TrapSignal::Real(n) => signal_number_to_name(n).unwrap_or_else(|| n.to_string()),
        };
        let text = action.as_deref().unwrap_or("");
        // Escape single quotes in action text via the standard bash
        // shell-quote idiom: ' → '\''
        let escaped = text.replace('\'', "'\\''");
        let _ = writeln!(out, "trap -- '{escaped}' {name}");
    }
}

/// Prints the trappable signal table in bash's 4-column format:
///   1) HUP   2) INT   3) QUIT  10) USR1
fn print_signal_table(out: &mut dyn Write) {
    print_sig_listing(out, crate::traps::name_table());
}

/// Returns the canonical name (no SIG prefix) for `signum`, or None
/// if `signum` is not in the trappable table.
fn signal_number_to_name(signum: i32) -> Option<String> {
    // Full table (incl. KILL/STOP) so a stored KILL/STOP trap disposition
    // renders by name in `trap -p`, matching bash.
    crate::traps::killable_signals()
        .iter()
        .find_map(|(name, n)| {
            if *n == signum {
                Some(name.to_string())
            } else {
                None
            }
        })
}

/// One step of `getopts` parsing — pure, no shell access (unit-testable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GetoptsStep {
    /// Value to assign to the `name` variable ("a", "?", or ":").
    pub name: String,
    /// `Some(v)` → set OPTARG to v; `None` → unset OPTARG.
    pub optarg: Option<String>,
    /// New OPTIND to write back.
    pub optind: usize,
    /// New within-word cursor to cache.
    pub sp: usize,
    /// Verbose-mode error message BODY (no "huck: " prefix); printed by the
    /// caller only when set AND OPTERR != "0". `None` in silent mode / success.
    pub error: Option<String>,
    /// true → options exhausted / non-option / `--` (caller returns rc 1);
    /// false → an option (possibly invalid) was processed (rc 0).
    pub done: bool,
}

/// Compute one `getopts` step. `optind` is 1-based into `args`; `sp` is the
/// 1-based char offset within the current word (1 = fresh word). Silent mode
/// is derived from a leading ':' in `optstring`. See the v111 spec.
pub(crate) fn getopts_step(
    optstring: &str,
    args: &[String],
    optind: usize,
    sp: usize,
) -> GetoptsStep {
    let silent = optstring.starts_with(':');
    let done = |optind: usize| GetoptsStep {
        name: "?".to_string(),
        optarg: None,
        optind,
        sp: 1,
        error: None,
        done: true,
    };

    // Options exhausted.
    if optind == 0 || optind > args.len() {
        return done(optind.max(1));
    }
    let word: Vec<char> = args[optind - 1].chars().collect();
    let mut sp = if sp == 0 { 1 } else { sp };

    // Defensive: a stale within-word cursor (e.g. inherited across a function
    // call, or an externally manipulated OPTIND) that points past the current
    // word must not index out of bounds — restart this word fresh.
    if sp > word.len() {
        sp = 1;
    }

    if sp == 1 {
        // Fresh word: must start with '-' and not be just "-".
        if word.first() != Some(&'-') || word.len() == 1 {
            return done(optind); // non-option, OPTIND unchanged
        }
        if word.len() == 2 && word[1] == '-' {
            return done(optind + 1); // "--" → end of options, advance past it
        }
        sp = 2; // skip the leading '-'
    }

    let c = word[sp - 1];
    let mut sp = sp + 1;
    let word_done = sp > word.len();

    // Look up `c` in optstring. A leading ':' (silent flag) is NOT a valid
    // option letter; ':' can never itself be an option char.
    let takes_arg = optstring_takes_arg(optstring, c);
    let known = c != ':' && optstring_has(optstring, c);

    if !known {
        // Invalid option.
        let mut next_optind = optind;
        if word_done {
            next_optind += 1;
            sp = 1;
        }
        return GetoptsStep {
            name: "?".to_string(),
            optarg: if silent { Some(c.to_string()) } else { None },
            optind: next_optind,
            sp,
            error: if silent {
                None
            } else {
                Some(format!("illegal option -- {c}"))
            },
            done: false,
        };
    }

    if takes_arg {
        if !word_done {
            // Attached arg: rest of the word.
            let arg: String = word[(sp - 1)..].iter().collect();
            return GetoptsStep {
                name: c.to_string(),
                optarg: Some(arg),
                optind: optind + 1,
                sp: 1,
                error: None,
                done: false,
            };
        }
        if optind < args.len() {
            // Separate arg: the next word.
            return GetoptsStep {
                name: c.to_string(),
                optarg: Some(args[optind].clone()),
                optind: optind + 2,
                sp: 1,
                error: None,
                done: false,
            };
        }
        // Missing argument.
        return GetoptsStep {
            name: if silent {
                ":".to_string()
            } else {
                "?".to_string()
            },
            optarg: if silent { Some(c.to_string()) } else { None },
            optind: optind + 1,
            sp: 1,
            error: if silent {
                None
            } else {
                Some(format!("option requires an argument -- {c}"))
            },
            done: false,
        };
    }

    // Plain valid option, no argument.
    let mut next_optind = optind;
    if word_done {
        next_optind += 1;
        sp = 1;
    }
    GetoptsStep {
        name: c.to_string(),
        optarg: None,
        optind: next_optind,
        sp,
        error: None,
        done: false,
    }
}

/// True if `c` appears as an option letter in `optstring` (ignoring a leading
/// ':' silent flag and the ':' arg-markers that follow letters).
fn optstring_has(optstring: &str, c: char) -> bool {
    let mut chars = optstring.chars().peekable();
    if chars.peek() == Some(&':') {
        chars.next();
    }
    for o in chars {
        if o == ':' {
            continue;
        } // arg-marker for the previous letter
        if o == c {
            return true;
        }
    }
    false
}

/// True if option letter `c` is immediately followed by ':' in `optstring`
/// (i.e. it takes an argument).
fn optstring_takes_arg(optstring: &str, c: char) -> bool {
    let mut chars = optstring.chars().peekable();
    if chars.peek() == Some(&':') {
        chars.next();
    }
    while let Some(o) = chars.next() {
        if o == ':' {
            continue;
        }
        if o == c {
            return chars.peek() == Some(&':');
        }
    }
    false
}

/// `getopts optstring name [arg ...]` — POSIX option parser (M-106). Reads/
/// writes OPTIND/OPTARG/OPTERR + the matched-letter `name`, holding the
/// within-word cursor in Shell. Delegates the state machine to `getopts_step`.
fn builtin_getopts(args: &[String], err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    const USAGE: &str = "getopts: usage: getopts optstring name [arg ...]";

    // getopts accepts no options of its own. A leading operand starting with
    // '-' (other than "-" or "--") is an invalid option, reported with the
    // builtin-error prologue plus a usage line (bash: internal_getopt("")).
    // A leading "--" is consumed as the option terminator.
    let mut args = args;
    if let Some(first) = args.first() {
        if first.starts_with('-') && first != "-" && first != "--" {
            let c = first.chars().nth(1).unwrap();
            crate::sh_error_to!(shell, err, Some("getopts"), "-{c}: invalid option");
            e!(err, "{USAGE}");
            return ExecOutcome::Continue(2);
        }
        if first == "--" {
            args = &args[1..];
        }
    }

    if args.len() < 2 {
        e!(err, "{USAGE}");
        return ExecOutcome::Continue(2);
    }
    let optstring = args[0].clone();
    let name = args[1].clone();

    // Parse explicit args if given, else the current positional parameters.
    let parse_args: Vec<String> = if args.len() > 2 {
        args[2..].to_vec()
    } else {
        shell.positional_args.clone()
    };
    // Read OPTIND (default 1; clamp <1 to 1).
    let optind = shell
        .lookup_var("OPTIND")
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(1);
    // Detect an external OPTIND reset → fresh within-word cursor.
    let sp = if optind != shell.getopts_optind_cache {
        1
    } else {
        shell.getopts_sp
    };

    let step = getopts_step(&optstring, &parse_args, optind, sp);

    // Bind OPTIND + cursor cache UNCONDITIONALLY, before the name/OPTARG
    // checks — bash's dogetopts binds OPTIND from the post-parse value
    // regardless of whether the name is a valid identifier, so an invalid
    // name (or readonly OPTARG) still advances OPTIND.
    shell.set("OPTIND", step.optind.to_string());
    shell.getopts_optind_cache = step.optind;
    shell.getopts_sp = step.sp;

    // OPTARG is bound before the name check (bash binds OPTARG in dogetopts
    // before getopts_bind_variable runs the identifier check). A readonly
    // OPTARG prints the prologue-prefixed readonly error (Task 1).
    match step.optarg {
        Some(v) => {
            let _ = shell.try_set("OPTARG", v);
        }
        None => shell.unset("OPTARG"),
    }

    // Validate the name AFTER OPTIND/OPTARG are bound. Invalid identifier is a
    // hard error (bash EXECUTION_FAILURE = 1) with the full builtin prologue.
    // This returns before the $0-prefixed option-diagnostic block below, so an
    // invalid optstring option AND an invalid name var together print only the
    // identifier error (bash prints both — an untested edge, accepted by spec).
    if !is_valid_name(&name) {
        crate::sh_error_to!(
            shell,
            err,
            Some("getopts"),
            "`{name}': not a valid identifier"
        );
        return ExecOutcome::Continue(1);
    }

    // Assign the matched letter (or '?' / ':').
    let _ = shell.try_set(&name, step.name.clone());

    // Verbose getopts-internal option diagnostic (suppressed by OPTERR=0),
    // prefixed with $0 (bash sets argv[0] = dollar_vars[0] for sh_getopt).
    if let Some(body) = step.error
        && shell.lookup_var("OPTERR").as_deref() != Some("0")
    {
        e!(err, "{}: {body}", shell.shell_argv0);
    }
    ExecOutcome::Continue(if step.done { 1 } else { 0 })
}

fn builtin_shift(args: &[String], err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    // bash parses the count as a signed integer: a negative count is a
    // "shift count out of range" error (naming the value), a non-numeric
    // argument is "numeric argument required".
    let n: i64 = match args.first() {
        None => 1,
        // bash parses via strtol, which skips surrounding whitespace; trim to
        // match (`shift " 2 "` is valid). Overflow still errors like bash.
        Some(s) => match s.trim().parse::<i64>() {
            Ok(n) => n,
            Err(_) => {
                crate::sh_error_to!(shell, err, None, "shift: {s}: numeric argument required");
                return ExecOutcome::Continue(1);
            }
        },
    };
    if n < 0 {
        crate::sh_error_to!(shell, err, None, "shift: {n}: shift count out of range");
        return ExecOutcome::Continue(1);
    }
    // A count larger than $# is a SILENT failure in bash (rc 1, no message);
    // only a negative count is a reported error.
    let n = n as usize;
    if n > shell.positional_args.len() {
        return ExecOutcome::Continue(1);
    }
    shell.positional_args.drain(0..n);
    ExecOutcome::Continue(0)
}

struct OptionInfo {
    name: &'static str,
    default: bool,
}

/// Names of the `set -o` options, in table order. Used by `compgen -A setopt`.
pub fn seto_option_names() -> impl Iterator<Item = &'static str> {
    SETO_TABLE.iter().map(|o| o.name)
}

/// Names of all `help` topics (builtins + keywords). Used by `compgen -A helptopic`.
pub fn help_topic_names() -> impl Iterator<Item = &'static str> {
    HELP_ENTRIES.iter().map(|e| e.name)
}

/// `SIG`-prefixed names of the real signals huck knows (excludes the trap
/// pseudo-signals EXIT/ERR/DEBUG/RETURN). Used by `compgen -A signal`.
pub fn signal_names() -> Vec<String> {
    crate::traps::name_table()
        .iter()
        .filter(|(n, _)| !matches!(*n, "EXIT" | "ERR" | "DEBUG" | "RETURN"))
        .map(|(n, _)| format!("SIG{n}"))
        .collect()
}

/// bash 5.2's full `set -o` option table, in bash's display order. Every name
/// is backed by real state in `Shell.shell_options` and is settable (v270);
/// only some options carry deeper behavior (see the `ShellOptions` doc). The
/// `default` here mirrors each field's non-interactive default and is only a
/// fallback for `option_get`.
const SETO_TABLE: &[OptionInfo] = &[
    OptionInfo {
        name: "allexport",
        default: false,
    },
    OptionInfo {
        name: "braceexpand",
        default: true,
    },
    OptionInfo {
        name: "emacs",
        default: false,
    },
    OptionInfo {
        name: "errexit",
        default: false,
    },
    OptionInfo {
        name: "errtrace",
        default: false,
    },
    OptionInfo {
        name: "functrace",
        default: false,
    },
    OptionInfo {
        name: "hashall",
        default: true,
    },
    OptionInfo {
        name: "histexpand",
        default: false,
    },
    OptionInfo {
        name: "history",
        default: false,
    },
    OptionInfo {
        name: "ignoreeof",
        default: false,
    },
    OptionInfo {
        name: "interactive-comments",
        default: true,
    },
    OptionInfo {
        name: "keyword",
        default: false,
    },
    OptionInfo {
        name: "monitor",
        default: false,
    },
    OptionInfo {
        name: "noclobber",
        default: false,
    },
    OptionInfo {
        name: "noexec",
        default: false,
    },
    OptionInfo {
        name: "noglob",
        default: false,
    },
    OptionInfo {
        name: "nolog",
        default: false,
    },
    OptionInfo {
        name: "notify",
        default: false,
    },
    OptionInfo {
        name: "nounset",
        default: false,
    },
    OptionInfo {
        name: "onecmd",
        default: false,
    },
    OptionInfo {
        name: "physical",
        default: false,
    },
    OptionInfo {
        name: "pipefail",
        default: false,
    },
    OptionInfo {
        name: "posix",
        default: false,
    },
    OptionInfo {
        name: "privileged",
        default: false,
    },
    OptionInfo {
        name: "verbose",
        default: false,
    },
    OptionInfo {
        name: "vi",
        default: false,
    },
    OptionInfo {
        name: "xtrace",
        default: false,
    },
];

/// Error from `option_set` for an unrecognized `set -o` name.
/// `Debug` is required because an existing test calls `option_set(...).unwrap()`.
#[derive(Debug)]
enum OptSetErr {
    /// Not a recognized `set -o` option name at all.
    Unknown,
}

/// Reads a `set -o` option: real state for the 3 implemented, the table
/// default for any other recognized name, `None` for an unknown name.
pub(crate) fn option_get(shell: &Shell, name: &str) -> Option<bool> {
    match name {
        "errexit" => Some(shell.shell_options.errexit),
        "nounset" => Some(shell.shell_options.nounset),
        "pipefail" => Some(shell.shell_options.pipefail),
        "verbose" => Some(shell.shell_options.verbose),
        "xtrace" => Some(shell.shell_options.xtrace),
        "noglob" => Some(shell.shell_options.noglob),
        "noclobber" => Some(shell.shell_options.noclobber),
        "noexec" => Some(shell.shell_options.noexec),
        "physical" => Some(shell.shell_options.physical),
        "posix" => Some(shell.shell_options.posix),
        "allexport" => Some(shell.shell_options.allexport),
        "braceexpand" => Some(shell.shell_options.braceexpand),
        "hashall" => Some(shell.shell_options.hashall),
        "histexpand" => Some(shell.shell_options.histexpand),
        "history" => Some(shell.shell_options.history),
        "ignoreeof" => Some(shell.shell_options.ignoreeof),
        "interactive-comments" => Some(shell.shell_options.interactive_comments),
        "keyword" => Some(shell.shell_options.keyword),
        "monitor" => Some(shell.shell_options.monitor),
        "notify" => Some(shell.shell_options.notify),
        "onecmd" => Some(shell.shell_options.onecmd),
        "functrace" => Some(shell.shell_options.functrace),
        "errtrace" => Some(shell.shell_options.errtrace),
        "emacs" => Some(shell.shell_options.emacs),
        "vi" => Some(shell.shell_options.vi),
        "nolog" => Some(shell.shell_options.nolog),
        "privileged" => Some(shell.shell_options.privileged),
        _ => None,
    }
}

/// Writes a `set -o` option. Every valid bash 5.2 option name is settable;
/// only `braceexpand`/`allexport` (and the pre-existing behavioral options)
/// carry semantics — the rest are faithful accept-and-store toggles (see the
/// `ShellOptions` doc-comment). An unrecognized name yields `OptSetErr::Unknown`.
fn option_set(shell: &mut Shell, name: &str, value: bool) -> Result<(), OptSetErr> {
    match name {
        "errexit" => shell.shell_options.errexit = value,
        "nounset" => shell.shell_options.nounset = value,
        "pipefail" => shell.shell_options.pipefail = value,
        "verbose" => shell.shell_options.verbose = value,
        "xtrace" => shell.shell_options.xtrace = value,
        "noglob" => shell.shell_options.noglob = value,
        "noclobber" => shell.shell_options.noclobber = value,
        "noexec" => shell.shell_options.noexec = value,
        "physical" => shell.shell_options.physical = value,
        "posix" => shell.shell_options.posix = value,
        "allexport" => shell.shell_options.allexport = value,
        "braceexpand" => shell.shell_options.braceexpand = value,
        "hashall" => shell.shell_options.hashall = value,
        "histexpand" => shell.shell_options.histexpand = value,
        "history" => shell.shell_options.history = value,
        "ignoreeof" => shell.shell_options.ignoreeof = value,
        "interactive-comments" => shell.shell_options.interactive_comments = value,
        "keyword" => shell.shell_options.keyword = value,
        "monitor" => shell.shell_options.monitor = value,
        "notify" => shell.shell_options.notify = value,
        "onecmd" => shell.shell_options.onecmd = value,
        "functrace" => shell.shell_options.functrace = value,
        "errtrace" => shell.shell_options.errtrace = value,
        "emacs" => shell.shell_options.emacs = value,
        "vi" => shell.shell_options.vi = value,
        "nolog" => shell.shell_options.nolog = value,
        "privileged" => shell.shell_options.privileged = value,
        _ => return Err(OptSetErr::Unknown),
    }
    Ok(())
}

/// Public entry for applying a command-line `-o <name>` / `+o <name>` option
/// (#159). Wraps the private `option_set` table so the CLI layer (huck-cli)
/// doesn't duplicate the option list. `Err(())` means the name is not a
/// recognized `set -o` option (the caller renders `<name>: invalid option name`).
pub fn set_o_option_by_name(shell: &mut Shell, name: &str, enable: bool) -> Result<(), ()> {
    match option_set(shell, name, enable) {
        Ok(()) => Ok(()),
        Err(OptSetErr::Unknown) => Err(()),
    }
}

fn print_options_table(out: &mut dyn Write, shell: &Shell) -> ExecOutcome {
    for opt in SETO_TABLE {
        let val = option_get(shell, opt.name).unwrap_or(opt.default);
        let _ = writeln!(out, "{:<15}\t{}", opt.name, if val { "on" } else { "off" });
    }
    ExecOutcome::Continue(0)
}

fn print_options_reinput(out: &mut dyn Write, shell: &Shell) -> ExecOutcome {
    for opt in SETO_TABLE {
        let val = option_get(shell, opt.name).unwrap_or(opt.default);
        let sign = if val { '-' } else { '+' };
        let _ = writeln!(out, "set {sign}o {}", opt.name);
    }
    ExecOutcome::Continue(0)
}

fn builtin_set(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    // POSIX case #1: a `set` option error exits a non-interactive posix shell
    // ONLY when an `-o`/`+o` option NAME is genuinely invalid
    // (`OptSetErr::Unknown`). Unimplemented-but-valid-in-bash options
    // (`set -o emacs`) and unknown single-char flags (`set -h`) are accepted
    // by bash and must NOT exit, so `builtin_set_inner` flags only the four
    // `OptSetErr::Unknown` arms via `shell.builtin_usage_error`.
    builtin_set_inner(args, out, err, shell)
}

fn builtin_set_inner(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    if crate::restricted::is_restricted(shell)
        && args.iter().any(|a| a == "+r")
        && let Err(msg) = crate::restricted::check_set_plus_r()
    {
        crate::sh_error_to!(shell, err, None, "{msg}");
        return ExecOutcome::Continue(1);
    }
    if args.is_empty() {
        let mut names: Vec<String> = shell.var_names().map(|s| s.to_string()).collect();
        names.sort();
        for name in &names {
            if let Some(v) = shell.lookup_var(name) {
                let _ = writeln!(out, "{}={}", name, set_escape_value(&v));
            }
        }
        return ExecOutcome::Continue(0);
    }

    // Parse leading flags. After flags (or `--`), remaining args replace
    // positional parameters. Reaching the end of args without seeing a non-
    // flag arg means flag-only invocation — positional args UNCHANGED.
    let mut i = 0;
    let mut saw_terminator = false;
    let mut saw_non_flag = false;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            saw_terminator = true;
            i += 1;
            break;
        }
        if arg == "-o" {
            i += 1;
            if i >= args.len() {
                return print_options_table(out, shell);
            }
            match option_set(shell, &args[i], true) {
                Ok(()) => {}
                Err(OptSetErr::Unknown) => {
                    crate::sh_error_to!(shell, err, None, "set: {}: invalid option name", args[i]);
                    shell.builtin_usage_error = Some(2);
                    return ExecOutcome::Continue(2);
                }
            }
            i += 1;
            continue;
        }
        if arg == "+o" {
            i += 1;
            if i >= args.len() {
                return print_options_reinput(out, shell);
            }
            match option_set(shell, &args[i], false) {
                Ok(()) => {}
                Err(OptSetErr::Unknown) => {
                    crate::sh_error_to!(shell, err, None, "set: {}: invalid option name", args[i]);
                    shell.builtin_usage_error = Some(2);
                    return ExecOutcome::Continue(2);
                }
            }
            i += 1;
            continue;
        }
        if arg.starts_with('-') && arg.len() >= 2 {
            // Short-flag cluster like `-e`, `-u`, `-eu`, or `-eo NAME`
            // where `o` inside the cluster consumes the NEXT arg as
            // the long-form option name (matches bash).
            for &c in &arg.as_bytes()[1..] {
                match c {
                    b'C' => shell.shell_options.noclobber = true,
                    b'e' => shell.shell_options.errexit = true,
                    b'f' => shell.shell_options.noglob = true,
                    b'u' => shell.shell_options.nounset = true,
                    b'v' => shell.shell_options.verbose = true,
                    b'x' => shell.shell_options.xtrace = true,
                    b'n' => shell.shell_options.noexec = true,
                    // bash 5.2 single-char aliases for long-form options.
                    b'a' => shell.shell_options.allexport = true,
                    b'b' => shell.shell_options.notify = true,
                    b'h' => shell.shell_options.hashall = true,
                    b'k' => shell.shell_options.keyword = true,
                    b'm' => shell.shell_options.monitor = true,
                    b't' => shell.shell_options.onecmd = true,
                    b'B' => shell.shell_options.braceexpand = true,
                    b'E' => shell.shell_options.errtrace = true,
                    b'H' => shell.shell_options.histexpand = true,
                    b'P' => shell.shell_options.physical = true,
                    b'T' => shell.shell_options.functrace = true,
                    b'p' => shell.shell_options.privileged = true,
                    b'o' => {
                        i += 1;
                        if i >= args.len() {
                            return print_options_table(out, shell);
                        }
                        match option_set(shell, &args[i], true) {
                            Ok(()) => {}
                            Err(OptSetErr::Unknown) => {
                                crate::sh_error_to!(
                                    shell,
                                    err,
                                    None,
                                    "set: {}: invalid option name",
                                    args[i]
                                );
                                shell.builtin_usage_error = Some(2);
                                return ExecOutcome::Continue(2);
                            }
                        }
                    }
                    other => {
                        crate::sh_error_to!(
                            shell,
                            err,
                            None,
                            "set: -{}: not yet supported in this version",
                            other as char
                        );
                        return ExecOutcome::Continue(2);
                    }
                }
            }
            i += 1;
            continue;
        }
        if arg.starts_with('+') && arg.len() >= 2 {
            for &c in &arg.as_bytes()[1..] {
                match c {
                    b'C' => shell.shell_options.noclobber = false,
                    b'e' => shell.shell_options.errexit = false,
                    b'f' => shell.shell_options.noglob = false,
                    b'u' => shell.shell_options.nounset = false,
                    b'v' => shell.shell_options.verbose = false,
                    b'x' => shell.shell_options.xtrace = false,
                    b'n' => shell.shell_options.noexec = false,
                    b'a' => shell.shell_options.allexport = false,
                    b'b' => shell.shell_options.notify = false,
                    b'h' => shell.shell_options.hashall = false,
                    b'k' => shell.shell_options.keyword = false,
                    b'm' => shell.shell_options.monitor = false,
                    b't' => shell.shell_options.onecmd = false,
                    b'B' => shell.shell_options.braceexpand = false,
                    b'E' => shell.shell_options.errtrace = false,
                    b'H' => shell.shell_options.histexpand = false,
                    b'P' => shell.shell_options.physical = false,
                    b'T' => shell.shell_options.functrace = false,
                    b'p' => shell.shell_options.privileged = false,
                    b'o' => {
                        i += 1;
                        if i >= args.len() {
                            return print_options_reinput(out, shell);
                        }
                        match option_set(shell, &args[i], false) {
                            Ok(()) => {}
                            Err(OptSetErr::Unknown) => {
                                crate::sh_error_to!(
                                    shell,
                                    err,
                                    None,
                                    "set: {}: invalid option name",
                                    args[i]
                                );
                                shell.builtin_usage_error = Some(2);
                                return ExecOutcome::Continue(2);
                            }
                        }
                    }
                    other => {
                        crate::sh_error_to!(
                            shell,
                            err,
                            None,
                            "set: +{}: not yet supported in this version",
                            other as char
                        );
                        return ExecOutcome::Continue(2);
                    }
                }
            }
            i += 1;
            continue;
        }
        // Non-flag arg — break out to positional-replacement.
        saw_non_flag = true;
        break;
    }

    // Positional-args replacement: triggered by an explicit `--` terminator
    // or by encountering a non-flag arg. Pure flag-only invocations leave
    // positional args alone.
    if saw_terminator || saw_non_flag {
        shell.positional_args = args[i..].to_vec();
    }
    ExecOutcome::Continue(0)
}

/// Formats one option line in bash's `%-15s\t%s` shopt/`set -o` format.
fn fmt_opt_line(name: &str, on: bool) -> String {
    format!("{:<15}\t{}", name, if on { "on" } else { "off" })
}

/// `shopt` builtin. Operates on the `shopt` option namespace, or — with
/// `-o` — bridges to the `set -o` namespace (`SETO_TABLE`).
fn builtin_shopt(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let (mut set_f, mut unset_f, mut quiet, mut print_f, mut o_bridge) =
        (false, false, false, false, false);
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            i += 1;
            break;
        }
        if a.len() >= 2 && a.starts_with('-') {
            for c in a[1..].chars() {
                match c {
                    's' => set_f = true,
                    'u' => unset_f = true,
                    'q' => quiet = true,
                    'p' => print_f = true,
                    'o' => o_bridge = true,
                    _ => {
                        crate::sh_error_to!(shell, err, None, "shopt: -{c}: invalid option");
                        e!(err, "shopt: usage: shopt [-pqsu] [-o] [optname ...]");
                        return ExecOutcome::Continue(2);
                    }
                }
            }
            i += 1;
        } else {
            break;
        }
    }
    if set_f && unset_f {
        crate::sh_error_to!(
            shell,
            err,
            None,
            "shopt: cannot set and unset shell options simultaneously"
        );
        return ExecOutcome::Continue(1);
    }
    let names = &args[i..];

    if o_bridge {
        return shopt_o_bridge(names, set_f, unset_f, quiet, print_f, out, err, shell);
    }

    // ---- shopt namespace ----
    if names.is_empty() {
        if quiet {
            // No names → vacuously "all set" (matches bash 5.2).
            return ExecOutcome::Continue(0);
        }
        for opt in SHOPT_TABLE {
            let on = shell.shopt_options.get(opt.name).unwrap_or(false);
            if set_f && !on {
                continue;
            }
            if unset_f && on {
                continue;
            }
            if print_f {
                let _ = writeln!(out, "shopt -{} {}", if on { 's' } else { 'u' }, opt.name);
            } else {
                let _ = writeln!(out, "{}", fmt_opt_line(opt.name, on));
            }
        }
        return ExecOutcome::Continue(0);
    }

    if set_f || unset_f {
        let mut rc = 0;
        for name in names {
            if !shell.shopt_options.set(name, set_f) {
                crate::sh_error_to!(shell, err, None, "shopt: {name}: invalid shell option name");
                rc = 1;
            }
        }
        return ExecOutcome::Continue(rc);
    }

    // query mode
    let mut all_set = true;
    for name in names {
        match shell.shopt_options.get(name) {
            Some(on) => {
                if !on {
                    all_set = false;
                }
                if !quiet {
                    if print_f {
                        let _ = writeln!(out, "shopt -{} {}", if on { 's' } else { 'u' }, name);
                    } else {
                        let _ = writeln!(out, "{}", fmt_opt_line(name, on));
                    }
                }
            }
            None => {
                crate::sh_error_to!(shell, err, None, "shopt: {name}: invalid shell option name");
                all_set = false;
            }
        }
    }
    ExecOutcome::Continue(if all_set { 0 } else { 1 })
}

/// The `-o` bridge: every `shopt` form operates on the `set -o` namespace.
#[allow(clippy::too_many_arguments)]
fn shopt_o_bridge(
    names: &[String],
    set_f: bool,
    unset_f: bool,
    quiet: bool,
    print_f: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    if names.is_empty() {
        if quiet {
            // No names → vacuously "all set" (matches bash 5.2).
            return ExecOutcome::Continue(0);
        }
        for opt in SETO_TABLE {
            let on = option_get(shell, opt.name).unwrap_or(opt.default);
            if set_f && !on {
                continue;
            }
            if unset_f && on {
                continue;
            }
            if print_f {
                let _ = writeln!(out, "set {}o {}", if on { '-' } else { '+' }, opt.name);
            } else {
                let _ = writeln!(out, "{}", fmt_opt_line(opt.name, on));
            }
        }
        return ExecOutcome::Continue(0);
    }

    if set_f || unset_f {
        let mut rc = 0;
        for name in names {
            match option_set(shell, name, set_f) {
                Ok(()) => {}
                Err(OptSetErr::Unknown) => {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "shopt: {name}: invalid shell option name"
                    );
                    rc = 1;
                }
            }
        }
        return ExecOutcome::Continue(rc);
    }

    // query mode
    let mut all_set = true;
    for name in names {
        match option_get(shell, name) {
            Some(on) => {
                if !on {
                    all_set = false;
                }
                if !quiet {
                    if print_f {
                        let _ = writeln!(out, "set {}o {}", if on { '-' } else { '+' }, name);
                    } else {
                        let _ = writeln!(out, "{}", fmt_opt_line(name, on));
                    }
                }
            }
            None => {
                crate::sh_error_to!(shell, err, None, "shopt: {name}: invalid shell option name");
                all_set = false;
            }
        }
    }
    ExecOutcome::Continue(if all_set { 0 } else { 1 })
}

fn set_escape_value(v: &str) -> String {
    // `set` (no args) lists variables in bash's POSIX `name=value` form, whose
    // value quoting is identical to the bare-`declare` form: bare when nothing
    // needs quoting, single-quoted (`'\''`-escaped) for shell metacharacters,
    // ANSI-C `$'…'` for control chars, and the lone-`'` → `\'` special case.
    declare_scalar_quote(v)
}

/// POSIX `eval`: joins args with spaces, re-parses the result,
/// and executes it in the current shell context via the same
/// `process_line` path that trap actions and `source` use.
/// Returns the exit status of the last command in the re-parsed
/// line. `exit N` / function-return / etc. propagate via the
/// returned ExecOutcome.
pub(crate) fn eval_in_sink(
    args: &[String],
    shell: &mut Shell,
    sink: &mut crate::executor::StdoutSink,
    err_sink: &mut crate::executor::StderrSink,
) -> ExecOutcome {
    if args.is_empty() {
        return ExecOutcome::Continue(0);
    }
    let joined = args.join(" ");
    if joined.trim().is_empty() {
        return ExecOutcome::Continue(0);
    }
    // PS4 depth-repeat: eval's body traces one level deeper (bash). The
    // `+ eval '…'` line was already emitted at the outer depth before dispatch.
    let saved_frame = shell.eval_frame;
    shell.eval_frame = Some(shell.current_lineno.max(1));
    let saved = shell.xtrace_depth;
    shell.xtrace_depth += 1;
    let r = crate::shell::process_line_in_sinks(&joined, shell, true, sink, err_sink);
    shell.xtrace_depth = saved;
    shell.eval_frame = saved_frame;
    r
}

fn builtin_eval(args: &[String], shell: &mut Shell) -> ExecOutcome {
    let mut sink = crate::executor::StdoutSink::Terminal;
    let mut err_sink = crate::executor::StderrSink::Terminal;
    eval_in_sink(args, shell, &mut sink, &mut err_sink)
}

/// `let EXPR...` — evaluate each argument as an arithmetic expression,
/// left-to-right, applying any side effects (assignments mutate shell vars).
/// Exit status is 0 if the LAST expression's value is non-zero, 1 if it is
/// zero — like `(( ))`. With no args, bash prints an error and exits 1.
/// Not a special builtin.
fn builtin_let(args: &[String], err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if args.is_empty() {
        crate::sh_error_to!(shell, err, None, "let: expression expected");
        return ExecOutcome::Continue(1);
    }
    let mut last: i64 = 0;
    for a in args {
        match crate::arith::parse(a).and_then(|e| crate::arith::eval(&e, shell)) {
            Ok(v) => last = v,
            Err(e) => {
                crate::sh_error_to!(
                    shell,
                    err,
                    Some("let"),
                    "{}",
                    crate::arith::render_error_body(a, &e)
                );
                return ExecOutcome::Continue(1);
            }
        }
    }
    ExecOutcome::Continue(if last != 0 { 0 } else { 1 })
}

struct HelpEntry {
    name: &'static str,
    synopsis: &'static str,
    description: &'static str,
}

static HELP_ENTRIES: &[HelpEntry] = &[
    HelpEntry {
        name: "!",
        synopsis: "! PIPELINE",
        description: "Negate the exit status of the following pipeline.\n\
                      If PIPELINE exits 0, the negated result is 1; otherwise 0.",
    },
    HelpEntry {
        name: ".",
        synopsis: ". FILENAME [ARGUMENTS]",
        description: "Execute commands from a file in the current shell.\n\
                      Reads and executes commands from FILENAME in the current shell\n\
                      context. If FILENAME does not contain a slash, $PATH is searched.\n\
                      Synonym: source.",
    },
    HelpEntry {
        name: ":",
        synopsis: ":",
        description: "Null command. Always exits 0.\n\
                      Arguments are expanded normally; useful for parameter-expansion\n\
                      side effects like `: ${VAR:=default}`.",
    },
    HelpEntry {
        name: "[",
        synopsis: "[ EXPRESSION ]",
        description: "Evaluate a conditional expression.\n\
                      Synonym for `test`; the closing `]` is required as the last argument.\n\
                      Returns 0 if EXPRESSION is true, 1 if false, 2 on usage error.",
    },
    HelpEntry {
        name: "[[",
        synopsis: "[[ EXPRESSION ]]",
        description: "Evaluate an extended conditional expression (shell keyword).\n\
                      Like `test` plus pattern matching (`==`/`!=` with glob RHS), regex\n\
                      matching (`=~`), lexicographic `<`/`>`, and short-circuit `&&`/`||`\n\
                      combinators. No word-splitting or pathname expansion on operands.",
    },
    HelpEntry {
        name: "]]",
        synopsis: "]]",
        description: "Closes a `[[ ... ]]` extended conditional expression.\n\
                      Always paired with a matching `[[`.",
    },
    HelpEntry {
        name: "alias",
        synopsis: "alias [-p] [NAME[=VALUE] ...]",
        description: "Define or display aliases.\n\
                      With no arguments, print all defined aliases. With NAME but no value,\n\
                      print that alias's value. With NAME=VALUE, define the alias.\n\
                      Aliases expand at command-name position in interactive input.",
    },
    HelpEntry {
        name: "bg",
        synopsis: "bg [job_spec ...]",
        description: "Resume jobs in the background.\n\
                      Each JOB_SPEC names a stopped job to resume without bringing it to\n\
                      the foreground. With no args, the current job (%+) is resumed.",
    },
    HelpEntry {
        name: "break",
        synopsis: "break [N]",
        description: "Exit from a for, while, or until loop.\n\
                      With argument N (default 1), break out of N enclosing loops.",
    },
    HelpEntry {
        name: "case",
        synopsis: "case WORD in [PATTERN [| PATTERN]...) COMMANDS ;; ]... esac",
        description: "Pattern-based multi-way branch (shell keyword).\n\
                      WORD is matched against each PATTERN in order; the first matching\n\
                      block's COMMANDS run. Patterns use glob syntax (*, ?, [abc]).\n\
                      Each block ends with `;;`, `;&` (fall through), or `;;&` (continue\n\
                      matching). `esac` ends the case.",
    },
    HelpEntry {
        name: "cd",
        synopsis: "cd [DIR]",
        description: "Change the shell working directory.\n\
                      With no argument, cd to $HOME. Updates $PWD and $OLDPWD.\n\
                      `cd -` cd's to $OLDPWD and prints the new PWD.",
    },
    HelpEntry {
        name: "command",
        synopsis: "command [-v|-V] NAME [ARGS ...]",
        description: "Print resolution of a command name.\n\
                      -v prints the path (or 'NAME' for builtins/keywords/aliases/functions).\n\
                      -V prints a human-readable description.\n\
                      Status 0 if all names resolve, 1 if any missing.\n\
                      Bare `command NAME ARGS` (bypass functions/aliases) is deferred.",
    },
    HelpEntry {
        name: "continue",
        synopsis: "continue [N]",
        description: "Resume the next iteration of a for/while/until loop.\n\
                      With argument N (default 1), continue at the Nth enclosing loop.",
    },
    HelpEntry {
        name: "declare",
        synopsis: "declare [-rxifFp] [+rxi] [NAME[=VALUE] ...]",
        description: "Declare variables and set attributes.\n\
                      -r readonly; -x export; -i integer (RHS arith-evaluated); -f list\n\
                      function names; -F same as -f; -p print declarations.\n\
                      +x un-export; +i unmark integer; +r errors (readonly cannot be removed).\n\
                      Inside a function (and without -g, which is deferred), declarations\n\
                      are local-scoped. Synonym: typeset.",
    },
    HelpEntry {
        name: "dirs",
        synopsis: "dirs [-clpv] [+N] [-N]",
        description: "List the directory stack.\n\
                      -c clear; -l no ~ collapse; -p one per line; -v numbered.\n\
                      +N / -N print the Nth entry (left/right indexed; 0-based).",
    },
    HelpEntry {
        name: "disown",
        synopsis: "disown [-h] [-ar] [jobspec ... | pid ...]",
        description: "Remove jobs from the active jobs table.\n\
                      -a all jobs; -r only running; -h mark for no SIGHUP on exit (the job\n\
                      stays in the table). Without flags, removes the named (or current)\n\
                      job from the table.",
    },
    HelpEntry {
        name: "do",
        synopsis: "do COMMANDS; done",
        description: "Begin the body of a for/while/until loop (shell keyword).\n\
                      Paired with `done`. The body executes once per iteration.",
    },
    HelpEntry {
        name: "done",
        synopsis: "done",
        description: "End the body of a for/while/until loop (shell keyword).\n\
                      Paired with the corresponding `do`.",
    },
    HelpEntry {
        name: "echo",
        synopsis: "echo [arg ...]",
        description: "Write arguments to standard output joined by spaces, followed by a\n\
                      newline.",
    },
    HelpEntry {
        name: "elif",
        synopsis: "elif COMMANDS; then COMMANDS",
        description: "\"else if\" branch in an `if` statement (shell keyword).\n\
                      Evaluates its own condition; the first matching branch's body runs.\n\
                      Multiple `elif` branches can chain.",
    },
    HelpEntry {
        name: "else",
        synopsis: "else COMMANDS",
        description: "Default branch of an `if` statement (shell keyword).\n\
                      Runs when no preceding `if`/`elif` condition succeeded.",
    },
    HelpEntry {
        name: "esac",
        synopsis: "esac",
        description: "End a `case` statement (shell keyword).\n\
                      Paired with the corresponding `case`.",
    },
    HelpEntry {
        name: "eval",
        synopsis: "eval [ARG ...]",
        description: "Re-parse and execute arguments as a shell command.\n\
                      Joins ARGS with spaces and runs the result in the current shell.\n\
                      Returns the exit status of the last command executed.",
    },
    HelpEntry {
        name: "exit",
        synopsis: "exit [N]",
        description: "Exit the shell with status N.\n\
                      With no argument, exit with the status of the last command.\n\
                      N is truncated to a byte (mod 256).",
    },
    HelpEntry {
        name: "export",
        synopsis: "export [-n] [NAME[=VALUE] ...]",
        description: "Mark variables for export to subsequent commands' environments.\n\
                      With NAME=VALUE, set + export. With NAME alone, set the export flag\n\
                      on an existing variable. -n removes the export attribute.",
    },
    HelpEntry {
        name: "false",
        synopsis: "false",
        description: "Always exits 1. Arguments ignored.",
    },
    HelpEntry {
        name: "fg",
        synopsis: "fg [job_spec]",
        description: "Resume a job in the foreground.\n\
                      Brings the named (or current) job into the foreground and waits for\n\
                      it to finish or stop.",
    },
    HelpEntry {
        name: "fi",
        synopsis: "fi",
        description: "End an `if` statement (shell keyword).\n\
                      Paired with the corresponding `if`.",
    },
    HelpEntry {
        name: "for",
        synopsis: "for NAME [in WORDS ...]; do COMMANDS; done",
        description: "Iterate a loop variable over a word list (shell keyword).\n\
                      Without `in WORDS`, iterates over the positional parameters.\n\
                      The body runs once per word with NAME set to the current word.",
    },
    HelpEntry {
        name: "function",
        synopsis: "function NAME { COMMANDS ; }",
        description: "Define a shell function (shell keyword).\n\
                      Alternative to the `NAME() { ... }` form. The body runs each time\n\
                      NAME is invoked, with positional parameters set from the call.",
    },
    HelpEntry {
        name: "hash",
        synopsis: "hash [-r] [-d NAME] [-p PATH NAME] [-lt] [NAME ...]",
        description: "Manage the command path cache.\n\
                      With no args, list cached entries. NAME alone resolves NAME via $PATH\n\
                      and caches the result. -r clears the table; -d NAME removes one entry;\n\
                      -p PATH NAME associates NAME with PATH directly; -l prints entries\n\
                      in re-input form; -t NAME prints the cached path.\n\
                      Note: huck's executor does not yet auto-populate the table.",
    },
    HelpEntry {
        name: "help",
        synopsis: "help [-sdm] [NAME ...]",
        description: "Display help on huck's builtins.\n\
                      With no args, list all builtins as `name: synopsis`. With NAME, print\n\
                      synopsis + description. -s shows just the synopsis line; -d shows just\n\
                      the description; -m formats the output as NAME/SYNOPSIS/DESCRIPTION\n\
                      sections.",
    },
    HelpEntry {
        name: "history",
        synopsis: "history [N]",
        description: "Display the command history.\n\
                      With argument N, show the last N entries. With no arg, show all.",
    },
    HelpEntry {
        name: "if",
        synopsis: "if COMMANDS; then COMMANDS; [elif ...] [else COMMANDS;] fi",
        description: "Conditional execution (shell keyword).\n\
                      Evaluates the `if` condition; if its exit status is 0, runs the\n\
                      `then` branch. Otherwise tries each `elif` branch in order; if\n\
                      none match, runs the `else` branch (if present).",
    },
    HelpEntry {
        name: "in",
        synopsis: "in",
        description: "Reserved word used in `for NAME in WORDS` and `case WORD in`.\n\
                      Has no standalone meaning outside those contexts.",
    },
    HelpEntry {
        name: "jobs",
        synopsis: "jobs [-lpnrs] [JOB_SPEC ...]",
        description: "List active jobs.\n\
                      -l include PIDs; -p PIDs only; -n only changed jobs; -r running;\n\
                      -s stopped. Without flags, lists all known jobs.",
    },
    HelpEntry {
        name: "kill",
        synopsis: "kill [-s SIGSPEC | -n SIGNUM | -SIGSPEC] PID|JOB ... | -l [SIGNUM]",
        description: "Send a signal to a process or job.\n\
                      SIGSPEC may be a number or a name (with or without SIG prefix).\n\
                      With -l, list signal names (or the name for a numeric signal).",
    },
    HelpEntry {
        name: "local",
        synopsis: "local NAME[=VALUE] ...",
        description: "Declare function-scoped variables.\n\
                      Each NAME is created in the current function's local scope; its\n\
                      pre-call state is snapshotted and restored when the function returns.\n\
                      Errors with status 1 when used outside a function.",
    },
    HelpEntry {
        name: "popd",
        synopsis: "popd [+N | -N]",
        description: "Pop a directory from the directory stack.\n\
                      With no args, remove the top entry and cd to the new top.\n\
                      With +N / -N, remove the Nth entry without cd (cd only if N == 0).",
    },
    HelpEntry {
        name: "printf",
        synopsis: "printf [-v VAR] FORMAT [ARGUMENTS]",
        description: "Format and print ARGUMENTS under control of FORMAT.\n\
                      Supports %s %d %i %u %o %x %X %c %% %b conversions; flags -+space#0;\n\
                      width and .N precision; standard backslash escapes; format cycling.\n\
                      With -v VAR, store the result in VAR instead of writing to stdout.\n\
                      Float conversions and %q are deferred.",
    },
    HelpEntry {
        name: "pushd",
        synopsis: "pushd [DIR | +N | -N]",
        description: "Push a directory onto the directory stack.\n\
                      pushd DIR pushes DIR and cd's to it. Bare `pushd` swaps the top two\n\
                      entries. pushd +N / -N rotates the stack so the Nth entry becomes top.",
    },
    HelpEntry {
        name: "pwd",
        synopsis: "pwd",
        description: "Print the current working directory.",
    },
    HelpEntry {
        name: "mapfile",
        synopsis: "mapfile [-d DELIM] [-n COUNT] [-O ORIGIN] [-s SKIP] [-t] [ARRAY]",
        description: "Read lines from standard input into an indexed array (default MAPFILE).\n\
                      -t strips the trailing delimiter; -d sets the delimiter (default newline);\n\
                      -n reads at most COUNT lines (0 = all); -O assigns from index ORIGIN\n\
                      (without clearing); -s discards the first SKIP lines.",
    },
    HelpEntry {
        name: "readarray",
        synopsis: "readarray [-d DELIM] [-n COUNT] [-O ORIGIN] [-s SKIP] [-t] [ARRAY]",
        description: "Synonym for mapfile.",
    },
    HelpEntry {
        name: "read",
        synopsis: "read [-r] [-p PROMPT] [-s] [-d DELIM] [-a ARRAY] [NAME ...]",
        description: "Read a line from standard input.\n\
                      With no NAME, store the line in REPLY. With one NAME, strip leading\n\
                      and trailing IFS-whitespace and assign. With multiple NAMES, IFS-split;\n\
                      the last NAME gets the unsplit remainder.\n\
                      -r raw (no backslash escape processing). -p PROMPT writes a prompt\n\
                      to stderr (tty only). -s suppresses echo (passwords). -d DELIM uses\n\
                      DELIM as the line terminator.\n\
                      -a ARRAY assigns the IFS-split words to the indexed array ARRAY.",
    },
    HelpEntry {
        name: "readonly",
        synopsis: "readonly [-p] [NAME[=VALUE] ...]",
        description: "Mark variables as readonly.\n\
                      Once readonly, the variable's value cannot change and the variable\n\
                      cannot be unset. With NAME=VALUE, sets + locks. With NAME alone,\n\
                      locks an existing variable (or creates an empty readonly variable).\n\
                      -p (or no names) lists all readonly vars.",
    },
    HelpEntry {
        name: "select",
        synopsis: "select NAME [in WORDS ...]; do COMMANDS; done",
        description: "Present a numbered menu of WORDS (or the positional parameters when `in WORDS` is omitted) on stderr, print the PS3 prompt, and read a line into REPLY. Set NAME to the chosen word (empty if the reply is not a valid item number) and run COMMANDS, repeating until end-of-input or a break. A blank line reprints the menu.",
    },
    HelpEntry {
        name: "return",
        synopsis: "return [N]",
        description: "Return from a shell function.\n\
                      With argument N, return that status; otherwise use $? from the last\n\
                      command. Errors if used outside a function or sourced file.",
    },
    HelpEntry {
        name: "set",
        synopsis: "set [-- ARGUMENTS ...]",
        description: "Set or replace positional parameters; or list all variables.\n\
                      `set` (no args) lists all shell variables sorted. `set --` replaces\n\
                      $1..$N with empty. `set -- A B C` replaces with A, B, C.\n\
                      Option flags (-e, -u, -x, -o) are not yet supported.",
    },
    HelpEntry {
        name: "shift",
        synopsis: "shift [N]",
        description: "Shift positional parameters.\n\
                      Removes the first N positional parameters (default 1). Errors if N\n\
                      exceeds the current count or is negative.",
    },
    HelpEntry {
        name: "source",
        synopsis: "source FILENAME [ARGUMENTS]",
        description: "Execute commands from a file in the current shell.\n\
                      Reads and executes commands from FILENAME in the current shell\n\
                      context. If FILENAME does not contain a slash, $PATH is searched.\n\
                      Synonym for `.`.",
    },
    HelpEntry {
        name: "test",
        synopsis: "test EXPRESSION",
        description: "Evaluate a conditional expression.\n\
                      Returns 0 if EXPRESSION is true, 1 if false, 2 on usage error.\n\
                      Supports file (-f -d -r -w -x -e -s -L), string (-n -z =, !=), and\n\
                      integer (-eq -ne -lt -gt -le -ge) tests; combinators (! && ||).\n\
                      Synonym: `[` (with closing `]`).",
    },
    HelpEntry {
        name: "then",
        synopsis: "then COMMANDS",
        description: "Begin the body of an `if` or `elif` branch (shell keyword).\n\
                      Paired with the corresponding `if`/`elif` condition.",
    },
    HelpEntry {
        name: "trap",
        synopsis: "trap [-lp] [ACTION] [SIGSPEC ...]",
        description: "Install signal/event handlers.\n\
                      `trap ACTION SIGSPEC` runs ACTION when SIGSPEC fires (re-parses\n\
                      ACTION at fire time). `trap - SIGSPEC` removes the handler.\n\
                      `trap '' SIGSPEC` ignores the signal. -p prints current traps;\n\
                      -l lists signal names. Pseudo-signals: EXIT, ERR, DEBUG, RETURN.",
    },
    HelpEntry {
        name: "true",
        synopsis: "true",
        description: "Always exits 0. Arguments ignored.",
    },
    HelpEntry {
        name: "type",
        synopsis: "type [-aftpP] NAME ...",
        description: "Describe how each NAME would be interpreted as a command.\n\
                      Default: print 'NAME is a shell builtin/keyword/function/alias' or\n\
                      'NAME is /path/to/exec'. -t prints just the type word.\n\
                      -a lists all matches (alias, function, builtin, keyword, every $PATH\n\
                      hit). -p prints the path only (silent for non-files). -P forces\n\
                      $PATH search. -f skips function lookup.",
    },
    HelpEntry {
        name: "typeset",
        synopsis: "typeset [-rxifFp] [+rxi] [NAME[=VALUE] ...]",
        description: "Synonym for `declare`. See `help declare`.",
    },
    HelpEntry {
        name: "unalias",
        synopsis: "unalias [-a] NAME ...",
        description: "Remove aliases.\n\
                      With -a, remove all aliases. Otherwise, remove each named alias.",
    },
    HelpEntry {
        name: "unset",
        synopsis: "unset NAME ...",
        description: "Unset variables.\n\
                      Each NAME is removed from the variable table. Errors with status 1\n\
                      if NAME is readonly.",
    },
    HelpEntry {
        name: "until",
        synopsis: "until COMMANDS; do COMMANDS; done",
        description: "Loop until a condition becomes true (shell keyword).\n\
                      Runs the body while the `until` condition exits non-zero. The\n\
                      mirror of `while`.",
    },
    HelpEntry {
        name: "wait",
        synopsis: "wait [-fn] [-p VAR] [PID|JOB_SPEC ...]",
        description: "Wait for processes to complete.\n\
                      With no args, wait for all known jobs. With PID/JOB_SPEC, wait for\n\
                      each. -n waits for any one to finish (returns its status). -p VAR\n\
                      stores the finishing job's PID in VAR. -f waits for full\n\
                      termination (huck's wait always does; accepted for compatibility).",
    },
    HelpEntry {
        name: "while",
        synopsis: "while COMMANDS; do COMMANDS; done",
        description: "Loop while a condition is true (shell keyword).\n\
                      Runs the body while the `while` condition exits 0. The mirror of\n\
                      `until`.",
    },
    HelpEntry {
        name: "{",
        synopsis: "{ COMMANDS ; }",
        description: "Begin a brace group (shell keyword).\n\
                      Groups COMMANDS into a single compound command that runs in the\n\
                      current shell (no subshell). Closing `}` is a separate token; the\n\
                      semicolon (or newline) before `}` is required.",
    },
    HelpEntry {
        name: "}",
        synopsis: "}",
        description: "End a brace group (shell keyword).\n\
                      Paired with the corresponding `{`.",
    },
];

fn find_help(name: &str) -> Option<&'static HelpEntry> {
    HELP_ENTRIES.iter().find(|e| e.name == name)
}

fn emit_help_entry(
    entry: &HelpEntry,
    out: &mut dyn std::io::Write,
    want_synopsis: bool,
    want_description: bool,
    want_man: bool,
) {
    if want_man {
        let _ = writeln!(out, "NAME");
        let _ = writeln!(out, "    {}", entry.name);
        let _ = writeln!(out);
        let _ = writeln!(out, "SYNOPSIS");
        let _ = writeln!(out, "    {}", entry.synopsis);
        let _ = writeln!(out);
        let _ = writeln!(out, "DESCRIPTION");
        for line in entry.description.lines() {
            let _ = writeln!(out, "    {}", line);
        }
        return;
    }
    if want_synopsis && !want_description {
        let _ = writeln!(out, "{}: {}", entry.name, entry.synopsis);
        return;
    }
    if want_description && !want_synopsis {
        for line in entry.description.lines() {
            let _ = writeln!(out, "{}", line);
        }
        return;
    }
    // Default (or -sd combined): synopsis + indented description.
    let _ = writeln!(out, "{}: {}", entry.name, entry.synopsis);
    for line in entry.description.lines() {
        let _ = writeln!(out, "    {}", line);
    }
}

fn builtin_help(
    args: &[String],
    out: &mut dyn std::io::Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let mut want_synopsis = false;
    let mut want_description = false;
    let mut want_man = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        if !arg.starts_with('-') || arg.len() < 2 {
            break;
        }
        for &c in &arg.as_bytes()[1..] {
            match c {
                b's' => want_synopsis = true,
                b'd' => want_description = true,
                b'm' => want_man = true,
                other => {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "help: -{}: invalid option",
                        other as char
                    );
                    return ExecOutcome::Continue(2);
                }
            }
        }
        i += 1;
    }
    let names = &args[i..];

    if names.is_empty() {
        for entry in HELP_ENTRIES {
            let _ = writeln!(out, "{}: {}", entry.name, entry.synopsis);
        }
        return ExecOutcome::Continue(0);
    }

    let mut exit: i32 = 0;
    for name in names {
        match find_help(name) {
            Some(entry) => emit_help_entry(entry, out, want_synopsis, want_description, want_man),
            None => {
                crate::sh_error_to!(shell, err, None, "help: no help topics match `{name}'");
                exit = 1;
            }
        }
    }
    ExecOutcome::Continue(exit)
}

pub(crate) fn source_in_sink(
    args: &[String],
    shell: &mut Shell,
    sink: &mut crate::executor::StdoutSink,
    err_sink: &mut crate::executor::StderrSink,
) -> ExecOutcome {
    if crate::restricted::is_restricted(shell)
        && let Some(path) = args.first()
        && let Err(msg) = crate::restricted::check_source_path(path)
    {
        let mut err = crate::executor::err_writer(err_sink, sink);
        crate::sh_error_to!(shell, &mut *err, None, "{msg}");
        return ExecOutcome::Continue(1);
    }
    // Materialize the redirect-aware err writer for the early-bail diagnostics
    // below (these don't recurse into the executor, so they must emit here
    // rather than via the thread-local sink — same reasoning as sh_error_to!
    // elsewhere: `sink`/`err_sink` carry the executor's in-memory redirect
    // swap for this `source`/`.` invocation).
    {
        let mut err = crate::executor::err_writer(err_sink, sink);
        if args.is_empty() {
            e!(&mut *err, ".: usage: . filename [arguments]");
            // POSIX case #1: missing-filename usage error (the not-found case at
            // resolve_source_path below was Task 2 and stays posix_fatal(1)).
            shell.builtin_usage_error = Some(2);
            return ExecOutcome::Continue(2);
        }
        if shell.source_depth >= 64 {
            crate::sh_error_to!(
                shell,
                &mut *err,
                None,
                ".: maximum source depth (64) exceeded"
            );
            return ExecOutcome::Continue(1);
        }
    }
    let filename = &args[0];
    let path = match resolve_source_path(filename, shell) {
        Some(p) => p,
        None => {
            let mut err = crate::executor::err_writer(err_sink, sink);
            // bash distinguishes a directory (opened, unusable → `.:` prefix) from a
            // genuinely-missing file (open fails → no `.:`, redirect-style).
            if std::path::Path::new(filename).is_dir() {
                crate::sh_error_to!(shell, &mut *err, Some("."), "{filename}: is a directory");
            } else {
                crate::sh_error_to!(
                    shell,
                    &mut *err,
                    None,
                    "{filename}: No such file or directory"
                );
            }
            shell.posix_fatal(1);
            return ExecOutcome::Continue(1);
        }
    };
    let contents = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            let mut err = crate::executor::err_writer(err_sink, sink);
            if e.kind() == std::io::ErrorKind::InvalidData {
                // Non-UTF-8 content: bash reports `.: <path>: cannot execute binary file`
                // and exits with status 126.
                crate::sh_error_to!(
                    shell,
                    &mut *err,
                    Some("."),
                    "{}: cannot execute binary file",
                    path.display()
                );
                return ExecOutcome::Continue(126);
            } else {
                // Open/read io error (permission, …): bash reports `<path>: <strerror>`
                // (redirect-style, no `.:`).
                crate::sh_error_to!(
                    shell,
                    &mut *err,
                    None,
                    "{}: {}",
                    path.display(),
                    crate::bash_io_error(&e)
                );
                return ExecOutcome::Continue(1);
            }
        }
    };
    let extra: Vec<String> = args[1..].to_vec();
    let saved_positional = if !extra.is_empty() {
        let saved = std::mem::take(&mut shell.positional_args);
        shell.positional_args = extra;
        Some(saved)
    } else {
        None
    };

    shell.source_depth += 1;
    shell.call_stack.push(crate::shell_state::Frame {
        funcname: "source".to_string(),
        source: path.to_string_lossy().into_owned(),
        call_line: shell.current_lineno,
        kind: crate::shell_state::FrameKind::Source,
    });
    shell.sync_call_arrays();
    let result = run_sourced_contents_in_sinks(&contents, &path, shell, sink, err_sink);
    shell.call_stack.pop();
    shell.sync_call_arrays();
    shell.source_depth -= 1;

    if let Some(saved) = saved_positional {
        shell.positional_args = saved;
    }
    result
}

fn builtin_source(args: &[String], err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let _ = err; // err writer not used: source_in_sink materializes from sinks
    let mut sink = crate::executor::StdoutSink::Terminal;
    let mut err_sink = crate::executor::StderrSink::Terminal;
    source_in_sink(args, shell, &mut sink, &mut err_sink)
}

fn resolve_source_path(
    filename: &str,
    shell: &crate::shell_state::Shell,
) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};
    // Accept any existing path that is NOT a directory: regular file, char/block
    // device, fifo, or a symlink to one (bash sources /dev/null, /dev/stdin,
    // fifos, and procsub /dev/fd/N). A directory is rejected here and reported as
    // "is a directory" by the caller's None branch.
    let usable = |p: &Path| -> bool {
        match std::fs::metadata(p) {
            // follows symlinks
            Ok(m) => !m.is_dir(),
            Err(_) => false,
        }
    };
    if filename.contains('/') {
        let p = PathBuf::from(filename);
        return usable(&p).then_some(p);
    }
    // No slash: PATH search is gated on `shopt sourcepath` (default on); when off,
    // or when the file is not found in PATH, fall back to the current directory.
    let sourcepath = shell.shopt_options.get("sourcepath").unwrap_or(true);
    if sourcepath {
        let path_var = shell.lookup_var("PATH").unwrap_or_default();
        for dir in path_var.split(':') {
            if dir.is_empty() {
                continue;
            }
            let candidate = PathBuf::from(dir).join(filename);
            if usable(&candidate) {
                return Some(candidate);
            }
        }
    }
    let cwd_candidate = PathBuf::from(filename); // ./filename
    usable(&cwd_candidate).then_some(cwd_candidate)
}

pub(crate) fn run_sourced_contents_in_sinks(
    contents: &str,
    _path: &std::path::Path,
    shell: &mut crate::shell_state::Shell,
    sink: &mut crate::executor::StdoutSink,
    err_sink: &mut crate::executor::StderrSink,
) -> ExecOutcome {
    // v315 follow-up (#209): `eval_frame` is per-eval-PARSE context, not
    // inherited by a file loaded via `source`/`.`. Without this, `eval
    // "source badfile"` left `eval_frame` set (from the outer `eval_in_sink`)
    // while badfile's own contents ran, so badfile's OWN syntax errors wrongly
    // got the `eval:` marker and an eval-shifted `line_base()` — reported the
    // wrong echoed source line. bash reports badfile's real name/line, no
    // marker. Clear it for the duration of this file's parse/exec loop and
    // restore on every exit path by funneling all of them through this thin
    // wrapper (the loop below has several early `return`s). The reverse case
    // — a `source`d file whose OWN body contains `eval "bad"` — still gets the
    // marker: `eval_in_sink` sets `eval_frame` fresh around its own nested
    // `process_line_in_sinks` call, independent of what this wrapper cleared.
    let saved_eval_frame = shell.eval_frame;
    shell.eval_frame = None;
    let result = run_sourced_contents_in_sinks_inner(contents, _path, shell, sink, err_sink);
    shell.eval_frame = saved_eval_frame;
    result
}

fn run_sourced_contents_in_sinks_inner(
    contents: &str,
    _path: &std::path::Path,
    shell: &mut crate::shell_state::Shell,
    sink: &mut crate::executor::StdoutSink,
    err_sink: &mut crate::executor::StderrSink,
) -> ExecOutcome {
    let mut last_status = shell.last_status();

    let line_of = |abs: usize| -> usize {
        1 + contents.as_bytes()[..abs.min(contents.len())]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
    };
    let next_line_start = |from: usize| -> usize {
        match contents[from.min(contents.len())..].find('\n') {
            Some(rel) => (from + rel + 1).min(contents.len()),
            None => contents.len(),
        }
    };

    let mut start = 0usize; // byte offset of the unconsumed remainder
    let mut prev_end = 0usize; // bytes already echoed for `set -v`

    'outer: loop {
        if start >= contents.len() {
            break;
        }
        let extglob = shell.shopt_options.get("extglob").unwrap_or(false);
        // v239 T6: drive the loop with a single live Lexer that expands aliases
        // at command position as the parser reads tokens. Between units the alias
        // map is refreshed (`set_aliases`) so cross-unit def-then-use works.
        let expand =
            shell.is_interactive || shell.shopt_options.get("expand_aliases").unwrap_or(false);
        // Top-level BATCH parse of a whole file / `-c` string / `source`d file:
        // an open here-document at end-of-input is delimited by EOF (bash warns but
        // parses the body collected so far), rather than erroring
        // `UnterminatedHeredoc`. bash applies the same EOF-closes rule to `source`.
        let opts = crate::lexer::LexerOptions {
            extglob,
            eof_closes_heredoc: true,
            ..Default::default()
        };
        let empty = std::collections::HashMap::new();
        let aliases_now = if expand { shell.aliases.clone() } else { empty };
        let mut iter = crate::lexer::Lexer::new(&contents[start..], &aliases_now, opts);
        // Make span line numbers file-absolute (1-based from the start of
        // `contents`) so $LINENO reports the true file line even when start > 0.
        let base_line = contents.as_bytes()[..start]
            .iter()
            .filter(|&&b| b == b'\n')
            .count() as u32;
        iter.set_base_line(base_line);

        // Sentinel byte position: used when peek_span returns None (EOF in chunk).
        let sentinel = contents.len() - start;

        loop {
            // Skip blank lines between units. A lex error while peeking here means
            // the NEXT unit begins with an invalid/unterminated token (e.g. an
            // unterminated quote as the first token). The old tokenize_partial path
            // surfaced this via its `total == 0 && terr.is_some()` guard; the live
            // pull must do the same or the error is silently dropped (the failed
            // scan runs the cursor to EOF and leaves the lexer with a half-built
            // token that would otherwise be emitted as a spurious empty Word).
            // `cursor_pos()` captured BEFORE the erroring peek is the failing
            // token's start offset (the pull is lazy: when fill_to must scan, the
            // cursor sits at the next token's start), giving the correct error line.
            loop {
                let tok_off = iter.cursor_pos();
                match iter.peek_kind() {
                    Ok(Some(crate::lexer::TokenKind::Newline)) => {
                        let _ = iter.next_kind();
                    }
                    Ok(_) => break,
                    Err(le) => {
                        let line = line_of(start + tok_off) as u32;
                        let err = crate::command::ParseError::Lex(Box::new(le));
                        crate::err_thread_local::install_err_sinks(sink, err_sink, || {
                            crate::render_syntax_diag(shell, &err, contents, line);
                        });
                        last_status = 2;
                        start = next_line_start(start + tok_off);
                        prev_end = start;
                        continue 'outer;
                    }
                }
            }
            // #175: between-command job-table maintenance. Before parsing and
            // executing the next unit, reap completed background children and
            // silently prune Done/Signaled entries (Running/Stopped are kept),
            // mirroring the interactive REPL's per-prompt cadence (`repl.rs`).
            // Printing is gated on `is_interactive`, so this prunes silently
            // non-interactively — matching bash's non-interactive pruning.
            crate::jobs::reap_and_notify(&mut *shell);
            // Byte offset of this unit's first token, read straight from its span.
            // peek_span cannot error here: the newline-skip above broke on an Ok
            // peek of this same token, so it is already scanned into history.
            let unit_start_off = iter
                .peek_span()
                .ok()
                .flatten()
                .map(|sp| sp.offset)
                .unwrap_or(sentinel);
            match crate::parser::parse_one_unit(&mut iter) {
                Ok(None) => {
                    break 'outer;
                }
                Ok(Some(seq)) => {
                    // End offset = next unparsed token's start (or the sentinel
                    // when this unit consumed the rest of the chunk). When
                    // peek_span returns Err (next token has a lex error, e.g., an
                    // extglob pattern with extglob=off), capture the error and use
                    // the start of the failing line as the boundary — mirrors the
                    // old tokenize_partial + line_start_of behavior. The captured
                    // error is handled below (after the extglob check) because the
                    // failed peek already advanced the cursor to EOF, so a
                    // subsequent peek_kind returns Ok(None) and the error would
                    // otherwise be silently swallowed.
                    // Cursor position BEFORE the peek: if peek_span has to scan and
                    // the next token errors, this is that token's start offset (the
                    // pull is lazy, so the cursor sits at the next token's start).
                    // The failed scan then runs the cursor to EOF, so we must capture
                    // the start here to report the error at the correct line.
                    let tok_off_before = iter.cursor_pos();
                    let (unit_end_off, pending_lex_err) = match iter.peek_span() {
                        Ok(Some(sp)) => (sp.offset, None),
                        Ok(None) => (sentinel, None),
                        Err(le) => {
                            let err_abs = (start + iter.cursor_pos()).min(contents.len());
                            let line_start_abs =
                                contents[..err_abs].rfind('\n').map(|i| i + 1).unwrap_or(0);
                            // unit_end (boundary for span / extglob-flip restart) =
                            // start of the failing line; carry the token-start offset
                            // separately for the error report + skip-restart.
                            (
                                line_start_abs.saturating_sub(start),
                                Some((le, tok_off_before)),
                            )
                        }
                    };
                    let unit_start_abs = start + unit_start_off;
                    let unit_end_abs = start + unit_end_off;

                    if shell.shell_options.verbose {
                        let mut err = crate::executor::err_writer(err_sink, sink);
                        let _ = write!(&mut *err, "{}", &contents[prev_end..unit_end_abs]);
                    }
                    prev_end = unit_end_abs;

                    let span = &contents[unit_start_abs..unit_end_abs];
                    let outcome =
                        crate::executor::execute_with_sink(&seq, shell, span, sink, err_sink);

                    match outcome {
                        ExecOutcome::Continue(c) => {
                            last_status = c;
                            // In a non-interactive shell, a fatal parameter-
                            // expansion error (set -u unbound var, ${x:?}, etc.)
                            // must abort the rest of the program like bash. Drain
                            // it mid-loop rather than only at the end. Gated on
                            // !is_interactive so interactive source/. and the rc
                            // path keep continuing past the error.
                            if !shell.is_interactive
                                && let Some(st) = shell.take_pending_fatal_status()
                            {
                                return ExecOutcome::Exit(st);
                            }
                        }
                        ExecOutcome::Exit(n) => return ExecOutcome::Exit(n),
                        ExecOutcome::FunctionReturn(n) => {
                            return ExecOutcome::Continue(n);
                        }
                        ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_) => {
                            last_status = 0;
                        }
                        // v312 (#3/#49): a fatal arithmetic-expansion DISCARD
                        // unwinds only THIS unit (bash `jump_to_top_level(DISCARD)`,
                        // status 1) — it does NOT exit the shell. Record status 1
                        // and keep reading the next unit (a later script line still
                        // runs). Sigint/Timeout still terminate the whole run.
                        ExecOutcome::Interrupted(InterruptReason::DiscardCommand) => {
                            last_status = 1;
                        }
                        ExecOutcome::Interrupted(r) => return ExecOutcome::Interrupted(r),
                    }

                    // Refresh the alias map in the live lexer so the next unit
                    // sees any aliases defined or removed by this unit.
                    if expand {
                        iter.set_aliases(shell.aliases.clone());
                    }

                    // A command may have flipped `shopt extglob` or
                    // `expand_aliases`; restart the outer loop to re-lex the
                    // remainder with the updated settings.
                    let new_extglob = shell.shopt_options.get("extglob").unwrap_or(false);
                    let new_expand = shell.is_interactive
                        || shell.shopt_options.get("expand_aliases").unwrap_or(false);
                    if new_extglob != extglob || new_expand != expand {
                        // pending_lex_err is intentionally discarded here: a
                        // settings flip (extglob / expand_aliases) triggers a
                        // full re-lex of the remainder with updated options, so
                        // the failing token will be re-evaluated there.
                        start = unit_end_abs;
                        prev_end = start;
                        continue 'outer;
                    }

                    // The token immediately after this unit triggered a lex error
                    // during peek_span (e.g. an unterminated `$(...)`). The failed
                    // scan advanced the cursor to EOF, so a subsequent peek_kind
                    // would return Ok(None) and the error would never reach
                    // parse_one_unit. Report it now and restart from the next line.
                    if let Some((le, tok_off)) = pending_lex_err {
                        // Report at the failing token's START line (not the cursor's
                        // post-scan EOF position), and restart just past that line.
                        let line = line_of(start + tok_off) as u32;
                        let err = crate::command::ParseError::Lex(Box::new(le));
                        crate::err_thread_local::install_err_sinks(sink, err_sink, || {
                            crate::render_syntax_diag(shell, &err, contents, line);
                        });
                        last_status = 2;
                        start = next_line_start(start + tok_off);
                        prev_end = start;
                        continue 'outer;
                    }
                }
                Err(e) => {
                    // A lex error surfaces as ParseError::Lex from the live
                    // lexer. Report it and restart the outer loop from the next
                    // line after where the scanner stopped — byte-identical to
                    // the old tokenize_partial foff path.
                    let is_lex = matches!(e, crate::command::ParseError::Lex(_));
                    let foff = if is_lex {
                        iter.cursor_pos()
                    } else {
                        unit_start_off
                    };
                    let line = line_of(start + foff) as u32;
                    crate::err_thread_local::install_err_sinks(sink, err_sink, || {
                        crate::render_syntax_diag(shell, &e, contents, line);
                    });
                    last_status = 2;
                    if is_lex {
                        start = next_line_start(start + foff);
                        prev_end = start;
                        continue 'outer;
                    }
                    // Regular parse error: skip tokens to the next newline and
                    // continue within the same lexer (no restart needed).
                    loop {
                        match iter.next_kind().ok().flatten() {
                            Some(crate::lexer::TokenKind::Newline) | None => break,
                            Some(_) => {}
                        }
                    }
                    prev_end = start
                        + iter
                            .peek_span()
                            .ok()
                            .flatten()
                            .map(|sp| sp.offset)
                            .unwrap_or(sentinel);
                }
            }
            // Break only on true EOF (Ok(None)). An Err result means the
            // next token has a lex error — let the next parse_one_unit call
            // surface and report it rather than silently stopping here.
            if matches!(iter.peek_kind(), Ok(None)) {
                break 'outer;
            }
        }
    }
    ExecOutcome::Continue(last_status)
}

/// Terminal-sink wrapper around [`run_sourced_contents_in_sinks`] — used by
/// script/`-c` mode (top-level sourcing, stdout → terminal).
pub(crate) fn run_sourced_contents(
    contents: &str,
    path: &std::path::Path,
    err: &mut dyn Write,
    shell: &mut crate::shell_state::Shell,
) -> ExecOutcome {
    let _ = err; // err is unused: in-sinks fn materializes writer from sinks.
    let mut sink = crate::executor::StdoutSink::Terminal;
    let mut err_sink = crate::executor::StderrSink::Terminal;
    run_sourced_contents_in_sinks(contents, path, shell, &mut sink, &mut err_sink)
}

fn is_valid_alias_name(s: &str) -> bool {
    !s.is_empty()
        && !s.contains('=')
        && s.chars()
            .all(|c| !c.is_whitespace() && !"|&;<>()$`\\\"'*?[]#~{}".contains(c))
}

pub(crate) fn escape_alias_value(v: &str) -> String {
    // Bash format: alias name='value' with single quotes inside
    // the value rewritten as '\''.
    v.replace('\'', r#"'\''"#)
}

fn builtin_alias(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    if args.is_empty() {
        let mut names: Vec<&String> = shell.aliases.keys().collect();
        names.sort();
        for name in names {
            let value = &shell.aliases[name];
            let _ = writeln!(out, "alias {}='{}'", name, escape_alias_value(value));
        }
        return ExecOutcome::Continue(0);
    }
    let mut any_failed = false;
    for arg in args {
        if let Some(eq) = arg.find('=') {
            let name = &arg[..eq];
            let value = &arg[eq + 1..];
            if !is_valid_alias_name(name) {
                crate::sh_error_to!(shell, err, None, "alias: `{name}': invalid alias name");
                any_failed = true;
                continue;
            }
            shell.aliases.insert(name.to_string(), value.to_string());
        } else {
            match shell.aliases.get(arg) {
                Some(v) => {
                    let _ = writeln!(out, "alias {}='{}'", arg, escape_alias_value(v));
                }
                None => {
                    crate::sh_error_to!(shell, err, None, "alias: {arg}: not found");
                    any_failed = true;
                }
            }
        }
    }
    ExecOutcome::Continue(if any_failed { 1 } else { 0 })
}

fn builtin_unalias(args: &[String], err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if args.is_empty() {
        e!(err, "unalias: usage: unalias [-a] name [name ...]");
        return ExecOutcome::Continue(2);
    }
    if args[0] == "-a" {
        shell.aliases.clear();
        return ExecOutcome::Continue(0);
    }
    let mut any_failed = false;
    for name in args {
        if shell.aliases.remove(name).is_none() {
            crate::sh_error_to!(shell, err, None, "unalias: {name}: not found");
            any_failed = true;
        }
    }
    ExecOutcome::Continue(if any_failed { 1 } else { 0 })
}

fn builtin_colon(_args: &[String], _shell: &mut Shell) -> ExecOutcome {
    ExecOutcome::Continue(0)
}

fn builtin_true(_args: &[String], _shell: &mut Shell) -> ExecOutcome {
    ExecOutcome::Continue(0)
}

fn builtin_false(_args: &[String], _shell: &mut Shell) -> ExecOutcome {
    ExecOutcome::Continue(1)
}

#[derive(Debug)]
enum CommandResolution {
    Alias(String),
    Function,
    Builtin,
    Keyword,
    File(std::path::PathBuf),
    NotFound,
}

fn is_shell_keyword(name: &str) -> bool {
    matches!(
        name,
        "if" | "then"
            | "elif"
            | "else"
            | "fi"
            | "while"
            | "until"
            | "do"
            | "done"
            | "for"
            | "in"
            | "select"
            | "case"
            | "esac"
            | "function"
            | "!"
            | "{"
            | "}"
            | "[["
            | "]]"
    )
}

fn is_executable_file(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(p) {
        Ok(md) => md.is_file() && (md.permissions().mode() & 0o111 != 0),
        Err(_) => false,
    }
}

/// Outcome of a PATH search for command runnability classification (#172).
/// Distinguishes "found an executable" from "found only a non-executable file"
/// (bash reports the latter as 126 "Permission denied") from "found nothing"
/// (127 "command not found"). `search_path_for` collapses the last two to `None`.
pub(crate) enum PathClassify {
    /// A PATH segment yielded an executable regular file (the resolved path is
    /// not carried — the caller re-searches PATH via `execvp` on the bare name).
    Executable,
    /// No executable, but at least one PATH segment yielded a non-executable
    /// regular file; carries the FIRST such resolved path (bash reports the
    /// first match in PATH order).
    NonExecutable(std::path::PathBuf),
    /// Nothing runnable and no non-executable regular-file match.
    NotFound,
}

/// Walk PATH the way bash's command search does, for runnability classification.
/// The first executable regular file wins (returns `Executable`); a directory or
/// other non-regular entry named `name` never matches; if only non-executable
/// regular files are found, the FIRST one is remembered and returned as
/// `NonExecutable`. Bare names only — callers handle slash-paths separately.
pub(crate) fn classify_path_search(name: &str, shell: &Shell) -> PathClassify {
    use std::os::unix::fs::PermissionsExt;
    let path_val = shell.lookup_var("PATH").unwrap_or_default();
    let mut first_nonexec: Option<std::path::PathBuf> = None;
    for segment in path_val.split(':') {
        if segment.is_empty() {
            continue;
        }
        let candidate = std::path::Path::new(segment).join(name);
        match std::fs::metadata(&candidate) {
            Ok(md) if md.is_file() => {
                if md.permissions().mode() & 0o111 != 0 {
                    return PathClassify::Executable;
                } else if first_nonexec.is_none() {
                    first_nonexec = Some(candidate);
                }
            }
            _ => {}
        }
    }
    match first_nonexec {
        Some(p) => PathClassify::NonExecutable(p),
        None => PathClassify::NotFound,
    }
}

pub(crate) fn search_path_for(name: &str, shell: &Shell) -> Option<std::path::PathBuf> {
    if name.contains('/') {
        let p = std::path::PathBuf::from(name);
        if is_executable_file(&p) {
            Some(p)
        } else {
            None
        }
    } else {
        let path_val = shell.lookup_var("PATH").unwrap_or_default();
        for segment in path_val.split(':') {
            if segment.is_empty() {
                continue;
            }
            let candidate = std::path::Path::new(segment).join(name);
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
        None
    }
}

fn resolve_command_name(name: &str, shell: &Shell) -> CommandResolution {
    if let Some(value) = shell.aliases.get(name) {
        return CommandResolution::Alias(value.clone());
    }
    if shell.functions.contains_key(name) {
        return CommandResolution::Function;
    }
    if builtin_active(name, shell) {
        return CommandResolution::Builtin;
    }
    if is_shell_keyword(name) {
        return CommandResolution::Keyword;
    }
    if let Some(path) = search_path_for(name, shell) {
        return CommandResolution::File(path);
    }
    CommandResolution::NotFound
}

/// Like `search_path_for` but returns ALL PATH entries whose
/// concatenation with `name` is an executable file. Preserves
/// PATH order. Empty Vec = not found. If `name` contains `/`,
/// returns the literal path iff it's executable (single match).
fn search_path_all(name: &str, shell: &Shell) -> Vec<std::path::PathBuf> {
    if name.contains('/') {
        let p = std::path::PathBuf::from(name);
        return if is_executable_file(&p) {
            vec![p]
        } else {
            vec![]
        };
    }
    let path_val = shell.lookup_var("PATH").unwrap_or_default();
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    for segment in path_val.split(':') {
        if segment.is_empty() {
            continue;
        }
        let candidate = std::path::Path::new(segment).join(name);
        if is_executable_file(&candidate) {
            out.push(candidate);
        }
    }
    out
}

/// Like `resolve_command_name` but skips the function-table
/// lookup when `skip_func` is true (for `type -f`). All other
/// resolution order is unchanged.
fn resolve_command_name_with(name: &str, shell: &Shell, skip_func: bool) -> CommandResolution {
    if let Some(v) = shell.aliases.get(name) {
        return CommandResolution::Alias(v.clone());
    }
    if !skip_func && shell.functions.contains_key(name) {
        return CommandResolution::Function;
    }
    if builtin_active(name, shell) {
        return CommandResolution::Builtin;
    }
    if is_shell_keyword(name) {
        return CommandResolution::Keyword;
    }
    if let Some(p) = search_path_for(name, shell) {
        return CommandResolution::File(p);
    }
    CommandResolution::NotFound
}

/// Returns ALL matches for `name` in bash's `type -a` order:
/// alias, function (unless skip_func), builtin, keyword, every
/// PATH entry containing an executable `name`.
fn resolve_command_name_all(name: &str, shell: &Shell, skip_func: bool) -> Vec<CommandResolution> {
    let mut out: Vec<CommandResolution> = Vec::new();
    if let Some(v) = shell.aliases.get(name) {
        out.push(CommandResolution::Alias(v.clone()));
    }
    if !skip_func && shell.functions.contains_key(name) {
        out.push(CommandResolution::Function);
    }
    if builtin_active(name, shell) {
        out.push(CommandResolution::Builtin);
    }
    if is_shell_keyword(name) {
        out.push(CommandResolution::Keyword);
    }
    for p in search_path_all(name, shell) {
        out.push(CommandResolution::File(p));
    }
    out
}

fn emit_type_entry(
    name: &str,
    res: &CommandResolution,
    type_only: bool,
    path_only: bool,
    out: &mut dyn std::io::Write,
    shell: &Shell,
) {
    if type_only {
        let word: &str = match res {
            CommandResolution::Alias(_) => "alias",
            CommandResolution::Function => "function",
            CommandResolution::Builtin => "builtin",
            CommandResolution::Keyword => "keyword",
            CommandResolution::File(_) => "file",
            CommandResolution::NotFound => return,
        };
        let _ = writeln!(out, "{word}");
        return;
    }
    if path_only {
        if let CommandResolution::File(p) = res {
            let _ = writeln!(out, "{}", p.display());
        }
        return;
    }
    match res {
        CommandResolution::Alias(value) => {
            let _ = writeln!(out, "{name} is aliased to `{value}'");
        }
        CommandResolution::Function => {
            let _ = writeln!(out, "{name} is a function");
            if let Some(body) = shell.functions.get(name) {
                let _ = writeln!(out, "{}", crate::generate::function_to_source(name, body));
            }
        }
        CommandResolution::Builtin => {
            let _ = writeln!(out, "{name} is a shell builtin");
        }
        CommandResolution::Keyword => {
            let _ = writeln!(out, "{name} is a shell keyword");
        }
        CommandResolution::File(p) => {
            let _ = writeln!(out, "{name} is {}", p.display());
        }
        CommandResolution::NotFound => {}
    }
}

fn builtin_type(
    args: &[String],
    out: &mut dyn std::io::Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let mut all = false;
    let mut type_only = false;
    let mut path_only = false;
    let mut force_path = false;
    let mut skip_func = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        if !arg.starts_with('-') || arg.len() < 2 {
            break;
        }
        for &c in &arg.as_bytes()[1..] {
            match c {
                b'a' => all = true,
                b't' => type_only = true,
                b'p' => path_only = true,
                b'P' => {
                    path_only = true;
                    force_path = true;
                }
                b'f' => skip_func = true,
                other => {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "type: -{}: invalid option",
                        other as char
                    );
                    return ExecOutcome::Continue(2);
                }
            }
        }
        i += 1;
    }
    let names = &args[i..];
    if names.is_empty() {
        return ExecOutcome::Continue(0);
    }

    let mut exit: i32 = 0;
    for name in names {
        let resolutions: Vec<CommandResolution> = if force_path {
            search_path_all(name, shell)
                .into_iter()
                .map(CommandResolution::File)
                .collect()
        } else if all {
            resolve_command_name_all(name, shell, skip_func)
        } else {
            match resolve_command_name_with(name, shell, skip_func) {
                CommandResolution::NotFound => Vec::new(),
                other => vec![other],
            }
        };

        if resolutions.is_empty() {
            if !type_only && !path_only {
                crate::sh_error_to!(shell, err, None, "type: {name}: not found");
            }
            exit = 1;
            continue;
        }
        for res in &resolutions {
            emit_type_entry(name, res, type_only, path_only, out, shell);
        }
    }
    ExecOutcome::Continue(exit)
}

fn builtin_hash(
    args: &[String],
    out: &mut dyn std::io::Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    // Mode-selector flags. Priority when multiple set:
    // reset > delete > set_path > list > type_only > default.
    let mut reset = false;
    let mut delete = false;
    let mut set_path = false;
    let mut list = false;
    let mut type_only = false;
    let mut explicit_path: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        if !arg.starts_with('-') || arg.len() < 2 {
            break;
        }
        // Walk the cluster. -p takes a value (rest-of-arg OR next arg).
        let bytes = arg.as_bytes();
        let mut j = 1;
        while j < bytes.len() {
            match bytes[j] {
                b'r' => reset = true,
                b'd' => delete = true,
                b'l' => list = true,
                b't' => type_only = true,
                b'p' => {
                    set_path = true;
                    if j + 1 < bytes.len() {
                        // -p inline: "-pPATH" (matches bash; any
                        // characters following -p are the value).
                        explicit_path = Some(String::from_utf8_lossy(&bytes[j + 1..]).into_owned());
                        break;
                    } else {
                        // -p separate: next arg
                        i += 1;
                        if i >= args.len() {
                            crate::sh_error_to!(
                                shell,
                                err,
                                None,
                                "hash: -p: option requires an argument"
                            );
                            return ExecOutcome::Continue(2);
                        }
                        explicit_path = Some(args[i].clone());
                        break;
                    }
                }
                c => {
                    crate::sh_error_to!(shell, err, None, "hash: -{}: invalid option", c as char);
                    return ExecOutcome::Continue(2);
                }
            }
            j += 1;
        }
        i += 1;
    }
    let names = &args[i..];

    if reset {
        Rc::make_mut(&mut shell.command_hash).clear();
        return ExecOutcome::Continue(0);
    }

    if delete {
        if names.is_empty() {
            crate::sh_error_to!(shell, err, None, "hash: -d: at least one name required");
            return ExecOutcome::Continue(2);
        }
        let mut exit: i32 = 0;
        let mut not_found: Vec<&String> = Vec::new();
        {
            let h = Rc::make_mut(&mut shell.command_hash);
            for name in names {
                if h.remove(name).is_none() {
                    not_found.push(name);
                    exit = 1;
                }
            }
        }
        for name in not_found {
            crate::sh_error_to!(shell, err, None, "hash: {name}: not found");
        }
        return ExecOutcome::Continue(exit);
    }

    if set_path {
        // Exactly one name required.
        if names.len() != 1 {
            crate::sh_error_to!(shell, err, None, "hash: -p: exactly one name required");
            return ExecOutcome::Continue(2);
        }
        let name = &names[0];
        if name.contains('/') {
            crate::sh_error_to!(shell, err, None, "hash: {name}: must not contain `/'");
            return ExecOutcome::Continue(1);
        }
        let path = explicit_path.unwrap(); // safe: set_path implies Some
        Rc::make_mut(&mut shell.command_hash)
            .insert(name.clone(), (std::path::PathBuf::from(path), 0u32));
        return ExecOutcome::Continue(0);
    }

    if list {
        // re-input form: `builtin hash -p PATH NAME`
        let mut entries: Vec<(&String, &(std::path::PathBuf, u32))> =
            shell.command_hash.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (name, (path, _)) in entries {
            let _ = writeln!(out, "builtin hash -p {} {}", path.display(), name);
        }
        return ExecOutcome::Continue(0);
    }

    if type_only {
        if names.is_empty() {
            crate::sh_error_to!(shell, err, None, "hash: -t: at least one name required");
            return ExecOutcome::Continue(2);
        }
        let mut exit: i32 = 0;
        for name in names {
            match shell.command_hash.get(name) {
                Some((path, _)) => {
                    if names.len() == 1 {
                        let _ = writeln!(out, "{}", path.display());
                    } else {
                        let _ = writeln!(out, "{}\t{}", name, path.display());
                    }
                }
                None => {
                    crate::sh_error_to!(shell, err, None, "hash: {name}: not found");
                    exit = 1;
                }
            }
        }
        return ExecOutcome::Continue(exit);
    }

    // Default: with names → resolve+add; without → list.
    if names.is_empty() {
        if shell.command_hash.is_empty() {
            let _ = writeln!(out, "hash: hash table empty");
            return ExecOutcome::Continue(0);
        }
        let mut entries: Vec<(&String, &(std::path::PathBuf, u32))> =
            shell.command_hash.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        let _ = writeln!(out, "hits\tcommand");
        for (_name, (path, hits)) in entries {
            let _ = writeln!(out, "{:>4}\t{}", hits, path.display());
        }
        return ExecOutcome::Continue(0);
    }

    let mut exit: i32 = 0;
    for name in names {
        if name.contains('/') {
            crate::sh_error_to!(shell, err, None, "hash: {name}: must not contain `/'");
            exit = 1;
            continue;
        }
        match search_path_for(name, shell) {
            Some(path) => {
                Rc::make_mut(&mut shell.command_hash).insert(name.clone(), (path, 0u32));
            }
            None => {
                crate::sh_error_to!(shell, err, None, "hash: {name}: not found");
                exit = 1;
            }
        }
    }
    ExecOutcome::Continue(exit)
}

fn builtin_command(
    args: &[String],
    out: &mut dyn std::io::Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let mut concise = false;
    let mut verbose = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-v" => {
                concise = true;
                i += 1;
            }
            "-V" => {
                verbose = true;
                i += 1;
            }
            "-p" => {
                i += 1;
            } // accept; introspection uses current $PATH
            "--" => {
                i += 1;
                break;
            }
            s if s.starts_with('-') && s.len() > 1 => {
                crate::sh_error_to!(shell, err, None, "command: {s}: invalid option");
                return ExecOutcome::Continue(2);
            }
            _ => break,
        }
    }
    let names = &args[i..];

    if !concise && !verbose {
        // Bare `command cmd args` (run cmd bypassing function/alias
        // lookup) is deferred to a later iteration. With no name and
        // no flag, return 0 — matches bash's silent success.
        if names.is_empty() {
            return ExecOutcome::Continue(0);
        }
        crate::sh_error_to!(
            shell,
            err,
            None,
            "command: bare form (without -v/-V) is not supported in this version"
        );
        return ExecOutcome::Continue(2);
    }

    if names.is_empty() {
        return ExecOutcome::Continue(0);
    }

    let mut any_not_found = false;
    for name in names {
        match resolve_command_name(name, shell) {
            CommandResolution::Alias(value) => {
                if concise {
                    let _ = writeln!(out, "alias {name}='{}'", escape_alias_value(&value));
                } else {
                    let _ = writeln!(out, "{name} is aliased to `{value}'");
                }
            }
            CommandResolution::Function => {
                if concise {
                    let _ = writeln!(out, "{name}");
                } else {
                    let _ = writeln!(out, "{name} is a function");
                    if let Some(body) = shell.functions.get(name) {
                        let _ =
                            writeln!(out, "{}", crate::generate::function_to_source(name, body));
                    }
                }
            }
            CommandResolution::Builtin => {
                if concise {
                    let _ = writeln!(out, "{name}");
                } else {
                    let _ = writeln!(out, "{name} is a shell builtin");
                }
            }
            CommandResolution::Keyword => {
                if concise {
                    let _ = writeln!(out, "{name}");
                } else {
                    let _ = writeln!(out, "{name} is a shell keyword");
                }
            }
            CommandResolution::File(path) => {
                if concise {
                    let _ = writeln!(out, "{}", path.display());
                } else {
                    let _ = writeln!(out, "{name} is {}", path.display());
                }
            }
            CommandResolution::NotFound => {
                any_not_found = true;
                if verbose {
                    crate::sh_error_to!(shell, err, None, "command: {name}: not found");
                }
            }
        }
    }
    ExecOutcome::Continue(if any_not_found { 1 } else { 0 })
}

fn builtin_test(name: &str, args: &[String], err: &mut dyn Write, shell: &Shell) -> ExecOutcome {
    let eval_args: &[String] = if name == "[" {
        match args.last() {
            Some(last) if last == "]" => &args[..args.len() - 1],
            _ => {
                crate::sh_error_to!(shell, err, None, "[: missing ']'");
                return ExecOutcome::Continue(2);
            }
        }
    } else {
        args
    };
    match crate::test_builtin::evaluate_with(eval_args, &|n| shell.element_or_var_is_set(n)) {
        Ok(true) => ExecOutcome::Continue(0),
        Ok(false) => ExecOutcome::Continue(1),
        Err(msg) => {
            crate::sh_error_to!(shell, err, None, "{name}: {msg}");
            ExecOutcome::Continue(2)
        }
    }
}

// ── pushd/popd/dirs (v63) ────────────────────────────────────────────

/// Parses "+N" / "-N" into a left-indexed stack position.
/// `+N` is index N from left (0 = top); `-N` is index N from right
/// (0 = bottom). Out-of-range or non-numeric returns Err.
fn parse_signed_index(s: &str, stack_len: usize) -> Result<usize, String> {
    let (sign_plus, digits) = if let Some(d) = s.strip_prefix('+') {
        (true, d)
    } else if let Some(d) = s.strip_prefix('-') {
        (false, d)
    } else {
        return Err(format!("{s}: not a +N or -N specifier"));
    };
    let n: usize = digits.parse().map_err(|_| format!("{s}: invalid number"))?;
    if n >= stack_len {
        return Err(format!("{s}: directory stack index out of range"));
    }
    Ok(if sign_plus { n } else { stack_len - 1 - n })
}

/// Returns the printable form of `path`. When `collapse` is true,
/// replaces the leading HOME with `~` (exact match → `~`; under
/// HOME/ → `~/rest`).
fn dir_display(path: &Path, shell: &Shell, collapse: bool) -> String {
    let s = path.display().to_string();
    if !collapse {
        return s;
    }
    let home = shell
        .lookup_var("HOME")
        .or_else(|| std::env::var("HOME").ok())
        .unwrap_or_default();
    if home.is_empty() {
        return s;
    }
    if s == home {
        return "~".to_string();
    }
    let with_slash = format!("{home}/");
    if let Some(rest) = s.strip_prefix(&with_slash) {
        return format!("~/{rest}");
    }
    s
}

/// Keep `dir_stack[0]` in sync with the current `$PWD` (or
/// `current_dir()` fallback). Creates a one-entry stack if empty.
fn sync_stack_top(shell: &mut Shell) {
    let cwd_str = shell
        .lookup_var("PWD")
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.display().to_string())
        })
        .unwrap_or_default();
    let p = std::path::PathBuf::from(cwd_str);
    if shell.dir_stack.is_empty() {
        shell.dir_stack.push(p);
    } else {
        shell.dir_stack[0] = p;
    }
}

/// Print the current stack to `out` per the flag knobs. Default
/// (per_line=false) emits one space-joined line; `per_line` emits
/// one entry per line, with optional `numbered` prefix.
fn print_stack(
    out: &mut dyn Write,
    shell: &Shell,
    collapse: bool,
    per_line: bool,
    numbered: bool,
) -> ExecOutcome {
    if per_line {
        for (i, p) in shell.dir_stack.iter().enumerate() {
            let disp = dir_display(p, shell, collapse);
            if numbered {
                let _ = writeln!(out, "{i:>2}  {disp}");
            } else {
                let _ = writeln!(out, "{disp}");
            }
        }
    } else {
        let parts: Vec<String> = shell
            .dir_stack
            .iter()
            .map(|p| dir_display(p, shell, collapse))
            .collect();
        let _ = writeln!(out, "{}", parts.join(" "));
    }
    ExecOutcome::Continue(0)
}

/// Detect `+N`/`-N` form: starts with `+`, or starts with `-` and
/// has a digit immediately after.
fn is_signed_index_arg(s: &str) -> bool {
    // Both `+N` and `-N` require a digit immediately after the
    // sign so a literal directory name like `+foo` or `-bar` is
    // treated as a path, not a misformatted index spec.
    (s.starts_with('+') || s.starts_with('-')) && s.len() > 1 && s.as_bytes()[1].is_ascii_digit()
}

fn builtin_pushd(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    sync_stack_top(shell);

    if args.is_empty() {
        // Swap top two.
        if shell.dir_stack.len() < 2 {
            crate::sh_error_to!(shell, err, None, "pushd: no other directory");
            return ExecOutcome::Continue(1);
        }
        shell.dir_stack.swap(0, 1);
        let target = shell.dir_stack[0].clone();
        let cd_args = vec![target.display().to_string()];
        if let ExecOutcome::Continue(c) = builtin_cd_as("pushd", &cd_args, out, err, shell)
            && c != 0
        {
            // Undo the swap on failure.
            shell.dir_stack.swap(0, 1);
            return ExecOutcome::Continue(c);
        }
        return print_stack(out, shell, true, false, false);
    }

    let arg = &args[0];
    if is_signed_index_arg(arg) {
        let idx = match parse_signed_index(arg, shell.dir_stack.len()) {
            Ok(i) => i,
            Err(e) => {
                crate::sh_error_to!(shell, err, None, "pushd: {e}");
                return ExecOutcome::Continue(1);
            }
        };
        if idx == 0 {
            return print_stack(out, shell, true, false, false);
        }
        shell.dir_stack.rotate_left(idx);
        let target = shell.dir_stack[0].clone();
        let cd_args = vec![target.display().to_string()];
        if let ExecOutcome::Continue(c) = builtin_cd_as("pushd", &cd_args, out, err, shell)
            && c != 0
        {
            // Undo rotation on cd failure.
            shell.dir_stack.rotate_right(idx);
            return ExecOutcome::Continue(c);
        }
        return print_stack(out, shell, true, false, false);
    }

    // pushd DIR
    let cd_args = vec![arg.clone()];
    if let ExecOutcome::Continue(c) = builtin_cd_as("pushd", &cd_args, out, err, shell)
        && c != 0
    {
        return ExecOutcome::Continue(c);
    }
    let new_cwd = shell
        .lookup_var("PWD")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| std::path::PathBuf::from(arg));
    shell.dir_stack.insert(0, new_cwd);
    print_stack(out, shell, true, false, false)
}

fn builtin_popd(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    sync_stack_top(shell);
    if shell.dir_stack.len() <= 1 {
        crate::sh_error_to!(shell, err, None, "popd: directory stack empty");
        return ExecOutcome::Continue(1);
    }

    let idx = if args.is_empty() {
        0
    } else {
        let arg = &args[0];
        if !is_signed_index_arg(arg) {
            crate::sh_error_to!(shell, err, None, "popd: {arg}: invalid argument");
            return ExecOutcome::Continue(1);
        }
        match parse_signed_index(arg, shell.dir_stack.len()) {
            Ok(i) => i,
            Err(e) => {
                crate::sh_error_to!(shell, err, None, "popd: {e}");
                return ExecOutcome::Continue(1);
            }
        }
    };

    // Save the entry being removed so we can restore on cd failure
    // (only matters when idx == 0, where popd does a cd to the new
    // top). Matches bash: popd leaves the stack unchanged when the
    // resulting cd fails.
    let saved = shell.dir_stack[idx].clone();
    shell.dir_stack.remove(idx);
    if idx == 0 {
        let target = shell.dir_stack[0].clone();
        let cd_args = vec![target.display().to_string()];
        if let ExecOutcome::Continue(c) = builtin_cd_as("popd", &cd_args, out, err, shell)
            && c != 0
        {
            // Restore the entry we just popped so the stack is
            // exactly as it was before the failing popd.
            shell.dir_stack.insert(0, saved);
            return ExecOutcome::Continue(c);
        }
    }
    print_stack(out, shell, true, false, false)
}

fn builtin_dirs(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    sync_stack_top(shell);

    let mut collapse = true;
    let mut per_line = false;
    let mut numbered = false;
    let mut clear = false;
    let mut index: Option<usize> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "-c" => {
                clear = true;
                i += 1;
            }
            "-l" => {
                collapse = false;
                i += 1;
            }
            "-p" => {
                per_line = true;
                i += 1;
            }
            "-v" => {
                per_line = true;
                numbered = true;
                i += 1;
            }
            s if is_signed_index_arg(s) => {
                match parse_signed_index(s, shell.dir_stack.len()) {
                    Ok(idx) => index = Some(idx),
                    Err(e) => {
                        crate::sh_error_to!(shell, err, None, "dirs: {e}");
                        return ExecOutcome::Continue(1);
                    }
                }
                i += 1;
            }
            s if s.starts_with('-') && s.len() > 1 => {
                crate::sh_error_to!(shell, err, None, "dirs: {s}: invalid option");
                return ExecOutcome::Continue(2);
            }
            _ => break,
        }
    }

    if clear {
        shell.dir_stack.truncate(1);
        return ExecOutcome::Continue(0);
    }
    if let Some(idx) = index {
        let entry = &shell.dir_stack[idx];
        let _ = writeln!(out, "{}", dir_display(entry, shell, collapse));
        return ExecOutcome::Continue(0);
    }
    print_stack(out, shell, collapse, per_line, numbered)
}

fn builtin_bind(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    use crate::readline_bind::{is_known_function, keyseq_is_valid, readline_function_names};
    const USAGE: &str = "bind: usage: bind [-lpsvPSVX] [-m keymap] [-f filename] [-q name] [-u name] [-r keyseq] [-x keyseq:shell-command] [keyseq:readline-function or readline-command]";

    let mut i = 0;
    let mut rc = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-v" => {
                for l in shell.readline_var_lines() {
                    let _ = writeln!(out, "{l}");
                }
            }
            "-V" => {
                for l in shell.readline_var_lines_verbose() {
                    let _ = writeln!(out, "{l}");
                }
            }
            "-l" => {
                for f in readline_function_names() {
                    let _ = writeln!(out, "{f}");
                }
            }
            "-p" => {
                for l in shell.active_bind_lines() {
                    let _ = writeln!(out, "{l}");
                }
            }
            "-P" => {
                for l in shell.active_bind_lines_verbose() {
                    let _ = writeln!(out, "{l}");
                }
            }
            "-s" | "-S" | "-X" => { /* no macros / shell-command bindings: empty */ }
            "-m" | "-q" | "-u" | "-f" => {
                i += 1; /* takes an arg; accept + no-op */
            }
            "-r" => {
                i += 1;
                if let Some(seq) = args.get(i) {
                    shell.add_unbind(seq);
                } else {
                    crate::sh_error_to!(shell, err, None, "bind: -r: option requires an argument");
                    rc = 2;
                }
            }
            "-x" => {
                i += 1; /* keyseq:shell-command — deferred no-op */
            }
            s if s.starts_with('-') && s.len() > 1 => {
                crate::sh_error_to!(shell, err, None, "bind: {s}: invalid option");
                e!(err, "{USAGE}");
                return ExecOutcome::Continue(2);
            }
            // Non-flag argument: `set VAR VALUE` (3-arg or inline), or `keyseq:function`.
            _ => {
                if a == "set" {
                    // 3-arg form: bind set VAR VALUE
                    let var = args.get(i + 1).cloned();
                    let val = args.get(i + 2).cloned();
                    if let (Some(var), Some(val)) = (var, val) {
                        if !validate_readline_var(&var, &val) {
                            crate::sh_error_to!(
                                shell,
                                err,
                                None,
                                "bind: {val}: invalid value for {var}"
                            );
                            rc = 1;
                        } else {
                            shell.set_readline_var(&var, &val);
                        }
                        i += 2;
                    }
                } else if let Some(rest) = a.strip_prefix("set ") {
                    // one-arg form: "set VAR VALUE"
                    let mut it = rest.split_whitespace();
                    if let (Some(var), Some(val)) = (it.next(), it.next()) {
                        if !validate_readline_var(var, val) {
                            crate::sh_error_to!(
                                shell,
                                err,
                                None,
                                "bind: {val}: invalid value for {var}"
                            );
                            rc = 1;
                        } else {
                            shell.set_readline_var(var, val);
                        }
                    }
                } else if let Some((seq, func)) = a.split_once(':') {
                    if !keyseq_is_valid(seq) {
                        crate::sh_error_to!(
                            shell,
                            err,
                            None,
                            "bind: {seq}: cannot parse key sequence"
                        );
                        rc = 1;
                    } else if !is_known_function(func) {
                        crate::sh_error_to!(
                            shell,
                            err,
                            None,
                            "bind: {func}: unknown function name"
                        );
                        rc = 1;
                    } else {
                        shell.add_bind(seq, func);
                    }
                } else {
                    crate::sh_error_to!(shell, err, None, "bind: {a}: unknown command");
                    rc = 1;
                }
            }
        }
        i += 1;
    }
    ExecOutcome::Continue(rc)
}

/// Validates a readline variable value for the 5 editor-mapped variables.
/// Unmapped variables accept any value (recorded for `bind -v` round-trip).
fn validate_readline_var(var: &str, val: &str) -> bool {
    match var {
        "editing-mode" => matches!(val, "emacs" | "vi"),
        "bell-style" => matches!(val, "none" | "audible" | "visible"),
        "show-all-if-ambiguous" => matches!(val, "on" | "off"),
        "completion-query-items" | "keyseq-timeout" => val.parse::<i64>().is_ok(),
        _ => true,
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod fg_bg_tests;

#[cfg(test)]
mod kill_tests;

#[cfg(test)]
mod cd_pwd_tests;

#[cfg(test)]
mod disown_tests;

#[cfg(test)]
mod history_tests;

#[cfg(test)]
mod special_builtin_tests;

#[cfg(test)]
mod alias_tests;

#[cfg(test)]
mod shift_tests;

#[cfg(test)]
mod set_tests;

#[cfg(test)]
mod source_tests;

#[cfg(test)]
mod local_tests;

#[cfg(test)]
mod colon_tests;

#[cfg(test)]
mod true_false_tests;

#[cfg(test)]
mod command_tests;

#[cfg(test)]
mod readonly_tests;

#[cfg(test)]
mod read_tests;

#[cfg(test)]
mod printf_tests;

#[cfg(test)]
mod exit_tests;

#[cfg(test)]
mod type_tests;

#[cfg(test)]
mod hash_tests;

#[cfg(test)]
mod dirstack_tests;

#[cfg(test)]
mod declare_tests;

#[cfg(test)]
mod integer_attr_tests;

#[cfg(test)]
mod eval_tests;

#[cfg(test)]
mod help_tests;

#[cfg(test)]
mod set_options_tests;

#[cfg(test)]
mod array_declare_tests;

#[cfg(test)]
mod assoc_declare_tests;

#[cfg(test)]
mod loop_levels_tests;

#[cfg(test)]
mod pipefail_option_tests;

#[cfg(test)]
mod getopts_step_tests;

// ── umask ──────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub(crate) enum SymErr {
    Char(char),
    Operator(char),
}

/// Parse an octal umask literal (digits 0-7 only). Err on any non-octal digit.
pub(crate) fn parse_octal_umask(s: &str) -> Result<u32, ()> {
    let mut val: u32 = 0;
    for ch in s.chars() {
        let d = ch.to_digit(8).ok_or(())?; // rejects 8,9 and non-digits
        val = val
            .checked_mul(8)
            .and_then(|v| v.checked_add(d))
            .ok_or(())?;
    }
    if s.is_empty() {
        return Err(());
    }
    Ok(val & 0o777)
}

/// Parse a symbolic umask string against the current mask. mask bit set = deny.
pub(crate) fn parse_symbolic_umask(s: &str, cur: u32) -> Result<u32, SymErr> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut mask = cur & 0o777;
    loop {
        // who
        let mut shifts: Vec<u32> = Vec::new();
        while i < chars.len() && matches!(chars[i], 'u' | 'g' | 'o' | 'a') {
            match chars[i] {
                'u' => shifts.push(6),
                'g' => shifts.push(3),
                'o' => shifts.push(0),
                'a' => {
                    shifts.extend([6, 3, 0]);
                }
                _ => unreachable!(),
            }
            i += 1;
        }
        if shifts.is_empty() {
            shifts = vec![6, 3, 0];
        }
        // operator
        if i >= chars.len() {
            return Err(SymErr::Operator('\0'));
        }
        let op = chars[i];
        if !matches!(op, '=' | '+' | '-') {
            return Err(SymErr::Operator(op));
        }
        i += 1;
        // perms
        let mut perm: u32 = 0;
        while i < chars.len() && matches!(chars[i], 'r' | 'w' | 'x') {
            perm |= match chars[i] {
                'r' => 4,
                'w' => 2,
                'x' => 1,
                _ => 0,
            };
            i += 1;
        }
        for sh in &shifts {
            match op {
                '=' => {
                    mask &= !(0o7 << sh);
                    mask |= (!perm & 0o7) << sh;
                }
                '+' => {
                    mask &= !(perm << sh);
                }
                '-' => {
                    mask |= perm << sh;
                }
                _ => unreachable!(),
            }
        }
        // clause boundary
        if i >= chars.len() {
            break;
        }
        if chars[i] == ',' {
            i += 1;
            continue;
        }
        return Err(SymErr::Char(chars[i]));
    }
    Ok(mask & 0o777)
}

/// Symbolic rendering of the ALLOWED perms (complement of mask) as `u=rwx,g=rx,o=rx`.
pub(crate) fn format_symbolic_umask(mask: u32) -> String {
    let mut parts = Vec::new();
    for (cls, sh) in [('u', 6u32), ('g', 3), ('o', 0)] {
        let allowed = (!mask >> sh) & 0o7;
        let mut p = String::new();
        if allowed & 4 != 0 {
            p.push('r');
        }
        if allowed & 2 != 0 {
            p.push('w');
        }
        if allowed & 1 != 0 {
            p.push('x');
        }
        parts.push(format!("{cls}={p}"));
    }
    parts.join(",")
}

fn builtin_umask(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let mut symbolic = false;
    let mut posix = false;
    let mut idx = 0;
    while idx < args.len() {
        let a = &args[idx];
        if a == "--" {
            idx += 1;
            break;
        }
        if a.len() > 1 && a.starts_with('-') {
            for c in a[1..].chars() {
                match c {
                    'S' => symbolic = true,
                    'p' => posix = true,
                    other => {
                        crate::sh_error_to!(shell, err, Some("umask"), "-{other}: invalid option");
                        e!(err, "umask: usage: umask [-p] [-S] [mode]");
                        return ExecOutcome::Continue(2);
                    }
                }
            }
            idx += 1;
        } else {
            break;
        }
    }
    // read current mask without disturbing it
    let cur = (unsafe {
        let m = libc::umask(0);
        libc::umask(m);
        m
    } as u32)
        & 0o777;

    if idx < args.len() {
        let mode = &args[idx];
        let first_digit = mode
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false);
        let new_mask = if first_digit {
            match parse_octal_umask(mode) {
                Ok(m) => m,
                Err(()) => {
                    crate::sh_error_to!(
                        shell,
                        err,
                        Some("umask"),
                        "{mode}: octal number out of range"
                    );
                    return ExecOutcome::Continue(1);
                }
            }
        } else {
            match parse_symbolic_umask(mode, cur) {
                Ok(m) => m,
                Err(se) => {
                    match se {
                        SymErr::Char(ch) => crate::sh_error_to!(
                            shell,
                            err,
                            Some("umask"),
                            "`{ch}': invalid symbolic mode character"
                        ),
                        SymErr::Operator(ch) => crate::sh_error_to!(
                            shell,
                            err,
                            Some("umask"),
                            "`{ch}': invalid symbolic mode operator"
                        ),
                    }
                    return ExecOutcome::Continue(1);
                }
            }
        };
        unsafe {
            libc::umask(new_mask as libc::mode_t);
        }
        // bash prints the symbolic mask when -S is given alongside a mode arg
        if symbolic {
            let body = format_symbolic_umask(new_mask);
            let _ = writeln!(out, "{body}");
        }
        return ExecOutcome::Continue(0);
    }

    let body = if symbolic {
        format_symbolic_umask(cur)
    } else {
        format!("{cur:04o}")
    };
    let line = match (posix, symbolic) {
        (true, true) => format!("umask -S {body}"),
        (true, false) => format!("umask {body}"),
        (false, _) => body,
    };
    let _ = writeln!(out, "{line}");
    ExecOutcome::Continue(0)
}

#[cfg(test)]
mod umask_tests;

// ─── ulimit ──────────────────────────────────────────────────────────────────

// `getrlimit`/`setrlimit` take a Linux-glibc-specific `__rlimit_resource_t` on
// Linux but a plain `c_int` on macOS/BSD. Alias so `RlimitResource` matches the
// type of the `RLIMIT_*` constants (and the syscall signature) on each platform.
#[cfg(target_os = "linux")]
type RlimitResource = libc::__rlimit_resource_t;
#[cfg(not(target_os = "linux"))]
type RlimitResource = libc::c_int;

struct UlimitRes {
    letter: char,
    resource: RlimitResource,
    mult: u64,           // value units per limit byte/raw; 1 = unscaled
    label: &'static str, // for `-a`
}

const ULIMIT_TABLE: &[UlimitRes] = &[
    UlimitRes {
        letter: 'c',
        resource: libc::RLIMIT_CORE,
        mult: 1024,
        label: "core file size          (blocks, -c)",
    },
    UlimitRes {
        letter: 'd',
        resource: libc::RLIMIT_DATA,
        mult: 1024,
        label: "data seg size           (kbytes, -d)",
    },
    // RLIMIT_NICE/SIGPENDING/MSGQUEUE/RTPRIO/LOCKS are Linux-only; macOS bash
    // likewise does not offer -e/-i/-q/-r/-x, so gate them out off-Linux.
    #[cfg(target_os = "linux")]
    UlimitRes {
        letter: 'e',
        resource: libc::RLIMIT_NICE,
        mult: 1,
        label: "scheduling priority             (-e)",
    },
    UlimitRes {
        letter: 'f',
        resource: libc::RLIMIT_FSIZE,
        mult: 1024,
        label: "file size               (blocks, -f)",
    },
    #[cfg(target_os = "linux")]
    UlimitRes {
        letter: 'i',
        resource: libc::RLIMIT_SIGPENDING,
        mult: 1,
        label: "pending signals                 (-i)",
    },
    UlimitRes {
        letter: 'l',
        resource: libc::RLIMIT_MEMLOCK,
        mult: 1024,
        label: "max locked memory       (kbytes, -l)",
    },
    UlimitRes {
        letter: 'm',
        resource: libc::RLIMIT_RSS,
        mult: 1024,
        label: "max memory size         (kbytes, -m)",
    },
    UlimitRes {
        letter: 'n',
        resource: libc::RLIMIT_NOFILE,
        mult: 1,
        label: "open files                      (-n)",
    },
    #[cfg(target_os = "linux")]
    UlimitRes {
        letter: 'q',
        resource: libc::RLIMIT_MSGQUEUE,
        mult: 1,
        label: "POSIX message queues     (bytes, -q)",
    },
    #[cfg(target_os = "linux")]
    UlimitRes {
        letter: 'r',
        resource: libc::RLIMIT_RTPRIO,
        mult: 1,
        label: "real-time priority              (-r)",
    },
    UlimitRes {
        letter: 's',
        resource: libc::RLIMIT_STACK,
        mult: 1024,
        label: "stack size              (kbytes, -s)",
    },
    UlimitRes {
        letter: 't',
        resource: libc::RLIMIT_CPU,
        mult: 1,
        label: "cpu time               (seconds, -t)",
    },
    UlimitRes {
        letter: 'u',
        resource: libc::RLIMIT_NPROC,
        mult: 1,
        label: "max user processes              (-u)",
    },
    UlimitRes {
        letter: 'v',
        resource: libc::RLIMIT_AS,
        mult: 1024,
        label: "virtual memory          (kbytes, -v)",
    },
    #[cfg(target_os = "linux")]
    UlimitRes {
        letter: 'x',
        resource: libc::RLIMIT_LOCKS,
        mult: 1,
        label: "file locks                      (-x)",
    },
];

fn ulimit_lookup(letter: char) -> Option<&'static UlimitRes> {
    ULIMIT_TABLE.iter().find(|r| r.letter == letter)
}

fn ulimit_get(res: &UlimitRes, hard: bool) -> Option<u64> {
    let mut rl = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(res.resource, &mut rl) } != 0 {
        return None;
    }
    let v = if hard { rl.rlim_max } else { rl.rlim_cur };
    if v == libc::RLIM_INFINITY {
        return Some(u64::MAX);
    } // sentinel for "unlimited"
    Some((v as u64) / res.mult)
}

/// Returns Err(io::Error) if setrlimit fails.
fn ulimit_set(res: &UlimitRes, raw: u64, set_soft: bool, set_hard: bool) -> std::io::Result<()> {
    let mut rl = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(res.resource, &mut rl) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let scaled: libc::rlim_t = if raw == u64::MAX {
        libc::RLIM_INFINITY
    } else {
        raw.saturating_mul(res.mult) as libc::rlim_t
    };
    if set_soft {
        rl.rlim_cur = scaled;
    }
    if set_hard {
        rl.rlim_max = scaled;
    }
    if unsafe { libc::setrlimit(res.resource, &rl) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn builtin_ulimit(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    const USAGE: &str = "ulimit: usage: ulimit [-SHabcdefiklmnpqrstuvxPRT] [limit]";
    let mut want_soft = false;
    let mut want_hard = false;
    let mut show_all = false;
    let mut letters: Vec<char> = Vec::new();
    let mut idx = 0;
    while idx < args.len() {
        let a = &args[idx];
        if a == "--" {
            idx += 1;
            break;
        }
        if a.len() > 1 && a.starts_with('-') {
            for c in a[1..].chars() {
                match c {
                    'S' => want_soft = true,
                    'H' => want_hard = true,
                    'a' => show_all = true,
                    'p' => letters.push('p'),
                    other if ulimit_lookup(other).is_some() => letters.push(other),
                    other => {
                        crate::sh_error_to!(shell, err, Some("ulimit"), "-{other}: invalid option");
                        e!(err, "{USAGE}");
                        return ExecOutcome::Continue(2);
                    }
                }
            }
            idx += 1;
        } else {
            break;
        }
    }
    let value_arg: Option<&String> = args.get(idx);

    if show_all {
        let hard = want_hard && !want_soft;
        for res in ULIMIT_TABLE {
            let v = ulimit_get(res, hard);
            let disp = match v {
                Some(u64::MAX) => "unlimited".to_string(),
                Some(n) => n.to_string(),
                None => "?".to_string(),
            };
            let _ = writeln!(out, "{} {}", res.label, disp);
        }
        return ExecOutcome::Continue(0);
    }

    if letters.is_empty() {
        letters.push('f');
    } // bash default resource

    // `-p` pipe pseudo-resource: bash reports 8 (512-byte blocks), set is a no-op.
    let do_hard = want_hard;
    let do_soft = want_soft || !want_hard; // query: soft by default; set: both unless one chosen
    let mut status = 0;

    if let Some(val) = value_arg {
        // SET
        let set_soft = want_soft || (!want_soft && !want_hard);
        let set_hard = want_hard || (!want_soft && !want_hard);
        for &lt in &letters {
            if lt == 'p' {
                continue;
            } // no-op success
            let res = ulimit_lookup(lt).unwrap();
            let raw = match val.as_str() {
                "unlimited" => u64::MAX,
                s => match s.parse::<u64>() {
                    Ok(n) => n,
                    Err(_) => {
                        crate::sh_error_to!(shell, err, Some("ulimit"), "{val}: invalid number");
                        return ExecOutcome::Continue(1);
                    }
                },
            };
            if let Err(e) = ulimit_set(res, raw, set_soft, set_hard) {
                crate::sh_error_to!(
                    shell,
                    err,
                    Some("ulimit"),
                    "{val}: cannot modify limit: {}",
                    crate::bash_io_error(&e)
                );
                status = 1;
            }
        }
    } else {
        // QUERY
        let hard = do_hard && !do_soft;
        let single = letters.len() == 1;
        for &lt in &letters {
            if lt == 'p' {
                if single {
                    let _ = writeln!(out, "8");
                } else {
                    let _ = writeln!(out, "pipe size            (512 bytes, -p) 8");
                }
                continue;
            }
            let res = ulimit_lookup(lt).unwrap();
            let disp = match ulimit_get(res, hard) {
                Some(u64::MAX) => "unlimited".to_string(),
                Some(n) => n.to_string(),
                None => {
                    status = 1;
                    continue;
                }
            };
            if single {
                let _ = writeln!(out, "{disp}");
            } else {
                let _ = writeln!(out, "{} {}", res.label, disp);
            }
        }
    }
    ExecOutcome::Continue(status)
}

fn builtin_times(
    _args: &[String],
    out: &mut dyn Write,
    _err: &mut dyn Write,
    _shell: &mut Shell,
) -> ExecOutcome {
    let mut t: libc::tms = unsafe { std::mem::zeroed() };
    unsafe {
        libc::times(&mut t);
    }
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let hz = if hz > 0 { hz as f64 } else { 100.0 };
    let fmt = |ticks: libc::clock_t| -> String {
        let secs = ticks as f64 / hz;
        let m = (secs / 60.0).floor() as u64;
        let s = secs - (m as f64) * 60.0;
        format!("{m}m{s:.3}s")
    };
    let _ = writeln!(out, "{} {}", fmt(t.tms_utime), fmt(t.tms_stime));
    let _ = writeln!(out, "{} {}", fmt(t.tms_cutime), fmt(t.tms_cstime));
    ExecOutcome::Continue(0)
}

fn builtin_enable(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    const USAGE: &str = "enable: usage: enable [-a] [-dnps] [-f filename] [name ...]";
    let mut disable = false; // -n
    let mut all = false; // -a
    let mut special = false; // -s
    let mut idx = 0;
    while idx < args.len() {
        let a = &args[idx];
        if a == "--" {
            idx += 1;
            break;
        }
        if a.len() > 1 && a.starts_with('-') {
            for c in a[1..].chars() {
                match c {
                    'n' => disable = true,
                    'a' => all = true,
                    's' => special = true,
                    'p' => {} // print format — the listing default
                    other => {
                        crate::sh_error_to!(shell, err, Some("enable"), "-{other}: invalid option");
                        e!(err, "{USAGE}");
                        return ExecOutcome::Continue(2);
                    }
                }
            }
            idx += 1;
        } else {
            break;
        }
    }
    let names = &args[idx..];

    if names.is_empty() {
        let mut cands: Vec<&str> = BUILTIN_NAMES
            .iter()
            .copied()
            .filter(|n| !special || is_special_builtin(n))
            .collect();
        cands.sort_unstable();
        for n in cands {
            let is_off = shell.disabled_builtins.contains(n);
            let show = if disable {
                is_off
            } else if all {
                true
            } else {
                !is_off
            };
            if !show {
                continue;
            }
            if is_off {
                let _ = writeln!(out, "enable -n {n}");
            } else {
                let _ = writeln!(out, "enable {n}");
            }
        }
        return ExecOutcome::Continue(0);
    }

    let mut status = 0;
    for name in names {
        if !is_builtin(name) {
            crate::sh_error_to!(shell, err, Some("enable"), "{name}: not a shell builtin");
            status = 1;
            continue;
        }
        if disable {
            shell.disabled_builtins.insert(name.clone());
        } else {
            shell.disabled_builtins.remove(name);
        }
    }
    ExecOutcome::Continue(status)
}

#[cfg(test)]
mod ulimit_tests;

#[cfg(test)]
mod enable_tests;

#[cfg(test)]
mod normalize_logical_tests;
