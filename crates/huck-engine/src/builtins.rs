use std::env;
use std::io::Write;
use std::path::Path;
use std::rc::Rc;

use crate::command::DeclArg;
use crate::shell_state::{Shell, SHOPT_TABLE};

/// Why an executor run was interrupted. Used to discriminate the top-level
/// exit code mapping (SIGINT -> 130, ExecBuilder::timeout -> 124).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptReason {
    Sigint,
    Timeout,
}

/// The result of running a command — either the shell continues (carrying the
/// command's exit status) or the shell should terminate with a code.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ExecOutcome {
    Continue(i32),
    Exit(i32),
    LoopBreak(u32, i32),    // (level: 1-based capped to loop_depth, terminal $?: 0 normal / 1 malformed-arg)
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
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return", "trap", "alias", "unalias",
    "set", "shopt", "shift", "getopts", ".", "source", "local",
    ":", "true", "false", "command", "builtin", "exec",
    "readonly", "read", "mapfile", "readarray", "printf", "type", "hash",
    "pushd", "popd", "dirs",
    "declare", "typeset",
    "eval",
    "let",
    "help",
    "complete", "compgen", "compopt",
    "bind",
];

pub fn is_builtin(name: &str) -> bool {
    BUILTIN_NAMES.contains(&name)
}

/// True for "declaration commands" (bash terminology). Their
/// assignment-shaped args (`a=(x y)`, `a[i]+=v`) are parsed as
/// `Assignment`s and routed through `apply_one_assignment`, NOT
/// expanded as ordinary Words. Non-assignment args (flags like
/// `-a`, bare names) flow through normal expansion. See `resolve()`
/// in src/executor.rs for the split logic.
pub fn is_declaration_command(name: &str) -> bool {
    matches!(name, "declare" | "typeset" | "local" | "readonly" | "export")
}

/// True for POSIX "special builtins" (2.14). Inline assignments preceding a
/// special builtin persist in the shell; assignments preceding a regular
/// builtin or external command are scoped to the command. The set is huck's
/// existing builtins intersected with the POSIX special list; expand here as
/// huck adds `exec`.
pub fn is_special_builtin(name: &str) -> bool {
    matches!(name,
        ":" | "." | "break" | "continue" | "eval" | "exec" | "exit" | "export" | "readonly" | "return"
        | "set" | "shift" | "source" | "trap" | "unset"
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
        "echo" => builtin_echo(args, out, err),
        "exit" => builtin_exit(args, err, shell),
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
            e!(err, "huck: exec: not supported in this context");
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
        "return" => builtin_return(args, shell),
        "bind" => builtin_bind(args, out, err, shell),
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
fn classify_loop_arg(args: &[String], cmd: &str, err: &mut dyn Write) -> LoopArg {
    if args.len() > 1 {
        e!(err, "huck: {cmd}: too many arguments");
        return LoopArg::BreakAll;
    }
    let Some(arg) = args.first() else { return LoopArg::Level(1) };
    match arg.parse::<i64>() {
        Ok(n) if n >= 1 => LoopArg::Level(n.min(u32::MAX as i64) as u32),
        Ok(_) => {
            e!(err, "huck: {cmd}: {arg}: loop count out of range");
            LoopArg::BreakAll
        }
        Err(_) => {
            e!(err, "huck: {cmd}: {arg}: numeric argument required");
            LoopArg::Fatal
        }
    }
}

fn builtin_break(args: &[String], err: &mut dyn Write, shell: &Shell) -> ExecOutcome {
    if shell.loop_depth == 0 {
        e!(err, "huck: break: only meaningful in a `for', `while', or `until' loop");
        return ExecOutcome::Continue(0);
    }
    match classify_loop_arg(args, "break", err) {
        LoopArg::Level(n) => ExecOutcome::LoopBreak(n.min(shell.loop_depth), 0),
        LoopArg::BreakAll => ExecOutcome::LoopBreak(shell.loop_depth, 1),
        LoopArg::Fatal => ExecOutcome::Exit(128),
    }
}

fn builtin_continue(args: &[String], err: &mut dyn Write, shell: &Shell) -> ExecOutcome {
    if shell.loop_depth == 0 {
        e!(err, "huck: continue: only meaningful in a `for', `while', or `until' loop");
        return ExecOutcome::Continue(0);
    }
    match classify_loop_arg(args, "continue", err) {
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
    use crate::command::{Assignment, AssignTarget};
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
                    value: Word(vec![WordPart::Literal { text: val, quoted: false }]),
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

pub(crate) fn builtin_cd(args: &[String], out: &mut dyn Write, err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if crate::restricted::is_restricted(shell)
        && let Err(msg) = crate::restricted::check_cd()
    {
        e!(err, "{msg}");
        return ExecOutcome::Continue(1);
    }
    // 1. Parse leading -L/-P flags (last wins) and `--`. `-` is NOT a flag (it
    //    is the OLDPWD shortcut / target).
    let mut physical_flag: Option<bool> = None;
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "-L" => { physical_flag = Some(false); idx += 1; }
            "-P" => { physical_flag = Some(true); idx += 1; }
            "--" => { idx += 1; break; }
            "-" => break, // OLDPWD shortcut, handled as the target below
            s if s.starts_with('-') && s.len() > 1 => {
                e!(err, "huck: cd: {s}: invalid option");
                e!(err, "huck: cd: usage: cd [-L|[-P [-e]] [-@]] [dir]");
                return ExecOutcome::Continue(2);
            }
            _ => break, // a target
        }
    }
    let rest = &args[idx..];
    if rest.len() > 1 {
        e!(err, "huck: cd: too many arguments");
        return ExecOutcome::Continue(1);
    }

    // 2. Effective mode: explicit flag, else the `physical` set-option.
    let physical = physical_flag.unwrap_or_else(|| option_get(shell, "physical").unwrap_or(false));

    // 3. Compute the target directory.
    let mut print_new_pwd = false;
    let target = match rest.first() {
        Some(dir) if dir == "-" => match shell.get("OLDPWD") {
            Some(oldpwd) if !oldpwd.is_empty() => { print_new_pwd = true; oldpwd.to_string() }
            _ => { e!(err, "huck: cd: OLDPWD not set"); return ExecOutcome::Continue(1); }
        },
        Some(dir) => dir.clone(),
        None => match shell.get("HOME") {
            Some(home) => home.to_string(),
            None => { e!(err, "huck: cd: HOME not set"); return ExecOutcome::Continue(1); }
        },
    };

    let prev_pwd = shell.get("PWD").map(str::to_string);

    let new_pwd: String = if physical {
        // Physical: chdir to the target, store the canonical cwd.
        if let Err(e) = env::set_current_dir(Path::new(&target)) {
            e!(err, "huck: cd: {target}: {e}");
            return ExecOutcome::Continue(1);
        }
        match env::current_dir() {
            Ok(p) => p.to_string_lossy().into_owned(),
            Err(e) => {
                e!(err, "huck: cd: warning: could not read current dir: {e}");
                prev_pwd.clone().unwrap_or_default()
            }
        }
    } else {
        // Logical: build curpath from $PWD (for relative targets), lexically
        // normalize, chdir to the normalized path, store it.
        let curpath = if target.starts_with('/') {
            target.clone()
        } else {
            let base = prev_pwd.clone().filter(|p| !p.is_empty()).unwrap_or_else(|| {
                env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
            });
            format!("{base}/{target}")
        };
        let normalized = normalize_logical(&curpath);
        if let Err(e) = env::set_current_dir(Path::new(&normalized)) {
            e!(err, "huck: cd: {target}: {e}");
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
    if print_new_pwd
        && let Err(e) = writeln!(out, "{new_pwd}")
    {
        e!(err, "huck: cd: {e}");
        return ExecOutcome::Continue(1);
    }
    ExecOutcome::Continue(0)
}

fn builtin_pwd(args: &[String], out: &mut dyn Write, err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    // Parse -L/-P (last wins); `--` ends flags; non-flag args are ignored
    // (bash prints pwd anyway). Unknown flag → invalid option, rc 2.
    let mut physical_flag: Option<bool> = None;
    for a in args {
        match a.as_str() {
            "-L" => physical_flag = Some(false),
            "-P" => physical_flag = Some(true),
            "--" => break,
            s if s.starts_with('-') && s.len() > 1 => {
                e!(err, "huck: pwd: {s}: invalid option");
                e!(err, "huck: pwd: usage: pwd [-LP]");
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

    if let Err(e) = writeln!(out, "{path}") {
        e!(err, "huck: pwd: {e}");
        return ExecOutcome::Continue(1);
    }
    ExecOutcome::Continue(0)
}

fn builtin_echo(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> ExecOutcome {
    let (mut suppress_newline, process_escapes, consumed) = parse_echo_flags(args);
    let joined = args[consumed..].join(" ");
    let bytes = if process_escapes {
        let (b, hit_c) = process_echo_escapes(&joined);
        if hit_c {
            suppress_newline = true;
        }
        b
    } else {
        joined.into_bytes()
    };

    if let Err(e) = out.write_all(&bytes) {
        e!(err, "huck: echo: {e}");
        return ExecOutcome::Continue(1);
    }
    if !suppress_newline
        && let Err(e) = out.write_all(b"\n")
    {
        e!(err, "huck: echo: {e}");
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

fn builtin_exit(args: &[String], err: &mut dyn Write, shell: &Shell) -> ExecOutcome {
    match args.first() {
        None => ExecOutcome::Exit(shell.last_status()),
        Some(code_str) => match code_str.parse::<i32>() {
            Ok(code) => ExecOutcome::Exit(code.rem_euclid(256)),
            Err(_) => {
                e!(err, "huck: exit: {code_str}: numeric argument required");
                ExecOutcome::Continue(2)
            }
        },
    }
}

pub(crate) fn is_valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else { return false; };
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
                e!(err, "huck: unset: {s}: invalid option");
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
                e!(err, "huck: unset: '{arg}': not a valid identifier");
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
                e!(err, "huck: unset: '{arg}': not a valid identifier");
                any_error = true;
                continue;
            }
            if shell.is_readonly(arg) {
                e!(err, "huck: unset: {arg}: readonly variable");
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
                crate::shell_state::ResolvedName::Element { name: base, subscript } => {
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
                            e!(err, "huck: unset: {e}");
                            any_error = true;
                        }
                    }
                }
                continue;
            }
            Ok(None) => {}
            Err(e) => {
                e!(err, "huck: unset: {e}");
                any_error = true;
                continue;
            }
        }
        if !is_valid_name(effective_arg) {
            e!(err, "huck: unset: '{effective_arg}': not a valid identifier");
            any_error = true;
            continue;
        }
        if shell.is_readonly(effective_arg) {
            e!(err, "huck: unset: {effective_arg}: readonly variable");
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
        && k.bytes().all(|b| {
            matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-')
        })
    {
        k.to_string()
    } else {
        format!("\"{}\"", crate::escape_double_quote_value(k))
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
                format!("=\"{}\"", crate::escape_double_quote_value(s))
            }
        }
        VarValue::Indexed(m) => {
            let parts: Vec<String> = m
                .iter()
                .map(|(k, v)| format!("[{k}]=\"{}\"", crate::escape_double_quote_value(v)))
                .collect();
            format!("=({})", parts.join(" "))
        }
        VarValue::Associative(pairs) => {
            let parts: Vec<String> = pairs
                .iter()
                .map(|(k, v)| {
                    format!(
                        "[{}]=\"{}\"",
                        quote_subscript_key(k),
                        crate::escape_double_quote_value(v)
                    )
                })
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
fn list_exported(out: &mut dyn Write, err: &mut dyn Write, shell: &Shell) -> ExecOutcome {
    let mut entries: Vec<(&String, &crate::shell_state::Variable)> =
        shell.iter_vars().filter(|(_, v)| v.exported).collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    for (name, var) in entries {
        if let Err(e) = writeln!(out, "{}", format_declare_line(name, var)) {
            e!(err, "huck: export: {e}");
            return ExecOutcome::Continue(1);
        }
    }
    ExecOutcome::Continue(0)
}

/// Lists exported functions (sorted) as `generate` body + `declare -fx NAME`.
fn list_exported_functions(out: &mut dyn Write, err: &mut dyn Write, shell: &Shell) -> ExecOutcome {
    for name in shell.exported_function_names() {
        if let Some(body) = shell.functions.get(&name)
            && (writeln!(out, "{}", crate::generate::function_to_source(&name, body)).is_err()
                || writeln!(out, "declare -fx {name}").is_err())
        {
            e!(err, "huck: export: write error");
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
fn declare_list_all_vars(
    out: &mut dyn std::io::Write,
    shell: &Shell,
    bare: bool,
) -> ExecOutcome {
    let mut entries: Vec<(&String, &crate::shell_state::Variable)> =
        shell.iter_vars().collect();
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
            emit_function(n, false, out, shell);
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
    out: &mut dyn std::io::Write,
    shell: &mut Shell,
) -> ExecOutcome {
    if names.is_empty() {
        let mut fnames: Vec<String> = shell.functions.keys().cloned().collect();
        fnames.sort();
        for n in &fnames {
            emit_function(n, names_only, out, shell);
        }
        return ExecOutcome::Continue(0);
    }
    let mut exit: i32 = 0;
    for name in names {
        if shell.functions.contains_key(name) {
            emit_function(name, names_only, out, shell);
        } else {
            // bash: `declare -f`/`-F` on a missing function is silent (rc 1).
            exit = 1;
        }
    }
    ExecOutcome::Continue(exit)
}

/// Emit a single existing function: the `-F` header for `names_only`,
/// otherwise the full normalized body via `generate::function_to_source`.
fn emit_function(
    name: &str,
    names_only: bool,
    out: &mut dyn std::io::Write,
    shell: &Shell,
) {
    if names_only {
        let _ = writeln!(out, "declare -f {name}");
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

/// True iff the `Word` value of an Assignment carries a trailing
/// `ArrayLiteral` (i.e. it's a compound-RHS form like `name=(x y)`).
fn assign_value_is_array(a: &crate::command::Assignment) -> bool {
    matches!(
        a.value.0.last(),
        Some(crate::lexer::WordPart::ArrayLiteral(_))
    )
}

/// `export` entry point with DeclArg input. Rejects array compound-RHS;
/// otherwise mirrors the legacy `builtin_export` behavior (scalar `=`
/// assigns + exports; bare `NAME` flips the export bit without checking
/// readonly).
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
                                e!(err, "huck: export: -{c}: invalid option");
                                e!(err,
                                    "export: usage: export [-fn] [name[=value] ...] or export -p"
                                );
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
            return list_exported_functions(out, err, shell);
        }
        if saw_a && !saw_p {
            return ExecOutcome::Continue(0);
        }
        return list_exported(out, err, shell);
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
                e!(err, "huck: export: {name}: not a function");
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
                        e!(err, "huck: export: '{s}': not a valid identifier");
                        any_error = true;
                        continue;
                    }
                    if shell.is_readonly(name) {
                        e!(err, "huck: export: {name}: readonly variable");
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
                        e!(err, "huck: export: '{s}': not a valid identifier");
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
                if assign_value_is_array(a) {
                    e!(err, "huck: export: cannot export arrays");
                    any_error = true;
                    continue;
                }
                if matches!(&a.target, crate::command::AssignTarget::Indexed { .. }) {
                    let name = a.target.name();
                    e!(err, "huck: export: `{name}': not a valid identifier");
                    any_error = true;
                    continue;
                }
                let name = a.target.name().to_string();
                if shell.is_readonly(&name) {
                    e!(err, "huck: export: {name}: readonly variable");
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
        e!(err, "huck: local: can only be used in a function");
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
                    e!(err, "huck: local: -{}: invalid option", other as char);
                    return ExecOutcome::Continue(1);
                }
            }
        }
        idx += 1;
    }
    if want_array && want_associative {
        e!(err, "huck: local: cannot specify both -a and -A");
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
                    e!(err, "huck: local: `{s}': not a valid identifier");
                    exit = 1;
                    continue;
                }
                if shell.is_readonly(name) {
                    e!(err, "huck: local: {name}: readonly variable");
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
                        e!(err,
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
                    e!(err,
                        "huck: local: `{name}': not a valid identifier"
                    );
                    exit = 1;
                    continue;
                }
                if shell.is_readonly(&name) {
                    e!(err, "huck: local: {name}: readonly variable");
                    exit = 1;
                    continue;
                }
                snapshot_for_local_scope(shell, &name);

                // `local -n NAME=target`: nameref bind — validate and store raw.
                if saw_minus_n {
                    // Expand the RHS word to obtain the target name string.
                    let target = crate::expand::expand_assignment(&a.value, shell);
                    if target == name {
                        e!(err,
                            "huck: local: {name}: nameref variable self references not allowed"
                        );
                        exit = 1;
                        continue;
                    }
                    let valid = is_valid_name(&target)
                        || matches!(parse_subscripted_arg(&target), Ok(Some((b, _))) if is_valid_name(b));
                    if !valid {
                        e!(err,
                            "huck: local: `{target}': invalid variable name for name reference"
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
                    e!(err,
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
                e!(err, "huck: readonly: {o}: invalid option");
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
            if let Err(e) = writeln!(out, "{line}") {
                e!(err, "huck: readonly: {e}");
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
                    e!(err,
                        "huck: readonly: `{s}': not a valid identifier"
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
                    e!(err,
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
                        e!(err,
                            "huck: readonly: {name}: readonly variable"
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
                        e!(err,
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
                    e!(err,
                        "huck: readonly: `{name}': cannot make subscripted-assignment target readonly"
                    );
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
        let DeclArg::Plain(arg) = &args[idx] else { break };
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
                    e!(err,
                        "huck: declare: +r: readonly attribute cannot be removed"
                    );
                    return ExecOutcome::Continue(1);
                }
                b'x' if minus => want_export = true,
                b'x' if plus => want_remove_export = true,
                b'i' if minus => want_integer = true,
                b'i' if plus => want_remove_integer = true,
                b'a' if minus => want_array = true,
                b'a' if plus => {
                    e!(err,
                        "huck: declare: +a: array attribute cannot be removed"
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
                    e!(err,
                        "huck: declare: +A: associative attribute cannot be removed"
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
                    e!(err,
                        "huck: declare: {sign}{}: invalid option",
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
        e!(err, "huck: declare: cannot specify both -a and -A");
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
            return list_exported_functions(out, err, shell);
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
        return declare_list_functions(&plain_names, function_names_only, out, shell);
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
            e!(err, "huck: declare: `{name}': not a valid identifier");
            exit = 1;
            continue;
        }

        if print_mode {
            match shell.snapshot_var(name) {
                Some(var) => {
                    let _ = writeln!(out, "{}", format_declare_line(name, &var));
                }
                None => {
                    e!(err, "huck: declare: {name}: not found");
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
            e!(err, "huck: declare: {name}: readonly variable");
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
                e!(err, "huck: declare: {name}: readonly variable");
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
            e!(err,
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
        if saw_plus_l
            && shell.case_fold_of(name) == Some(crate::shell_state::CaseFold::Lower)
        {
            shell.set_case_fold(name, None);
        }
        if saw_plus_u
            && shell.case_fold_of(name) == Some(crate::shell_state::CaseFold::Upper)
        {
            shell.set_case_fold(name, None);
        }

        // Nameref (-n / +n) handling. Must come BEFORE the compound-assignment
        // path so that the target is stored raw (not through apply_one_assignment).
        if saw_minus_n {
            let target_opt: Option<String> = assign_opt.map(|a| {
                crate::expand::expand_assignment(&a.value, shell)
            });
            if let Some(ref target) = target_opt {
                // Direct self-reference is a hard error.
                if target == name {
                    e!(err,
                        "huck: declare: {name}: nameref variable self references not allowed"
                    );
                    exit = 1;
                    continue;
                }
                // Target must be a valid variable name OR name[subscript].
                let valid = is_valid_name(target)
                    || matches!(parse_subscripted_arg(target), Ok(Some((b, _))) if is_valid_name(b));
                if !valid {
                    e!(err,
                        "huck: declare: `{target}': invalid variable name for name reference"
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
                e!(err, "huck: declare: {name}: readonly variable");
                exit = 1;
                continue;
            }
            if shell.is_readonly(name) {
                e!(err, "huck: {name}: readonly variable");
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

fn read_one_line<R: std::io::Read>(
    r: &mut R,
    raw: bool,
    delim: u8,
) -> std::io::Result<Option<String>> {
    let mut out: Vec<u8> = Vec::new();
    let mut any_byte_read = false;
    loop {
        let mut byte = [0u8; 1];
        let n = r.read(&mut byte)?;
        if n == 0 {
            if !any_byte_read {
                return Ok(None);
            }
            break;
        }
        any_byte_read = true;
        let b = byte[0];
        if b == delim {
            break;
        }
        if !raw && b == b'\\' {
            let mut nxt = [0u8; 1];
            let m = r.read(&mut nxt)?;
            if m == 0 {
                // Trailing backslash at EOF: keep it.
                out.push(b'\\');
                break;
            }
            // any_byte_read already true
            if nxt[0] == b'\n' {
                continue; // line continuation
            }
            out.push(nxt[0]); // escape removal: \X → X
            continue;
        }
        out.push(b);
    }
    Ok(Some(String::from_utf8_lossy(&out).into_owned()))
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
fn split_into_names(
    line: &str,
    names: &[String],
    ifs: &str,
) -> Vec<(String, String)> {
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

    // Last field: rest of line from position i, with trailing
    // ws-IFS stripped.
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
        return if line.is_empty() { Vec::new() } else { vec![line.to_string()] };
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
unsafe fn silent_disable_echo() -> Option<libc::termios> {
    use std::os::unix::io::AsRawFd;
    let fd = std::io::stdin().as_raw_fd();
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
unsafe fn silent_restore_echo(saved: libc::termios) {
    use std::os::unix::io::AsRawFd;
    let fd = std::io::stdin().as_raw_fd();
    let _ = unsafe { libc::tcsetattr(fd, libc::TCSANOW, &saved) };
}

/// Reads one byte at a time from STDIN_FILENO via `libc::read`,
/// bypassing Rust's shared `std::io::stdin()` BufReader. Necessary
/// because rustyline's non-tty `readline_direct` path fills that same
/// BufReader with script-ahead bytes; using it here would return
/// cached script bytes instead of the redirected fd 0.
struct RawStdinReader;

impl RawStdinReader {
    fn new() -> Self {
        RawStdinReader
    }
}

impl std::io::Read for RawStdinReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let n = unsafe {
                libc::read(
                    libc::STDIN_FILENO,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                )
            };
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
    fn num_val(args: &[String], i: &mut usize, j: usize, bytes: &[u8], opt: char, err: &mut dyn Write) -> Result<usize, ()> {
        let s = if j + 1 < bytes.len() {
            String::from_utf8_lossy(&bytes[j + 1..]).into_owned()
        } else {
            *i += 1;
            if *i >= args.len() {
                e!(err, "huck: mapfile: -{opt}: option requires an argument");
                return Err(());
            }
            args[*i].clone()
        };
        match s.trim().parse::<usize>() {
            Ok(n) => Ok(n),
            Err(_) => {
                e!(err, "huck: mapfile: {s}: invalid number");
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
                    let s = if j + 1 < bytes.len() {
                        String::from_utf8_lossy(&bytes[j + 1..]).into_owned()
                    } else {
                        i += 1;
                        if i >= args.len() {
                            e!(err, "huck: mapfile: -d: option requires an argument");
                            return ExecOutcome::Continue(2);
                        }
                        args[i].clone()
                    };
                    delim = s.bytes().next().unwrap_or(0u8); // empty -> NUL
                    consumed_rest = true;
                }
                b'n' => match num_val(args, &mut i, j, bytes, 'n', err) {
                    Ok(n) => { count = n; consumed_rest = true; }
                    Err(()) => return ExecOutcome::Continue(2),
                },
                b's' => match num_val(args, &mut i, j, bytes, 's', err) {
                    Ok(n) => { skip = n; consumed_rest = true; }
                    Err(()) => return ExecOutcome::Continue(2),
                },
                b'O' => match num_val(args, &mut i, j, bytes, 'O', err) {
                    Ok(n) => { origin = Some(n); consumed_rest = true; }
                    Err(()) => return ExecOutcome::Continue(2),
                },
                c => {
                    e!(err, "huck: mapfile: -{}: invalid option", c as char);
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

    let array_name = args.get(i).cloned().unwrap_or_else(|| "MAPFILE".to_string());
    if !is_valid_name(&array_name) {
        e!(err, "huck: mapfile: `{array_name}': not a valid array name");
        return ExecOutcome::Continue(1);
    }

    let mut handle = RawStdinReader::new();
    // Skip the first `skip` records.
    for _ in 0..skip {
        match read_one_record(&mut handle, delim) {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => {
                e!(err, "huck: mapfile: {e}");
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
                e!(err, "huck: mapfile: {e}");
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
                        prompt = Some(
                            String::from_utf8_lossy(&bytes[j + 1..]).into_owned(),
                        );
                    } else {
                        i += 1;
                        if i >= args.len() {
                            e!(err, "huck: read: -p: option requires an argument");
                            return ExecOutcome::Continue(2);
                        }
                        prompt = Some(args[i].clone());
                    }
                    break;
                }
                b'd' => {
                    let d_val: String = if j + 1 < bytes.len() {
                        String::from_utf8_lossy(&bytes[j + 1..]).into_owned()
                    } else {
                        i += 1;
                        if i >= args.len() {
                            e!(err, "huck: read: -d: option requires an argument");
                            return ExecOutcome::Continue(2);
                        }
                        args[i].clone()
                    };
                    // Empty DELIM means NUL byte.
                    delim = d_val.bytes().next().unwrap_or(0u8);
                    break;
                }
                b'a' => {
                    let v: String = if j + 1 < bytes.len() {
                        String::from_utf8_lossy(&bytes[j + 1..]).into_owned()
                    } else {
                        i += 1;
                        if i >= args.len() {
                            e!(err, "huck: read: -a: option requires an argument");
                            return ExecOutcome::Continue(2);
                        }
                        args[i].clone()
                    };
                    array_name = Some(v);
                    break;
                }
                c => {
                    e!(err, "huck: read: -{}: invalid option", c as char);
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
            e!(err, "huck: read: `{name}': not a valid identifier");
            return ExecOutcome::Continue(1);
        }
    }
    if let Some(arr) = &array_name
        && !is_valid_name(arr)
    {
        e!(err, "huck: read: `{arr}': not a valid identifier");
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

    // -s silent: toggle ECHO off on stdin's tty for the duration of
    // the read, then restore.
    #[cfg(unix)]
    let saved_term = if silent {
        unsafe { silent_disable_echo() }
    } else {
        None
    };

    // Read directly from STDIN_FILENO via libc::read, bypassing Rust's
    // BufReader-backed std::io::stdin(). The static BufReader is shared
    // with rustyline's non-tty `readline_direct` path, which fills it
    // with subsequent script lines on a single underlying read; using
    // BufReader here would return cached script bytes instead of the
    // redirected fd 0 (e.g. our `<<<` here-string pipe).
    let mut handle = RawStdinReader::new();
    let line_opt = match read_one_line(&mut handle, raw, delim) {
        Ok(opt) => opt,
        Err(e) => {
            e!(err, "huck: read: {e}");
            #[cfg(unix)]
            if let Some(s) = saved_term {
                unsafe {
                    silent_restore_echo(s);
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
            silent_restore_echo(s);
        }
    }
    if was_silenced {
        e!(err, "");
    }

    let line = match line_opt {
        Some(l) => l,
        None => return ExecOutcome::Continue(1), // EOF, nothing read
    };

    // Assignment.
    let ifs = shell.ifs();
    if let Some(arr) = array_name {
        let fields = split_read_fields(&line, &ifs);
        let map: std::collections::BTreeMap<usize, String> =
            fields.into_iter().enumerate().collect();
        if shell.replace_indexed(&arr, map).is_err() {
            return ExecOutcome::Continue(1); // replace_indexed printed the readonly message
        }
        return ExecOutcome::Continue(0);
    }
    let assignments: Vec<(String, String)> = if names.is_empty() {
        vec![("REPLY".to_string(), line)]
    } else {
        split_into_names(&line, &names, &ifs)
    };

    let mut exit = 0;
    for (name, value) in assignments {
        if shell.try_set(&name, value).is_err() {
            e!(err, "huck: read: {name}: readonly variable");
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
        let digit_part: Vec<u8> =
            if spec.precision == Some(0) && digits.iter().all(|&b| b == b'0') {
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
        let use_zero =
            spec.flags.zero_pad && !spec.flags.left_align && spec.precision.is_none();
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
                    e!(err, "huck: printf: -v: option requires an argument");
                    return ExecOutcome::Continue(2);
                }
                let target = &args[i];
                let valid = is_valid_name(target)
                    || crate::expand::split_name_subscript(target)
                        .map(|(name, sub)| is_valid_name(&name) && !sub.is_empty())
                        .unwrap_or(false);
                if !valid {
                    e!(err, "huck: printf: `{target}': not a valid identifier");
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
                e!(err, "huck: printf: {s}: invalid option");
                return ExecOutcome::Continue(2);
            }
            _ => break,
        }
    }

    if i >= args.len() {
        e!(err, "huck: printf: usage: printf [-v var] format [arguments]");
        return ExecOutcome::Continue(2);
    }

    let format = args[i].clone();
    let rest_args: &[String] = &args[i + 1..];

    let parts = match parse_format(&format) {
        Ok(p) => p,
        Err(e) => {
            e!(err, "huck: printf: {e}");
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
                            e!(err, "huck: printf: {msg}");
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
                            e!(err, "huck: printf: {msg}");
                            exit = 1;
                        }
                        spec.precision = if n < 0 { None } else { Some(n as usize) };
                    }
                    let arg = next_arg(&mut arg_idx);
                    match format_one(&spec, arg, &mut buf) {
                        Ok(true) => {}
                        Ok(false) => halted = true,
                        Err(msg) => {
                            e!(err, "huck: printf: {msg}");
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
                    subscript: crate::lexer::Word(vec![
                        crate::lexer::WordPart::Literal { text: sub, quoted: false },
                    ]),
                },
                value: crate::lexer::Word(vec![
                    crate::lexer::WordPart::Literal { text: s, quoted: true },
                ]),
                append: false,
            };
            if crate::executor::apply_one_assignment(&assignment, shell, err).is_err() {
                // apply_one_assignment already printed the specific diagnostic
                // (readonly / type mismatch / bad subscript).
                return ExecOutcome::Continue(1);
            }
        } else if shell.try_set(&var, s).is_err() {
            e!(err, "huck: printf: {var}: readonly variable");
            return ExecOutcome::Continue(1);
        }
    } else if let Err(e) = out.write_all(&buf) {
        e!(err, "huck: printf: {e}");
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
fn parse_jobs_args(args: &[String], err: &mut dyn Write, shell: &Shell) -> Result<JobsArgs, ExecOutcome> {
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
                        e!(err, "huck: jobs: -{c}: invalid option");
                        e!(err, "huck: jobs: usage: jobs [-lpnrs] [%spec ...]");
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
            e!(err, "huck: jobs: {arg}: no such job");
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

fn builtin_jobs(args: &[String], out: &mut dyn Write, err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
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
        if let Err(e) = write_result {
            e!(err, "huck: jobs: {e}");
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
fn parse_wait_args(args: &[String], err: &mut dyn Write, shell: &Shell) -> Result<WaitArgs, ExecOutcome> {
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
            "-p" => {
                if idx + 1 >= args.len() {
                    e!(err, "huck: wait: -p: option requires a variable name");
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
                e!(err, "huck: wait: {s}: invalid option");
                e!(err, "huck: wait: usage: wait [-n] [-p var] [id ...]");
                return Err(ExecOutcome::Continue(2));
            }
            _ => break,
        }
    }

    if pid_var.is_some() && !wait_any {
        e!(err, "huck: wait: -p: option requires -n");
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
                    e!(err, "huck: wait: {arg}: not a pid or valid job spec");
                    return Err(ExecOutcome::Continue(2));
                }
            }
        }
        idx += 1;
    }

    Ok(WaitArgs { wait_any, pid_var, targets })
}

fn builtin_wait(args: &[String], _out: &mut dyn Write, err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
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
        let mut status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if r > 0 {
            shell.jobs.reap(r, status);
            if !libc::WIFSTOPPED(status) {
                shell.reap_coproc(r);
            }
        } else {
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
        let terminal = shell.jobs.iter()
            .find(|j| j.id == id)
            .and_then(|j| match j.state {
                crate::jobs::JobState::Done(c) => Some(c),
                crate::jobs::JobState::Signaled(s) => Some(128 + s),
                _ => None,
            });
        if let Some(code) = terminal {
            return ExecOutcome::Continue(code);
        }
        if let Some(o) = crate::executor::check_interrupt(shell) {
            return o;
        }
        let mut status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if r > 0 {
            shell.jobs.reap(r, status);
            if !libc::WIFSTOPPED(status) {
                shell.reap_coproc(r);
            }
        } else {
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
            return ExecOutcome::Continue(code);
        }
        if r < 0 {
            // ECHILD: not a child (or already reaped). On the first call,
            // surface as "not a child." On a subsequent call, treat as a
            // race we can't recover from.
            if first {
                e!(err, "huck: wait: pid {pid} is not a child of this shell");
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

        let still_present = shell
            .jobs
            .iter()
            .any(|j| snapshot.contains(&j.id));
        if !still_present {
            if let Some(name) = &pid_var {
                shell.set(name, String::new());
            }
            return ExecOutcome::Continue(127);
        }

        if let Some(o) = crate::executor::check_interrupt(shell) {
            return o;
        }
        let mut status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if r > 0 {
            shell.jobs.reap(r, status);
            if !libc::WIFSTOPPED(status) {
                shell.reap_coproc(r);
            }
        } else {
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
        let mut status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if r > 0 {
            shell.jobs.reap(r, status);
            if !libc::WIFSTOPPED(status) {
                shell.reap_coproc(r);
            }
        } else {
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
                        crate::jobs::JobState::Signaled(s) => {
                            return Some((job.pgid, 128 + s))
                        }
                        _ => {}
                    }
                }
            }
            WaitTarget::Pid(pid) => {
                if let Some(job) = shell.jobs.iter().find(|j| j.pids.contains(pid)) {
                    match job.state {
                        crate::jobs::JobState::Done(c) => return Some((*pid, c)),
                        crate::jobs::JobState::Signaled(s) => {
                            return Some((*pid, 128 + s))
                        }
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

fn handle_kill_l(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> ExecOutcome {
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
                    e!(err, "huck: kill: {arg}: invalid signal specification");
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
                    e!(err, "huck: kill: {arg}: invalid signal specification");
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
        e!(err, "huck: {builtin}: {arg}: bad job spec");
        ExecOutcome::Continue(1)
    })?;
    match shell.jobs.resolve(&spec) {
        Ok(id) => Ok(id),
        Err(crate::jobs::JobSpecResolveError::NotFound) => {
            e!(err, "huck: {builtin}: {arg}: no such job");
            Err(ExecOutcome::Continue(1))
        }
        Err(crate::jobs::JobSpecResolveError::Ambiguous) => {
            e!(err, "huck: {builtin}: {arg}: ambiguous job spec");
            Err(ExecOutcome::Continue(1))
        }
    }
}

fn builtin_kill(args: &[String], out: &mut dyn Write, err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if matches!(args.first().map(|s| s.as_str()), Some("-l")) {
        return handle_kill_l(&args[1..], out, err);
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
                    e!(err, "huck: kill: {rest}: invalid signal number");
                    return ExecOutcome::Continue(1);
                }
                Err(_) => match signal_by_name(rest) {
                    Some(n) => n,
                    None => {
                        e!(err, "huck: kill: {rest}: invalid signal");
                        return ExecOutcome::Continue(1);
                    }
                },
            };
            if args.len() < 2 {
                e!(err, "huck: kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | %job ...");
                return ExecOutcome::Continue(2);
            }
            (sig, &args[1..])
        } else {
            (libc::SIGTERM, args)
        }
    } else {
        e!(err, "huck: kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | %job ...");
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
            e!(err, "huck: kill: -s: option requires an argument");
            return ExecOutcome::Continue(2);
        }
    };
    let sig = match signal_by_name(name) {
        Some(n) => n,
        None => {
            e!(err, "huck: kill: {name}: invalid signal specification");
            return ExecOutcome::Continue(1);
        }
    };
    let targets = &args[1..];
    if targets.is_empty() {
        e!(err, "huck: kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | %job ...");
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
            e!(err, "huck: kill: -n: option requires an argument");
            return ExecOutcome::Continue(2);
        }
    };
    let n = match num_arg.parse::<i32>() {
        Ok(n) if (1..=64).contains(&n) => n,
        _ => {
            e!(err, "huck: kill: {num_arg}: invalid signal specification");
            return ExecOutcome::Continue(1);
        }
    };
    if !crate::traps::killable_signals()
        .iter()
        .any(|(_, num)| *num == n)
    {
        e!(err, "huck: kill: {num_arg}: invalid signal specification");
        return ExecOutcome::Continue(1);
    }
    let targets = &args[1..];
    if targets.is_empty() {
        e!(err, "huck: kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | %job ...");
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
                    e!(err, "huck: kill: {target}: no such job");
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
                e!(err, "huck: kill: ({target}) - {errno}");
                any_failed = true;
            }
        } else {
            match target.parse::<i32>() {
                Ok(pid) if pid > 0 => {
                    let rc = unsafe { libc::kill(pid, sig) };
                    if rc != 0 {
                        let errno = std::io::Error::last_os_error();
                        e!(err, "huck: kill: ({pid}) - {errno}");
                        any_failed = true;
                    }
                }
                _ => {
                    e!(err, "huck: kill: {target}: arguments must be process or job IDs");
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
                        e!(err, "huck: disown: -{c}: invalid option");
                        e!(err, "huck: disown: usage: disown [-ahr] [%job ...]");
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
                    Ok(pid) if pid > 0 => {
                        match shell.jobs.iter().find(|j| j.pids.contains(&pid)) {
                            Some(job) => ids.push(job.id),
                            None => {
                                e!(err, "huck: disown: {arg}: no such job");
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    _ => {
                        e!(err, "huck: disown: {arg}: not a valid job spec");
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
                e!(err, "huck: disown: no current job");
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

fn builtin_fg(args: &[String], err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let id = match args.len() {
        0 => match shell.jobs.current_id() {
            Some(id) => id,
            None => {
                e!(err, "huck: fg: no current job");
                return ExecOutcome::Continue(1);
            }
        },
        1 if args[0].starts_with('%') => match resolve_spec_or_error(&args[0], "fg", err, shell) {
            Ok(id) => id,
            Err(outcome) => return outcome,
        },
        _ => {
            e!(err, "huck: fg: usage: fg [%job]");
            return ExecOutcome::Continue(2);
        }
    };
    let (pgid, pids, command) = {
        if let Some(job) = shell.jobs.jobs_mut().iter_mut().find(|j| j.id == id) {
            job.state = crate::jobs::JobState::Running;
            job.notified = true;
            (job.pgid, job.pids.clone(), job.command.clone())
        } else {
            e!(err, "huck: fg: no current job");
            return ExecOutcome::Continue(1);
        }
    };

    e!(err, "{command}");

    unsafe {
        libc::tcsetpgrp(libc::STDIN_FILENO, pgid);
        libc::killpg(pgid, libc::SIGCONT);
    }

    let mut last_status = 0;
    let mut stopped_sig: Option<i32> = None;
    let mut completed = 0;
    let total = pids.len();
    loop {
        if completed == total { break; }
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

    unsafe { libc::tcsetpgrp(libc::STDIN_FILENO, shell.shell_pgid); }

    if let Some(sig) = stopped_sig {
        if let Some(job) = shell.jobs.jobs_mut().iter_mut().find(|j| j.id == id) {
            job.state = crate::jobs::JobState::Stopped(sig);
            job.notified = true;
        }
        let line = shell.jobs.iter()
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

fn builtin_bg(args: &[String], _out: &mut dyn std::io::Write, err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let id = match args.len() {
        0 => match shell.jobs.current_stopped_id() {
            Some(id) => id,
            None => {
                e!(err, "huck: bg: no current job");
                return ExecOutcome::Continue(1);
            }
        },
        1 if args[0].starts_with('%') => {
            let id = match resolve_spec_or_error(&args[0], "bg", err, shell) {
                Ok(id) => id,
                Err(outcome) => return outcome,
            };
            // Verify the resolved job is actually Stopped.
            let is_stopped = shell.jobs.iter()
                .find(|j| j.id == id)
                .map(|j| matches!(j.state, crate::jobs::JobState::Stopped(_)))
                .unwrap_or(false);
            if !is_stopped {
                e!(err, "huck: bg: job %{id} already running");
                return ExecOutcome::Continue(1);
            }
            id
        }
        _ => {
            e!(err, "huck: bg: usage: bg [%job]");
            return ExecOutcome::Continue(2);
        }
    };
    let (pgid, command) = {
        if let Some(job) = shell.jobs.jobs_mut().iter_mut().find(|j| j.id == id) {
            job.state = crate::jobs::JobState::Running;
            job.notified = true;
            (job.pgid, job.command.clone())
        } else {
            e!(err, "huck: bg: no current job");
            return ExecOutcome::Continue(1);
        }
    };

    unsafe { libc::killpg(pgid, libc::SIGCONT); }

    e!(err, "[{id}]+ {command} &");
    ExecOutcome::Continue(0)
}

fn builtin_history(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    match args.first().map(|s| s.as_str()) {
        None => {
            for (number, command) in shell.history.entries() {
                if writeln!(out, "{number:>5}\t{command}").is_err() {
                    return ExecOutcome::Continue(1);
                }
            }
            ExecOutcome::Continue(0)
        }
        Some("-c") => {
            Rc::make_mut(&mut shell.history).clear();
            ExecOutcome::Continue(0)
        }
        Some(other) => {
            e!(err, "huck: history: {other}: invalid option");
            ExecOutcome::Continue(1)
        }
    }
}

fn builtin_trap(args: &[String], out: &mut dyn Write, err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    use crate::traps::{TrapSignal, install, reset, parse_trap_signal};

    // No args: same as `trap -p`.
    if args.is_empty() {
        print_active_traps(out, shell, None);
        return ExecOutcome::Continue(0);
    }

    // -l: list signal name/number pairs.
    if args[0] == "-l" {
        if args.len() != 1 {
            e!(err, "huck: trap: -l takes no arguments");
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
                    e!(err, "huck: trap: {msg}");
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
            e!(err, "huck: trap: usage: trap [-lp] [[arg] signal_spec ...]");
            return ExecOutcome::Continue(1);
        }
        for name in &args[1..] {
            let sig = match parse_trap_signal(name) {
                Ok(s) => s,
                Err(msg) => {
                    e!(err, "huck: trap: {msg}");
                    return ExecOutcome::Continue(1);
                }
            };
            if let Err(msg) = reset(shell, sig) {
                e!(err, "huck: trap: {msg}");
                return ExecOutcome::Continue(1);
            }
        }
        return ExecOutcome::Continue(0);
    }

    // `trap ACTION SIGNAL...`: install action for each signal.
    if args.len() < 2 {
        e!(err, "huck: trap: usage: trap [-lp] [[arg] signal_spec ...]");
        return ExecOutcome::Continue(1);
    }
    let action_text = args[0].clone();
    let action = if action_text.is_empty() {
        None  // empty string → ignore
    } else {
        Some(action_text)
    };
    for name in &args[1..] {
        let sig = match parse_trap_signal(name) {
            Ok(s) => s,
            Err(msg) => {
                e!(err, "huck: trap: {msg}");
                return ExecOutcome::Continue(1);
            }
        };
        if let Err(msg) = install(shell, sig, action.clone()) {
            e!(err, "huck: trap: {msg}");
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
    crate::traps::killable_signals().iter().find_map(|(name, n)| {
        if *n == signum { Some(name.to_string()) } else { None }
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
        name: "?".to_string(), optarg: None, optind, sp: 1, error: None, done: true,
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
        if word_done { next_optind += 1; sp = 1; }
        return GetoptsStep {
            name: "?".to_string(),
            optarg: if silent { Some(c.to_string()) } else { None },
            optind: next_optind,
            sp,
            error: if silent { None } else { Some(format!("illegal option -- {c}")) },
            done: false,
        };
    }

    if takes_arg {
        if !word_done {
            // Attached arg: rest of the word.
            let arg: String = word[(sp - 1)..].iter().collect();
            return GetoptsStep {
                name: c.to_string(), optarg: Some(arg),
                optind: optind + 1, sp: 1, error: None, done: false,
            };
        }
        if optind < args.len() {
            // Separate arg: the next word.
            return GetoptsStep {
                name: c.to_string(), optarg: Some(args[optind].clone()),
                optind: optind + 2, sp: 1, error: None, done: false,
            };
        }
        // Missing argument.
        return GetoptsStep {
            name: if silent { ":".to_string() } else { "?".to_string() },
            optarg: if silent { Some(c.to_string()) } else { None },
            optind: optind + 1, sp: 1,
            error: if silent { None } else { Some(format!("option requires an argument -- {c}")) },
            done: false,
        };
    }

    // Plain valid option, no argument.
    let mut next_optind = optind;
    if word_done { next_optind += 1; sp = 1; }
    GetoptsStep {
        name: c.to_string(), optarg: None, optind: next_optind, sp, error: None, done: false,
    }
}

/// True if `c` appears as an option letter in `optstring` (ignoring a leading
/// ':' silent flag and the ':' arg-markers that follow letters).
fn optstring_has(optstring: &str, c: char) -> bool {
    let mut chars = optstring.chars().peekable();
    if chars.peek() == Some(&':') { chars.next(); }
    for o in chars {
        if o == ':' { continue; } // arg-marker for the previous letter
        if o == c { return true; }
    }
    false
}

/// True if option letter `c` is immediately followed by ':' in `optstring`
/// (i.e. it takes an argument).
fn optstring_takes_arg(optstring: &str, c: char) -> bool {
    let mut chars = optstring.chars().peekable();
    if chars.peek() == Some(&':') { chars.next(); }
    while let Some(o) = chars.next() {
        if o == ':' { continue; }
        if o == c { return chars.peek() == Some(&':'); }
    }
    false
}

/// `getopts optstring name [arg ...]` — POSIX option parser (M-106). Reads/
/// writes OPTIND/OPTARG/OPTERR + the matched-letter `name`, holding the
/// within-word cursor in Shell. Delegates the state machine to `getopts_step`.
fn builtin_getopts(args: &[String], err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if args.len() < 2 {
        e!(err, "huck: getopts: usage: getopts optstring name [arg]");
        return ExecOutcome::Continue(2);
    }
    let optstring = args[0].clone();
    let name = args[1].clone();
    if !is_valid_name(&name) {
        e!(err, "huck: getopts: `{name}': not a valid identifier");
        return ExecOutcome::Continue(2);
    }
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
    let sp = if optind != shell.getopts_optind_cache { 1 } else { shell.getopts_sp };

    let step = getopts_step(&optstring, &parse_args, optind, sp);

    // Write back OPTIND + cursor cache.
    shell.set("OPTIND", step.optind.to_string());
    shell.getopts_optind_cache = step.optind;
    shell.getopts_sp = step.sp;
    // Assign the matched letter (or '?' / ':').
    let _ = shell.try_set(&name, step.name.clone());
    // OPTARG: set or unset.
    match step.optarg {
        Some(v) => { let _ = shell.try_set("OPTARG", v); }
        None => shell.unset("OPTARG"),
    }
    // Verbose error message (suppressed by OPTERR=0).
    if let Some(body) = step.error
        && shell.lookup_var("OPTERR").as_deref() != Some("0")
    {
        e!(err, "huck: {body}");
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
                e!(err, "huck: shift: {s}: numeric argument required");
                return ExecOutcome::Continue(1);
            }
        },
    };
    if n < 0 {
        e!(err, "huck: shift: {n}: shift count out of range");
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

/// bash 5.2's full `set -o` option table, in bash's display order.
/// `errexit`/`nounset`/`pipefail` are implemented (real state via
/// `Shell.shell_options`); the rest are recognized for listing + querying
/// only (their `default` is reported) and cannot be enabled.
const SETO_TABLE: &[OptionInfo] = &[
    OptionInfo { name: "allexport", default: false },
    OptionInfo { name: "braceexpand", default: true },
    OptionInfo { name: "emacs", default: false },
    OptionInfo { name: "errexit", default: false },
    OptionInfo { name: "errtrace", default: false },
    OptionInfo { name: "functrace", default: false },
    OptionInfo { name: "hashall", default: true },
    OptionInfo { name: "histexpand", default: false },
    OptionInfo { name: "history", default: false },
    OptionInfo { name: "ignoreeof", default: false },
    OptionInfo { name: "interactive-comments", default: true },
    OptionInfo { name: "keyword", default: false },
    OptionInfo { name: "monitor", default: false },
    OptionInfo { name: "noclobber", default: false },
    OptionInfo { name: "noexec", default: false },
    OptionInfo { name: "noglob", default: false },
    OptionInfo { name: "nolog", default: false },
    OptionInfo { name: "notify", default: false },
    OptionInfo { name: "nounset", default: false },
    OptionInfo { name: "onecmd", default: false },
    OptionInfo { name: "physical", default: false },
    OptionInfo { name: "pipefail", default: false },
    OptionInfo { name: "posix", default: false },
    OptionInfo { name: "privileged", default: false },
    OptionInfo { name: "verbose", default: false },
    OptionInfo { name: "vi", default: false },
    OptionInfo { name: "xtrace", default: false },
];

/// Error from `option_set` for a non-settable `set -o` name.
/// `Debug` is required because an existing test calls `option_set(...).unwrap()`.
#[derive(Debug)]
enum OptSetErr {
    /// Known bash option huck does not implement (e.g. `xtrace`, `posix`).
    Unimplemented,
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
        other => SETO_TABLE.iter().find(|o| o.name == other).map(|o| o.default),
    }
}

/// Writes a `set -o` option. Only the behaviorally-implemented options are
/// settable; the rest of `SETO_TABLE` is inert (`Unimplemented`).
fn option_set(shell: &mut Shell, name: &str, value: bool) -> Result<(), OptSetErr> {
    match name {
        "errexit" => { shell.shell_options.errexit = value; Ok(()) }
        "nounset" => { shell.shell_options.nounset = value; Ok(()) }
        "pipefail" => { shell.shell_options.pipefail = value; Ok(()) }
        "verbose" => { shell.shell_options.verbose = value; Ok(()) }
        "xtrace" => { shell.shell_options.xtrace = value; Ok(()) }
        "noglob" => { shell.shell_options.noglob = value; Ok(()) }
        "noclobber" => { shell.shell_options.noclobber = value; Ok(()) }
        "noexec" => { shell.shell_options.noexec = value; Ok(()) }
        "physical" => { shell.shell_options.physical = value; Ok(()) }
        "posix" => {
            // Accept as a silent no-op. huck is POSIX-respecting by default;
            // `set +o posix` is a no-op against that default, and `set -o
            // posix` does not unlock additional strict-POSIX semantics.
            // Scripts that toggle the option for bash compatibility pass
            // through cleanly. The "huck doesn't implement strict POSIX
            // mode" gap is a known minor divergence.
            let _ = value;
            Ok(())
        }
        other => {
            if SETO_TABLE.iter().any(|o| o.name == other) {
                Err(OptSetErr::Unimplemented)
            } else {
                Err(OptSetErr::Unknown)
            }
        }
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

fn builtin_set(args: &[String], out: &mut dyn Write, err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if crate::restricted::is_restricted(shell)
        && args.iter().any(|a| a == "+r")
        && let Err(msg) = crate::restricted::check_set_plus_r()
    {
        e!(err, "{msg}");
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
                Err(OptSetErr::Unimplemented) => {
                    e!(err, "huck: set: {}: not yet supported in this version", args[i]);
                    return ExecOutcome::Continue(2);
                }
                Err(OptSetErr::Unknown) => {
                    e!(err, "huck: set: -o: invalid option name: {}", args[i]);
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
                Err(OptSetErr::Unimplemented) => {
                    e!(err, "huck: set: {}: not yet supported in this version", args[i]);
                    return ExecOutcome::Continue(2);
                }
                Err(OptSetErr::Unknown) => {
                    e!(err, "huck: set: +o: invalid option name: {}", args[i]);
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
                    b'o' => {
                        i += 1;
                        if i >= args.len() {
                            return print_options_table(out, shell);
                        }
                        match option_set(shell, &args[i], true) {
                            Ok(()) => {}
                            Err(OptSetErr::Unimplemented) => {
                                e!(err,
                                    "huck: set: {}: not yet supported in this version",
                                    args[i]
                                );
                                return ExecOutcome::Continue(2);
                            }
                            Err(OptSetErr::Unknown) => {
                                e!(err,
                                    "huck: set: -o: invalid option name: {}",
                                    args[i]
                                );
                                return ExecOutcome::Continue(2);
                            }
                        }
                    }
                    other => {
                        e!(err,
                            "huck: set: -{}: not yet supported in this version",
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
                    b'o' => {
                        i += 1;
                        if i >= args.len() {
                            return print_options_reinput(out, shell);
                        }
                        match option_set(shell, &args[i], false) {
                            Ok(()) => {}
                            Err(OptSetErr::Unimplemented) => {
                                e!(err,
                                    "huck: set: {}: not yet supported in this version",
                                    args[i]
                                );
                                return ExecOutcome::Continue(2);
                            }
                            Err(OptSetErr::Unknown) => {
                                e!(err,
                                    "huck: set: +o: invalid option name: {}",
                                    args[i]
                                );
                                return ExecOutcome::Continue(2);
                            }
                        }
                    }
                    other => {
                        e!(err,
                            "huck: set: +{}: not yet supported in this version",
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
fn builtin_shopt(args: &[String], out: &mut dyn Write, err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let (mut set_f, mut unset_f, mut quiet, mut print_f, mut o_bridge) =
        (false, false, false, false, false);
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" { i += 1; break; }
        if a.len() >= 2 && a.starts_with('-') {
            for c in a[1..].chars() {
                match c {
                    's' => set_f = true,
                    'u' => unset_f = true,
                    'q' => quiet = true,
                    'p' => print_f = true,
                    'o' => o_bridge = true,
                    _ => {
                        e!(err, "huck: shopt: -{c}: invalid option");
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
        e!(err, "huck: shopt: cannot set and unset shell options simultaneously");
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
            if set_f && !on { continue; }
            if unset_f && on { continue; }
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
                e!(err, "huck: shopt: {name}: invalid shell option name");
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
                if !on { all_set = false; }
                if !quiet {
                    if print_f {
                        let _ = writeln!(out, "shopt -{} {}", if on { 's' } else { 'u' }, name);
                    } else {
                        let _ = writeln!(out, "{}", fmt_opt_line(name, on));
                    }
                }
            }
            None => {
                e!(err, "huck: shopt: {name}: invalid shell option name");
                all_set = false;
            }
        }
    }
    ExecOutcome::Continue(if all_set { 0 } else { 1 })
}

/// The `-o` bridge: every `shopt` form operates on the `set -o` namespace.
#[allow(clippy::too_many_arguments)]
fn shopt_o_bridge(
    names: &[String], set_f: bool, unset_f: bool, quiet: bool, print_f: bool,
    out: &mut dyn Write, err: &mut dyn Write, shell: &mut Shell,
) -> ExecOutcome {
    if names.is_empty() {
        if quiet {
            // No names → vacuously "all set" (matches bash 5.2).
            return ExecOutcome::Continue(0);
        }
        for opt in SETO_TABLE {
            let on = option_get(shell, opt.name).unwrap_or(opt.default);
            if set_f && !on { continue; }
            if unset_f && on { continue; }
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
                Err(OptSetErr::Unimplemented) => {
                    e!(err, "huck: shopt: {name}: not yet supported in this version");
                    rc = 1;
                }
                Err(OptSetErr::Unknown) => {
                    e!(err, "huck: shopt: {name}: invalid shell option name");
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
                if !on { all_set = false; }
                if !quiet {
                    if print_f {
                        let _ = writeln!(out, "set {}o {}", if on { '-' } else { '+' }, name);
                    } else {
                        let _ = writeln!(out, "{}", fmt_opt_line(name, on));
                    }
                }
            }
            None => {
                e!(err, "huck: shopt: {name}: invalid shell option name");
                all_set = false;
            }
        }
    }
    ExecOutcome::Continue(if all_set { 0 } else { 1 })
}

fn set_escape_value(v: &str) -> String {
    format!("'{}'", v.replace('\'', r#"'\''"#))
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
    let saved = shell.xtrace_depth;
    shell.xtrace_depth += 1;
    let r = crate::shell::process_line_in_sinks(&joined, shell, true, sink, err_sink);
    shell.xtrace_depth = saved;
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
        e!(err, "huck: let: expression expected");
        return ExecOutcome::Continue(1);
    }
    let mut last: i64 = 0;
    for a in args {
        match crate::arith::parse(a).and_then(|e| crate::arith::eval(&e, shell)) {
            Ok(v) => last = v,
            Err(e) => {
                let prefix = shell.error_prefix(Some("let"));
                e!(err, "{prefix}{}", crate::arith::render_error_body(a, &e));
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
        synopsis: "bg [JOB_SPEC ...]",
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
        synopsis: "disown [-ahr] [JOB_SPEC ...]",
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
        synopsis: "fg [JOB_SPEC]",
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
        synopsis: "wait [-n] [-p VAR] [PID|JOB_SPEC ...]",
        description: "Wait for processes to complete.\n\
                      With no args, wait for all known jobs. With PID/JOB_SPEC, wait for\n\
                      each. -n waits for any one to finish (returns its status). -p VAR\n\
                      stores the finishing job's PID in VAR.",
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
    _shell: &mut Shell,
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
                    e!(err, "huck: help: -{}: invalid option", other as char);
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
            Some(entry) => emit_help_entry(
                entry,
                out,
                want_synopsis,
                want_description,
                want_man,
            ),
            None => {
                e!(err, "huck: help: no help topics match `{name}'");
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
        e!(&mut *err, "{msg}");
        return ExecOutcome::Continue(1);
    }
    // Materialize a fallback err writer for the early-bail diagnostics that don't
    // recurse into the executor.
    {
        let mut err = crate::executor::err_writer(err_sink, sink);
        if args.is_empty() {
            e!(&mut *err, "huck: .: usage: . filename [arguments]");
            return ExecOutcome::Continue(2);
        }
        if shell.source_depth >= 64 {
            e!(&mut *err, "huck: .: maximum source depth (64) exceeded");
            return ExecOutcome::Continue(1);
        }
    }
    let filename = &args[0];
    let path = match resolve_source_path(filename, shell) {
        Some(p) => p,
        None => {
            { let mut err = crate::executor::err_writer(err_sink, sink); e!(&mut *err, "huck: .: {filename}: file not found"); }
            return ExecOutcome::Continue(1);
        }
    };
    let contents = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            { let mut errw = crate::executor::err_writer(err_sink, sink); e!(&mut *errw, "huck: .: {}: {e}", path.display()); }
            return ExecOutcome::Continue(1);
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
    use std::path::PathBuf;
    if filename.contains('/') {
        let p = PathBuf::from(filename);
        return if p.is_file() { Some(p) } else { None };
    }
    let path_var = shell.lookup_var("PATH").unwrap_or_default();
    for dir in path_var.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = PathBuf::from(dir).join(filename);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// A parse error that only signals "the input ended mid-compound". When the
/// source chunk was truncated by a lex error, such an error means the trailing
/// unit was cut off (not genuinely malformed), so it should be re-lexed rather
/// than reported.
fn is_unterminated(e: &crate::command::ParseError) -> bool {
    use crate::command::ParseError::*;
    matches!(
        e,
        UnterminatedFunction
            | UnterminatedLoop
            | UnterminatedIf
            | UnterminatedCase
            | UnterminatedBrace
            | UnterminatedSubshell
            | UnterminatedDoubleBracket
    )
}

pub(crate) fn run_sourced_contents_in_sinks(
    contents: &str,
    path: &std::path::Path,
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
    // Byte offset of the START of the line containing `abs`. Used to resume after
    // a lex truncation whose failing construct produced no token (e.g. an array
    // literal `a=($(…!(x)…))`): the failure byte sits mid-construct, so the clean
    // re-lex boundary is the start of the failing line, not the failure byte.
    let line_start_of = |abs: usize| -> usize {
        let a = abs.min(contents.len());
        contents[..a].rfind('\n').map(|i| i + 1).unwrap_or(0)
    };

    let mut start = 0usize; // byte offset of the unconsumed remainder
    let mut prev_end = 0usize; // bytes already echoed for `set -v`

    'outer: loop {
        if start >= contents.len() {
            break;
        }
        let extglob = shell.shopt_options.get("extglob").unwrap_or(false);
        // Partial tokenize: keep the tokens produced BEFORE any lex error so the
        // complete units (e.g. an earlier `shopt -s extglob`) can run first; the
        // truncated trailing unit is re-lexed with the now-current extglob.
        let (tokens, offsets, lex_lines, terr) = crate::lexer::tokenize_partial(
            &contents[start..],
            crate::lexer::LexerOptions { extglob },
        );
        let total = tokens.len();
        if total == 0 {
            if let Some((le, foff)) = terr {
                {
                    let mut err = crate::executor::err_writer(err_sink, sink);
                    e!(&mut *err,
                        "huck: {}: line {}: syntax error{}",
                        path.display(),
                        line_of(start + foff),
                        crate::lex_error_message(&le)
                    );
                }
                last_status = 2;
                start = next_line_start(start + foff);
                prev_end = start;
                continue 'outer;
            }
            break;
        }
        // The lexer's line numbers are 1-based relative to &contents[start..].
        // Add the base line offset (number of newlines before `start` in the file)
        // so each token line reflects its true position in the file.
        // lex_lines.len() == total + 1 (includes sentinel); slice to total.
        let base_line = contents.as_bytes()[..start].iter().filter(|&&b| b == b'\n').count() as u32;
        let token_lines: Vec<u32> = lex_lines[..total].iter().map(|&l| l + base_line).collect();
        let mut iter = crate::command::TokenCursor::new(tokens, token_lines);

        loop {
            while matches!(iter.peek(), Some(crate::lexer::Token::Newline)) {
                iter.next();
            }
            let unit_start_idx = total - iter.len();
            if iter.peek().is_none() {
                // Consumed every complete token. If the chunk truncated at a lex
                // error, the un-lexed tail begins at `prev_end` (the end of the
                // last executed unit). Re-lex that tail ONLY if a command in this
                // chunk flipped extglob — otherwise re-lexing would fail
                // identically (an infinite loop), so report the error instead.
                if let Some((le, foff)) = &terr {
                    let now_extglob = shell.shopt_options.get("extglob").unwrap_or(false);
                    // Resume from the start of the failing line (a clean boundary)
                    // rather than `prev_end`, which may be the mid-construct lex
                    // failure byte when the failing construct produced no token.
                    let resume = line_start_of(start + *foff);
                    if now_extglob != extglob && resume > start {
                        start = resume;
                        prev_end = start;
                        continue 'outer;
                    }
                    {
                        let mut err = crate::executor::err_writer(err_sink, sink);
                        e!(&mut *err,
                            "huck: {}: line {}: syntax error{}",
                            path.display(),
                            line_of(start + foff),
                            crate::lex_error_message(le)
                        );
                    }
                    last_status = 2;
                    start = next_line_start(start + foff);
                    prev_end = start;
                    continue 'outer;
                }
                break 'outer;
            }
            match crate::command::parse_one_unit(&mut iter) {
                Ok(None) => {
                    break 'outer;
                }
                Ok(Some(seq)) => {
                    let unit_end_idx = total - iter.len();
                    let unit_start_abs = start + offsets[unit_start_idx];
                    let unit_end_abs = start + offsets[unit_end_idx];

                    if shell.shell_options.verbose {
                        let mut err = crate::executor::err_writer(err_sink, sink);
                        let _ = write!(&mut *err, "{}", &contents[prev_end..unit_end_abs]);
                    }
                    prev_end = unit_end_abs;

                    let span = &contents[unit_start_abs..unit_end_abs];
                    let outcome = crate::executor::execute_with_sink(&seq, shell, span, sink, err_sink);

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
                                && let Some(st) = shell.take_pending_fatal_pe_error()
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
                        ExecOutcome::Interrupted(r) => return ExecOutcome::Interrupted(r),
                    }

                    // A command may have flipped `shopt extglob`, which changes
                    // how the remainder must be lexed. Re-lex from here.
                    let new_extglob = shell.shopt_options.get("extglob").unwrap_or(false);
                    if new_extglob != extglob {
                        // If this flipping unit was the last complete token before
                        // a lex truncation, the un-lexed tail begins at the start
                        // of the failing line — `offsets[total]` is the
                        // mid-construct failure byte, not a clean boundary.
                        start = match &terr {
                            Some((_, foff)) if unit_end_idx == total => {
                                line_start_of(start + *foff)
                            }
                            _ => unit_end_abs,
                        };
                        prev_end = start;
                        continue 'outer;
                    }
                }
                Err(e) if terr.is_some() && is_unterminated(&e) => {
                    // The trailing unit parsed as "unterminated" only because the
                    // chunk was truncated by a lex error. Re-lex this unit from
                    // its start ONLY if a command earlier in this chunk flipped
                    // extglob (so an earlier `shopt -s extglob` now applies);
                    // otherwise re-lexing fails identically (an infinite loop), so
                    // report the lex error instead. The `> start` guard also
                    // covers the first-unit case (nothing ran, prefix offset 0).
                    let now_extglob = shell.shopt_options.get("extglob").unwrap_or(false);
                    let resume = start + offsets[unit_start_idx];
                    if now_extglob != extglob && resume > start {
                        start = resume;
                        prev_end = start;
                        continue 'outer;
                    }
                    let (le, foff) = terr.clone().unwrap();
                    {
                        let mut err = crate::executor::err_writer(err_sink, sink);
                        e!(&mut *err,
                            "huck: {}: line {}: syntax error{}",
                            path.display(),
                            line_of(start + foff),
                            crate::lex_error_message(&le)
                        );
                    }
                    last_status = 2;
                    start = next_line_start(start + foff);
                    prev_end = start;
                    continue 'outer;
                }
                Err(e) => {
                    {
                        let mut err = crate::executor::err_writer(err_sink, sink);
                        e!(&mut *err,
                            "huck: {}: line {}: syntax error: {}",
                            path.display(),
                            line_of(start + offsets[unit_start_idx]),
                            crate::parse_error_message(&e)
                        );
                    }
                    last_status = 2;
                    for t in iter.by_ref() {
                        if matches!(t, crate::lexer::Token::Newline) {
                            break;
                        }
                    }
                    prev_end = start + offsets[total - iter.len()];
                }
            }
            // When the chunk lexed cleanly, exiting here once the tokens run out
            // is correct. But if `terr` is Some, the chunk was truncated by a lex
            // error: loop back to the top so the iter-empty truncation branch can
            // re-lex the tail (if extglob flipped) or report the lex error.
            if iter.peek().is_none() && terr.is_none() {
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
        && s.chars().all(|c| !c.is_whitespace() && !"|&;<>()$`\\\"'*?[]#~{}".contains(c))
}

pub(crate) fn escape_alias_value(v: &str) -> String {
    // Bash format: alias name='value' with single quotes inside
    // the value rewritten as '\''.
    v.replace('\'', r#"'\''"#)
}

fn builtin_alias(args: &[String], out: &mut dyn Write, err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
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
                e!(err, "huck: alias: `{name}': invalid alias name");
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
                    e!(err, "huck: alias: {arg}: not found");
                    any_failed = true;
                }
            }
        }
    }
    ExecOutcome::Continue(if any_failed { 1 } else { 0 })
}

fn builtin_unalias(args: &[String], err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if args.is_empty() {
        e!(err, "huck: unalias: usage: unalias [-a] name [name ...]");
        return ExecOutcome::Continue(2);
    }
    if args[0] == "-a" {
        shell.aliases.clear();
        return ExecOutcome::Continue(0);
    }
    let mut any_failed = false;
    for name in args {
        if shell.aliases.remove(name).is_none() {
            e!(err, "huck: unalias: {name}: not found");
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
        "if" | "then" | "elif" | "else" | "fi"
        | "while" | "until" | "do" | "done"
        | "for" | "in" | "select"
        | "case" | "esac"
        | "function"
        | "!"
        | "{" | "}"
        | "[[" | "]]"
    )
}

fn is_executable_file(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(p) {
        Ok(md) => md.is_file() && (md.permissions().mode() & 0o111 != 0),
        Err(_) => false,
    }
}

pub(crate) fn search_path_for(name: &str, shell: &Shell) -> Option<std::path::PathBuf> {
    if name.contains('/') {
        let p = std::path::PathBuf::from(name);
        if is_executable_file(&p) { Some(p) } else { None }
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
    if is_builtin(name) {
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
        return if is_executable_file(&p) { vec![p] } else { vec![] };
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
fn resolve_command_name_with(
    name: &str,
    shell: &Shell,
    skip_func: bool,
) -> CommandResolution {
    if let Some(v) = shell.aliases.get(name) {
        return CommandResolution::Alias(v.clone());
    }
    if !skip_func && shell.functions.contains_key(name) {
        return CommandResolution::Function;
    }
    if is_builtin(name) {
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
fn resolve_command_name_all(
    name: &str,
    shell: &Shell,
    skip_func: bool,
) -> Vec<CommandResolution> {
    let mut out: Vec<CommandResolution> = Vec::new();
    if let Some(v) = shell.aliases.get(name) {
        out.push(CommandResolution::Alias(v.clone()));
    }
    if !skip_func && shell.functions.contains_key(name) {
        out.push(CommandResolution::Function);
    }
    if is_builtin(name) {
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
                    e!(err, "huck: type: -{}: invalid option", other as char);
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
                e!(err, "huck: type: {name}: not found");
            }
            exit = 1;
            continue;
        }
        for res in &resolutions {
            emit_type_entry(name, res, type_only, path_only, out);
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
                        explicit_path = Some(
                            String::from_utf8_lossy(&bytes[j + 1..]).into_owned(),
                        );
                        break;
                    } else {
                        // -p separate: next arg
                        i += 1;
                        if i >= args.len() {
                            e!(err, "huck: hash: -p: option requires an argument");
                            return ExecOutcome::Continue(2);
                        }
                        explicit_path = Some(args[i].clone());
                        break;
                    }
                }
                c => {
                    e!(err, "huck: hash: -{}: invalid option", c as char);
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
            e!(err, "huck: hash: -d: at least one name required");
            return ExecOutcome::Continue(2);
        }
        let mut exit: i32 = 0;
        let h = Rc::make_mut(&mut shell.command_hash);
        for name in names {
            if h.remove(name).is_none() {
                e!(err, "huck: hash: {name}: not found");
                exit = 1;
            }
        }
        return ExecOutcome::Continue(exit);
    }

    if set_path {
        // Exactly one name required.
        if names.len() != 1 {
            e!(err, "huck: hash: -p: exactly one name required");
            return ExecOutcome::Continue(2);
        }
        let name = &names[0];
        if name.contains('/') {
            e!(err, "huck: hash: {name}: must not contain `/'");
            return ExecOutcome::Continue(1);
        }
        let path = explicit_path.unwrap(); // safe: set_path implies Some
        Rc::make_mut(&mut shell.command_hash).insert(
            name.clone(),
            (std::path::PathBuf::from(path), 0u32),
        );
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
            e!(err, "huck: hash: -t: at least one name required");
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
                    e!(err, "huck: hash: {name}: not found");
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
            e!(err, "huck: hash: {name}: must not contain `/'");
            exit = 1;
            continue;
        }
        match search_path_for(name, shell) {
            Some(path) => {
                Rc::make_mut(&mut shell.command_hash).insert(name.clone(), (path, 0u32));
            }
            None => {
                e!(err, "huck: hash: {name}: not found");
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
            "-v" => { concise = true; i += 1; }
            "-V" => { verbose = true; i += 1; }
            "-p" => { i += 1; } // accept; introspection uses current $PATH
            "--" => { i += 1; break; }
            s if s.starts_with('-') && s.len() > 1 => {
                e!(err, "huck: command: {s}: invalid option");
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
        e!(err,
            "huck: command: bare form (without -v/-V) is not supported in this version"
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
                    e!(err, "huck: command: {name}: not found");
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
                e!(err, "huck: [: missing ']'");
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
            e!(err, "huck: {name}: {msg}");
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
    let n: usize = digits
        .parse()
        .map_err(|_| format!("{s}: invalid number"))?;
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
    (s.starts_with('+') || s.starts_with('-'))
        && s.len() > 1
        && s.as_bytes()[1].is_ascii_digit()
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
            e!(err, "huck: pushd: no other directory");
            return ExecOutcome::Continue(1);
        }
        shell.dir_stack.swap(0, 1);
        let target = shell.dir_stack[0].clone();
        let cd_args = vec![target.display().to_string()];
        if let ExecOutcome::Continue(c) = builtin_cd(&cd_args, out, err, shell)
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
                e!(err, "huck: pushd: {e}");
                return ExecOutcome::Continue(1);
            }
        };
        if idx == 0 {
            return print_stack(out, shell, true, false, false);
        }
        shell.dir_stack.rotate_left(idx);
        let target = shell.dir_stack[0].clone();
        let cd_args = vec![target.display().to_string()];
        if let ExecOutcome::Continue(c) = builtin_cd(&cd_args, out, err, shell)
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
    if let ExecOutcome::Continue(c) = builtin_cd(&cd_args, out, err, shell)
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
        e!(err, "huck: popd: directory stack empty");
        return ExecOutcome::Continue(1);
    }

    let idx = if args.is_empty() {
        0
    } else {
        let arg = &args[0];
        if !is_signed_index_arg(arg) {
            e!(err, "huck: popd: {arg}: invalid argument");
            return ExecOutcome::Continue(1);
        }
        match parse_signed_index(arg, shell.dir_stack.len()) {
            Ok(i) => i,
            Err(e) => {
                e!(err, "huck: popd: {e}");
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
        if let ExecOutcome::Continue(c) = builtin_cd(&cd_args, out, err, shell)
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
                        e!(err, "huck: dirs: {e}");
                        return ExecOutcome::Continue(1);
                    }
                }
                i += 1;
            }
            s if s.starts_with('-') && s.len() > 1 => {
                e!(err, "huck: dirs: {s}: invalid option");
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

fn builtin_bind(args: &[String], out: &mut dyn Write, err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    use crate::readline_bind::{is_known_function, keyseq_is_valid, readline_function_names};
    const USAGE: &str = "bind: usage: bind [-lpsvPSVX] [-m keymap] [-f filename] [-q name] [-u name] [-r keyseq] [-x keyseq:shell-command] [keyseq:readline-function or readline-command]";

    let mut i = 0;
    let mut rc = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-v" => { for l in shell.readline_var_lines() { let _ = writeln!(out, "{l}"); } }
            "-V" => { for l in shell.readline_var_lines_verbose() { let _ = writeln!(out, "{l}"); } }
            "-l" => { for f in readline_function_names() { let _ = writeln!(out, "{f}"); } }
            "-p" => { for l in shell.active_bind_lines() { let _ = writeln!(out, "{l}"); } }
            "-P" => { for l in shell.active_bind_lines_verbose() { let _ = writeln!(out, "{l}"); } }
            "-s" | "-S" | "-X" => { /* no macros / shell-command bindings: empty */ }
            "-m" | "-q" | "-u" | "-f" => { i += 1; /* takes an arg; accept + no-op */ }
            "-r" => {
                i += 1;
                if let Some(seq) = args.get(i) {
                    shell.add_unbind(seq);
                } else {
                    e!(err, "huck: bind: -r: option requires an argument");
                    rc = 2;
                }
            }
            "-x" => { i += 1; /* keyseq:shell-command — deferred no-op */ }
            s if s.starts_with('-') && s.len() > 1 => {
                e!(err, "huck: bind: {s}: invalid option");
                e!(err, "huck: {USAGE}");
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
                            e!(err, "huck: bind: {val}: invalid value for {var}");
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
                            e!(err, "huck: bind: {val}: invalid value for {var}");
                            rc = 1;
                        } else {
                            shell.set_readline_var(var, val);
                        }
                    }
                } else if let Some((seq, func)) = a.split_once(':') {
                    if !keyseq_is_valid(seq) {
                        e!(err, "huck: bind: {seq}: cannot parse key sequence");
                        rc = 1;
                    } else if !is_known_function(func) {
                        e!(err, "huck: bind: {func}: unknown function name");
                        rc = 1;
                    } else {
                        shell.add_bind(seq, func);
                    }
                } else {
                    e!(err, "huck: bind: {a}: unknown command");
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
mod tests {
    use super::*;

    #[test]
    fn declare_scalar_quote_matches_bash_listing() {
        // bash bare-declare / set -x style minimal quoting (verified vs bash 5.x)
        assert_eq!(declare_scalar_quote("hello"), "hello");
        assert_eq!(declare_scalar_quote(""), "");            // empty -> bare (name=)
        assert_eq!(declare_scalar_quote("a b"), "'a b'");
        assert_eq!(declare_scalar_quote("x;y"), "'x;y'");
        assert_eq!(declare_scalar_quote("gl*ob"), "'gl*ob'");
        assert_eq!(declare_scalar_quote("d$ollar"), "'d$ollar'");
        assert_eq!(declare_scalar_quote("bang!x"), "'bang!x'");
        assert_eq!(declare_scalar_quote("lt<gt>"), "'lt<gt>'");
        assert_eq!(declare_scalar_quote("br[ack]"), "'br[ack]'");
        assert_eq!(declare_scalar_quote("back`tick"), "'back`tick'");
        assert_eq!(declare_scalar_quote("qu'ote"), "'qu'\\''ote'");
        // not metacharacters in this context -> stay bare
        assert_eq!(declare_scalar_quote("ti~lde"), "ti~lde");
        assert_eq!(declare_scalar_quote("eq=ual"), "eq=ual");
        assert_eq!(declare_scalar_quote("hash#x"), "hash#x");
        // control char -> ANSI-C
        assert_eq!(declare_scalar_quote("ta\tb"), "$'ta\\tb'");
    }

    #[test]
    fn format_declare_bare_line_scalar_and_array() {
        use crate::shell_state::{VarValue, Variable};
        // scalar needing quotes -> single-quoted
        let zs = Variable::scalar("a b".to_string());
        assert_eq!(format_declare_bare_line("zs", &zs), "zs='a b'");
        // bare scalar -> unquoted
        let zp = Variable::scalar("plain".to_string());
        assert_eq!(format_declare_bare_line("zp", &zp), "zp=plain");
        // indexed array -> name=([0]="p" [1]="q r") (matches declare -p minus prefix)
        let mut m = std::collections::BTreeMap::new();
        m.insert(0usize, "p".to_string());
        m.insert(1usize, "q r".to_string());
        let za = Variable {
            value: VarValue::Indexed(m),
            exported: false,
            readonly: false,
            integer: false,
            case_fold: None,
            nameref: false,
        };
        assert_eq!(
            format_declare_bare_line("za", &za),
            r#"za=([0]="p" [1]="q r")"#
        );
    }

    #[test]
    fn assoc_key_bareword_for_identifier() {
        use crate::shell_state::{VarValue, Variable};
        let var = Variable {
            value: VarValue::Associative(vec![("foo".into(), "v".into())]),
            exported: false,
            readonly: false,
            integer: false,
            case_fold: None,
            nameref: false,
        };
        let out = render_declare_value_part(&var);
        assert_eq!(out, r#"=([foo]="v" )"#);
    }

    #[test]
    fn assoc_key_quoted_for_metachar() {
        use crate::shell_state::{VarValue, Variable};
        let var = Variable {
            value: VarValue::Associative(vec![("a b".into(), "v".into())]),
            exported: false,
            readonly: false,
            integer: false,
            case_fold: None,
            nameref: false,
        };
        let out = render_declare_value_part(&var);
        assert_eq!(out, r#"=(["a b"]="v" )"#);
    }

    #[test]
    fn indexed_has_no_trailing_space() {
        use std::collections::BTreeMap;
        use crate::shell_state::{VarValue, Variable};
        let mut m = BTreeMap::new();
        m.insert(0usize, "x".to_string());
        m.insert(1usize, "y".to_string());
        let var = Variable {
            value: VarValue::Indexed(m),
            exported: false,
            readonly: false,
            integer: false,
            case_fold: None,
            nameref: false,
        };
        let out = render_declare_value_part(&var);
        assert_eq!(out, r#"=([0]="x" [1]="y")"#);
    }

    #[test]
    fn bare_declare_lists_name_value_and_functions() {
        let mut shell = crate::shell_state::Shell::new();
        // Set a scalar and define a function via the normal command path.
        shell.set("zsv", "hello".to_string());
        let _ =
            crate::shell::process_line("zf(){ echo hi; }", &mut shell, false);
        let mut buf: Vec<u8> = Vec::new();
        let _ = run_declaration_builtin("declare", &[], &mut buf,&mut std::io::stderr(),  &mut shell);
        let s = String::from_utf8(buf).unwrap();
        assert!(
            s.contains("zsv=hello"),
            "bare declare should list zsv=hello: {s}"
        );
        assert!(
            !s.contains("declare -- zsv"),
            "bare declare must not use the -p form: {s}"
        );
        assert!(
            s.contains("zf ()"),
            "bare declare should list function zf: {s}"
        );
    }

    #[test]
    fn printf_q_quoting() {
        assert_eq!(printf_q("plain"), "plain");
        assert_eq!(printf_q("a b"), "a\\ b");
        assert_eq!(printf_q("c'd"), "c\\'d");
        assert_eq!(printf_q("a$b"), "a\\$b");
        assert_eq!(printf_q("x\"y"), "x\\\"y");
        assert_eq!(printf_q("*"), "\\*");
        assert_eq!(printf_q(""), "''");
        assert_eq!(printf_q("p/q-r.s"), "p/q-r.s"); // /,-,. not escaped
        assert_eq!(printf_q("a\tb"), "$'a\\tb'");    // control -> $'...'
        assert_eq!(printf_q("ünï"), "ünï");          // UTF-8 as-is
        assert_eq!(printf_q("~a"), "\\~a");   // leading ~ escaped
        assert_eq!(printf_q("a~"), "a~");      // trailing ~ not escaped
        assert_eq!(printf_q("b~c"), "b~c");    // mid ~ not escaped
        assert_eq!(printf_q("#a"), "\\#a");   // leading # escaped
        assert_eq!(printf_q("a#"), "a#");      // trailing # not escaped
        assert_eq!(printf_q("a$b"), "a\\$b");  // $ special at any position
    }

    #[test]
    fn seto_option_names_includes_errexit_in_table_order() {
        let names: Vec<&str> = seto_option_names().collect();
        assert!(names.contains(&"errexit"));
        assert_eq!(names.len(), 27);
        assert_eq!(names[0], "allexport"); // table order
    }
    #[test]
    fn signal_names_are_sig_prefixed_and_exclude_pseudo() {
        let names = signal_names();
        assert!(names.contains(&"SIGINT".to_string()));
        assert!(names.iter().all(|n| n.starts_with("SIG")));
        assert!(!names.iter().any(|n| n.contains("EXIT")));
    }
    #[test]
    fn help_topic_names_nonempty() {
        assert!(help_topic_names().count() >= 40);
    }

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
        let shell = crate::shell_state::Shell::new();
        assert!(matches!(builtin_exit(&[],&mut std::io::stderr(),  &shell), ExecOutcome::Exit(0)));
    }

    #[test]
    fn exit_with_code() {
        let shell = crate::shell_state::Shell::new();
        assert!(matches!(
            builtin_exit(&["3".to_string()],&mut std::io::stderr(),  &shell),
            ExecOutcome::Exit(3)
        ));
    }

    #[test]
    fn exit_with_bad_code_continues() {
        let shell = crate::shell_state::Shell::new();
        assert!(matches!(
            builtin_exit(&["abc".to_string()],&mut std::io::stderr(),  &shell),
            ExecOutcome::Continue(_)
        ));
    }

    #[test]
    fn exit_masks_value_greater_than_255() {
        let shell = crate::shell_state::Shell::new();
        assert!(matches!(
            builtin_exit(&["300".to_string()],&mut std::io::stderr(),  &shell),
            ExecOutcome::Exit(44)
        ));
    }

    #[test]
    fn exit_masks_negative_value() {
        let shell = crate::shell_state::Shell::new();
        assert!(matches!(
            builtin_exit(&["-1".to_string()],&mut std::io::stderr(),  &shell),
            ExecOutcome::Exit(255)
        ));
    }

    #[test]
    fn exit_masks_exact_256_to_zero() {
        let shell = crate::shell_state::Shell::new();
        assert!(matches!(
            builtin_exit(&["256".to_string()],&mut std::io::stderr(),  &shell),
            ExecOutcome::Exit(0)
        ));
    }

    #[test]
    fn echo_writes_args_joined_by_spaces() {
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_echo(&["hello".to_string(), "world".to_string()], &mut out, &mut std::io::stderr());
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(out, b"hello world\n");
    }

    #[test]
    fn echo_with_no_args_writes_a_blank_line() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&[], &mut out, &mut std::io::stderr());
        assert_eq!(out, b"\n");
    }

    #[test]
    fn echo_n_suppresses_trailing_newline() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-n".to_string(), "hello".to_string()], &mut out, &mut std::io::stderr());
        assert_eq!(out, b"hello");
    }

    #[test]
    fn echo_n_alone_writes_nothing() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-n".to_string()], &mut out, &mut std::io::stderr());
        assert_eq!(out, b"");
    }

    #[test]
    fn echo_e_processes_basic_escapes() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-e".to_string(), r"a\tb\nc".to_string()], &mut out, &mut std::io::stderr());
        assert_eq!(out, b"a\tb\nc\n");
    }

    #[test]
    fn echo_capital_e_keeps_backslashes_literal() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-E".to_string(), r"a\tb".to_string()], &mut out, &mut std::io::stderr());
        assert_eq!(out, b"a\\tb\n");
    }

    #[test]
    fn echo_default_keeps_backslashes_literal() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&[r"a\tb".to_string()], &mut out, &mut std::io::stderr());
        assert_eq!(out, b"a\\tb\n");
    }

    #[test]
    fn echo_combined_ne_flag() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-ne".to_string(), r"a\tb".to_string()], &mut out, &mut std::io::stderr());
        assert_eq!(out, b"a\tb");
    }

    #[test]
    fn echo_e_then_capital_e_disables_escapes() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-eE".to_string(), r"a\tb".to_string()], &mut out, &mut std::io::stderr());
        assert_eq!(out, b"a\\tb\n");
    }

    #[test]
    fn echo_non_flag_arg_stops_flag_parsing() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(
            &["-n".to_string(), "foo".to_string(), "-n".to_string(), "bar".to_string()],
            &mut out, &mut std::io::stderr(),
        );
        assert_eq!(out, b"foo -n bar");
    }

    #[test]
    fn echo_unknown_flag_is_literal() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-x".to_string(), "foo".to_string()], &mut out, &mut std::io::stderr());
        assert_eq!(out, b"-x foo\n");
    }

    #[test]
    fn echo_single_dash_is_literal() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-".to_string()], &mut out, &mut std::io::stderr());
        assert_eq!(out, b"-\n");
    }

    #[test]
    fn echo_double_dash_is_literal() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["--".to_string(), "foo".to_string()], &mut out, &mut std::io::stderr());
        assert_eq!(out, b"-- foo\n");
    }

    #[test]
    fn echo_e_c_escape_terminates_output() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-e".to_string(), r"abc\cdef".to_string()], &mut out, &mut std::io::stderr());
        assert_eq!(out, b"abc");
    }

    #[test]
    fn echo_e_octal_escape() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-e".to_string(), r"\0101".to_string()], &mut out, &mut std::io::stderr());
        assert_eq!(out, b"A\n");
    }

    #[test]
    fn echo_e_hex_escape() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-e".to_string(), r"\x41".to_string()], &mut out, &mut std::io::stderr());
        assert_eq!(out, b"A\n");
    }

    #[test]
    fn echo_e_unknown_escape_keeps_backslash() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-e".to_string(), r"\z".to_string()], &mut out, &mut std::io::stderr());
        assert_eq!(out, b"\\z\n");
    }

    #[test]
    fn pwd_writes_the_current_directory() {
        let mut out: Vec<u8> = Vec::new();
        let mut shell = Shell::new();
        let outcome = builtin_pwd(&[], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let written = String::from_utf8(out).unwrap();
        // With no $PWD set, logical mode falls back to getcwd.
        let expected = env::current_dir().unwrap();
        assert_eq!(written.trim_end(), expected.to_str().unwrap());
    }

    fn dp(s: &str) -> DeclArg {
        DeclArg::Plain(s.to_string())
    }

    #[test]
    fn export_nf_unexports_function() {
        let mut shell = Shell::new();
        let _ = crate::shell::process_line("uf(){ echo hi; }", &mut shell, false);
        shell.mark_function_exported("uf");
        assert!(shell.is_function_exported("uf"));
        let mut out = Vec::new();
        // export -nf uf  -> remove the export mark
        let oc = builtin_export_decl(&[dp("-n"), dp("-f"), dp("uf")], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)), "{oc:?}");
        assert!(
            !shell.is_function_exported("uf"),
            "export -nf must un-export the function"
        );
    }

    #[test]
    fn declare_fx_marks_via_runtime_path() {
        let mut shell = Shell::new();
        let _ = crate::shell::process_line("dfn(){ echo hi; }", &mut shell, false);
        assert!(!shell.is_function_exported("dfn"));
        // `declare -fx NAME` must mark it exported (runtime declaration path).
        let _ = crate::shell::process_line("declare -fx dfn", &mut shell, false);
        assert!(
            shell.is_function_exported("dfn"),
            "declare -fx did not mark via the runtime path"
        );
    }

    #[test]
    fn declare_fx_no_names_lists_via_runtime_path() {
        let mut shell = Shell::new();
        let _ = crate::shell::process_line("dfn2(){ echo hi; }", &mut shell, false);
        shell.mark_function_exported("dfn2");
        // capture stdout of `declare -fx`: route through builtin_declare_decl directly.
        let mut out = Vec::new();
        let oc = builtin_declare_decl(&[dp("-fx")], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)), "{oc:?}");
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("dfn2 ()"), "{s}");
        assert!(s.contains("declare -fx dfn2"), "{s}");
    }

    #[test]
    fn export_p_lists_in_declare_x_format() {
        let mut shell = Shell::new();
        shell.export_set("EXP_A", "1".to_string());
        shell.export_set("EXP_B", "two".to_string());
        let mut out = Vec::new();
        let oc = builtin_export_decl(&[dp("-p")], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("declare -x EXP_A=\"1\""), "{s}");
        assert!(s.contains("declare -x EXP_B=\"two\""), "{s}");
        assert!(!s.contains("export EXP_A=1"), "old format must be gone: {s}");
    }

    #[test]
    fn bare_export_uses_declare_x_format() {
        let mut shell = Shell::new();
        shell.export_set("EXP_C", "z".to_string());
        let mut out = Vec::new();
        let _ = builtin_export_decl(&[], &mut out,&mut std::io::stderr(),  &mut shell);
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("declare -x EXP_C=\"z\""), "{s}");
    }

    #[test]
    fn export_n_unexports_keeps_value() {
        let mut shell = Shell::new();
        shell.export_set("EXP_D", "keep".to_string());
        assert!(shell.is_exported("EXP_D"));
        let mut out = Vec::new();
        let oc = builtin_export_decl(&[dp("-n"), dp("EXP_D")], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(!shell.is_exported("EXP_D"), "must be unexported");
        assert_eq!(shell.get("EXP_D"), Some("keep"), "value kept");
    }

    #[test]
    fn export_n_with_assignment_sets_then_unexports() {
        let mut shell = Shell::new();
        shell.export_set("EXP_E", "1".to_string());
        let mut out = Vec::new();
        let _ = builtin_export_decl(&[dp("-n"), dp("EXP_E=2")], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(!shell.is_exported("EXP_E"));
        assert_eq!(shell.get("EXP_E"), Some("2"));
    }

    #[test]
    fn export_n_unset_name_is_noop() {
        let mut shell = Shell::new();
        let mut out = Vec::new();
        let oc = builtin_export_decl(&[dp("-n"), dp("NOPE_X")], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(!shell.is_exported("NOPE_X"));
    }

    #[test]
    fn export_invalid_flag_rc2() {
        let mut shell = Shell::new();
        let mut out = Vec::new();
        let oc = builtin_export_decl(&[dp("-z")], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(2)), "{oc:?}");
    }

    #[test]
    fn export_p_with_operand_exports_it_no_listing() {
        let mut shell = Shell::new();
        shell.set("EXP_F", "v".to_string());
        assert!(!shell.is_exported("EXP_F"));
        let mut out = Vec::new();
        let oc = builtin_export_decl(&[dp("-p"), dp("EXP_F")], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(
            shell.is_exported("EXP_F"),
            "operand with -p should be exported (bash)"
        );
        assert!(
            String::from_utf8(out).unwrap().is_empty(),
            "no listing when operands present"
        );
    }

    #[test]
    fn export_f_does_not_create_variable() {
        let mut shell = Shell::new();
        let mut out = Vec::new();
        // `export -f somefunc` for a nonexistent function: rc 1, no variable.
        let oc = builtin_export_decl(&[dp("-f"), dp("somefunc")], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
        assert!(shell.get("somefunc").is_none(), "must NOT create a variable");
        assert!(!shell.is_exported("somefunc"));
    }

    #[test]
    fn export_f_marks_existing_function() {
        let mut shell = Shell::new();
        let _ = crate::shell::process_line("myfn(){ echo hi; }", &mut shell, false);
        let mut out = Vec::new();
        let oc = builtin_export_decl(&[dp("-f"), dp("myfn")], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.is_function_exported("myfn"));
    }

    #[test]
    fn export_f_not_a_function_rc1() {
        let mut shell = Shell::new();
        let mut out = Vec::new();
        let oc = builtin_export_decl(&[dp("-f"), dp("nope")], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)), "{oc:?}");
        assert!(!shell.is_function_exported("nope"));
    }

    #[test]
    fn export_f_no_operands_lists_functions() {
        let mut shell = Shell::new();
        let _ = crate::shell::process_line("af(){ echo hi; }", &mut shell, false);
        shell.mark_function_exported("af");
        let mut out = Vec::new();
        let oc = builtin_export_decl(&[dp("-f")], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("af ()"), "{s}");
        assert!(s.contains("declare -fx af"), "{s}");
    }

    #[test]
    fn export_a_bare_no_listing() {
        let mut shell = Shell::new();
        shell.export_set("EXP_HIDE", "1".to_string());
        let mut out = Vec::new();
        let oc = builtin_export_decl(&[dp("-a")], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(String::from_utf8(out).unwrap().is_empty(), "export -a must NOT list");
    }

    #[test]
    fn export_f_bare_no_listing() {
        let mut shell = Shell::new();
        shell.export_set("EXP_HIDE2", "1".to_string());
        let mut out = Vec::new();
        let oc = builtin_export_decl(&[dp("-f")], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(String::from_utf8(out).unwrap().is_empty(), "export -f must NOT list vars");
    }

    #[test]
    fn unset_removes_variable() {
        let mut shell = Shell::new();
        shell.set("HUCK_RM", "v".to_string());
        let outcome = builtin_unset(&["HUCK_RM".to_string()],&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.get("HUCK_RM"), None);
    }

    #[test]
    fn unset_invalid_name_is_error() {
        let mut shell = Shell::new();
        let outcome = builtin_unset(&["1BAD".to_string()],&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn unset_unknown_name_is_silent_ok() {
        let mut shell = Shell::new();
        let outcome = builtin_unset(&["NEVER_SET_HUCK_XYZ".to_string()],&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn jobs_with_empty_table_prints_nothing_and_returns_zero() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_jobs(&[], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(out.is_empty());
    }

    #[test]
    fn jobs_lists_synthetic_done_entry() {
        let mut shell = Shell::new();
        let _ = shell.jobs.add_synthetic_done("echo hi".to_string(), 0);
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_jobs(&[], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("[1]"));
        assert!(s.contains("Done"));
        assert!(s.contains("echo hi"));
    }

    #[test]
    fn jobs_lists_stopped_without_ampersand_suffix() {
        let mut shell = Shell::new();
        shell.jobs.add(100, vec![100], "sleep 100".to_string());
        shell.jobs.jobs_mut()[0].state = crate::jobs::JobState::Stopped(libc::SIGTSTP);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &[], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("Stopped"), "got: {out:?}");
        assert!(!out.trim_end().ends_with('&'), "Stopped line must NOT end with &; got: {out:?}");
    }

    #[test]
    fn jobs_l_includes_pid_for_single_stage() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "sleep 30".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &["-l".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("1234"), "expected pid 1234 in: {out:?}");
        assert!(out.contains("[1]"), "expected job number in: {out:?}");
    }

    #[test]
    fn jobs_l_multistage_shows_all_pids() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234, 1235, 1236], "a | b | c".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &["-l".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("1234"), "missing 1234 in: {out:?}");
        assert!(out.contains("1235"), "missing 1235 in: {out:?}");
        assert!(out.contains("1236"), "missing 1236 in: {out:?}");
        let line_count = out.lines().count();
        assert!(line_count >= 3, "expected >=3 lines, got {line_count}: {out:?}");
    }

    #[test]
    fn jobs_p_prints_pgids_only() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "a".to_string());
        shell.jobs.add(2345, vec![2345], "b".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &["-p".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "expected 2 lines, got {lines:?}");
        for l in &lines {
            assert!(
                l.parse::<i32>().is_ok(),
                "expected each line to be an int, got {l:?}"
            );
        }
    }

    #[test]
    fn jobs_r_filters_running() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "running_cmd".to_string()); // %1 Running
        shell.jobs.add_synthetic_done("done_cmd".to_string(), 0);     // %2 Done
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &["-r".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("running_cmd"), "missing running_cmd: {out:?}");
        assert!(!out.contains("done_cmd"), "should not contain done_cmd: {out:?}");
    }

    #[test]
    fn jobs_s_filters_stopped() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "running_cmd".to_string()); // %1 Running
        shell.jobs.add(2345, vec![2345], "stopped_cmd".to_string()); // %2 then forced Stopped
        shell.jobs.jobs_mut()[1].state = crate::jobs::JobState::Stopped(libc::SIGTSTP);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &["-s".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("stopped_cmd"), "missing stopped_cmd: {out:?}");
        assert!(!out.contains("running_cmd"), "should not contain running_cmd: {out:?}");
    }

    #[test]
    fn jobs_n_filters_notified_false_and_marks() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "a".to_string()); // notified=false default
        shell.jobs.add(2345, vec![2345], "b".to_string()); // notified=false default
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &["-n".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("[1]"), "first call should show [1]: {out:?}");
        assert!(out.contains("[2]"), "first call should show [2]: {out:?}");

        // Second call: both jobs are now marked notified -> empty output.
        let mut buf2: Vec<u8> = Vec::new();
        let outcome2 = run_builtin("jobs", &["-n".to_string()], &mut buf2,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome2, ExecOutcome::Continue(0)));
        let out2 = String::from_utf8(buf2).unwrap();
        assert!(out2.is_empty(), "second call should be empty: {out2:?}");
    }

    #[test]
    fn jobs_positional_spec_filters_to_target() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "first_cmd".to_string());  // %1
        shell.jobs.add(2345, vec![2345], "second_cmd".to_string()); // %2
        shell.jobs.add(3456, vec![3456], "third_cmd".to_string());  // %3
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &["%2".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("second_cmd"), "missing second_cmd: {out:?}");
        assert!(!out.contains("first_cmd"), "should not contain first_cmd: {out:?}");
        assert!(!out.contains("third_cmd"), "should not contain third_cmd: {out:?}");
    }

    #[test]
    fn jobs_invalid_flag_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &["-x".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn jobs_p_overrides_l() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "sleep".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &["-lp".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        // -p output is just digits + newline, no [N] prefix.
        assert!(!out.contains("[1]"), "expected -p override, got: {out:?}");
        assert_eq!(out.trim(), "1234");
    }

    #[test]
    fn wait_with_no_jobs_returns_zero_immediately() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_wait(&[], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn is_builtin_recognizes_jobs_and_wait() {
        assert!(is_builtin("jobs"));
        assert!(is_builtin("wait"));
    }

    #[test]
    fn builtin_names_const_matches_is_builtin() {
        for name in BUILTIN_NAMES {
            assert!(is_builtin(name), "{name} should be a builtin");
        }
        assert!(!is_builtin("definitely_not_a_builtin"));
    }

    #[test]
    fn builtin_names_includes_history() {
        assert!(BUILTIN_NAMES.contains(&"history"));
    }

    #[test]
    fn builtin_test_true_expression() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let args = vec!["-n".to_string(), "x".to_string()];
        let outcome = run_builtin("test", &args, &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn builtin_test_false_expression() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let args = vec!["-z".to_string(), "x".to_string()];
        let outcome = run_builtin("test", &args, &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn builtin_test_usage_error() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let args = vec!["3".to_string(), "-eq".to_string(), "abc".to_string()];
        let outcome = run_builtin("test", &args, &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn builtin_bracket_strips_trailing_bracket() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let args = vec![
            "-n".to_string(),
            "x".to_string(),
            "]".to_string(),
        ];
        let outcome = run_builtin("[", &args, &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn builtin_bracket_missing_close_is_error() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let args = vec!["-n".to_string(), "x".to_string()];
        let outcome = run_builtin("[", &args, &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn builtin_bracket_empty_is_error() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let outcome = run_builtin("[", &[], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn builtin_break_returns_loop_break() {
        let mut shell = Shell::new();
        shell.loop_depth = 1;
        let mut out: Vec<u8> = Vec::new();
        let outcome = run_builtin("break", &[], &mut out,&mut std::io::stderr(),  &mut shell);
        assert_eq!(outcome, ExecOutcome::LoopBreak(1, 0));
    }

    #[test]
    fn builtin_continue_returns_loop_continue() {
        let mut shell = Shell::new();
        shell.loop_depth = 1;
        let mut out: Vec<u8> = Vec::new();
        let outcome = run_builtin("continue", &[], &mut out,&mut std::io::stderr(),  &mut shell);
        assert_eq!(outcome, ExecOutcome::LoopContinue(1));
    }

    #[test]
    fn builtin_return_with_arg_returns_function_return() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            run_builtin("return", &["7".to_string()], &mut out,&mut std::io::stderr(),  &mut shell),
            ExecOutcome::FunctionReturn(7)
        );
    }

    #[test]
    fn builtin_return_no_arg_returns_last_status() {
        let mut shell = Shell::new();
        shell.set_last_status(42);
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            run_builtin("return", &[], &mut out,&mut std::io::stderr(),  &mut shell),
            ExecOutcome::FunctionReturn(42)
        );
    }

    #[test]
    fn builtin_return_invalid_arg_falls_back_to_last_status() {
        let mut shell = Shell::new();
        shell.set_last_status(13);
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            run_builtin("return", &["not-a-num".to_string()], &mut out,&mut std::io::stderr(),  &mut shell),
            ExecOutcome::FunctionReturn(13)
        );
    }

    #[test]
    fn is_builtin_trap() {
        assert!(is_builtin("trap"));
    }

    #[test]
    fn is_special_builtin_trap() {
        assert!(is_special_builtin("trap"));
    }

    #[test]
    fn trap_exit_action_signal_registers() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["echo bye".to_string(), "EXIT".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(shell.traps.contains_key(&crate::traps::TrapSignal::Exit));
    }

    #[test]
    fn trap_empty_action_ignores_signal() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["".to_string(), "EXIT".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(
            shell.traps.get(&crate::traps::TrapSignal::Exit),
            Some(&None),  // None = ignore
        );
    }

    #[test]
    fn trap_dash_resets_signal() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        // Install first.
        let _ = run_builtin(
            "trap",
            &["echo bye".to_string(), "EXIT".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        // Then reset.
        let outcome = run_builtin(
            "trap",
            &["-".to_string(), "EXIT".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(!shell.traps.contains_key(&crate::traps::TrapSignal::Exit));
    }

    #[test]
    fn trap_p_prints_active_traps_in_re_readable_form() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        // Register a trap.
        let _ = run_builtin(
            "trap",
            &["echo bye".to_string(), "EXIT".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        // Clear the buffer (the install printed nothing, but be defensive).
        buf.clear();
        // List.
        let outcome = run_builtin(
            "trap",
            &["-p".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("trap -- 'echo bye' EXIT"),
            "expected trap -p to print 'trap -- echo bye EXIT', got: {out}"
        );
    }

    #[test]
    fn trap_no_args_same_as_dash_p() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let _ = run_builtin(
            "trap",
            &["echo bye".to_string(), "EXIT".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        buf.clear();
        let outcome = run_builtin("trap", &[], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("trap -- 'echo bye' EXIT"));
    }

    #[test]
    fn trap_l_lists_signals() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["-l".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("2) SIGINT"), "stdout: {out}");
        assert!(out.contains("15) SIGTERM"), "stdout: {out}");
    }

    #[test]
    fn trap_unknown_signal_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["echo bye".to_string(), "NOPE".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn trap_kill_signal_accepted_silently() {
        // bash accepts `trap … KILL` (rc 0, no error) and stores the
        // disposition; it just never fires (OS can't catch SIGKILL).
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["echo nope".to_string(), "KILL".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(buf.is_empty(), "no error output expected, got: {:?}", String::from_utf8_lossy(&buf));
        assert!(shell.traps.contains_key(&crate::traps::TrapSignal::Real(libc::SIGKILL)));
    }

    #[test]
    fn trap_no_signals_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["echo bye".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }
}

#[cfg(test)]
mod fg_bg_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn fg_with_no_jobs_errors() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("fg", &[], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn bg_with_no_jobs_errors() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &[], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn fg_with_percent_spec_arg_and_no_job_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("fg", &["%1".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn bg_with_percent_spec_arg_and_no_job_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &["%1".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn bg_on_running_job_returns_no_current_job() {
        let mut shell = Shell::new();
        shell.jobs.add(4242, vec![4242], "sleep 100".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &[], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn is_builtin_recognizes_fg_and_bg() {
        assert!(is_builtin("fg"));
        assert!(is_builtin("bg"));
    }

    #[test]
    fn fg_with_bad_job_spec_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("fg", &["%abc".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn fg_with_no_such_job_spec_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("fg", &["%99".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn fg_with_non_percent_arg_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("fg", &["1".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn fg_with_multiple_args_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "fg",
            &["%1".to_string(), "%2".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn bg_with_bad_job_spec_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &["%abc".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn bg_with_no_such_job_spec_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &["%99".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn bg_with_running_spec_errors_already_running() {
        let mut shell = Shell::new();
        shell.jobs.add(4242, vec![4242], "sleep 100".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &["%1".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn bg_with_multiple_args_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "bg",
            &["%1".to_string(), "%2".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn wait_with_bad_spec_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["%abc".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn wait_with_no_such_spec_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["%99".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn wait_multiarg_unparseable_returns_usage_status_2() {
        // Multi-arg wait is now valid; only bad arg syntax should usage-error.
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "wait",
            &["1234".to_string(), "abc".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn wait_with_unparseable_pid_arg_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["abc".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn wait_with_done_spec_returns_decoded_status_immediately() {
        let mut shell = Shell::new();
        // Synthetic Done job — wait should see it's already terminal and
        // return decode(0) → 0 without blocking.
        shell.jobs.add_synthetic_done("echo hi".to_string(), 0);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["%1".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn wait_with_done_spec_returns_nonzero_for_exit_n() {
        let mut shell = Shell::new();
        shell.jobs.add_synthetic_done("false".to_string(), 1);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["%1".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn wait_multiarg_two_done_returns_last_status() {
        let mut shell = Shell::new();
        shell.jobs.add_synthetic_done("true".to_string(), 0);
        shell.jobs.add_synthetic_done("exit 5".to_string(), 5);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "wait",
            &["%1".to_string(), "%2".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(5)));
    }

    #[test]
    fn wait_multiarg_unparseable_rejects_before_waiting() {
        let mut shell = Shell::new();
        shell.jobs.add_synthetic_done("true".to_string(), 0);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "wait",
            &["%1".to_string(), "abc".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn wait_n_with_no_jobs_returns_127() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["-n".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(127)));
    }

    #[test]
    fn wait_n_with_only_done_jobs_returns_127() {
        let mut shell = Shell::new();
        shell.jobs.add_synthetic_done("true".to_string(), 0);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["-n".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(127)));
    }

    #[test]
    fn wait_n_with_explicit_already_done_returns_its_status() {
        let mut shell = Shell::new();
        shell.jobs.add_synthetic_done("exit 7".to_string(), 7);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "wait",
            &["-n".to_string(), "%1".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(7)));
    }

    #[test]
    fn wait_n_p_var_captures_pgid_via_explicit_target() {
        let mut shell = Shell::new();
        shell.jobs.add_synthetic_done("true".to_string(), 0);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "wait",
            &[
                "-n".to_string(),
                "-p".to_string(),
                "PID".to_string(),
                "%1".to_string(),
            ],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("PID").as_deref(), Some("0"));
    }

    #[test]
    fn wait_p_without_n_is_usage_error() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "wait",
            &["-p".to_string(), "PID".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn wait_n_p_without_var_name_is_usage_error() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "wait",
            &["-n".to_string(), "-p".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn wait_invalid_flag_is_usage_error() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["-x".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }
}

#[cfg(test)]
mod kill_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn is_builtin_recognizes_kill() {
        assert!(is_builtin("kill"));
    }

    #[test]
    fn kill_no_args_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &[], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn kill_sig_flag_with_no_targets_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &["-TERM".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn kill_invalid_signal_name_returns_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-ABC".to_string(), "%1".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn kill_invalid_signal_number_returns_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-9999".to_string(), "%1".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn kill_unparseable_target_returns_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &["abc".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn kill_no_such_job_spec_returns_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &["%99".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn signal_by_name_table_recognizes_common_signals() {
        assert_eq!(signal_by_name("HUP"), Some(libc::SIGHUP));
        assert_eq!(signal_by_name("SIGHUP"), Some(libc::SIGHUP));
        assert_eq!(signal_by_name("hup"), Some(libc::SIGHUP));
        assert_eq!(signal_by_name("sighup"), Some(libc::SIGHUP));
        assert_eq!(signal_by_name("INT"), Some(libc::SIGINT));
        assert_eq!(signal_by_name("KILL"), Some(libc::SIGKILL));
        assert_eq!(signal_by_name("TERM"), Some(libc::SIGTERM));
        assert_eq!(signal_by_name("STOP"), Some(libc::SIGSTOP));
        assert_eq!(signal_by_name("CONT"), Some(libc::SIGCONT));
        assert_eq!(signal_by_name("USR1"), Some(libc::SIGUSR1));
        assert_eq!(signal_by_name("USR2"), Some(libc::SIGUSR2));
        assert_eq!(signal_by_name("TSTP"), Some(libc::SIGTSTP));
        assert_eq!(signal_by_name("PIPE"), Some(libc::SIGPIPE));
        assert_eq!(signal_by_name("ALRM"), Some(libc::SIGALRM));
        assert_eq!(signal_by_name("CHLD"), Some(libc::SIGCHLD));
        assert_eq!(signal_by_name("TTIN"), Some(libc::SIGTTIN));
        assert_eq!(signal_by_name("TTOU"), Some(libc::SIGTTOU));
        assert_eq!(signal_by_name("ABC"), None);
        assert_eq!(signal_by_name(""), None);
    }

    #[test]
    fn kill_signal_zero_is_accepted_as_valid_numeric() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        // No targets after the signal → usage(2) — but the signal itself
        // must parse without "invalid signal number" status 1.
        let outcome = run_builtin("kill", &["-0".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)),
            "kill -0 (no targets) should reach usage check, not signal check");
    }

    #[test]
    fn kill_l_no_args_lists_all_standard_signals() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &["-l".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let s = String::from_utf8(buf).unwrap();
        // Common signals that were already listed before v189.
        assert!(s.contains("KILL"), "output missing KILL: {s}");
        assert!(s.contains("TERM"), "output missing TERM: {s}");
        assert!(s.contains("WINCH"), "output missing WINCH: {s}");
        // The point of v189: the listing must now include the newly-added
        // standard signals by name (bare-name format at this stage).
        for sig in ["ABRT", "SEGV", "BUS", "FPE", "ILL"] {
            assert!(s.contains(sig), "kill -l listing missing {sig}: {s}");
        }
    }

    #[test]
    fn kill_l_listing_matches_bash_format() {
        let mut buf = Vec::new();
        print_killable_table(&mut buf);
        let s = String::from_utf8(buf).unwrap();
        // bash: ` 1) SIGHUP\t 2) SIGINT\t 3) SIGQUIT\t 4) SIGILL\t 5) SIGTRAP\n…`
        let first = s.lines().next().unwrap();
        assert_eq!(first, " 1) SIGHUP\t 2) SIGINT\t 3) SIGQUIT\t 4) SIGILL\t 5) SIGTRAP");
        // SIG prefix everywhere, 5 columns per full row
        assert!(s.contains("SIGABRT"), "missing SIGABRT: {s}");
        assert!(s.contains("11) SIGSEGV"));
    }

    #[test]
    fn kill_l_with_name_returns_number() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-l".to_string(), "TERM".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s.trim(), libc::SIGTERM.to_string());
    }

    #[test]
    fn kill_l_with_sig_prefix_returns_number() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-l".to_string(), "SIGTERM".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s.trim(), libc::SIGTERM.to_string());
    }

    #[test]
    fn kill_l_lowercase_name_returns_number() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-l".to_string(), "term".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s.trim(), libc::SIGTERM.to_string());
    }

    #[test]
    fn kill_l_with_number_returns_name() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-l".to_string(), libc::SIGTERM.to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s.trim(), "TERM");
    }

    #[test]
    fn kill_l_status_decode() {
        let arg = (128 + libc::SIGKILL).to_string();
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-l".to_string(), arg],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s.trim(), "KILL");
    }

    #[test]
    fn kill_l_unknown_name_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-l".to_string(), "xyz".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn kill_l_invalid_number_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-l".to_string(), "99".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn kill_l_multiple_args_decodes_each() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &[
                "-l".to_string(),
                libc::SIGHUP.to_string(),
                libc::SIGKILL.to_string(),
                libc::SIGTERM.to_string(),
            ],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let s = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines, vec!["HUP", "KILL", "TERM"]);
    }

    #[test]
    fn signal_by_name_resolves_winch() {
        assert_eq!(signal_by_name("WINCH"), Some(libc::SIGWINCH));
        assert_eq!(signal_by_name("SIGWINCH"), Some(libc::SIGWINCH));
        assert_eq!(signal_by_name("winch"), Some(libc::SIGWINCH));
    }

    #[test]
    fn kill_s_with_name_resolves_and_dispatches() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let pid = unsafe { libc::getpid() }.to_string();
        let outcome = run_builtin(
            "kill",
            &["-s".to_string(), "WINCH".to_string(), pid],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn kill_s_with_sig_prefix_resolves() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let pid = unsafe { libc::getpid() }.to_string();
        let outcome = run_builtin(
            "kill",
            &["-s".to_string(), "SIGWINCH".to_string(), pid],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn kill_s_lowercase_name_resolves() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let pid = unsafe { libc::getpid() }.to_string();
        let outcome = run_builtin(
            "kill",
            &["-s".to_string(), "winch".to_string(), pid],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn kill_s_missing_arg_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &["-s".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn kill_s_invalid_name_returns_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-s".to_string(), "BOGUS".to_string(), "99999".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn kill_s_no_targets_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-s".to_string(), "TERM".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn kill_n_with_number_resolves_and_dispatches() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let pid = unsafe { libc::getpid() }.to_string();
        let outcome = run_builtin(
            "kill",
            &[
                "-n".to_string(),
                libc::SIGWINCH.to_string(),
                pid,
            ],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn kill_n_missing_arg_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &["-n".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn kill_n_invalid_number_returns_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-n".to_string(), "99".to_string(), "12345".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn kill_dash_sig_short_form_still_works_after_refactor() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let pid = unsafe { libc::getpid() }.to_string();
        let outcome = run_builtin(
            "kill",
            &["-WINCH".to_string(), pid],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }
}

#[cfg(test)]
mod cd_pwd_tests {
    use super::*;
    use crate::shell_state::Shell;
    use crate::test_support::CWD_LOCK;

    /// What `cd <path>` will end up storing in PWD: `builtin_cd` reads
    /// the post-chdir cwd via `env::current_dir()`, which canonicalizes
    /// symlinks. On Linux `/tmp` is a real directory so this is `/tmp`;
    /// on macOS `/tmp` is a symlink to `/private/tmp` and the kernel
    /// returns the resolved path. Computing it at test time keeps the
    /// assertions portable.
    #[test]
    fn cd_sets_pwd_to_target_directory() {
        let _g = CWD_LOCK.lock().unwrap();
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let prev = std::env::current_dir().unwrap();
        let outcome = builtin_cd(&["/tmp".to_string()], &mut out,&mut std::io::stderr(),  &mut shell);
        // Restore for any other tests.
        let _ = std::env::set_current_dir(&prev);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        // Logical PWD (v162): `cd /tmp` stores the logical path, not the
        // symlink-resolved one (matters on macOS where /tmp -> /private/tmp).
        assert_eq!(shell.get("PWD"), Some("/tmp"));
        assert!(shell.exported_env().any(|(k, _)| k == "PWD"));
        assert!(out.is_empty());
    }

    #[test]
    fn cd_sets_oldpwd_to_previous_pwd() {
        let _g = CWD_LOCK.lock().unwrap();
        let mut shell = Shell::new();
        shell.export_set("PWD", "/var".to_string());
        let mut out: Vec<u8> = Vec::new();
        let prev = std::env::current_dir().unwrap();
        let outcome = builtin_cd(&["/tmp".to_string()], &mut out,&mut std::io::stderr(),  &mut shell);
        let _ = std::env::set_current_dir(&prev);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.get("OLDPWD"), Some("/var"));
        assert!(shell.exported_env().any(|(k, _)| k == "OLDPWD"));
    }

    #[test]
    fn cd_with_pwd_initially_unset_does_not_set_oldpwd() {
        let _g = CWD_LOCK.lock().unwrap();
        let mut shell = Shell::new();
        shell.unset("PWD");
        shell.unset("OLDPWD");
        let mut out: Vec<u8> = Vec::new();
        let prev = std::env::current_dir().unwrap();
        let outcome = builtin_cd(&["/tmp".to_string()], &mut out,&mut std::io::stderr(),  &mut shell);
        let _ = std::env::set_current_dir(&prev);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.get("OLDPWD"), None);
        // Logical PWD (v162): `cd /tmp` stores the logical path, not the
        // symlink-resolved one (matters on macOS where /tmp -> /private/tmp).
        assert_eq!(shell.get("PWD"), Some("/tmp"));
    }

    #[test]
    fn cd_dash_uses_oldpwd_as_target() {
        let _g = CWD_LOCK.lock().unwrap();
        let mut shell = Shell::new();
        shell.export_set("OLDPWD", "/tmp".to_string());
        shell.export_set("PWD", "/var".to_string());
        let mut out: Vec<u8> = Vec::new();
        let prev = std::env::current_dir().unwrap();
        let outcome = builtin_cd(&["-".to_string()], &mut out,&mut std::io::stderr(),  &mut shell);
        let _ = std::env::set_current_dir(&prev);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        // Logical PWD (v162): `cd /tmp` stores the logical path, not the
        // symlink-resolved one (matters on macOS where /tmp -> /private/tmp).
        assert_eq!(shell.get("PWD"), Some("/tmp"));
        assert_eq!(shell.get("OLDPWD"), Some("/var"));
    }

    #[test]
    fn cd_dash_prints_new_pwd_on_stdout() {
        let _g = CWD_LOCK.lock().unwrap();
        let mut shell = Shell::new();
        shell.export_set("OLDPWD", "/tmp".to_string());
        shell.export_set("PWD", "/var".to_string());
        let mut out: Vec<u8> = Vec::new();
        let prev = std::env::current_dir().unwrap();
        let outcome = builtin_cd(&["-".to_string()], &mut out,&mut std::io::stderr(),  &mut shell);
        let _ = std::env::set_current_dir(&prev);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        // `cd -` echoes the logical PWD (v162): the OLDPWD value as-typed.
        assert_eq!(String::from_utf8(out).unwrap(), "/tmp\n");
    }

    #[test]
    fn cd_dash_errors_when_oldpwd_unset() {
        let mut shell = Shell::new();
        shell.unset("OLDPWD");
        let mut out: Vec<u8> = Vec::new();
        let prev = std::env::current_dir().unwrap();
        let outcome = builtin_cd(&["-".to_string()], &mut out,&mut std::io::stderr(),  &mut shell);
        let _ = std::env::set_current_dir(&prev);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        assert!(out.is_empty());
    }

    #[test]
    fn cd_dash_errors_when_oldpwd_empty() {
        let mut shell = Shell::new();
        shell.export_set("OLDPWD", String::new());
        let mut out: Vec<u8> = Vec::new();
        let prev = std::env::current_dir().unwrap();
        let outcome = builtin_cd(&["-".to_string()], &mut out,&mut std::io::stderr(),  &mut shell);
        let _ = std::env::set_current_dir(&prev);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        assert!(out.is_empty());
    }
}

#[cfg(test)]
mod disown_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn is_builtin_recognizes_disown() {
        assert!(is_builtin("disown"));
    }

    #[test]
    fn disown_no_args_with_no_current_job_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &[], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn disown_no_args_removes_current_job() {
        let mut shell = Shell::new();
        shell.jobs.add(4242, vec![4242], "sleep 100".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &[], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.jobs.iter().count(), 0);
    }

    #[test]
    fn disown_with_spec_removes_specified_job() {
        let mut shell = Shell::new();
        shell.jobs.add(100, vec![100], "a".to_string());
        shell.jobs.add(200, vec![200], "b".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["%1".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let remaining: Vec<u32> = shell.jobs.iter().map(|j| j.id).collect();
        assert_eq!(remaining, vec![2]);
    }

    #[test]
    fn disown_with_bad_spec_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["%abc".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn disown_with_non_percent_arg_returns_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["1".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn disown_drops_pending_done_notification() {
        let mut shell = Shell::new();
        // Synthetic Done job with notified=false would trigger a "[1] Done"
        // line at the next prompt. Disown should remove the job and
        // suppress that notification.
        shell.jobs.add_synthetic_done("echo hi".to_string(), 0);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["%1".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.jobs.iter().count(), 0);
    }

    use crate::jobs::JobState;

    #[test]
    fn disown_a_removes_all_jobs() {
        let mut shell = Shell::new();
        shell.jobs.add_synthetic_done("a".to_string(), 0);
        shell.jobs.add_synthetic_done("b".to_string(), 0);
        shell.jobs.add_synthetic_done("c".to_string(), 0);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["-a".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.jobs.iter().count(), 0);
    }

    #[test]
    fn disown_r_filters_to_running_only() {
        let mut shell = Shell::new();
        // 2 Running + 1 Done — verifies bare `disown -r` removes BOTH
        // running jobs (bash semantics), not just the current.
        shell.jobs.add(1234, vec![1234], "sleep a".to_string()); // %1 Running
        shell.jobs.add(1235, vec![1235], "sleep b".to_string()); // %2 Running
        shell.jobs.add_synthetic_done("c".to_string(), 0);       // %3 Done
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["-r".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        // Both Running jobs gone; only %3 (Done) remains.
        let states: Vec<JobState> = shell.jobs.iter().map(|j| j.state.clone()).collect();
        assert_eq!(states.len(), 1);
        assert!(matches!(states[0], JobState::Done(_)));
    }

    #[test]
    fn disown_h_marks_for_nohup_keeps_in_table() {
        let mut shell = Shell::new();
        let id = shell.jobs.add(1234, vec![1234], "sleep".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "disown",
            &["-h".to_string(), "%1".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let job = shell.jobs.iter().find(|j| j.id == id).expect("job removed!");
        assert!(job.marked_for_nohup);
    }

    #[test]
    fn disown_multiple_args_processes_each() {
        let mut shell = Shell::new();
        shell.jobs.add_synthetic_done("a".to_string(), 0); // %1
        shell.jobs.add_synthetic_done("b".to_string(), 0); // %2
        shell.jobs.add_synthetic_done("c".to_string(), 0); // %3
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "disown",
            &["%1".to_string(), "%2".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let ids: Vec<u32> = shell.jobs.iter().map(|j| j.id).collect();
        assert_eq!(ids, vec![3]);
    }

    #[test]
    fn disown_ah_marks_all() {
        let mut shell = Shell::new();
        let id1 = shell.jobs.add(1234, vec![1234], "a".to_string());
        let id2 = shell.jobs.add(1235, vec![1235], "b".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["-ah".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.jobs.iter().count(), 2);
        assert!(shell.jobs.iter().find(|j| j.id == id1).unwrap().marked_for_nohup);
        assert!(shell.jobs.iter().find(|j| j.id == id2).unwrap().marked_for_nohup);
    }

    #[test]
    fn disown_ar_removes_all_running() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "a".to_string()); // %1 Running
        shell.jobs.add(1235, vec![1235], "b".to_string()); // %2 Running
        shell.jobs.add_synthetic_done("c".to_string(), 0); // %3 Done
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["-ar".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let states: Vec<JobState> = shell.jobs.iter().map(|j| j.state.clone()).collect();
        assert_eq!(states.len(), 1);
        assert!(matches!(states[0], JobState::Done(_)));
    }

    #[test]
    fn disown_arh_marks_all_running() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "a".to_string()); // %1 Running
        shell.jobs.add(1235, vec![1235], "b".to_string()); // %2 Running
        shell.jobs.add_synthetic_done("c".to_string(), 0); // %3 Done
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["-arh".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.jobs.iter().count(), 3);
        for job in shell.jobs.iter() {
            match job.state {
                JobState::Running => assert!(job.marked_for_nohup, "running job not marked"),
                _ => assert!(!job.marked_for_nohup, "non-running job got marked"),
            }
        }
    }

    #[test]
    fn disown_invalid_flag_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["-x".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn disown_a_ignores_positional_args() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "a".to_string()); // %1
        shell.jobs.add(1235, vec![1235], "b".to_string()); // %2
        shell.jobs.add(1236, vec![1236], "c".to_string()); // %3
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "disown",
            &["-a".to_string(), "%1".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.jobs.iter().count(), 0);
    }

    #[test]
    fn disown_bare_pid_matches_job_leader() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "sleep".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["1234".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.jobs.iter().count(), 0);
    }

    #[test]
    fn disown_bare_pid_matches_pipeline_stage() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234, 1235, 1236], "a | b | c".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["1235".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.jobs.iter().count(), 0);
    }

    #[test]
    fn disown_unknown_pid_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["99999".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn disown_h_with_bare_pid_marks_job() {
        let mut shell = Shell::new();
        let id = shell.jobs.add(1234, vec![1234], "sleep".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "disown",
            &["-h".to_string(), "1234".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let job = shell.jobs.iter().find(|j| j.id == id).expect("job removed!");
        assert!(job.marked_for_nohup);
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn history_lists_numbered_entries() {
        let mut shell = Shell::new();
        Rc::make_mut(&mut shell.history).add("first cmd".to_string());
        Rc::make_mut(&mut shell.history).add("second cmd".to_string());
        let mut out: Vec<u8> = Vec::new();
        let outcome = run_builtin("history", &[], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("first cmd"), "output: {text}");
        assert!(text.contains("second cmd"), "output: {text}");
        assert!(text.contains("1"), "output should have numbers: {text}");
    }

    #[test]
    fn history_dash_c_clears() {
        let mut shell = Shell::new();
        Rc::make_mut(&mut shell.history).add("doomed".to_string());
        let mut out: Vec<u8> = Vec::new();
        let outcome = run_builtin("history", &["-c".to_string()], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.history.last(), None);
    }

    #[test]
    fn history_invalid_option_errors() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let outcome = run_builtin("history", &["--bogus".to_string()], &mut out,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }
}

#[cfg(test)]
mod special_builtin_tests {
    use super::*;

    #[test]
    fn is_special_builtin_recognises_posix_specials() {
        for name in ["break", "continue", "exit", "export", "return", "unset"] {
            assert!(is_special_builtin(name), "expected {name} to be special");
        }
    }

    #[test]
    fn is_special_builtin_rejects_regular_builtins() {
        for name in ["cd", "pwd", "echo", "jobs", "wait", "fg", "bg", "kill", "disown", "history", "test", "["] {
            assert!(!is_special_builtin(name), "expected {name} to be regular");
        }
    }

    #[test]
    fn is_special_builtin_rejects_unknowns() {
        assert!(!is_special_builtin("not_a_builtin"));
        assert!(!is_special_builtin(""));
    }

    #[test]
    fn trap_err_pseudo_signal_registers() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["echo err".to_string(), "ERR".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(shell.traps.contains_key(&crate::traps::TrapSignal::Err));
    }

    #[test]
    fn trap_debug_pseudo_signal_registers() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["echo dbg".to_string(), "DEBUG".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(shell.traps.contains_key(&crate::traps::TrapSignal::Debug));
    }

    #[test]
    fn trap_return_pseudo_signal_registers() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["echo ret".to_string(), "RETURN".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(shell.traps.contains_key(&crate::traps::TrapSignal::Return));
    }

    #[test]
    fn trap_p_lists_pseudo_signals_in_order() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        // Register four pseudo-signals (intentionally not in EXIT/ERR/DEBUG/RETURN order).
        for (action, sig) in [
            ("a-return", "RETURN"),
            ("a-debug", "DEBUG"),
            ("a-exit", "EXIT"),
            ("a-err", "ERR"),
        ] {
            let _ = run_builtin(
                "trap",
                &[action.to_string(), sig.to_string()],
                &mut buf,&mut std::io::stderr(), 
                &mut shell,
            );
        }
        buf.clear();
        let outcome = run_builtin("trap", &["-p".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        // The four pseudo-signals should appear in EXIT, ERR, DEBUG, RETURN order.
        let pseudo_lines: Vec<&&str> = lines.iter()
            .filter(|l| l.contains("EXIT") || l.contains("ERR") || l.contains("DEBUG") || l.contains("RETURN"))
            .collect();
        assert_eq!(pseudo_lines.len(), 4, "expected 4 pseudo-signal lines, got: {out}");
        assert!(pseudo_lines[0].contains("EXIT"), "first line should be EXIT: {}", pseudo_lines[0]);
        assert!(pseudo_lines[1].contains("ERR"), "second line should be ERR: {}", pseudo_lines[1]);
        assert!(pseudo_lines[2].contains("DEBUG"), "third line should be DEBUG: {}", pseudo_lines[2]);
        assert!(pseudo_lines[3].contains("RETURN"), "fourth line should be RETURN: {}", pseudo_lines[3]);
    }
}

#[cfg(test)]
mod alias_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn alias_no_args_lists_empty() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("alias", &[], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(buf.is_empty(), "expected empty output, got {:?}", String::from_utf8_lossy(&buf));
    }

    #[test]
    fn alias_no_args_lists_sorted() {
        let mut shell = Shell::new();
        shell.aliases.insert("ll".to_string(), "ls -l".to_string());
        shell.aliases.insert("la".to_string(), "ls -A".to_string());
        shell.aliases.insert("l".to_string(), "ls".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("alias", &[], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines,
            vec![
                "alias l='ls'",
                "alias la='ls -A'",
                "alias ll='ls -l'",
            ]
        );
    }

    #[test]
    fn alias_defines_simple() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "alias",
            &["ll=ls -l".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.aliases.get("ll").map(|s| s.as_str()), Some("ls -l"));
    }

    #[test]
    fn alias_lookup_existing_prints() {
        let mut shell = Shell::new();
        shell.aliases.insert("ll".to_string(), "ls -l".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("alias", &["ll".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out, "alias ll='ls -l'\n");
    }

    #[test]
    fn alias_lookup_missing_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("alias", &["xyz".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn unalias_removes_existing() {
        let mut shell = Shell::new();
        shell.aliases.insert("ll".to_string(), "ls -l".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("unalias", &["ll".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(!shell.aliases.contains_key("ll"));
    }

    #[test]
    fn unalias_missing_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("unalias", &["xyz".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn unalias_dash_a_clears_all() {
        let mut shell = Shell::new();
        shell.aliases.insert("ll".to_string(), "ls -l".to_string());
        shell.aliases.insert("la".to_string(), "ls -A".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("unalias", &["-a".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(shell.aliases.is_empty());
    }

    #[test]
    fn unalias_no_args_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("unalias", &[], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }
}

#[cfg(test)]
mod shift_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn shift_no_args_removes_first() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("shift", &[], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.positional_args, vec!["b", "c"]);
    }

    #[test]
    fn shift_n_removes_n() {
        let mut shell = Shell::new();
        shell.positional_args = vec![
            "a".to_string(), "b".to_string(), "c".to_string(), "d".to_string(),
        ];
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("shift", &["2".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.positional_args, vec!["c", "d"]);
    }

    #[test]
    fn shift_default_when_no_args_equals_one() {
        let mut shell_a = Shell::new();
        shell_a.positional_args = vec!["x".to_string(), "y".to_string()];
        let mut shell_b = Shell::new();
        shell_b.positional_args = vec!["x".to_string(), "y".to_string()];

        let mut buf: Vec<u8> = Vec::new();
        let _ = run_builtin("shift", &[], &mut buf,&mut std::io::stderr(),  &mut shell_a);
        let _ = run_builtin("shift", &["1".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell_b);

        assert_eq!(shell_a.positional_args, shell_b.positional_args);
        assert_eq!(shell_a.positional_args, vec!["y"]);
    }

    #[test]
    fn shift_too_large_fails_status_1_silently() {
        // bash: an over-range positive count fails (rc 1) SILENTLY — no message.
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("shift", &["5".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        // Positional unchanged after the failed shift.
        assert_eq!(shell.positional_args, vec!["a"]);
    }

    #[test]
    fn shift_negative_count_errors_status_1() {
        // bash: a negative count is "shift count out of range" (rc 1), distinct
        // from the non-numeric "numeric argument required".
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "b".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("shift", &["-1".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        assert_eq!(shell.positional_args, vec!["a", "b"]);
    }

    #[test]
    fn shift_zero_is_noop() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "b".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("shift", &["0".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.positional_args, vec!["a", "b"]);
    }

    #[test]
    fn shift_non_numeric_errors_status_1() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("shift", &["abc".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        assert_eq!(shell.positional_args, vec!["a"]);
    }

    #[test]
    fn shift_negative_errors_status_1() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "b".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        // `-1` fails parse::<usize>() because usize can't be negative.
        let outcome = run_builtin("shift", &["-1".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        assert_eq!(shell.positional_args, vec!["a", "b"]);
    }
}

#[cfg(test)]
mod set_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn set_no_args_lists_sorted_vars() {
        let mut shell = Shell::new();
        // Use unique names unlikely to collide with environment.
        shell.set("ZZTEST_C", "three".to_string());
        shell.set("ZZTEST_A", "one".to_string());
        shell.set("ZZTEST_B", "two".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("set", &[], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        // Find the three target lines and confirm they appear in
        // sorted order relative to each other.
        let a_idx = out.find("ZZTEST_A=").expect("missing A");
        let b_idx = out.find("ZZTEST_B=").expect("missing B");
        let c_idx = out.find("ZZTEST_C=").expect("missing C");
        assert!(a_idx < b_idx, "A should come before B");
        assert!(b_idx < c_idx, "B should come before C");
        // Format check: value should be single-quoted.
        assert!(out.contains("ZZTEST_A='one'"), "expected single-quoted value: {out:?}");
    }

    #[test]
    fn set_double_dash_alone_clears_positional() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "b".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("set", &["--".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(shell.positional_args.is_empty());
    }

    #[test]
    fn set_double_dash_with_args_replaces() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "set",
            &["--".to_string(), "one".to_string(), "two".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.positional_args, vec!["one", "two"]);
    }

    #[test]
    fn set_bare_args_replaces_positional() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "set",
            &["one".to_string(), "two".to_string(), "three".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.positional_args, vec!["one", "two", "three"]);
    }

    #[test]
    fn set_dash_x_enables_xtrace() {
        // -x (xtrace) implemented in v103.
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("set", &["-x".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(shell.shell_options.xtrace);
    }

    #[test]
    fn set_plus_x_disables_xtrace() {
        let mut shell = Shell::new();
        shell.shell_options.xtrace = true;
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("set", &["+x".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(!shell.shell_options.xtrace);
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn source_no_args_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(".", &[], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn source_missing_file_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            ".",
            &["/nonexistent/file/path/huck-v51-test".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn source_depth_limit_errors_status_1() {
        let mut shell = Shell::new();
        shell.source_depth = 64;
        let mut buf: Vec<u8> = Vec::new();
        // Use a path that would otherwise resolve fine — depth check
        // fires before the path resolution.
        let outcome = run_builtin(
            ".",
            &["/etc/hostname".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        // Counter unchanged because the early return bypasses the
        // increment.
        assert_eq!(shell.source_depth, 64);
    }

    #[test]
    fn is_builtin_recognises_dot_and_source() {
        assert!(is_builtin("."));
        assert!(is_builtin("source"));
    }

    #[test]
    fn is_special_builtin_includes_dot_and_source() {
        assert!(is_special_builtin("."));
        assert!(is_special_builtin("source"));
    }
}

#[cfg(test)]
mod local_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn local_outside_function_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        // local_scopes is empty (we never pushed a frame).
        let outcome = run_declaration_builtin_strs(
            "local",
            &["X=hi".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn local_with_value_sets_and_records_snapshot() {
        let mut shell = Shell::new();
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_declaration_builtin_strs(
            "local",
            &["XYZ_LOCAL_T1=hi".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("XYZ_LOCAL_T1").as_deref(), Some("hi"));
        // Snapshot recorded: X was unset before, so snapshot is None.
        let frame = shell.local_scopes.last().unwrap();
        assert!(frame.contains_key("XYZ_LOCAL_T1"));
        assert!(frame["XYZ_LOCAL_T1"].is_none());
    }

    #[test]
    fn local_without_value_leaves_unset() {
        // Bare `local NAME` declares the var function-local but UNSET, matching
        // bash (verified: `f(){ local x; [[ -v x ]] && echo S || echo U; }; f`
        // prints `U`). It used to be set-empty; that was the M-111 bug.
        let mut shell = Shell::new();
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_declaration_builtin_strs(
            "local",
            &["XYZ_LOCAL_T2".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("XYZ_LOCAL_T2").as_deref(), None);
    }

    #[test]
    fn local_snapshots_existing_var() {
        let mut shell = Shell::new();
        shell.set("XYZ_LOCAL_T3", "outer".to_string());
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_declaration_builtin_strs(
            "local",
            &["XYZ_LOCAL_T3=inner".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        // After `local`, the var has the inner value.
        assert_eq!(shell.lookup_var("XYZ_LOCAL_T3").as_deref(), Some("inner"));
        // The frame holds the snapshot of the outer value.
        let snapshot = shell
            .local_scopes
            .last()
            .unwrap()
            .get("XYZ_LOCAL_T3")
            .cloned()
            .unwrap();
        let v = snapshot.expect("expected Some snapshot for previously-set var");
        assert!(matches!(&v.value, crate::shell_state::VarValue::Scalar(s) if s == "outer"));
    }

    #[test]
    fn local_idempotent_in_same_frame() {
        let mut shell = Shell::new();
        shell.set("XYZ_LOCAL_T4", "outer".to_string());
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        // First `local`: snapshot the outer value.
        let _ = run_declaration_builtin_strs(
            "local",
            &["XYZ_LOCAL_T4=first".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        // Second `local` for the same name in the same frame: must NOT
        // re-snapshot (otherwise it would overwrite the outer snapshot
        // with "first").
        let _ = run_declaration_builtin_strs(
            "local",
            &["XYZ_LOCAL_T4=second".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        // Current value reflects the second assignment.
        assert_eq!(shell.lookup_var("XYZ_LOCAL_T4").as_deref(), Some("second"));
        // Snapshot still holds the original outer value.
        let snapshot = shell
            .local_scopes
            .last()
            .unwrap()
            .get("XYZ_LOCAL_T4")
            .cloned()
            .unwrap();
        let v = snapshot.expect("expected Some outer snapshot");
        assert!(matches!(&v.value, crate::shell_state::VarValue::Scalar(s) if s == "outer"));
    }

    #[test]
    fn local_invalid_identifier_errors() {
        let mut shell = Shell::new();
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_declaration_builtin_strs(
            "local",
            &["1foo=bar".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn local_dash_i_marks_integer_and_coerces_rhs() {
        // `local -i x=3+4` evaluates the RHS arithmetically (→ 7) and flags
        // the local integer, like `declare -i`.
        let mut shell = Shell::new();
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_declaration_builtin_strs(
            "local",
            &["-i".to_string(), "XYZ_LI=3+4".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("XYZ_LI").as_deref(), Some("7"));
        assert!(shell.is_integer("XYZ_LI"));
    }

    #[test]
    fn local_dash_i_bare_then_assign_coerces() {
        // `local -i x` followed by `x=2+3` coerces on assignment (→ 5).
        let mut shell = Shell::new();
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        let _ = run_declaration_builtin_strs(
            "local",
            &["-i".to_string(), "XYZ_LIB".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(shell.is_integer("XYZ_LIB"));
        let _ = shell.try_set("XYZ_LIB", "2+3".to_string());
        assert_eq!(shell.lookup_var("XYZ_LIB").as_deref(), Some("5"));
    }

    #[test]
    fn local_dash_r_marks_readonly() {
        let mut shell = Shell::new();
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_declaration_builtin_strs(
            "local",
            &["-r".to_string(), "XYZ_LR=fixed".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("XYZ_LR").as_deref(), Some("fixed"));
        assert!(shell.is_readonly("XYZ_LR"));
    }

    #[test]
    fn local_clustered_ri_applies_both_attrs() {
        // `local -ri n=5+5`: integer (→ 10) AND readonly.
        let mut shell = Shell::new();
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_declaration_builtin_strs(
            "local",
            &["-ri".to_string(), "XYZ_LRI=5+5".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("XYZ_LRI").as_deref(), Some("10"));
        assert!(shell.is_integer("XYZ_LRI"));
        assert!(shell.is_readonly("XYZ_LRI"));
    }

    #[test]
    fn local_nameref_invalid_target_errors() {
        // `local -n XYZ_LU=1` — target "1" is not a valid identifier → rc 1.
        let mut shell = Shell::new();
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_declaration_builtin_strs(
            "local",
            &["-n".to_string(), "XYZ_LU=1".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn local_case_fold_lower_upper() {
        // `local -l v=HELLO` folds to lowercase.
        let mut shell = Shell::new();
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_declaration_builtin_strs(
            "local",
            &["-l".to_string(), "V=HELLO".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("V").as_deref(), Some("hello"));

        // `local -u v=hello` folds to uppercase.
        let mut shell2 = Shell::new();
        shell2.local_scopes.push(std::collections::HashMap::new());
        let mut buf2: Vec<u8> = Vec::new();
        let outcome2 = run_declaration_builtin_strs(
            "local",
            &["-u".to_string(), "W=hello".to_string()],
            &mut buf2,&mut std::io::stderr(), 
            &mut shell2,
        );
        assert!(matches!(outcome2, ExecOutcome::Continue(0)));
        assert_eq!(shell2.lookup_var("W").as_deref(), Some("HELLO"));
    }
}

#[cfg(test)]
mod colon_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn colon_exits_zero() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(":", &[], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn colon_with_args_exits_zero() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["one".to_string(), "two".to_string()];
        let outcome = run_builtin(":", &args, &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }
}

#[cfg(test)]
mod true_false_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn true_exits_zero() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("true", &[], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn false_exits_one() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("false", &[], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn true_and_false_ignore_args() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["ignored".to_string()];
        let t = run_builtin("true", &args, &mut buf,&mut std::io::stderr(),  &mut shell);
        let f = run_builtin("false", &args, &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(t, ExecOutcome::Continue(0)));
        assert!(matches!(f, ExecOutcome::Continue(1)));
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn command_no_args_exits_zero() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("command", &[], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn command_builtin_bare_form_still_errors_when_called_directly() {
        // As of v99 the bare form `command CMD args` is handled in the executor
        // (`run_exec_single` rewrites the program and bypasses function lookup
        // before the `command` builtin is ever reached). The builtin itself
        // retains its defensive rejection for the bare form when invoked
        // directly (e.g. via run_builtin), which this test asserts.
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["echo".to_string(), "hi".to_string()];
        let outcome = run_builtin("command", &args, &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn command_dash_v_builtin_concise() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["-v".to_string(), "echo".to_string()];
        let outcome = run_builtin("command", &args, &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out.trim_end(), "echo");
    }

    #[test]
    fn command_dash_v_notfound_silent_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["-v".to_string(), "__no_such_cmd_xyzzy__".to_string()];
        let outcome = run_builtin("command", &args, &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.is_empty(), "expected silent stdout, got: {out:?}");
    }

    #[test]
    fn command_dash_v_builtin_verbose() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["-V".to_string(), "echo".to_string()];
        let outcome = run_builtin("command", &args, &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out.trim_end(), "echo is a shell builtin");
    }

    #[test]
    fn command_dash_v_keyword_verbose() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["-V".to_string(), "if".to_string()];
        let outcome = run_builtin("command", &args, &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out.trim_end(), "if is a shell keyword");
    }

    #[test]
    fn command_dash_v_function() {
        let mut shell = Shell::new();
        // Register a function directly. The body shape is irrelevant for
        // resolution; any Command value works. Use a no-op assignment list.
        let body = Box::new(crate::command::Command::Simple(
            crate::command::SimpleCommand::Assign(vec![], 0),
        ));
        shell.define_function("myfn".to_string(), body);
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["-v".to_string(), "myfn".to_string()];
        let outcome = run_builtin("command", &args, &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out.trim_end(), "myfn");
    }

    #[test]
    fn command_dash_v_alias_with_single_quote_escapes() {
        let mut shell = Shell::new();
        shell
            .aliases
            .insert("greet".to_string(), "echo it's me".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["-v".to_string(), "greet".to_string()];
        let outcome = run_builtin("command", &args, &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out.trim_end(), r"alias greet='echo it'\''s me'");
    }
}

#[cfg(test)]
mod readonly_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn readonly_with_value_sets_and_locks() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["X=hi".to_string()];
        let outcome = run_declaration_builtin_strs("readonly", &args, &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X").as_deref(), Some("hi"));
        assert!(shell.is_readonly("X"));
    }

    #[test]
    fn readonly_no_value_creates_empty_and_locks() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["X".to_string()];
        let outcome = run_declaration_builtin_strs("readonly", &args, &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X").as_deref(), Some(""));
        assert!(shell.is_readonly("X"));
    }

    #[test]
    fn readonly_no_value_keeps_existing_value() {
        let mut shell = Shell::new();
        shell.set("X", "prev".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["X".to_string()];
        let outcome = run_declaration_builtin_strs("readonly", &args, &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X").as_deref(), Some("prev"));
        assert!(shell.is_readonly("X"));
    }

    #[test]
    fn readonly_multi_arg_mixed_forms() {
        let mut shell = Shell::new();
        shell.set("B", "had".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["A=1".to_string(), "B".to_string(), "C=3".to_string()];
        let outcome = run_declaration_builtin_strs("readonly", &args, &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("A").as_deref(), Some("1"));
        assert_eq!(shell.lookup_var("B").as_deref(), Some("had"));
        assert_eq!(shell.lookup_var("C").as_deref(), Some("3"));
        assert!(shell.is_readonly("A"));
        assert!(shell.is_readonly("B"));
        assert!(shell.is_readonly("C"));
    }

    #[test]
    fn readonly_invalid_identifier_errors() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["1foo=bar".to_string()];
        let outcome = run_declaration_builtin_strs("readonly", &args, &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        assert!(shell.lookup_var("1foo").is_none());
    }

    #[test]
    fn readonly_listing_no_args() {
        let mut shell = Shell::new();
        shell.set("X", "v".to_string());
        shell.mark_readonly("X");
        shell.set("Y", "w".to_string());
        shell.mark_readonly("Y");
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_declaration_builtin_strs("readonly", &[], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        // declare -p style listing; scalars render with `-r` attrs.
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines.contains(&r#"declare -r X="v""#));
        assert!(lines.contains(&r#"declare -r Y="w""#));
    }

    #[test]
    fn readonly_dash_p_same_as_no_args() {
        let mut shell = Shell::new();
        shell.set("X", "v".to_string());
        shell.mark_readonly("X");
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_declaration_builtin_strs("readonly", &["-p".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.lines().any(|l| l == r#"declare -r X="v""#));
    }

    #[test]
    fn readonly_overwrite_existing_readonly_errors() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        run_declaration_builtin_strs("readonly", &["X=first".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        let outcome = run_declaration_builtin_strs(
            "readonly",
            &["X=second".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        assert_eq!(shell.lookup_var("X").as_deref(), Some("first"));
        assert!(shell.is_readonly("X"));
    }

    #[test]
    fn unset_readonly_errors_status_1() {
        let mut shell = Shell::new();
        shell.set("X", "v".to_string());
        shell.mark_readonly("X");
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("unset", &["X".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        assert_eq!(shell.lookup_var("X").as_deref(), Some("v"));
    }

    #[test]
    fn export_readonly_value_errors_but_bare_export_succeeds() {
        let mut shell = Shell::new();
        shell.set("X", "v".to_string());
        shell.mark_readonly("X");
        let mut buf: Vec<u8> = Vec::new();
        // `export X=newval` should error and not overwrite.
        let bad = run_declaration_builtin_strs(
            "export",
            &["X=newval".to_string()],
            &mut buf,&mut std::io::stderr(), 
            &mut shell,
        );
        assert!(matches!(bad, ExecOutcome::Continue(1)));
        assert_eq!(shell.lookup_var("X").as_deref(), Some("v"));
        // `export X` (bare) should succeed and flip the export flag.
        let bare = run_declaration_builtin_strs("export", &["X".to_string()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(bare, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X").as_deref(), Some("v"));
        assert!(shell.is_readonly("X"));
    }

    #[test]
    fn export_set_preserves_readonly_flag_on_existing_var() {
        // Regression: export_set must not silently strip the readonly
        // flag on an already-present Variable. Without the fix, a
        // future Task 2 caller (apply_inline_assignments) that bypasses
        // the is_readonly check would clobber readonly state.
        let mut shell = Shell::new();
        shell.set("X", "outer".to_string());
        shell.mark_readonly("X");
        // Direct call to export_set on an already-readonly var.
        shell.export_set("X", "new".to_string());
        // Value updated, but readonly flag must stay set.
        assert!(shell.is_readonly("X"));
    }
}

#[cfg(test)]
mod read_tests {
    use super::*;
    use std::io::Cursor;

    // ── read_one_line ──────────────────────────────────────────

    #[test]
    fn read_one_line_basic() {
        let mut c = Cursor::new(b"hello\n".as_slice());
        let r = read_one_line(&mut c, false, b'\n').unwrap();
        assert_eq!(r.as_deref(), Some("hello"));
    }

    #[test]
    fn read_one_line_eof_returns_none() {
        let mut c = Cursor::new(b"".as_slice());
        let r = read_one_line(&mut c, false, b'\n').unwrap();
        assert_eq!(r, None);
    }

    #[test]
    fn read_one_line_eof_partial_returns_some() {
        let mut c = Cursor::new(b"abc".as_slice());
        let r = read_one_line(&mut c, false, b'\n').unwrap();
        assert_eq!(r.as_deref(), Some("abc"));
    }

    #[test]
    fn read_one_line_escape_removal() {
        // "a\\bc\n" — non-raw → "abc" (\\b → b).
        let mut c = Cursor::new(b"a\\bc\n".as_slice());
        let r = read_one_line(&mut c, false, b'\n').unwrap();
        assert_eq!(r.as_deref(), Some("abc"));
    }

    #[test]
    fn read_one_line_line_continuation() {
        // "a\\\nb\n" — non-raw → "ab".
        let mut c = Cursor::new(b"a\\\nb\n".as_slice());
        let r = read_one_line(&mut c, false, b'\n').unwrap();
        assert_eq!(r.as_deref(), Some("ab"));
    }

    #[test]
    fn read_one_line_raw_preserves_backslash() {
        // "a\\b\n" — raw → "a\\b".
        let mut c = Cursor::new(b"a\\b\n".as_slice());
        let r = read_one_line(&mut c, true, b'\n').unwrap();
        assert_eq!(r.as_deref(), Some("a\\b"));
    }

    #[test]
    fn read_one_line_custom_delim() {
        let mut c = Cursor::new(b"foo:bar\n".as_slice());
        let r = read_one_line(&mut c, false, b':').unwrap();
        assert_eq!(r.as_deref(), Some("foo"));
    }

    #[test]
    fn read_one_line_nul_delim() {
        let mut c = Cursor::new(b"foo\0bar".as_slice());
        let r = read_one_line(&mut c, false, 0u8).unwrap();
        assert_eq!(r.as_deref(), Some("foo"));
    }

    // ── read_one_record ────────────────────────────────────────

    #[test]
    fn read_one_record_newline_delim() {
        let mut r = std::io::Cursor::new(b"a\nb\n".to_vec());
        assert_eq!(read_one_record(&mut r, b'\n').unwrap(), Some(("a".to_string(), true)));
        assert_eq!(read_one_record(&mut r, b'\n').unwrap(), Some(("b".to_string(), true)));
        assert_eq!(read_one_record(&mut r, b'\n').unwrap(), None);
    }

    #[test]
    fn read_one_record_unterminated_last() {
        let mut r = std::io::Cursor::new(b"a\nb".to_vec());
        assert_eq!(read_one_record(&mut r, b'\n').unwrap(), Some(("a".to_string(), true)));
        assert_eq!(read_one_record(&mut r, b'\n').unwrap(), Some(("b".to_string(), false)));
        assert_eq!(read_one_record(&mut r, b'\n').unwrap(), None);
    }

    #[test]
    fn read_one_record_custom_delim_keeps_other_bytes() {
        let mut r = std::io::Cursor::new(b"a:b:c\n".to_vec());
        assert_eq!(read_one_record(&mut r, b':').unwrap(), Some(("a".to_string(), true)));
        assert_eq!(read_one_record(&mut r, b':').unwrap(), Some(("b".to_string(), true)));
        assert_eq!(read_one_record(&mut r, b':').unwrap(), Some(("c\n".to_string(), false)));
        assert_eq!(read_one_record(&mut r, b':').unwrap(), None);
    }

    // ── split_into_names ───────────────────────────────────────

    #[test]
    fn split_into_names_single_name_strip_ws() {
        let names = vec!["X".to_string()];
        let r = split_into_names("  hi  ", &names, " \t\n");
        assert_eq!(r, vec![("X".to_string(), "hi".to_string())]);
    }

    #[test]
    fn split_into_names_multi_simple() {
        let names = vec!["X".to_string(), "Y".to_string(), "Z".to_string()];
        let r = split_into_names("a b c d", &names, " \t\n");
        assert_eq!(
            r,
            vec![
                ("X".to_string(), "a".to_string()),
                ("Y".to_string(), "b".to_string()),
                ("Z".to_string(), "c d".to_string()),
            ]
        );
    }

    #[test]
    fn split_into_names_more_names_than_fields() {
        let names = vec!["X".to_string(), "Y".to_string(), "Z".to_string()];
        let r = split_into_names("a b", &names, " \t\n");
        assert_eq!(
            r,
            vec![
                ("X".to_string(), "a".to_string()),
                ("Y".to_string(), "b".to_string()),
                ("Z".to_string(), String::new()),
            ]
        );
    }

    #[test]
    fn split_into_names_custom_ifs_colon() {
        let names = vec!["X".to_string(), "Y".to_string()];
        let r = split_into_names("a:b:c", &names, ":");
        assert_eq!(
            r,
            vec![
                ("X".to_string(), "a".to_string()),
                ("Y".to_string(), "b:c".to_string()),
            ]
        );
    }

    #[test]
    fn split_read_fields_default_ws() {
        assert_eq!(split_read_fields("a b c", " \t\n"), vec!["a", "b", "c"]);
        assert_eq!(split_read_fields("  x   y  ", " \t\n"), vec!["x", "y"]); // trim + collapse
        assert_eq!(split_read_fields("", " \t\n"), Vec::<String>::new());   // empty -> none
    }

    #[test]
    fn split_read_fields_nonws_ifs() {
        assert_eq!(split_read_fields("a:b:c", ":"), vec!["a", "b", "c"]);
        assert_eq!(split_read_fields("x:y:", ":"), vec!["x", "y"]);       // trailing delim: NO empty
        assert_eq!(split_read_fields(":x", ":"), vec!["", "x"]);          // leading delim: empty first
        assert_eq!(split_read_fields("x::y", ":"), vec!["x", "", "y"]);   // adjacent: empty between
    }

    #[test]
    fn split_read_fields_mixed_and_empty_ifs() {
        assert_eq!(split_read_fields("x : y", " :"), vec!["x", "y"]);     // ws around nonws collapses
        assert_eq!(split_read_fields("a b c", ""), vec!["a b c"]);        // empty IFS -> one field
        assert_eq!(split_read_fields("", ""), Vec::<String>::new());      // empty IFS + empty -> none
    }
}

#[cfg(test)]
mod printf_tests {
    use super::*;

    // ── escape decoder ─────────────────────────────────────────

    #[test]
    fn escape_basic() {
        assert_eq!(decode_printf_escape(b"n"), (b"\n".to_vec(), 1));
        assert_eq!(decode_printf_escape(b"t"), (b"\t".to_vec(), 1));
        assert_eq!(decode_printf_escape(b"\\"), (b"\\".to_vec(), 1));
    }

    #[test]
    fn escape_octal() {
        // \101 → 'A'
        assert_eq!(decode_printf_escape(b"101"), (b"A".to_vec(), 3));
        // \0101 → still 'A' (\0 prefix allows up to 4 digits)
        let (v, n) = decode_printf_escape(b"0101");
        assert_eq!(v, b"A".to_vec());
        assert_eq!(n, 4);
    }

    #[test]
    fn escape_hex() {
        // \x41 → 'A'
        assert_eq!(decode_printf_escape(b"x41"), (b"A".to_vec(), 3));
        // \x4 → byte 0x04 (one hex digit consumed)
        let (v, n) = decode_printf_escape(b"x4");
        assert_eq!(v, vec![0x04]);
        assert_eq!(n, 2);
    }

    #[test]
    fn escape_unknown_preserved() {
        // \z → literal "\\z"
        assert_eq!(decode_printf_escape(b"z"), (b"\\z".to_vec(), 1));
    }

    #[test]
    fn escape_trailing_backslash() {
        // Empty rest after `\` → literal "\\"
        assert_eq!(decode_printf_escape(b""), (b"\\".to_vec(), 0));
    }

    // ── parse_printf_int ───────────────────────────────────────

    #[test]
    fn parse_printf_int_decimal() {
        let (v, e) = parse_printf_int("42");
        assert_eq!(v, 42);
        assert!(e.is_none());
    }

    #[test]
    fn parse_printf_int_negative_hex_octal() {
        assert_eq!(parse_printf_int("-42").0, -42);
        assert_eq!(parse_printf_int("0x1F").0, 31);
        assert_eq!(parse_printf_int("017").0, 15);
    }

    #[test]
    fn parse_printf_int_char_literal() {
        assert_eq!(parse_printf_int("'A").0, 65);
        assert_eq!(parse_printf_int("\"A").0, 65);
    }

    #[test]
    fn parse_printf_int_trailing_garbage() {
        let (v, e) = parse_printf_int("42abc");
        assert_eq!(v, 42);
        assert!(e.is_some(), "expected error message");
    }

    // ── parse_format ───────────────────────────────────────────

    #[test]
    fn parse_format_literal_only() {
        let p = parse_format("hello\\n").unwrap();
        assert_eq!(p.len(), 1);
        match &p[0] {
            FormatPart::Literal(b) => assert_eq!(b, b"hello\n"),
            _ => panic!(),
        }
    }

    #[test]
    fn parse_format_simple_conv() {
        let p = parse_format("%s").unwrap();
        assert_eq!(p.len(), 1);
        match &p[0] {
            FormatPart::Conv(c) => {
                assert_eq!(c.conv, ConvChar::S);
                assert_eq!(c.width, None);
                assert_eq!(c.precision, None);
                assert_eq!(c.flags, ConvFlags::default());
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_format_flags_width_prec() {
        let p = parse_format("%-5.3d").unwrap();
        assert_eq!(p.len(), 1);
        match &p[0] {
            FormatPart::Conv(c) => {
                assert!(c.flags.left_align);
                assert_eq!(c.width, Some(5));
                assert_eq!(c.precision, Some(3));
                assert_eq!(c.conv, ConvChar::D);
            }
            _ => panic!(),
        }
    }

    // ── format_one ─────────────────────────────────────────────

    #[test]
    fn format_s_basic() {
        let mut out = Vec::new();
        let spec = ConvSpec {
            flags: ConvFlags::default(),
            width: None,
            precision: None,
            width_star: false,
            prec_star: false,
            conv: ConvChar::S,
        };
        format_one(&spec, "hi", &mut out).unwrap();
        assert_eq!(out, b"hi");
    }

    #[test]
    fn format_s_width() {
        let mut out = Vec::new();
        let spec = ConvSpec {
            flags: ConvFlags::default(),
            width: Some(5),
            precision: None,
            width_star: false,
            prec_star: false,
            conv: ConvChar::S,
        };
        format_one(&spec, "hi", &mut out).unwrap();
        assert_eq!(out, b"   hi");
    }

    #[test]
    fn format_s_left_align() {
        let mut out = Vec::new();
        let spec = ConvSpec {
            flags: ConvFlags {
                left_align: true,
                ..ConvFlags::default()
            },
            width: Some(5),
            precision: None,
            width_star: false,
            prec_star: false,
            conv: ConvChar::S,
        };
        format_one(&spec, "hi", &mut out).unwrap();
        assert_eq!(out, b"hi   ");
    }

    #[test]
    fn format_s_precision_truncates() {
        let mut out = Vec::new();
        let spec = ConvSpec {
            flags: ConvFlags::default(),
            width: None,
            precision: Some(3),
            width_star: false,
            prec_star: false,
            conv: ConvChar::S,
        };
        format_one(&spec, "hello", &mut out).unwrap();
        assert_eq!(out, b"hel");
    }

    #[test]
    fn format_d_basic() {
        let mut out = Vec::new();
        let spec = ConvSpec {
            flags: ConvFlags::default(),
            width: None,
            precision: None,
            width_star: false,
            prec_star: false,
            conv: ConvChar::D,
        };
        format_one(&spec, "42", &mut out).unwrap();
        assert_eq!(out, b"42");
    }

    #[test]
    fn format_d_zero_pad() {
        let mut out = Vec::new();
        let spec = ConvSpec {
            flags: ConvFlags {
                zero_pad: true,
                ..ConvFlags::default()
            },
            width: Some(5),
            precision: None,
            width_star: false,
            prec_star: false,
            conv: ConvChar::D,
        };
        format_one(&spec, "42", &mut out).unwrap();
        assert_eq!(out, b"00042");
    }

    #[test]
    fn format_x_alt_form() {
        let mut out = Vec::new();
        let spec_x = ConvSpec {
            flags: ConvFlags {
                alt: true,
                ..ConvFlags::default()
            },
            width: None,
            precision: None,
            width_star: false,
            prec_star: false,
            conv: ConvChar::X,
        };
        format_one(&spec_x, "255", &mut out).unwrap();
        assert_eq!(out, b"0xff");

        let mut out2 = Vec::new();
        let spec_bigx = ConvSpec {
            flags: ConvFlags {
                alt: true,
                ..ConvFlags::default()
            },
            width: None,
            precision: None,
            width_star: false,
            prec_star: false,
            conv: ConvChar::BigX,
        };
        format_one(&spec_bigx, "255", &mut out2).unwrap();
        assert_eq!(out2, b"0XFF");
    }

    #[test]
    fn format_b_arg_escapes() {
        let mut out = Vec::new();
        let spec = ConvSpec {
            flags: ConvFlags::default(),
            width: None,
            precision: None,
            width_star: false,
            prec_star: false,
            conv: ConvChar::B,
        };
        format_one(&spec, "a\\tb", &mut out).unwrap();
        assert_eq!(out, b"a\tb");
    }

    #[test]
    fn format_d_precision_zero_with_value_zero_emits_empty() {
        // POSIX: precision 0 + value 0 produces no digits.
        // Regression for `%.0d` of 0 returning "0" instead of "".
        let mut out = Vec::new();
        let spec = ConvSpec {
            flags: ConvFlags::default(),
            width: None,
            precision: Some(0),
            width_star: false,
            prec_star: false,
            conv: ConvChar::D,
        };
        format_one(&spec, "0", &mut out).unwrap();
        assert_eq!(out, b"");

        // Sanity: precision 0 with NON-zero value still produces digits.
        let mut out2 = Vec::new();
        format_one(&spec, "5", &mut out2).unwrap();
        assert_eq!(out2, b"5");
    }

    #[test]
    fn format_float_via_snprintf() {
        let mut out = Vec::new();
        let spec = ConvSpec {
            flags: ConvFlags::default(),
            width: Some(5),
            precision: Some(2),
            width_star: false,
            prec_star: false,
            conv: ConvChar::Float(b'f'),
        };
        format_one(&spec, "3.14159", &mut out).unwrap();
        assert_eq!(out, b" 3.14");
    }

    #[test]
    fn format_float_invalid_arg_reports_err() {
        let mut out = Vec::new();
        let spec = ConvSpec {
            flags: ConvFlags::default(),
            width: None,
            precision: None,
            width_star: false,
            prec_star: false,
            conv: ConvChar::Float(b'f'),
        };
        // Non-numeric arg → 0.000000 plus an error (caller sets rc 1).
        let err = format_one(&spec, "abc", &mut out).unwrap_err();
        assert!(err.contains("invalid number"));
        assert_eq!(out, b"0.000000");
    }

    #[test]
    fn parse_format_accepts_star_and_floats() {
        // `*` width/precision flagged; float convs parsed.
        let parts = parse_format("%*.*f").unwrap();
        match &parts[0] {
            FormatPart::Conv(c) => {
                assert!(c.width_star);
                assert!(c.prec_star);
                assert_eq!(c.conv, ConvChar::Float(b'f'));
            }
            _ => panic!("expected a conv part"),
        }
    }
}

#[cfg(test)]
mod exit_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn exit_no_args_inherits_last_status() {
        let mut shell = Shell::new();
        shell.set_last_status(42);
        let outcome = builtin_exit(&[],&mut std::io::stderr(),  &shell);
        assert!(matches!(outcome, ExecOutcome::Exit(42)));
    }

    #[test]
    fn exit_no_args_inherits_zero_when_clean() {
        let shell = Shell::new();
        let outcome = builtin_exit(&[],&mut std::io::stderr(),  &shell);
        assert!(matches!(outcome, ExecOutcome::Exit(0)));
    }
}

#[cfg(test)]
mod type_tests {
    use super::*;
    use crate::shell_state::Shell;

    fn run(args: &[&str], shell: &mut Shell) -> (ExecOutcome, String) {
        let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("type", &args_owned, &mut buf,&mut std::io::stderr(),  shell);
        (outcome, String::from_utf8(buf).unwrap())
    }

    #[test]
    fn type_default_builtin() {
        let mut shell = Shell::new();
        let (oc, out) = run(&["echo"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(out.trim_end(), "echo is a shell builtin");
    }

    #[test]
    fn type_default_keyword() {
        let mut shell = Shell::new();
        let (oc, out) = run(&["if"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(out.trim_end(), "if is a shell keyword");
    }

    #[test]
    fn type_default_function() {
        let mut shell = Shell::new();
        let body = Box::new(crate::command::Command::Simple(
            crate::command::SimpleCommand::Assign(vec![], 0),
        ));
        shell.define_function("myfn".to_string(), body);
        let (oc, out) = run(&["myfn"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(out.trim_end(), "myfn is a function");
    }

    #[test]
    fn type_default_alias() {
        let mut shell = Shell::new();
        shell.aliases.insert("ll".to_string(), "ls -l".to_string());
        let (oc, out) = run(&["ll"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(out.trim_end(), "ll is aliased to `ls -l'");
    }

    #[test]
    fn type_default_not_found() {
        let mut shell = Shell::new();
        let (oc, out) = run(&["__xyz_no_such_cmd__"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
        assert!(out.is_empty(), "stdout should be empty, got: {out:?}");
    }

    #[test]
    fn type_t_builtin() {
        let mut shell = Shell::new();
        let (oc, out) = run(&["-t", "echo"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(out.trim_end(), "builtin");
    }

    #[test]
    fn type_t_keyword() {
        let mut shell = Shell::new();
        let (oc, out) = run(&["-t", "if"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(out.trim_end(), "keyword");
    }

    #[test]
    fn type_t_function() {
        let mut shell = Shell::new();
        let body = Box::new(crate::command::Command::Simple(
            crate::command::SimpleCommand::Assign(vec![], 0),
        ));
        shell.define_function("myfn".to_string(), body);
        let (oc, out) = run(&["-t", "myfn"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(out.trim_end(), "function");
    }

    #[test]
    fn type_t_not_found_silent() {
        let mut shell = Shell::new();
        let (oc, out) = run(&["-t", "__xyz_no_such_cmd__"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
        assert!(out.is_empty(), "stdout should be empty, got: {out:?}");
    }

    #[test]
    fn type_p_builtin_silent() {
        let mut shell = Shell::new();
        let (oc, out) = run(&["-p", "echo"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(out.is_empty(), "stdout should be empty, got: {out:?}");
    }

    #[test]
    fn type_a_alias_and_builtin() {
        // alias "echo=foo" + builtin "echo": -a should list both.
        let mut shell = Shell::new();
        shell.aliases.insert("echo".to_string(), "foo".to_string());
        let (oc, out) = run(&["-a", "echo"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        let lines: Vec<&str> = out.lines().collect();
        assert!(
            lines.iter().any(|l| l.contains("aliased to `foo'")),
            "expected alias line; got: {lines:?}",
        );
        assert!(
            lines.contains(&"echo is a shell builtin"),
            "expected builtin line; got: {lines:?}",
        );
    }

    #[test]
    fn type_f_skips_function() {
        let mut shell = Shell::new();
        let body = Box::new(crate::command::Command::Simple(
            crate::command::SimpleCommand::Assign(vec![], 0),
        ));
        shell.define_function("myfn".to_string(), body);
        // Without -f: would find the function.
        let (oc, _) = run(&["-f", "myfn"], &mut shell);
        // With -f: function ignored, no other match → not found.
        assert!(matches!(oc, ExecOutcome::Continue(1)));
    }

    #[test]
    fn type_capital_p_force_path() {
        // type -P sh: skip builtin precedence, look up sh in PATH.
        // Test environment is expected to have /bin/sh or /usr/bin/sh.
        let mut shell = Shell::new();
        let (oc, out) = run(&["-P", "sh"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(
            out.lines().any(|l| l.ends_with("/sh")),
            "expected a path ending in /sh; got: {out:?}",
        );
    }

    #[test]
    fn type_multi_name_first_found_second_missing() {
        let mut shell = Shell::new();
        let (oc, out) = run(&["echo", "__xyz_no_such_cmd__"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
        assert!(
            out.lines().any(|l| l == "echo is a shell builtin"),
            "stdout should have echo line; got: {out:?}",
        );
    }

    #[test]
    fn type_invalid_option_status_2() {
        let mut shell = Shell::new();
        let (oc, _out) = run(&["-X", "echo"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(2)));
    }
}

#[cfg(test)]
mod hash_tests {
    use super::*;
    use crate::shell_state::Shell;

    fn run(args: &[&str], shell: &mut Shell) -> (ExecOutcome, String) {
        let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("hash", &args_owned, &mut buf,&mut std::io::stderr(),  shell);
        (outcome, String::from_utf8(buf).unwrap())
    }

    #[test]
    fn hash_empty_lists_empty() {
        let mut shell = Shell::new();
        let (oc, out) = run(&[], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(out, "hash: hash table empty\n");
    }

    #[test]
    fn hash_p_adds_direct() {
        let mut shell = Shell::new();
        let (oc, _out) = run(&["-p", "/custom", "mycmd"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        let entry = shell.command_hash.get("mycmd");
        assert!(entry.is_some());
        let (path, hits) = entry.unwrap();
        assert_eq!(path, &std::path::PathBuf::from("/custom"));
        assert_eq!(*hits, 0);
    }

    #[test]
    fn hash_r_clears() {
        let mut shell = Shell::new();
        run(&["-p", "/custom", "mycmd"], &mut shell);
        let (oc, _) = run(&["-r"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.command_hash.is_empty());
    }

    #[test]
    fn hash_d_removes() {
        let mut shell = Shell::new();
        run(&["-p", "/custom", "mycmd"], &mut shell);
        let (oc, _) = run(&["-d", "mycmd"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.command_hash.is_empty());
    }

    #[test]
    fn hash_d_missing_errors() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-d", "mycmd"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
    }

    #[test]
    fn hash_l_re_input_form() {
        let mut shell = Shell::new();
        run(&["-p", "/foo", "a"], &mut shell);
        let (oc, out) = run(&["-l"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(out, "builtin hash -p /foo a\n");
    }

    #[test]
    fn hash_t_single_name() {
        let mut shell = Shell::new();
        run(&["-p", "/foo", "a"], &mut shell);
        let (oc, out) = run(&["-t", "a"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(out, "/foo\n");
    }

    #[test]
    fn hash_t_multi_name_tabs() {
        let mut shell = Shell::new();
        run(&["-p", "/foo", "a"], &mut shell);
        run(&["-p", "/bar", "b"], &mut shell);
        let (oc, out) = run(&["-t", "a", "b"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        // Order matches the input args, not HashMap order.
        assert_eq!(out, "a\t/foo\nb\t/bar\n");
    }

    #[test]
    fn hash_t_missing_errors_status_1() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-t", "a"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
    }

    #[test]
    fn hash_path_like_name_rejected() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["a/b"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
        assert!(shell.command_hash.is_empty());
    }

    #[test]
    fn hash_invalid_option_status_2() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-X"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(2)));
    }

    #[test]
    fn hash_p_no_arg_status_2() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-p"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(2)));
    }
}

#[cfg(test)]
mod dirstack_tests {
    use super::*;
    use crate::shell_state::Shell;
    use std::path::PathBuf;

    // ── parse_signed_index ────────────────────────────────────

    #[test]
    fn parse_signed_index_plus() {
        assert_eq!(parse_signed_index("+0", 10).unwrap(), 0);
        assert_eq!(parse_signed_index("+2", 10).unwrap(), 2);
        assert_eq!(parse_signed_index("+5", 10).unwrap(), 5);
    }

    #[test]
    fn parse_signed_index_minus() {
        // length 10: -0 = last (9); -1 = 8; -9 = 0.
        assert_eq!(parse_signed_index("-0", 10).unwrap(), 9);
        assert_eq!(parse_signed_index("-1", 10).unwrap(), 8);
        assert_eq!(parse_signed_index("-9", 10).unwrap(), 0);
    }

    #[test]
    fn parse_signed_index_out_of_range() {
        assert!(parse_signed_index("+10", 10).is_err());
        assert!(parse_signed_index("-10", 10).is_err());
    }

    #[test]
    fn parse_signed_index_invalid() {
        assert!(parse_signed_index("+abc", 10).is_err());
    }

    #[test]
    fn parse_signed_index_no_sign() {
        assert!(parse_signed_index("2", 10).is_err());
    }

    // ── dir_display ───────────────────────────────────────────

    #[test]
    fn dir_display_no_home_unchanged() {
        let mut shell = Shell::new();
        shell.set("HOME", String::new());
        // Also clear process env to be safe.
        let saved = std::env::var("HOME").ok();
        unsafe {
            std::env::remove_var("HOME");
        }
        let out = dir_display(&PathBuf::from("/etc"), &shell, true);
        unsafe {
            if let Some(h) = saved {
                std::env::set_var("HOME", h);
            }
        }
        assert_eq!(out, "/etc");
    }

    #[test]
    fn dir_display_home_match_collapses() {
        let mut shell = Shell::new();
        shell.set("HOME", "/h/me".to_string());
        assert_eq!(dir_display(&PathBuf::from("/h/me"), &shell, true), "~",);
    }

    #[test]
    fn dir_display_home_subdir_collapses() {
        let mut shell = Shell::new();
        shell.set("HOME", "/h/me".to_string());
        assert_eq!(
            dir_display(&PathBuf::from("/h/me/x"), &shell, true),
            "~/x",
        );
    }

    #[test]
    fn dir_display_no_collapse_flag() {
        let mut shell = Shell::new();
        shell.set("HOME", "/h/me".to_string());
        assert_eq!(
            dir_display(&PathBuf::from("/h/me/x"), &shell, false),
            "/h/me/x",
        );
    }

    #[test]
    fn dir_display_unrelated_path_passes_through() {
        let mut shell = Shell::new();
        shell.set("HOME", "/h/me".to_string());
        assert_eq!(
            dir_display(&PathBuf::from("/etc/foo"), &shell, true),
            "/etc/foo",
        );
    }

    // ── is_signed_index_arg ───────────────────────────────────

    #[test]
    fn is_signed_index_arg_recognizes_numeric_forms() {
        assert!(is_signed_index_arg("+0"));
        assert!(is_signed_index_arg("+12"));
        assert!(is_signed_index_arg("-0"));
        assert!(is_signed_index_arg("-5"));
    }

    #[test]
    fn is_signed_index_arg_rejects_alpha_after_sign() {
        // Regression: previously the `+` branch had no digit guard,
        // so `+foo` (a literal directory name) was misclassified
        // as an index specifier. Match the symmetric `-foo` rule.
        assert!(!is_signed_index_arg("+foo"));
        assert!(!is_signed_index_arg("+bar"));
        assert!(!is_signed_index_arg("-foo"));
        assert!(!is_signed_index_arg("-bar"));
    }

    #[test]
    fn is_signed_index_arg_rejects_bare_signs_and_paths() {
        assert!(!is_signed_index_arg("+"));
        assert!(!is_signed_index_arg("-"));
        assert!(!is_signed_index_arg("/tmp"));
        assert!(!is_signed_index_arg("relative"));
        assert!(!is_signed_index_arg(""));
    }
}

#[cfg(test)]
mod declare_tests {
    use super::*;
    use crate::shell_state::Shell;

    fn run(args: &[&str], shell: &mut Shell) -> (ExecOutcome, String) {
        let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_declaration_builtin_strs("declare", &args_owned, &mut buf,&mut std::io::stderr(),  shell);
        (outcome, String::from_utf8(buf).unwrap())
    }

    fn run_typeset(args: &[&str], shell: &mut Shell) -> ExecOutcome {
        let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut buf: Vec<u8> = Vec::new();
        run_declaration_builtin_strs("typeset", &args_owned, &mut buf,&mut std::io::stderr(),  shell)
    }

    #[test]
    fn declare_bare_sets_var() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["X_DECL=hi"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X_DECL").as_deref(), Some("hi"));
    }

    #[test]
    fn declare_r_sets_and_locks() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-r", "X_DECL_R=hi"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X_DECL_R").as_deref(), Some("hi"));
        assert!(shell.is_readonly("X_DECL_R"));
    }

    #[test]
    fn declare_x_sets_and_exports() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-x", "X_DECL_X=hi"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X_DECL_X").as_deref(), Some("hi"));
        assert!(shell.is_exported("X_DECL_X"));
    }

    #[test]
    fn declare_rx_combines() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-rx", "X_DECL_RX=hi"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.is_readonly("X_DECL_RX"));
        assert!(shell.is_exported("X_DECL_RX"));
    }

    #[test]
    fn declare_plus_x_unexports() {
        let mut shell = Shell::new();
        shell.export_set("X_DECL_UNEX", "v".to_string());
        assert!(shell.is_exported("X_DECL_UNEX"));
        let (oc, _) = run(&["+x", "X_DECL_UNEX"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X_DECL_UNEX").as_deref(), Some("v"));
        assert!(!shell.is_exported("X_DECL_UNEX"));
    }

    #[test]
    fn declare_plus_r_errors() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["+r", "X_FOO"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
    }

    #[test]
    fn declare_p_prints_known_var() {
        let mut shell = Shell::new();
        shell.set("X_DECL_P", "hi".to_string());
        let (oc, out) = run(&["-p", "X_DECL_P"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(out, "declare -- X_DECL_P=\"hi\"\n");
    }

    #[test]
    fn declare_p_missing_errors() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-p", "X_DECL_MISSING"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
    }

    #[test]
    fn declare_f_lists_functions() {
        let mut shell = Shell::new();
        let body = Box::new(crate::command::Command::Simple(
            crate::command::SimpleCommand::Assign(vec![], 0),
        ));
        shell.define_function("fn1".to_string(), body.clone());
        shell.define_function("fn2".to_string(), body);
        let (oc, out) = run(&["-f"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        // v146: `-f` prints the normalized function body via `generate`, so
        // each function shows its `NAME ()` header (not the old `declare -f`
        // stub line). Sorted; both present.
        assert!(out.contains("fn1 ()"), "got {out:?}");
        assert!(out.contains("fn2 ()"), "got {out:?}");
        assert!(
            out.find("fn1").unwrap() < out.find("fn2").unwrap(),
            "expected sorted; got {out:?}",
        );
    }

    #[test]
    fn declare_f_missing_is_silent() {
        // bash: `declare -f`/`-F` on a missing function emits nothing on
        // stdout and returns rc 1 (the "not found" stderr line is gone).
        let mut shell = Shell::new();
        let (oc, out) = run(&["-f", "nope"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
        assert_eq!(out, "");
    }

    #[test]
    fn declare_f_named_function_found() {
        let mut shell = Shell::new();
        let body = Box::new(crate::command::Command::Simple(
            crate::command::SimpleCommand::Assign(vec![], 0),
        ));
        shell.define_function("fn1".to_string(), body);
        let (oc, out) = run(&["-F", "fn1"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(out, "declare -f fn1\n");
    }

    #[test]
    fn declare_f_named_function_missing() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-F", "fn_none"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
    }

    #[test]
    fn declare_invalid_identifier() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["1foo=bar"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
        assert!(shell.lookup_var("1foo").is_none());
    }

    #[test]
    fn declare_typeset_alias() {
        let mut shell = Shell::new();
        let oc = run_typeset(&["-r", "X_TS=hi"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X_TS").as_deref(), Some("hi"));
        assert!(shell.is_readonly("X_TS"));
    }

    #[test]
    fn declare_nameref_basic() {
        // `declare -n r=x` binds r as a nameref pointing at x.
        let mut shell = Shell::new();
        let (oc, _) = run(&["-n", "r=x"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.is_nameref("r"));
        // After Task 3: lookup_var dereferences the nameref; x is unset → None.
        assert_eq!(shell.lookup_var("r"), None);
        // The raw target name is still "x".
        assert_eq!(shell.nameref_raw_target("r").as_deref(), Some("x"));
    }

    #[test]
    fn declare_nameref_self_ref_errors() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-n", "r=r"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
    }

    #[test]
    fn declare_nameref_invalid_target_errors() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-n", "r=a b"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
    }

    #[test]
    fn declare_nameref_subscript_target() {
        // `declare -n e=arr[0]` should succeed.
        let mut shell = Shell::new();
        let (oc, _) = run(&["-n", "e=arr[0]"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.is_nameref("e"));
        // After Task 3: lookup_var dereferences the nameref; arr is unset → None.
        assert_eq!(shell.lookup_var("e"), None);
        // The raw target name is still "arr[0]".
        assert_eq!(shell.nameref_raw_target("e").as_deref(), Some("arr[0]"));
    }

    #[test]
    fn declare_plus_n_removes_nameref() {
        // `declare +n r` removes the nameref attribute.
        let mut shell = Shell::new();
        let (oc, _) = run(&["-n", "r=x"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.is_nameref("r"));
        let (oc2, _) = run(&["+n", "r"], &mut shell);
        assert!(matches!(oc2, ExecOutcome::Continue(0)));
        assert!(!shell.is_nameref("r"));
        // Value remains "x" after nameref removal.
        assert_eq!(shell.lookup_var("r").as_deref(), Some("x"));
    }

    #[test]
    fn declare_nameref_bare_unbound() {
        // `declare -n r` (no value) creates an unbound nameref with empty value.
        let mut shell = Shell::new();
        let (oc, _) = run(&["-n", "r"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.is_nameref("r"));
    }

    #[test]
    fn declare_lu_cancel_no_fold() {
        // `declare -lu x=AbC` — both -l and -u cancel to no attribute;
        // the stored value must be unchanged (AbC).
        let mut shell = Shell::new();
        let (oc, _) = run(&["-lu", "X_LU_CANCEL=AbC"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X_LU_CANCEL").as_deref(), Some("AbC"));
        assert_eq!(shell.case_fold_of("X_LU_CANCEL"), None);
    }

    #[test]
    fn declare_plus_l_removes_lower_attr() {
        // `declare -l x` then `declare +l x` then assign x=ABC → stored ABC
        // (the lowercase attribute was removed, so no fold occurs).
        let mut shell = Shell::new();
        let (oc, _) = run(&["-l", "X_PL=hello"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X_PL").as_deref(), Some("hello"));
        let (oc2, _) = run(&["+l", "X_PL"], &mut shell);
        assert!(matches!(oc2, ExecOutcome::Continue(0)));
        assert_eq!(shell.case_fold_of("X_PL"), None);
        let _ = run(&["X_PL=ABC"], &mut shell);
        assert_eq!(shell.lookup_var("X_PL").as_deref(), Some("ABC"));
    }

    #[test]
    fn declare_plus_u_removes_upper_attr() {
        // `declare -u x` then `declare +u x` then assign x=abc → stored abc.
        let mut shell = Shell::new();
        let (oc, _) = run(&["-u", "X_PU=HELLO"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X_PU").as_deref(), Some("HELLO"));
        let (oc2, _) = run(&["+u", "X_PU"], &mut shell);
        assert!(matches!(oc2, ExecOutcome::Continue(0)));
        assert_eq!(shell.case_fold_of("X_PU"), None);
        let _ = run(&["X_PU=abc"], &mut shell);
        assert_eq!(shell.lookup_var("X_PU").as_deref(), Some("abc"));
    }

    #[test]
    fn declare_plus_l_noop_on_upper_attr() {
        // `declare -u x` then `declare +l x` → +l is a no-op (x has Upper,
        // not Lower), so assigning abc still yields ABC.
        let mut shell = Shell::new();
        let (oc, _) = run(&["-u", "X_PL_NOP=hello"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X_PL_NOP").as_deref(), Some("HELLO"));
        let (oc2, _) = run(&["+l", "X_PL_NOP"], &mut shell);
        assert!(matches!(oc2, ExecOutcome::Continue(0)));
        // Upper attribute must still be present.
        assert_eq!(
            shell.case_fold_of("X_PL_NOP"),
            Some(crate::shell_state::CaseFold::Upper)
        );
        let _ = run(&["X_PL_NOP=abc"], &mut shell);
        assert_eq!(shell.lookup_var("X_PL_NOP").as_deref(), Some("ABC"));
    }
}

#[cfg(test)]
mod integer_attr_tests {
    use super::*;
    use crate::shell_state::Shell;

    // ── try_set integer-eval ────────────────────────────────

    #[test]
    fn try_set_non_integer_passes_through() {
        let mut shell = Shell::new();
        assert!(shell.try_set("X_INT_T1", "2+3".to_string()).is_ok());
        assert_eq!(shell.lookup_var("X_INT_T1").as_deref(), Some("2+3"));
    }

    #[test]
    fn try_set_integer_simple_arith() {
        let mut shell = Shell::new();
        shell.mark_integer("X_INT_T2");
        assert!(shell.try_set("X_INT_T2", "2+3".to_string()).is_ok());
        assert_eq!(shell.lookup_var("X_INT_T2").as_deref(), Some("5"));
    }

    #[test]
    fn try_set_integer_negative_result() {
        let mut shell = Shell::new();
        shell.mark_integer("X_INT_T3");
        assert!(shell.try_set("X_INT_T3", "0-5".to_string()).is_ok());
        assert_eq!(shell.lookup_var("X_INT_T3").as_deref(), Some("-5"));
    }

    #[test]
    fn try_set_integer_invalid_silently_zero() {
        let mut shell = Shell::new();
        shell.mark_integer("X_INT_T4");
        assert!(shell.try_set("X_INT_T4", "abc".to_string()).is_ok());
        assert_eq!(shell.lookup_var("X_INT_T4").as_deref(), Some("0"));
    }

    #[test]
    fn try_set_integer_with_var_ref() {
        let mut shell = Shell::new();
        shell.set("Y_INT_T5", "10".to_string());
        shell.mark_integer("X_INT_T5");
        assert!(shell.try_set("X_INT_T5", "Y_INT_T5*2".to_string()).is_ok());
        assert_eq!(shell.lookup_var("X_INT_T5").as_deref(), Some("20"));
    }

    #[test]
    fn try_set_readonly_checked_before_integer() {
        let mut shell = Shell::new();
        shell.set("X_INT_T6", "outer".to_string());
        shell.mark_readonly("X_INT_T6");
        shell.mark_integer("X_INT_T6");
        // try_set must return Err on readonly; value should NOT
        // change to "5".
        assert!(shell.try_set("X_INT_T6", "5".to_string()).is_err());
        assert_eq!(shell.lookup_var("X_INT_T6").as_deref(), Some("outer"));
    }

    // ── builtin_declare wiring ──────────────────────────────

    fn run_declare(args: &[&str], shell: &mut Shell) -> (ExecOutcome, String) {
        let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_declaration_builtin_strs("declare", &args_owned, &mut buf,&mut std::io::stderr(),  shell);
        (outcome, String::from_utf8(buf).unwrap())
    }

    #[test]
    fn declare_i_marks_and_evals() {
        let mut shell = Shell::new();
        let (oc, _) = run_declare(&["-i", "X_INT_D1=2+3"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X_INT_D1").as_deref(), Some("5"));
        assert!(shell.is_integer("X_INT_D1"));
    }

    #[test]
    fn declare_plus_i_unmarks() {
        let mut shell = Shell::new();
        run_declare(&["-i", "X_INT_D2=5"], &mut shell);
        assert!(shell.is_integer("X_INT_D2"));
        let (oc, _) = run_declare(&["+i", "X_INT_D2"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(!shell.is_integer("X_INT_D2"));
        // Value preserved.
        assert_eq!(shell.lookup_var("X_INT_D2").as_deref(), Some("5"));
    }

    #[test]
    fn declare_i_existing_var_no_reeval() {
        let mut shell = Shell::new();
        shell.set("X_INT_D3", "2+3".to_string());
        let (oc, _) = run_declare(&["-i", "X_INT_D3"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        // Value preserved; no re-eval on flag set without =.
        assert_eq!(shell.lookup_var("X_INT_D3").as_deref(), Some("2+3"));
        assert!(shell.is_integer("X_INT_D3"));
    }

    #[test]
    fn declare_i_on_readonly_errors() {
        let mut shell = Shell::new();
        shell.set("X_INT_D4", "outer".to_string());
        shell.mark_readonly("X_INT_D4");
        let (oc, _) = run_declare(&["-i", "X_INT_D4"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
        // Integer flag NOT set on a readonly var.
        assert!(!shell.is_integer("X_INT_D4"));
    }

    #[test]
    fn declare_ri_on_readonly_errors_without_corrupting_attrs() {
        // Regression: previously `declare -ri X=5` on already-readonly X
        // skipped the integer-readonly guard because want_readonly was
        // also true, then mark_integer ran before the inner -r readonly
        // check fired. Result: the variable's integer flag was set even
        // though the command errored. Bash leaves attributes unchanged
        // when the declare fails.
        let mut shell = Shell::new();
        shell.set("X_INT_D5", "outer".to_string());
        shell.mark_readonly("X_INT_D5");
        let (oc, _) = run_declare(&["-ri", "X_INT_D5=5"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
        // Integer flag must NOT be set; value unchanged.
        assert!(!shell.is_integer("X_INT_D5"));
        assert_eq!(shell.lookup_var("X_INT_D5").as_deref(), Some("outer"));
    }
}

#[cfg(test)]
mod eval_tests {
    use super::*;
    use crate::shell_state::Shell;

    fn run(args: &[&str], shell: &mut Shell) -> ExecOutcome {
        let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut buf: Vec<u8> = Vec::new();
        run_builtin("eval", &args_owned, &mut buf,&mut std::io::stderr(),  shell)
    }

    #[test]
    fn eval_no_args_exits_zero() {
        let mut shell = Shell::new();
        assert!(matches!(run(&[], &mut shell), ExecOutcome::Continue(0)));
    }

    #[test]
    fn eval_empty_arg_exits_zero() {
        let mut shell = Shell::new();
        assert!(matches!(run(&[""], &mut shell), ExecOutcome::Continue(0)));
    }

    #[test]
    fn eval_simple_command_runs() {
        let mut shell = Shell::new();
        // process_line writes to process stdout (not the builtin's
        // `out` writer), so assert the side effect on shell state.
        let oc = run(&["X_EVAL_T3=hello"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X_EVAL_T3").as_deref(), Some("hello"));
    }

    #[test]
    fn eval_assignment_persists() {
        let mut shell = Shell::new();
        let oc = run(&["X_EVAL_T4=42"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X_EVAL_T4").as_deref(), Some("42"));
    }

    #[test]
    fn eval_false_returns_one() {
        let mut shell = Shell::new();
        let oc = run(&["false"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
    }

    #[test]
    fn eval_exit_propagates() {
        let mut shell = Shell::new();
        let oc = run(&["exit", "7"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Exit(7)));
    }
}

#[cfg(test)]
mod help_tests {
    use super::*;
    use crate::shell_state::Shell;

    fn run(args: &[&str]) -> (ExecOutcome, String) {
        let mut shell = Shell::new();
        let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("help", &args_owned, &mut buf,&mut std::io::stderr(),  &mut shell);
        (outcome, String::from_utf8(buf).unwrap())
    }

    #[test]
    fn help_no_args_lists_all() {
        let (oc, out) = run(&[]);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        // Sample a few we know exist.
        assert!(out.lines().any(|l| l.starts_with("cd:")));
        assert!(out.lines().any(|l| l.starts_with("echo:")));
        assert!(out.lines().any(|l| l.starts_with("eval:")));
    }

    #[test]
    fn help_named_builtin_default_form() {
        let (oc, out) = run(&["cd"]);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(out.lines().any(|l| l.starts_with("cd:")));
        // At least one indented continuation line.
        assert!(out.lines().any(|l| l.starts_with("    ")));
    }

    #[test]
    fn help_synopsis_only() {
        let (oc, out) = run(&["-s", "echo"]);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        // Exactly one line starting with "echo:"; no indentation.
        let lines: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("echo:"));
    }

    #[test]
    fn help_description_only() {
        let (oc, out) = run(&["-d", "echo"]);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        // No line starts with "echo:".
        assert!(out.lines().all(|l| !l.starts_with("echo:")));
        // Has actual description text.
        assert!(!out.trim().is_empty());
    }

    #[test]
    fn help_man_format() {
        let (oc, out) = run(&["-m", "echo"]);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(out.lines().any(|l| l == "NAME"));
        assert!(out.lines().any(|l| l == "SYNOPSIS"));
        assert!(out.lines().any(|l| l == "DESCRIPTION"));
    }

    #[test]
    fn help_invalid_option() {
        let (oc, _) = run(&["-X"]);
        assert!(matches!(oc, ExecOutcome::Continue(2)));
    }

    #[test]
    fn help_not_found() {
        let (oc, _) = run(&["__no_such_builtin__"]);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
    }

    #[test]
    fn help_multi_name_partial_miss() {
        let (oc, out) = run(&["cd", "__no_such_builtin__"]);
        // Overall exit 1 because of the miss; cd's content still in stdout.
        assert!(matches!(oc, ExecOutcome::Continue(1)));
        assert!(out.lines().any(|l| l.starts_with("cd:")));
    }

    #[test]
    fn help_keyword_lookup_works() {
        // Shell keywords (if/for/while/etc.) have their own HelpEntry
        // alongside builtins, so `help if` resolves rather than
        // erroring with "no help topics match".
        for kw in ["if", "for", "while", "case", "function", "[[", "{", "select"] {
            let (oc, out) = run(&[kw]);
            assert!(
                matches!(oc, ExecOutcome::Continue(0)),
                "expected exit 0 for `help {kw}`",
            );
            assert!(
                out.lines().any(|l| l.starts_with(&format!("{kw}:"))),
                "expected `{kw}:` line in stdout for `help {kw}`; got: {out:?}",
            );
        }
    }
}

#[cfg(test)]
mod set_options_tests {
    use super::*;
    use crate::shell_state::Shell;

    fn run(args: &[&str], shell: &mut Shell) -> (ExecOutcome, String) {
        let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("set", &args_owned, &mut buf,&mut std::io::stderr(),  shell);
        (outcome, String::from_utf8(buf).unwrap())
    }

    #[test]
    fn set_e_enables_errexit() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-e"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.shell_options.errexit);
    }

    #[test]
    fn set_plus_e_disables() {
        let mut shell = Shell::new();
        shell.shell_options.errexit = true;
        let (oc, _) = run(&["+e"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(!shell.shell_options.errexit);
    }

    #[test]
    fn set_o_errexit_long_form() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-o", "errexit"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.shell_options.errexit);
    }

    #[test]
    fn set_plus_o_errexit_disables() {
        let mut shell = Shell::new();
        shell.shell_options.errexit = true;
        let (oc, _) = run(&["+o", "errexit"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(!shell.shell_options.errexit);
    }

    #[test]
    fn set_dollar_dash_reflects_flags() {
        let mut shell = Shell::new();
        // No flags set, not interactive by default in tests.
        let dash = shell.lookup_var("-").unwrap_or_default();
        assert!(dash.is_empty() || dash == "i");
        // Enable errexit.
        run(&["-e"], &mut shell);
        let dash = shell.lookup_var("-").unwrap_or_default();
        assert!(dash.contains('e'));
        // Enable nounset.
        run(&["-u"], &mut shell);
        let dash = shell.lookup_var("-").unwrap_or_default();
        assert!(dash.contains('e'));
        assert!(dash.contains('u'));
    }

    #[test]
    fn set_invalid_o_name_errors() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-o", "nope_no_such_opt"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(2)));
    }

    #[test]
    fn set_v_short_flag_toggles_verbose() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-v"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.shell_options.verbose);
        let (oc, _) = run(&["+v"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(!shell.shell_options.verbose);
    }

    #[test]
    fn set_o_verbose_long_form_enables() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-o", "verbose"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.shell_options.verbose);
    }

    #[test]
    fn set_x_short_flag_toggles_xtrace() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-x"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.shell_options.xtrace);
        let (oc, _) = run(&["+x"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(!shell.shell_options.xtrace);
    }

    #[test]
    fn set_o_xtrace_long_form_enables() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-o", "xtrace"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.shell_options.xtrace);
    }

    #[test]
    fn option_set_xtrace_round_trips() {
        let mut shell = Shell::new();
        assert!(option_set(&mut shell, "xtrace", true).is_ok());
        assert_eq!(option_get(&shell, "xtrace"), Some(true));
        assert!(option_set(&mut shell, "xtrace", false).is_ok());
        assert_eq!(option_get(&shell, "xtrace"), Some(false));
    }

    #[test]
    fn set_posix_option_is_accepted_as_noop_via_option_set() {
        let mut shell = Shell::new();
        assert!(option_set(&mut shell, "posix", true).is_ok());
        assert!(option_set(&mut shell, "posix", false).is_ok());
    }

    #[test]
    fn option_get_posix_returns_table_default() {
        let shell = Shell::new();
        // SETO_TABLE default for posix is `false`.
        assert_eq!(option_get(&shell, "posix"), Some(false));
    }

    #[test]
    fn set_o_listing_shows_state() {
        let mut shell = Shell::new();
        let (oc, out) = run(&["-o"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(out.lines().any(|l| l.starts_with("errexit")));
        assert!(out.lines().any(|l| l.starts_with("nounset")));
    }

    #[test]
    fn set_plus_o_listing_reinput_form() {
        let mut shell = Shell::new();
        let (oc, out) = run(&["+o"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        // Both off by default.
        assert!(out.lines().any(|l| l == "set +o errexit"));
        assert!(out.lines().any(|l| l == "set +o nounset"));
    }

    #[test]
    fn set_eu_cluster() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-eu"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.shell_options.errexit);
        assert!(shell.shell_options.nounset);
    }

    #[test]
    fn set_dash_dash_resets_positional() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-e", "--", "a", "b", "c"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.shell_options.errexit);
        assert_eq!(shell.positional_args, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }

    #[test]
    fn set_dash_eo_cluster_consumes_next_arg_as_name() {
        // Regression: bash treats `-eo NAME` as enabling -e then
        // -o NAME (the o-in-cluster consumes the next arg as the
        // option name). Previously huck rejected the o-in-cluster
        // as "not yet supported".
        let mut shell = Shell::new();
        let (oc, _) = run(&["-eo", "nounset"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.shell_options.errexit, "expected errexit on");
        assert!(shell.shell_options.nounset, "expected nounset on");
    }

    #[test]
    fn set_plus_eo_cluster_consumes_next_arg_as_name() {
        // Symmetric: `+eo NAME` disables -e then -o NAME.
        let mut shell = Shell::new();
        shell.shell_options.errexit = true;
        shell.shell_options.nounset = true;
        let (oc, _) = run(&["+eo", "nounset"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(!shell.shell_options.errexit, "expected errexit off");
        assert!(!shell.shell_options.nounset, "expected nounset off");
    }

    #[test]
    fn set_o_lists_full_27_name_table_tab_format() {
        let mut shell = Shell::new();
        let (oc, out) = run(&["-o"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 27, "set -o must list all 27 names; got {lines:?}");
        // bash format: name left-justified in 15, a TAB, then on/off.
        assert_eq!(lines[0], "allexport      \toff");
        assert_eq!(lines[3], "errexit        \toff");
        // long name (>=15 chars): no padding, just name + TAB + value.
        assert!(lines.contains(&"interactive-comments\ton"));
        assert!(lines.contains(&"braceexpand    \ton"));
        assert!(lines.contains(&"hashall        \ton"));
    }

    #[test]
    fn set_o_reflects_real_state_for_implemented() {
        let mut shell = Shell::new();
        shell.shell_options.errexit = true;
        let (_, out) = run(&["-o"], &mut shell);
        assert!(out.lines().any(|l| l == "errexit        \ton"));
    }

    #[test]
    fn set_o_enable_unimplemented_says_not_supported() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-o", "allexport"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(2)));
    }

    #[test]
    fn set_o_enable_unknown_name_is_invalid() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-o", "nope_no_such_opt"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(2)));
    }

    #[test]
    fn set_dash_c_enables_noclobber() {
        let mut shell = Shell::new();
        let _ = run(&["-C"], &mut shell);
        assert!(shell.shell_options.noclobber);
        assert_eq!(option_get(&shell, "noclobber"), Some(true));
    }

    #[test]
    fn set_plus_c_disables_noclobber() {
        let mut shell = Shell::new();
        let _ = run(&["-C"], &mut shell);
        let _ = run(&["+C"], &mut shell);
        assert!(!shell.shell_options.noclobber);
    }

    #[test]
    fn set_o_noclobber_enables() {
        let mut shell = Shell::new();
        let _ = run(&["-o", "noclobber"], &mut shell);
        assert_eq!(option_get(&shell, "noclobber"), Some(true));
    }
}

#[cfg(test)]
mod array_declare_tests {
    use super::*;
    use crate::shell_state::Shell;

    fn run(shell: &mut Shell, line: &str) -> ExecOutcome {
        crate::shell::process_line(line, shell, false)
    }

    #[test]
    fn declare_dash_a_creates_empty_array() {
        let mut s = Shell::new();
        let _ = run(&mut s, "declare -a a");
        assert!(s.get_indexed("a").is_some());
        assert_eq!(s.get_indexed("a").unwrap().len(), 0);
    }

    #[test]
    fn declare_dash_a_with_value() {
        let mut s = Shell::new();
        let _ = run(&mut s, "declare -a a=(x y)");
        let m = s.get_indexed("a").unwrap();
        assert_eq!(m.get(&0).map(String::as_str), Some("x"));
        assert_eq!(m.get(&1).map(String::as_str), Some("y"));
    }

    #[test]
    fn declare_p_formats_array() {
        let mut s = Shell::new();
        let _ = run(&mut s, "a=(x y)");
        let (_, v) = s
            .iter_vars()
            .find(|(n, _)| n.as_str() == "a")
            .expect("a is set");
        let line = format_declare_line("a", v);
        assert_eq!(line, r#"declare -a a=([0]="x" [1]="y")"#);
    }

    #[test]
    fn declare_dash_ai_creates_integer_array() {
        // L-49: `declare -ai` now creates an integer-flagged indexed array
        // whose element values arith-coerce on assignment.
        let mut s = Shell::new();
        let outcome = run(&mut s, "declare -ai a=(2+3 4*5)");
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(s.is_integer("a"));
        let m = s.get_indexed("a").unwrap();
        assert_eq!(m.get(&0).map(String::as_str), Some("5"));
        assert_eq!(m.get(&1).map(String::as_str), Some("20"));
    }

    #[test]
    fn readonly_array_blocks_element_write() {
        let mut s = Shell::new();
        let _ = run(&mut s, "readonly a=(x y)");
        let _ = run(&mut s, "a[2]=z");
        let m = s.get_indexed("a").unwrap();
        assert!(m.get(&2).is_none());
    }

    #[test]
    fn export_array_rejects() {
        let mut s = Shell::new();
        let outcome = run(&mut s, "export a=(x y)");
        assert!(matches!(
            outcome,
            ExecOutcome::Continue(1) | ExecOutcome::Exit(1)
        ));
        assert!(s.get_indexed("a").is_none());
    }

    #[test]
    fn readonly_p_lists_array_with_full_elements() {
        // Regression: `readonly -p` used to route through scalar_view and
        // collapse arrays to element 0. The fix routes through
        // format_declare_line so all elements survive.
        let mut s = Shell::new();
        let _ = run(&mut s, "readonly a=(x y z)");
        let (_, v) = s
            .iter_vars()
            .find(|(n, _)| n.as_str() == "a")
            .expect("a is set");
        let line = format_declare_line("a", v);
        assert_eq!(line, r#"declare -ar a=([0]="x" [1]="y" [2]="z")"#);

        // Also exercise the dispatched listing path end-to-end so we
        // don't drift on the writeln formatting.
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_declaration_builtin(
            "readonly",
            &[DeclArg::Plain("-p".to_string())],
            &mut buf,&mut std::io::stderr(), 
            &mut s,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.lines()
                .any(|l| l == r#"declare -ar a=([0]="x" [1]="y" [2]="z")"#),
            "stdout: {out:?}",
        );
    }
}

#[cfg(test)]
mod assoc_declare_tests {
    use super::*;
    use crate::shell_state::Shell;

    fn run(shell: &mut Shell, line: &str) -> ExecOutcome {
        crate::shell::process_line(line, shell, false)
    }

    #[test]
    fn declare_dash_cap_a_creates_empty_associative() {
        let mut s = Shell::new();
        let _ = run(&mut s, "declare -A m");
        assert!(s.get_associative("m").is_some());
        assert_eq!(s.get_associative("m").unwrap().len(), 0);
    }

    #[test]
    fn declare_dash_cap_a_with_value() {
        let mut s = Shell::new();
        let _ = run(&mut s, "declare -A m=([foo]=bar [baz]=qux)");
        assert_eq!(s.lookup_associative_element("m", "foo"), Some("bar".into()));
        assert_eq!(s.lookup_associative_element("m", "baz"), Some("qux".into()));
    }

    #[test]
    fn declare_p_formats_associative() {
        let mut s = Shell::new();
        s.declare_associative("m").unwrap();
        s.set_associative_element("m", "k1".into(), "v1".into()).unwrap();
        s.set_associative_element("m", "k2".into(), "v2".into()).unwrap();
        let v = s.iter_vars().find(|(n, _)| n.as_str() == "m").unwrap().1;
        let line = format_declare_line("m", v);
        assert_eq!(line, r#"declare -A m=([k1]="v1" [k2]="v2" )"#);
    }

    #[test]
    fn declare_dash_cap_a_i_creates_integer_assoc() {
        // L-49: `declare -Ai` creates an integer-flagged associative array
        // whose VALUES arith-coerce on assignment (keys are not coerced).
        let mut s = Shell::new();
        let outcome = run(&mut s, "declare -Ai m=([x]=2+3 [y]=10)");
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(s.is_integer("m"));
        let pairs = s.get_associative("m").unwrap();
        assert_eq!(
            pairs.iter().find(|(k, _)| k == "x").map(|(_, v)| v.as_str()),
            Some("5")
        );
        assert_eq!(
            pairs.iter().find(|(k, _)| k == "y").map(|(_, v)| v.as_str()),
            Some("10")
        );
    }

    #[test]
    fn declare_dash_a_cap_a_errors() {
        let mut s = Shell::new();
        let outcome = run(&mut s, "declare -aA m");
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn declare_dash_cap_a_on_existing_indexed_errors() {
        let mut s = Shell::new();
        let _ = run(&mut s, "a=(x y z)");
        let outcome = run(&mut s, "declare -A a");
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        assert!(s.get_indexed("a").is_some());
        assert!(s.get_associative("a").is_none());
    }

    #[test]
    fn declare_dash_cap_a_on_existing_scalar_errors() {
        let mut s = Shell::new();
        let _ = run(&mut s, "s=hello");
        let outcome = run(&mut s, "declare -A s");
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn readonly_dash_cap_a_creates_readonly_associative() {
        let mut s = Shell::new();
        let _ = run(&mut s, "readonly -A m=([k]=v)");
        assert!(s.get_associative("m").is_some());
        let _ = run(&mut s, "m[k2]=v2");
        assert!(s.lookup_associative_element("m", "k2").is_none());
    }

    #[test]
    fn export_associative_rejects() {
        let mut s = Shell::new();
        let outcome = run(&mut s, "export m=([k]=v)");
        assert!(matches!(outcome, ExecOutcome::Continue(1) | ExecOutcome::Exit(1)));
        assert!(s.get_associative("m").is_none());
    }
}

#[cfg(test)]
mod loop_levels_tests {
    use super::*;
    use crate::shell_state::Shell;

    // ----- break: valid levels (terminal $? = 0) -----

    #[test]
    fn break_no_args_emits_level_1_status_0() {
        let mut sh = Shell::new();
        sh.loop_depth = 1;
        let outcome = builtin_break(&[],&mut std::io::stderr(),  &sh);
        assert_eq!(outcome, ExecOutcome::LoopBreak(1, 0));
    }

    #[test]
    fn break_with_arg_n_emits_level_n_when_in_loop() {
        let mut sh = Shell::new();
        sh.loop_depth = 3;
        let outcome = builtin_break(&["2".to_string()],&mut std::io::stderr(),  &sh);
        assert_eq!(outcome, ExecOutcome::LoopBreak(2, 0));
    }

    #[test]
    fn break_caps_to_loop_depth() {
        let mut sh = Shell::new();
        sh.loop_depth = 2;
        let outcome = builtin_break(&["999".to_string()],&mut std::io::stderr(),  &sh);
        assert_eq!(outcome, ExecOutcome::LoopBreak(2, 0));
    }

    // ----- break: outside a loop → exit status 0, no break -----

    #[test]
    fn break_outside_loop_errors_with_status_0() {
        let sh = Shell::new();
        // sh.loop_depth = 0 by default.
        // Bash 5.2: break/continue outside a loop prints the diagnostic to
        // stderr but returns $? = 0 and does NOT break anything. Arg
        // validation is skipped entirely.
        assert_eq!(builtin_break(&[],&mut std::io::stderr(),  &sh), ExecOutcome::Continue(0));
        assert_eq!(builtin_break(&["abc".to_string()],&mut std::io::stderr(),  &sh), ExecOutcome::Continue(0));
        assert_eq!(builtin_break(&["0".to_string()],&mut std::io::stderr(),  &sh), ExecOutcome::Continue(0));
        assert_eq!(
            builtin_break(&["1".to_string(), "2".to_string(), "3".to_string()],&mut std::io::stderr(),  &sh),
            ExecOutcome::Continue(0)
        );
    }

    // ----- break: malformed N<=0 → break ALL loops, terminal $? = 1 -----

    #[test]
    fn break_zero_breaks_all_loops_status_1() {
        let mut sh = Shell::new();
        sh.loop_depth = 2;
        let outcome = builtin_break(&["0".to_string()],&mut std::io::stderr(),  &sh);
        assert_eq!(outcome, ExecOutcome::LoopBreak(2, 1));
    }

    #[test]
    fn break_negative_breaks_all_loops_status_1() {
        let mut sh = Shell::new();
        sh.loop_depth = 1;
        let outcome = builtin_break(&["-1".to_string()],&mut std::io::stderr(),  &sh);
        assert_eq!(outcome, ExecOutcome::LoopBreak(1, 1));
    }

    // ----- break: too many args → break ALL loops, terminal $? = 1 -----

    #[test]
    fn break_too_many_args_breaks_all_loops_status_1() {
        let mut sh = Shell::new();
        sh.loop_depth = 2;
        let outcome = builtin_break(&["1".to_string(), "2".to_string()],&mut std::io::stderr(),  &sh);
        assert_eq!(outcome, ExecOutcome::LoopBreak(2, 1));
    }

    // ----- break: non-numeric → abort script with exit 128 -----

    #[test]
    fn break_non_numeric_exits_with_status_128() {
        let mut sh = Shell::new();
        sh.loop_depth = 1;
        let outcome = builtin_break(&["abc".to_string()],&mut std::io::stderr(),  &sh);
        assert_eq!(outcome, ExecOutcome::Exit(128));
    }

    // ----- continue: valid levels (LoopContinue) -----

    #[test]
    fn continue_no_args_emits_level_1() {
        let mut sh = Shell::new();
        sh.loop_depth = 1;
        let outcome = builtin_continue(&[],&mut std::io::stderr(),  &sh);
        assert_eq!(outcome, ExecOutcome::LoopContinue(1));
    }

    #[test]
    fn continue_caps_to_loop_depth() {
        let mut sh = Shell::new();
        sh.loop_depth = 1;
        let outcome = builtin_continue(&["5".to_string()],&mut std::io::stderr(),  &sh);
        assert_eq!(outcome, ExecOutcome::LoopContinue(1));
    }

    // ----- continue: outside a loop → exit status 0, no continue -----

    #[test]
    fn continue_outside_loop_errors_with_status_0() {
        let sh = Shell::new();
        assert_eq!(builtin_continue(&[],&mut std::io::stderr(),  &sh), ExecOutcome::Continue(0));
        assert_eq!(builtin_continue(&["abc".to_string()],&mut std::io::stderr(),  &sh), ExecOutcome::Continue(0));
        assert_eq!(builtin_continue(&["0".to_string()],&mut std::io::stderr(),  &sh), ExecOutcome::Continue(0));
    }

    // ----- continue: malformed N<=0 / too-many → break ALL loops, $? = 1 -----

    #[test]
    fn continue_zero_breaks_all_loops_status_1() {
        let mut sh = Shell::new();
        sh.loop_depth = 2;
        let outcome = builtin_continue(&["0".to_string()],&mut std::io::stderr(),  &sh);
        assert_eq!(outcome, ExecOutcome::LoopBreak(2, 1));
    }

    #[test]
    fn continue_negative_breaks_all_loops_status_1() {
        let mut sh = Shell::new();
        sh.loop_depth = 3;
        let outcome = builtin_continue(&["-5".to_string()],&mut std::io::stderr(),  &sh);
        assert_eq!(outcome, ExecOutcome::LoopBreak(3, 1));
    }

    #[test]
    fn continue_too_many_args_breaks_all_loops_status_1() {
        let mut sh = Shell::new();
        sh.loop_depth = 2;
        let outcome = builtin_continue(&["1".to_string(), "2".to_string()],&mut std::io::stderr(),  &sh);
        assert_eq!(outcome, ExecOutcome::LoopBreak(2, 1));
    }

    // ----- continue: non-numeric → abort script with exit 128 -----

    #[test]
    fn continue_non_numeric_exits_with_status_128() {
        let mut sh = Shell::new();
        sh.loop_depth = 1;
        let outcome = builtin_continue(&["abc".to_string()],&mut std::io::stderr(),  &sh);
        assert_eq!(outcome, ExecOutcome::Exit(128));
    }
}

#[cfg(test)]
mod pipefail_option_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn pipefail_option_round_trips() {
        let mut sh = Shell::new();
        assert_eq!(option_get(&sh, "pipefail"), Some(false));
        option_set(&mut sh, "pipefail", true).unwrap();
        assert_eq!(option_get(&sh, "pipefail"), Some(true));
        assert!(sh.shell_options.pipefail);
        option_set(&mut sh, "pipefail", false).unwrap();
        assert_eq!(option_get(&sh, "pipefail"), Some(false));
    }

    #[test]
    fn pipefail_not_in_dollar_dash() {
        // pipefail has no short flag, so it must never appear in `$-`.
        let mut sh = Shell::new();
        option_set(&mut sh, "pipefail", true).unwrap();
        assert!(!sh.dollar_dash_value().contains('p'), "$- must not include pipefail");
    }

    #[test]
    fn pipefail_listed_in_shell_options() {
        assert!(SETO_TABLE.iter().any(|o| o.name == "pipefail" && !o.default));
    }

    #[test]
    fn shopt_bare_lists_all_57() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let oc = builtin_shopt(&[], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out.lines().count(), 57);
        assert_eq!(out.lines().next().unwrap(), "autocd         \toff");
        assert!(out.lines().any(|l| l == "checkwinsize   \ton"));
        assert!(out.lines().any(|l| l == "assoc_expand_once\toff")); // long name, no pad
    }

    #[test]
    fn shopt_o_lists_27() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let oc = builtin_shopt(&["-o".into()], &mut buf,&mut std::io::stderr(),  &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(String::from_utf8(buf).unwrap().lines().count(), 27);
    }
}

#[cfg(test)]
mod getopts_step_tests {
    use super::getopts_step;

    fn args(v: &[&str]) -> Vec<String> { v.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn plain_option_then_advance() {
        // "-a" at optind=1, sp=1 (fresh): consume 'a', word done -> optind=2, sp=1.
        let s = getopts_step("ab", &args(&["-a"]), 1, 1);
        assert_eq!(s.name, "a");
        assert_eq!(s.optarg, None);
        assert_eq!((s.optind, s.sp), (2, 1));
        assert!(!s.done);
        assert!(s.error.is_none());
    }

    #[test]
    fn clustered_options_walk_within_word() {
        // "-ab": first call consumes 'a' (optind stays 1, sp 1->3),
        let s1 = getopts_step("ab", &args(&["-ab"]), 1, 1);
        assert_eq!(s1.name, "a");
        assert_eq!((s1.optind, s1.sp), (1, 3));
        assert!(!s1.done);
        assert!(s1.error.is_none());
        // second call (sp=3) consumes 'b', word done -> optind=2, sp=1.
        let s2 = getopts_step("ab", &args(&["-ab"]), 1, 3);
        assert_eq!(s2.name, "b");
        assert_eq!((s2.optind, s2.sp), (2, 1));
    }

    #[test]
    fn option_with_attached_arg() {
        // "-bval": 'b' takes an arg; rest of word "val" is OPTARG; optind=2.
        let s = getopts_step("ab:", &args(&["-bval"]), 1, 1);
        assert_eq!(s.name, "b");
        assert_eq!(s.optarg.as_deref(), Some("val"));
        assert_eq!((s.optind, s.sp), (2, 1));
    }

    #[test]
    fn option_with_separate_arg() {
        // "-b" "val": arg is the next word; optind jumps to 3.
        let s = getopts_step("ab:", &args(&["-b", "val"]), 1, 1);
        assert_eq!(s.name, "b");
        assert_eq!(s.optarg.as_deref(), Some("val"));
        assert_eq!((s.optind, s.sp), (3, 1));
    }

    #[test]
    fn exhausted_returns_done_question() {
        let s = getopts_step("ab", &args(&["-a"]), 2, 1); // optind past end
        assert_eq!(s.name, "?");
        assert!(s.done);
        assert_eq!(s.optind, 2); // optind.max(1), unchanged
        assert_eq!(s.optarg, None);
    }

    #[test]
    fn non_option_terminates() {
        let s = getopts_step("ab", &args(&["foo"]), 1, 1);
        assert_eq!(s.name, "?");
        assert!(s.done);
        assert_eq!(s.optind, 1); // OPTIND unchanged
    }

    #[test]
    fn double_dash_terminates_and_advances() {
        let s = getopts_step("ab", &args(&["--", "x"]), 1, 1);
        assert_eq!(s.name, "?");
        assert!(s.done);
        assert_eq!(s.optind, 2); // advanced past "--"
    }

    #[test]
    fn invalid_option_verbose() {
        let s = getopts_step("ab", &args(&["-z"]), 1, 1);
        assert_eq!(s.name, "?");
        assert_eq!(s.optarg, None);
        assert!(!s.done); // invalid option is NOT terminating (rc 0, keep going)
        assert_eq!(s.optind, 2); // "-z" exhausts the word → optind advances
        assert_eq!(s.error.as_deref(), Some("illegal option -- z"));
    }

    #[test]
    fn invalid_option_silent() {
        let s = getopts_step(":ab", &args(&["-z"]), 1, 1);
        assert_eq!(s.name, "?");
        assert_eq!(s.optarg.as_deref(), Some("z")); // silent: OPTARG = offending char
        assert!(!s.done); // still rc 0 (keep processing)
        assert_eq!(s.optind, 2);
        assert!(s.error.is_none());
    }

    #[test]
    fn missing_arg_verbose() {
        let s = getopts_step("ab:", &args(&["-b"]), 1, 1);
        assert_eq!(s.name, "?");
        assert_eq!(s.optarg, None);
        assert_eq!(s.error.as_deref(), Some("option requires an argument -- b"));
    }

    #[test]
    fn missing_arg_silent() {
        let s = getopts_step(":ab:", &args(&["-b"]), 1, 1);
        assert_eq!(s.name, ":");
        assert_eq!(s.optarg.as_deref(), Some("b"));
        assert!(s.error.is_none());
    }
}

#[cfg(test)]
mod normalize_logical_tests {
    use super::normalize_logical;

    #[test]
    fn normalize_logical_collapses_lexically() {
        assert_eq!(normalize_logical("/a/b/../c"), "/a/c");
        assert_eq!(normalize_logical("/a/./b"), "/a/b");
        assert_eq!(normalize_logical("/a//b"), "/a/b");
        assert_eq!(normalize_logical("/a/b/.."), "/a");
        assert_eq!(normalize_logical("/.."), "/");
        assert_eq!(normalize_logical("/a/../.."), "/");
        assert_eq!(normalize_logical("/"), "/");
        assert_eq!(normalize_logical("/tmp/m/link/.."), "/tmp/m");
    }
}
