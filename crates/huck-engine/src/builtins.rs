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
    /// #442: a trap action ran `exit N`. Unwinds like `DiscardCommand` — out of
    /// loops, functions, subshells and command substitutions — but the shell
    /// DOES exit, with `n`, after the EXIT trap has had its turn. Raised by
    /// `executor::check_interrupt` from `Shell::pending_exit`.
    ExitRequested(i32),
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
    "caller",
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
        "fg" => builtin_fg(args, out, err, shell),
        "bg" => builtin_bg(args, out, err, shell),
        "kill" => builtin_kill(args, out, err, shell),
        "disown" => builtin_disown(args, err, shell),
        "history" => builtin_history(args, out, err, shell),
        "trap" => builtin_trap(args, out, err, shell),
        "set" => builtin_set(args, out, err, shell),
        "shopt" => builtin_shopt(args, out, err, shell),
        "shift" => builtin_shift(args, err, shell),
        "getopts" => builtin_getopts(args, err, shell),
        "." | "source" => builtin_source(name, args, err, shell),
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
        "mapfile" | "readarray" => builtin_mapfile(name, args, err, shell),
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
            builtin_return(args, err, shell)
        }
        "bind" => builtin_bind(args, out, err, shell),
        "umask" => builtin_umask(args, out, err, shell),
        "ulimit" => builtin_ulimit(args, out, err, shell),
        "times" => builtin_times(args, out, err, shell),
        "enable" => builtin_enable(args, out, err, shell),
        "caller" => builtin_caller(args, out, err, shell),
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

/// bash's `legal_number()` (lib/sh/shquote.c's neighbour in general.c): a
/// base-10 integer with optional surrounding whitespace and sign. NOT
/// `parse::<i32>()`: bash accepts `" 3 "` and rejects `0x10` and the empty
/// string, and an out-of-range value is a failure rather than a saturation.
fn legal_number(s: &str) -> Option<i64> {
    let t = s.trim_matches(|c: char| c == ' ' || c == '\t' || c == '\n');
    if t.is_empty() {
        return None;
    }
    let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    t.parse::<i64>().ok()
}

/// `return [N]` builtin. bash's `return_builtin` + `get_exitstat`:
///
///   * more than one argument is `return: too many arguments` and a HARD
///     abort of the shell with status 1 — not catchable by `||` or `if`
///     (measured; a `( return 3 4 )` kills only the subshell);
///   * a leading `--` is skipped, so `return -- 3` returns 3;
///   * the argument must be a `legal_number`, else
///     `return: <arg>: numeric argument required` and the function returns
///     with status 2 (the rest of its body is skipped);
///   * the value is masked to `& 255`, so `return -1` is 255 and
///     `return 300` is 44. huck returned the raw `i32`, so `$?` could hold
///     -1 or 300.
///
/// With no argument at all the status is `$?`, unchanged.
fn builtin_return(args: &[String], err: &mut dyn Write, shell: &Shell) -> ExecOutcome {
    // `--` first: bash's `get_exitstat` skips it before counting.
    let args = match args.first() {
        Some(a) if a == "--" => &args[1..],
        _ => args,
    };
    if args.len() > 1 {
        crate::sh_error_to!(shell, err, None, "return: too many arguments");
        return ExecOutcome::Exit(1);
    }
    let code = match args.first() {
        Some(a) => match legal_number(a) {
            Some(n) => (n & 255) as i32,
            None => {
                crate::sh_error_to!(shell, err, None, "return: {a}: numeric argument required");
                2
            }
        },
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
        "declare" | "typeset" => builtin_declare_decl(name, decl_args, out, err, shell),
        "local" => builtin_local_decl(name, decl_args, err, shell),
        "readonly" => builtin_readonly_decl(name, decl_args, out, err, shell),
        "export" => builtin_export_decl(name, decl_args, out, err, shell),
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
    if let Err(msg) = shell.policy.check(crate::policy::Op::Cd) {
        crate::sh_error_to!(shell, err, None, "{msg}");
        return ExecOutcome::Continue(1);
    }
    // 1. Parse leading -L/-P/-e flags (last of -L/-P wins) and `--`. `-` is NOT
    //    a flag (it is the OLDPWD shortcut / target).
    //
    //    `@` is in bash's own usage string (`cd [-L|[-P [-e]] [-@]] [dir]`)
    //    but it's gated on HAVE_XATTR at bash's compile time — the ubuntu-24.04
    //    bash 5.2.21 this project targets does NOT have it built in and
    //    rejects `-@` itself (verified: `bash -c 'cd -@ /tmp'` ->
    //    `cd: -@: invalid option`). Leaving `@` out of the spec so huck
    //    rejects it too is therefore a byte-for-byte MATCH with the target
    //    bash, not a gap.
    let mut physical_flag: Option<bool> = None;
    let mut want_e = false;
    let mut g =
        crate::builtin_opts::Getopt::new("cd", crate::builtin_opts::ArgView::Plain(args), "LPe");
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                'L' => physical_flag = Some(false),
                'P' => physical_flag = Some(true),
                // -e: only takes effect below, in the -P branch, when
                // `env::current_dir()` fails after a successful chdir.
                'e' => want_e = true,
                _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }
    let rest = &args[g.rest_index()..];
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

    // Set when a `-P` chdir succeeds but bash's own working-directory
    // re-derivation can't confirm it — the case `-e` names: "if the -P
    // option is supplied, and the current working directory cannot be
    // determined successfully, exit with a non-zero status." That covers
    // TWO distinct failure shapes, both bash-verified:
    //
    //  1. `env::current_dir()` (the getcwd(2) syscall) itself fails.
    //
    //  2. `getcwd(2)` SUCCEEDS (it walks the kernel's dentry tree, which
    //     bypasses directory search-permission checks) but a plain
    //     NAME-based lookup of that same path does not — e.g. an ancestor
    //     directory loses search (`x`) permission after huck is already
    //     resident inside it. Reproduced: `mkdir -p /tmp/t9/sub; cd
    //     /tmp/t9/sub; chmod 000 /tmp/t9; cd -P -e .` exits 1 in bash even
    //     though `pwd -P` (bash's own builtin, same getcwd() call) still
    //     succeeds there — bash's `-e` is checking name-based
    //     reachability, not raw getcwd() success. A `stat` of the
    //     getcwd()-reported path fails identically under the same setup,
    //     which is what shape 2 below probes for.
    let mut pwd_undetermined = false;

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
            Ok(p) => {
                let p = p.to_string_lossy().into_owned();
                // Shape 2 above. Only probed when `-e` is set — an extra
                // stat() per `cd -P` would be needless overhead for every
                // caller who never asked for `-e`'s stronger guarantee.
                if want_e && std::fs::metadata(&p).is_err() {
                    pwd_undetermined = true;
                }
                p
            }
            Err(e) => {
                crate::sh_error_to!(
                    shell,
                    err,
                    None,
                    "cd: warning: could not read current dir: {}",
                    crate::bash_io_error(&e)
                );
                pwd_undetermined = true;
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
        match env::set_current_dir(Path::new(&normalized)) {
            Ok(()) => normalized,
            Err(e) => {
                // #517: bash's `change_to_directory` does NOT give up here. When
                // the canonicalized path cannot be entered it retries the
                // argument AS WRITTEN, and on success takes the resulting
                // directory from getcwd() instead of the canonical name. Two
                // measured cases need it:
                //
                //   * an ancestor that has lost search permission since the
                //     shell moved inside it — `chdir("/a/b/c")` is EACCES while
                //     `chdir(".")` succeeds, so bash's `cd .` is a no-op success
                //     where huck reported `Permission denied`;
                //   * a logical path through a symlink whose canonical form does
                //     not exist — from a symlinked `lnk -> p/q`, bash's
                //     `cd ../q` lands in `p/q` (PWD becomes the PHYSICAL path)
                //     while huck reported `No such file or directory`.
                //
                // On a second failure the FIRST error is the one bash reports:
                // `cd ./nosuch` under the revoked-ancestor setup says
                // `Permission denied` (the canonical attempt) and not the
                // `No such file or directory` the literal attempt would give.
                if env::set_current_dir(Path::new(&target)).is_err() {
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
                    // getcwd() failed after a successful chdir: bash forgets the
                    // working directory here and `-e` calls that "cannot be
                    // determined". Keep the canonical name as the best guess.
                    Err(_) => {
                        pwd_undetermined = true;
                        normalized
                    }
                }
            }
        }
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
    ExecOutcome::Continue(if want_e && pwd_undetermined { 1 } else { 0 })
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
    let mut g =
        crate::builtin_opts::Getopt::new("pwd", crate::builtin_opts::ArgView::Plain(args), "LP");
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                'L' => physical_flag = Some(false),
                'P' => physical_flag = Some(true),
                _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
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

/// v349 (#343, Root B): under a declaration builtin's `-a`/`-A` flag, a scalar
/// assignment value of the shape `(...)` is re-parsed as an ARRAY LITERAL,
/// matching bash (which re-runs array-literal parsing plus full word expansion
/// on the value). `readonly -a 'd=(4)'` / `export -a r='(7)'` therefore land as
/// `d=([0]="4")` rather than the literal scalar `(4)`. Returns the re-parsed
/// value `Word` (a single `WordPart::ArrayLiteral`) on success, or `None` when
/// the value is not a lone `(...)`-shaped scalar literal or fails to parse as
/// an array assignment (in which case the caller keeps the literal scalar —
/// so a NON-`-a` `readonly 'c=(3)'` still stores `c[0]="(3)"`).
fn reparse_paren_scalar_as_array(
    name: &str,
    value: &crate::lexer::Word,
) -> Option<crate::lexer::Word> {
    use crate::lexer::WordPart;
    // The value must be a pure SCALAR literal (Root D's single quoted Literal,
    // or a quoted RHS like `r='(7)'` whose parts are `Literal("r=")` + a
    // `Quoted` literal). Any expansion/substitution part, or an already-parsed
    // `ArrayLiteral` (the unquoted `d=(…)` path), disqualifies it → keep the
    // scalar. `word_scalar_literal_text` flattens the literal text or returns
    // None.
    fn word_scalar_literal_text(w: &crate::lexer::Word) -> Option<String> {
        let mut out = String::new();
        for p in &w.0 {
            match p {
                WordPart::Literal { text, .. } => out.push_str(text),
                WordPart::Quoted { parts, .. } => {
                    for ip in parts {
                        match ip {
                            WordPart::Literal { text, .. } => out.push_str(text),
                            _ => return None,
                        }
                    }
                }
                _ => return None,
            }
        }
        Some(out)
    }

    let text = word_scalar_literal_text(value)?;
    if !(text.starts_with('(') && text.ends_with(')')) {
        return None;
    }
    let src = format!("{name}={text}");
    let seq = crate::parser::parse(&src).ok().flatten()?;
    if !seq.rest.is_empty() {
        return None;
    }
    // A bare `name=(…)` line parses as a single-stage `Pipeline` wrapping a
    // `Simple(Assign(…))` (or, in some contexts, the bare `Simple` directly).
    let cmd = match seq.first {
        crate::command::Command::Pipeline(mut p) if p.commands.len() == 1 => {
            p.commands.pop().expect("len == 1")
        }
        other => other,
    };
    let assigns = match cmd {
        crate::command::Command::Simple(crate::command::SimpleCommand::Assign(a, _)) => a,
        _ => return None,
    };
    let mut it = assigns.into_iter();
    let first = it.next()?;
    if it.next().is_some() {
        return None;
    }
    // Confirm the value parsed AS an array literal (guards against a value that
    // is not really `(...)`-shaped once lexed, e.g. `(` inside further quoting).
    if first
        .value
        .0
        .iter()
        .any(|p| matches!(p, WordPart::ArrayLiteral(_)))
    {
        Some(first.value)
    } else {
        None
    }
}

fn builtin_unset(args: &[String], err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    // Leading flags select the namespace and apply to all following names:
    // `-f` => function namespace, `-v` (or no flag) => variable namespace.
    // `-n` => variable namespace but unset the nameref variable ITSELF (no deref).
    let mut mode_fn = false;
    let mut saw_v = false;
    let mut unset_nameref = false;
    let mut g =
        crate::builtin_opts::Getopt::new("unset", crate::builtin_opts::ArgView::Plain(args), "fvn");
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                'f' => mode_fn = true,
                'v' => saw_v = true,
                'n' => unset_nameref = true,
                _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }
    // bash: `-f` and `-v` together is a runtime semantic error (not a usage
    // error — the scanner already accepted both flags), status 1, script
    // continues.
    if mode_fn && saw_v {
        crate::sh_error_to!(
            shell,
            err,
            None,
            "unset: cannot simultaneously unset a function and a variable"
        );
        return ExecOutcome::Continue(1);
    }
    let names = &args[g.rest_index()..];
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
                crate::sh_error_to!(
                    shell,
                    err,
                    None,
                    "unset: {arg}: cannot unset: readonly variable"
                );
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
                            // #572: `unset` names the SUBSCRIPT in brackets and
                            // nothing else — `unset 'a[-3]'` is
                            // `unset: [-3]: bad array subscript`, status 1, and
                            // the shell carries on. An ARITHMETIC failure in
                            // the subscript is a different animal: it is an
                            // expansion error, reported BARE (no `unset:`
                            // prefix) and fatal to the command list.
                            if e.is_arith() {
                                if !shell.discard_pending()
                                    && let Some(m) = e.message("")
                                {
                                    crate::sh_error_to!(shell, err, None, "{m}");
                                }
                                shell.report_error(crate::error_fatality::ErrorKind::Expansion);
                                return ExecOutcome::Continue(1);
                            }
                            if let Some(m) = e.message(&format!("[{sub_text}]")) {
                                crate::sh_error_to!(shell, err, None, "unset: {m}");
                            }
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
                "unset: {effective_arg}: cannot unset: readonly variable"
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
        Some(crate::shell_state::CaseFold::Capitalize) => attrs.push('c'),
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
            // L-44 (#32): render in bash hash-table iteration order, not
            // insertion order. Local view only — storage stays insertion-ordered.
            let ordered = crate::assoc_order::assoc_ordered_pairs(pairs.pairs());
            let parts: Vec<String> = ordered
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
    name: &str,
    args: &[DeclArg],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    // `-a` is a huck-specific no-op (mise emits `export -a chpwd_functions`);
    // `-p` lists (only when no operands); `-n` unexports; `-f` is function
    // export.
    let mut unexport = false;
    let mut func = false;
    let mut saw_p = false;
    let mut saw_a = false;
    let mut g =
        crate::builtin_opts::Getopt::new(name, crate::builtin_opts::ArgView::Decl(args), "pnfa");
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                'p' => saw_p = true,
                'a' => saw_a = true, // huck-specific no-op (mise `export -a chpwd_functions`)
                'n' => unexport = true,
                'f' => func = true,
                _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }
    let operands = &args[g.rest_index()..];

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
            // Under `-f`, operands are function NAMES, never assignments —
            // bash looks up (and reports) the whole token. A `name=value`
            // operand reaches us as a `DeclArg::Assign` only because the
            // executor's Root-D (#343) split fires ahead of flag parsing; here
            // we reconstruct the original `name=value` token so the lookup and
            // the `not a function` error use the full string (bash: `export -f
            // foo=bar` → `foo=bar: not a function`, not `foo:`).
            let assign_token;
            let name: &str = match arg {
                DeclArg::Plain(s) => s.as_str(),
                DeclArg::Assign(a) => {
                    // #346: expand the value (so a `$var` part renders its
                    // value) rather than concatenating only Literal parts —
                    // `export -f foo=$x` (x=bar) must report `foo=bar`, not
                    // `foo=`.
                    let mut t = a.target.name().to_string();
                    t.push_str(if a.append { "+=" } else { "=" });
                    t.push_str(&crate::expand::expand_assignment(&a.value, shell));
                    assign_token = t;
                    assign_token.as_str()
                }
            };
            if unexport {
                // export -nf NAME: remove the export mark (lenient — no-op if not
                // exported, matching bash's -n).
                shell.unmark_function_exported(name);
            } else if !shell.functions.contains_key(name) {
                crate::sh_error_to!(shell, err, None, "export: {name}: not a function");
                any_error = true;
            } else if name.contains('=') || name.contains('/') {
                // `=` can't be encoded into `BASH_FUNC_<name>%%` (the env-var
                // key would split at the `=`); `/` is rejected too — matches
                // bash 5.2.21 empirically (`function foo=bar`/`function
                // /bin/echo` define fine but `export -f` on either name gives
                // "cannot export", even though hyphens and most other
                // punctuation in a name export just fine).
                crate::sh_error_to!(shell, err, None, "export: {name}: cannot export");
                any_error = true;
            } else {
                shell.mark_function_exported(name);
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
                            "export: `{s}': not a valid identifier"
                        );
                        any_error = true;
                        continue;
                    }
                    if shell.is_readonly(name) {
                        // bash 5.2.21 emits NO `export:` prefix here:
                        //   $ bash -c 'readonly FOO=1; export FOO=2'
                        //   bash: line 1: FOO: readonly variable
                        crate::sh_error_to!(shell, err, None, "{name}: readonly variable");
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
                            "export: `{s}': not a valid identifier"
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
                if let crate::command::AssignTarget::Indexed { name, subscript } = &a.target {
                    // #585: the LIST rule comes first — `export a[0]=(x y)` is
                    // `a[0]: cannot assign list to array member`, not the
                    // identifier rejection below.
                    if crate::executor::reject_list_to_array_member(
                        name, subscript, &a.value, shell, err,
                    ) {
                        any_error = true;
                        continue;
                    }
                    // #114: name the FULL `name[subscript]` source, not the bare
                    // name. bash shows the subscript after word expansion but
                    // NOT arithmetic eval (`AA[$x]`→`AA[9]`, `AA[2+2]` stays).
                    let sub = crate::expand::expand_assignment(subscript, shell);
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "export: `{name}[{sub}]': not a valid identifier"
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
                    // bash 5.2.21 emits NO `export:` prefix here:
                    //   $ bash -c 'readonly FOO=1; export FOO=2'
                    //   bash: line 1: FOO: readonly variable
                    crate::sh_error_to!(shell, err, None, "{name}: readonly variable");
                    any_error = true;
                    continue;
                }
                // v349 (#343, Root B): `export -a NAME='(v)'` coerces the quoted
                // scalar `(...)` value into an array literal (matches bash).
                let reparsed_owned;
                let a = if saw_a && let Some(value) = reparse_paren_scalar_as_array(&name, &a.value)
                {
                    reparsed_owned = crate::command::Assignment {
                        target: a.target.clone(),
                        value,
                        append: a.append,
                    };
                    &reparsed_owned
                } else {
                    a
                };
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
fn builtin_local_decl(
    name: &str,
    args: &[DeclArg],
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
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
    let mut saw_minus_c = false;
    let mut saw_minus_n = false;
    // `local` DOES take `+`-style options — the comment here used to claim it
    // did not, and every `+anything` fell through to be reported as an invalid
    // identifier (#507). Measured on bash 5.2.21:
    //
    //   local +r x=1   -> rc 0, x=1        (accepted)
    //   local +x x=1   -> rc 0, x=1        (accepted)
    //   local +z x=1   -> `local: +z: invalid option` + usage, rc 2
    //
    // The `-` and `+` runs interleave, so scan them alternately, as `declare`
    // does. `+FLAG` means "do not give the new local this attribute", which is
    // already the default: bash's `local` creates a FRESH variable carrying
    // only the attributes its `-` flags ask for. So the accepted letters are
    // no-ops rather than removals, and that is faithful, not a shortcut.
    let mut idx = 0usize;
    loop {
        let pre_idx = idx;
        let mut g = crate::builtin_opts::Getopt::new(
            name,
            crate::builtin_opts::ArgView::Decl(&args[idx..]),
            "aAirlucn",
        );
        loop {
            match g.next_opt(shell, err) {
                Ok(Some(o)) => match o.ch {
                    'a' => want_array = true,
                    'A' => want_associative = true,
                    'i' => want_integer = true,
                    'r' => want_readonly = true,
                    'l' => saw_minus_l = true,
                    'u' => saw_minus_u = true,
                    'c' => saw_minus_c = true,
                    'n' => saw_minus_n = true,
                    _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
                },
                Ok(None) => break,
                Err(code) => return ExecOutcome::Continue(code),
            }
        }
        idx += g.rest_index();

        // `--` ends option processing for the `+` side too.
        if idx > pre_idx && matches!(&args[idx - 1], DeclArg::Plain(s) if s == "--") {
            break;
        }
        let Some(DeclArg::Plain(arg)) = args.get(idx) else {
            break;
        };
        if !(arg.starts_with('+') && arg.len() > 1) {
            break;
        }
        for &c in &arg.as_bytes()[1..] {
            match c {
                // Accepted, and a no-op: see the note above.
                b'a' | b'A' | b'i' | b'r' | b'l' | b'u' | b'c' | b'n' | b'x' => {}
                other => {
                    crate::builtin_opts::emit_invalid_plus_option(name, other, shell, err);
                    return ExecOutcome::Continue(2);
                }
            }
        }
        idx += 1;
    }
    if want_array && want_associative {
        crate::sh_error_to!(shell, err, None, "local: cannot specify both -a and -A");
        return ExecOutcome::Continue(1);
    }

    // Net case-fold attribute from this command's flags. bash: any TWO (or
    // three) of -l/-u/-c together in one invocation cancel to no attribute
    // (verified on bash 5.2.21: `declare -u -c v=x` / `declare -l -c v=x` /
    // `declare -u -l -c v=x` all yield `declare -- v="x"`, unfolded); exactly
    // one wins.
    let minus_case_fold: Option<Option<crate::shell_state::CaseFold>> =
        match [saw_minus_l, saw_minus_u, saw_minus_c]
            .iter()
            .filter(|b| **b)
            .count()
        {
            0 => None, // no minus case-fold flag this command
            1 if saw_minus_l => Some(Some(crate::shell_state::CaseFold::Lower)),
            1 if saw_minus_u => Some(Some(crate::shell_state::CaseFold::Upper)),
            1 => Some(Some(crate::shell_state::CaseFold::Capitalize)),
            _ => Some(None), // 2 or 3 together cancel → clear
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
                // v349 (#343, Root B): under -a/-A, coerce a quoted scalar
                // `(...)` value into an array literal before applying.
                let reparsed_owned;
                let a = if (want_array || want_associative)
                    && let Some(value) = reparse_paren_scalar_as_array(&name, &a.value)
                {
                    reparsed_owned = crate::command::Assignment {
                        target: a.target.clone(),
                        value,
                        append: a.append,
                    };
                    &reparsed_owned
                } else {
                    a
                };
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
    name: &str,
    args: &[DeclArg],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let mut want_list = false;
    let mut want_associative = false;
    let mut want_indexed = false;
    let mut g =
        crate::builtin_opts::Getopt::new(name, crate::builtin_opts::ArgView::Decl(args), "paA");
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                'p' => want_list = true,
                'a' => want_indexed = true,
                'A' => want_associative = true,
                _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }
    let rest = &args[g.rest_index()..];

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
                // `readonly -a NAME` (no value): ensure name is an indexed
                // array before marking readonly (mirrors want_associative
                // above; declare/local's -a bare-case pattern — promote an
                // existing scalar to element 0, or create an empty array).
                // Skip when NAME is already associative (e.g. `-aA` together,
                // or a pre-existing `-A` array): `-A` wins, matching bash.
                if want_indexed
                    && shell.get_associative(name).is_none()
                    && shell.get_indexed(name).is_none()
                {
                    let mut empty = std::collections::BTreeMap::new();
                    if let Some(scalar) = shell.get(name) {
                        empty.insert(0, scalar.to_string());
                    }
                    if shell.replace_indexed(name, empty).is_err() {
                        // assign() already emitted the readonly-variable
                        // error (bare `{name}: readonly variable`, no prefix).
                        exit = 1;
                        continue;
                    }
                }
                shell.mark_readonly(name);
            }
            DeclArg::Assign(a) => match &a.target {
                crate::command::AssignTarget::Bare(name) => {
                    if shell.is_readonly(name) {
                        // v349 (#343, Root C): bash prefixes this error with
                        // `readonly:` ONLY when an attribute flag (`-a`/`-A`)
                        // is given AND the RHS is not an unquoted array literal
                        // — i.e. an ATTRIBUTE-CHANGE attempt on a readonly var
                        // (`readonly -a x=2`, `readonly -a x='(7)'`). A plain
                        // assignment (`readonly x=2`) or an array-literal RHS
                        // (`readonly -a x=(2)`) fails at the assignment level →
                        // bare `{name}: readonly variable`.
                        let is_array_lit = a
                            .value
                            .0
                            .iter()
                            .any(|p| matches!(p, crate::lexer::WordPart::ArrayLiteral(_)));
                        if (want_indexed || want_associative) && !is_array_lit {
                            crate::sh_error_to!(
                                shell,
                                err,
                                None,
                                "readonly: {name}: readonly variable"
                            );
                        } else {
                            crate::sh_error_to!(shell, err, None, "{name}: readonly variable");
                        }
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
                    // `readonly -a NAME=value` / `readonly -a NAME=(...)`:
                    // ensure NAME is an indexed array BEFORE
                    // apply_one_assignment (mirrors want_associative above)
                    // so a scalar RHS lands as element 0 too (bash: `-a
                    // s=hello` -> `declare -ar s=([0]="hello")`), not just a
                    // compound-literal RHS (which self-creates an array
                    // regardless of `-a`). Skip when NAME is already
                    // associative: `-A` wins (matches bash `-aA` together).
                    if want_indexed
                        && shell.get_associative(name).is_none()
                        && shell.get_indexed(name).is_none()
                    {
                        let mut empty = std::collections::BTreeMap::new();
                        if let Some(scalar) = shell.get(name) {
                            empty.insert(0, scalar.to_string());
                        }
                        if shell.replace_indexed(name, empty).is_err() {
                            exit = 1;
                            continue;
                        }
                    }
                    // v349 (#343, Root B): under -a/-A, coerce a quoted scalar
                    // `(...)` value into an array literal before applying (the
                    // readonly-variable check above already saw the original
                    // scalar, preserving Root C's prefix decision).
                    let reparsed_owned;
                    let a = if (want_indexed || want_associative)
                        && let Some(value) = reparse_paren_scalar_as_array(name, &a.value)
                    {
                        reparsed_owned = crate::command::Assignment {
                            target: a.target.clone(),
                            value,
                            append: a.append,
                        };
                        &reparsed_owned
                    } else {
                        a
                    };
                    if crate::executor::apply_one_assignment(a, shell, err).is_err() {
                        exit = 1;
                        continue;
                    }
                    shell.mark_readonly(name);
                }
                crate::command::AssignTarget::Indexed { name, subscript } => {
                    // #585: the LIST rule comes first, as for `export`.
                    if crate::executor::reject_list_to_array_member(
                        name, subscript, &a.value, shell, err,
                    ) {
                        exit = 1;
                        continue;
                    }
                    // #585: bash rejects a subscripted lvalue here exactly as
                    // `export` does — `` `a[0]': not a valid identifier ``,
                    // naming the whole lvalue as written. huck had its own
                    // wording, naming only the variable.
                    //
                    // The LIST case never reaches this: a compound `(…)` RHS on
                    // an element is caught first, by the same check that serves
                    // a plain `a[0]=(x y)` (#76).
                    let lvalue = format!(
                        "{name}[{}]",
                        crate::expand::reconstruct_word_source(subscript)
                    );
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "readonly: `{lvalue}': not a valid identifier"
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
    name: &str,
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
    let mut saw_minus_c = false;
    let mut saw_plus_l = false;
    let mut saw_plus_u = false;
    let mut saw_plus_c = false;
    let mut saw_minus_n = false;
    let mut saw_plus_n = false;

    // Parse leading flags from Plain args. The `-` side is scanned by the
    // shared `Getopt` (bash's own `internal_getopt` does not handle `+`
    // either — `declare` special-cases it); a `+`-prefixed arg makes
    // `Getopt` stop immediately (it doesn't start with `-`), so this loop
    // alternates: run the scanner for a `-` run, then hand-process one `+`
    // run, then try the scanner again — so `-r +x -i` etc. interleave
    // exactly as bash allows. `--` terminates BOTH sides: `Getopt` consumes
    // it internally, so a `+`-looking arg right after `--` must NOT be
    // treated as an option; detect that by checking whether the run's last
    // consumed slot was the literal `--` terminator.
    let mut idx = 0;
    loop {
        let pre_idx = idx;
        let mut g = crate::builtin_opts::Getopt::new(
            name,
            crate::builtin_opts::ArgView::Decl(&args[idx..]),
            "rxiaAlucnfFpg",
        );
        loop {
            match g.next_opt(shell, err) {
                Ok(Some(o)) => match o.ch {
                    'r' => want_readonly = true,
                    'x' => want_export = true,
                    'i' => want_integer = true,
                    'a' => want_array = true,
                    'A' => want_associative = true,
                    'l' => saw_minus_l = true,
                    'u' => saw_minus_u = true,
                    'c' => saw_minus_c = true,
                    'n' => saw_minus_n = true,
                    'f' => function_mode = true,
                    'F' => {
                        function_mode = true;
                        function_names_only = true;
                    }
                    'p' => print_mode = true,
                    'g' => global = true,
                    _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
                },
                Ok(None) => break,
                Err(code) => return ExecOutcome::Continue(code),
            }
        }
        idx += g.rest_index();

        // `--` terminates option processing entirely, even for a `+`-look
        // arg that follows it.
        if idx > pre_idx && matches!(&args[idx - 1], DeclArg::Plain(s) if s == "--") {
            break;
        }

        let Some(DeclArg::Plain(arg)) = args.get(idx) else {
            break;
        };
        if !(arg.starts_with('+') && arg.len() > 1) {
            break;
        }
        for &c in &arg.as_bytes()[1..] {
            match c {
                // #507: bash ACCEPTS `+r` and does nothing — it cannot remove
                // readonly, but that is silent, not an error. The only failure
                // in `declare -r w=1; declare +r w=2` is the ASSIGNMENT to a
                // readonly variable (`declare: w: readonly variable`), which
                // the assignment path already reports. Measured: on a
                // non-readonly variable `declare +r v=2` succeeds and assigns.
                b'r' => {}
                b'x' => want_remove_export = true,
                b'i' => want_remove_integer = true,
                b'a' => {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "{name}: +a: array attribute cannot be removed"
                    );
                    return ExecOutcome::Continue(1);
                }
                b'A' => {
                    // TODO: bash compat — bash silently ignores `+A` on
                    // existing associatives (the attribute can't be
                    // removed once set). We mirror `+a`'s conservative
                    // rejection for now; revisit if real scripts need
                    // silent-ignore behavior.
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "{name}: +A: associative attribute cannot be removed"
                    );
                    return ExecOutcome::Continue(1);
                }
                b'l' => saw_plus_l = true,
                b'u' => saw_plus_u = true,
                b'c' => saw_plus_c = true,
                b'n' => saw_plus_n = true,
                other => {
                    crate::builtin_opts::emit_invalid_plus_option(name, other, shell, err);
                    return ExecOutcome::Continue(2);
                }
            }
        }
        idx += 1;
    }
    let names = &args[idx..];

    // Net case-fold attribute from this command's flags. bash: any TWO (or
    // three) of -l/-u/-c together in one invocation cancel to no attribute
    // (verified on bash 5.2.21: `declare -u -c v=x` / `declare -l -c v=x` /
    // `declare -u -l -c v=x` all yield `declare -- v="x"`, unfolded); exactly
    // one wins.
    let minus_case_fold: Option<Option<crate::shell_state::CaseFold>> =
        match [saw_minus_l, saw_minus_u, saw_minus_c]
            .iter()
            .filter(|b| **b)
            .count()
        {
            0 => None, // no minus case-fold flag this command
            1 if saw_minus_l => Some(Some(crate::shell_state::CaseFold::Lower)),
            1 if saw_minus_u => Some(Some(crate::shell_state::CaseFold::Upper)),
            1 => Some(Some(crate::shell_state::CaseFold::Capitalize)),
            _ => Some(None), // 2 or 3 together cancel → clear
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
        if saw_plus_c && shell.case_fold_of(name) == Some(crate::shell_state::CaseFold::Capitalize)
        {
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
            } else if let Some(cur) = shell.get(name) {
                // Value-less `declare -n NAME`: bash validates the variable's
                // EXISTING value as the reference target, and this check fires
                // BEFORE the readonly check (verified on bash 5.2.21:
                // `readonly PATH; declare -n PATH` reports the invalid-name
                // error). An unset NAME is accepted and simply gains the `-n`
                // attribute.
                //
                // DIVERGENCE, unset + readonly: bash reports `readonly
                // variable` there, huck reports the invalid-name error with an
                // empty value. Not reachable as an escape (rc 1, nothing is
                // applied) — it falls out of huck having no attribute-without-
                // value state, so `readonly FOO` on an unset FOO creates FOO
                // as set-to-empty and `shell.get` returns Some(""). See #225.
                //
                // #227: an array-valued name cannot become a nameref — bash
                // refuses BEFORE validating the value (`shell.get` on an array
                // returns element 0, which would otherwise slip through).
                if shell.get_indexed(name).is_some() || shell.get_associative(name).is_some() {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "declare: {name}: reference variable cannot be an array"
                    );
                    exit = 1;
                    continue;
                }
                let cur = cur.to_string();
                let valid = is_valid_name(&cur)
                    || matches!(parse_subscripted_arg(&cur), Ok(Some((b, _))) if is_valid_name(b));
                if !valid {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "declare: `{cur}': invalid variable name for name reference"
                    );
                    exit = 1;
                    continue;
                }
            }
            // A nameref BIND writes through the `Shell::set` leaf, which does
            // not itself enforce readonly — so the readonly gate must live
            // here. Without it `declare -n PATH=EVIL; EVIL=...` would let the
            // nameref deref hand out arbitrary control of a readonly variable
            // (a sandbox escape under a restricted policy, which marks PATH et
            // al readonly). bash 5.2.21 refuses the bind and applies NOTHING —
            // not even the `-n` attribute:
            //   $ bash -c 'readonly FOO=1; declare -n FOO=BAR; declare -p FOO'
            //   bash: declare: FOO: readonly variable
            //   declare -r FOO="1"
            // The value-LESS form (`declare -n FOO`) is refused too: bash
            // treats FOO's existing value as the target name, so applying `-n`
            // to a readonly FOO would make `FOO=x` write through to whatever
            // that value names — the readonly gate bypassed entirely, because
            // `resolve_assign_target` checks readonly on the RESOLVED name.
            //   $ bash -c 'readonly RO=safe; declare -nx RO; declare -p RO'
            //   bash: declare: RO: readonly variable
            //   declare -r RO="safe"      # neither -n nor -x applied
            if shell.is_readonly(name) {
                crate::sh_error_to!(shell, err, None, "declare: {name}: readonly variable");
                exit = 1;
                continue;
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
            // `=VALUE` must not clobber an existing readonly, with or without
            // a co-requested `-r`.
            if shell.is_readonly(name) {
                crate::sh_error_to!(shell, err, None, "declare: {name}: readonly variable");
                exit = 1;
                continue;
            }
            // v349 (#343, Root B): under -a/-A, coerce a quoted scalar `(...)`
            // value into an array literal before applying (bash re-parses the
            // value as an array literal).
            let reparsed_owned;
            let a = if (want_array || want_associative)
                && let Some(value) = reparse_paren_scalar_as_array(name, &a.value)
            {
                reparsed_owned = crate::command::Assignment {
                    target: a.target.clone(),
                    value,
                    append: a.append,
                };
                &reparsed_owned
            } else {
                a
            };
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
            if pr < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue; // EINTR: re-check the deadline and re-poll
            }
            // Other poll errors: fall through and attempt the read, as before.
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

/// Accumulate one word from `bytes` starting at `i`, then consume the
/// separator run that follows it, returning the word text and the position
/// AFTER the separator run. This is huck's port of bash's
/// `get_word_from_string`: a non-ws IFS char delimits with exactly one
/// occurrence + trailing ws-IFS; a ws-IFS run collapses, then optionally one
/// non-ws IFS + trailing ws-IFS is consumed. No leading-IFS-whitespace is
/// skipped here — the caller positions `i` at a word start.
fn next_word(
    bytes: &[u8],
    mut i: usize,
    is_ws: impl Fn(u8) -> bool,
    is_nonws: impl Fn(u8) -> bool,
    is_any: impl Fn(u8) -> bool,
) -> (String, usize) {
    let start = i;
    while i < bytes.len() && !is_any(bytes[i]) {
        i += 1;
    }
    let word = String::from_utf8_lossy(&bytes[start..i]).into_owned();
    if i < bytes.len() {
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
    (word, i)
}

/// POSIX/bash `read`-style field splitting. Assigns fields to
/// `names` left-to-right; the LAST name gets the remainder of the line via
/// bash's `read.def` rule (extract one more word — if it exhausts the line the
/// trailing delimiter is dropped, otherwise the raw remainder is kept with only
/// trailing IFS-whitespace stripped). For a single name, the line is assigned
/// whole with leading + trailing IFS-whitespace stripped.
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

    // Field walk. For a single name, the n-1 loop runs zero times and the
    // last-field block below applies bash's read.def rule to the whole line
    // (so `IFS=: read x` on `a:` yields `a`, not `a:` — a sole trailing
    // non-ws delimiter is dropped, exactly as for the last of many names).
    let mut fields: Vec<String> = Vec::new();
    let mut i = 0;

    // Skip leading IFS-whitespace.
    while i < bytes.len() && is_ws(bytes[i]) {
        i += 1;
    }

    while fields.len() < names.len() - 1 && i < bytes.len() {
        // Extract one word + consume its separator run.
        let (word, next) = next_word(bytes, i, is_ws, is_nonws, is_any_ifs);
        fields.push(word);
        i = next;
    }

    // Pad missing fields.
    while fields.len() < names.len() - 1 {
        fields.push(String::new());
    }

    // Last field: bash's read.def last-variable rule (read.def ~1009-1037).
    // The remainder starting at `i` becomes the last field, EXCEPT that a sole
    // trailing delimiter run is dropped: extract one word with `next_word`; if
    // that word + its separator run consumes to end of line, the last field is
    // just the word (trailing delimiter dropped). Otherwise the last field is
    // the RAW remainder with only trailing IFS-WHITESPACE stripped (interior
    // and extra trailing non-ws delimiters KEPT).
    let last = if i >= bytes.len() {
        String::new()
    } else {
        let (word, p) = next_word(bytes, i, is_ws, is_nonws, is_any_ifs);
        if p >= bytes.len() {
            word
        } else {
            let mut end = bytes.len();
            while end > i && is_ws(bytes[end - 1]) {
                end -= 1;
            }
            String::from_utf8_lossy(&bytes[i..end]).into_owned()
        }
    };
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
        let (word, next) = next_word(bytes, i, is_ws, is_nonws, is_any);
        fields.push(word);
        i = next;
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

/// `mapfile [-d DELIM] [-n COUNT] [-O ORIGIN] [-s SKIP] [-t] [ARRAY]`
/// (alias `readarray`). Reads delimiter-separated records from stdin into the
/// indexed array ARRAY (default MAPFILE). Core option set (v140); `-u`/`-C`/`-c`
/// are not implemented.
fn builtin_mapfile(
    name: &str,
    args: &[String],
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let mut delim: u8 = b'\n';
    let mut strip_t = false;
    let mut count: usize = 0; // 0 = unlimited
    let mut skip: usize = 0;
    let mut origin: Option<usize> = None;
    let mut read_fd: Option<std::os::unix::io::RawFd> = None;
    let mut callback: Option<String> = None;
    // bash's default quantum is 5000, so `-C` with no `-c` fires only on very
    // large inputs — measured: `-C cb` over 3 lines fires nothing.
    let mut quantum: usize = 5000;

    // Parse a numeric option value.
    /// `what` names the thing bash could not parse — it uses a DIFFERENT
    /// message per option, not one generic string (#513), measured on
    /// bash 5.2.21:
    ///
    ///   -n, -s  ->  "invalid line count"
    ///   -O      ->  "invalid array origin"
    ///   -c      ->  "invalid callback quantum"
    ///
    /// and exits 1, not the 2 huck used for all four.
    fn num_val(
        s: &str,
        what: &str,
        err: &mut dyn Write,
        shell: &Shell,
        name: &str,
    ) -> Result<usize, ()> {
        match s.trim().parse::<usize>() {
            Ok(n) => Ok(n),
            Err(_) => {
                crate::sh_error_to!(shell, err, None, "{name}: {s}: invalid {what}");
                Err(())
            }
        }
    }

    let mut g = crate::builtin_opts::Getopt::new(
        name,
        crate::builtin_opts::ArgView::Plain(args),
        "d:n:O:s:tu:C:c:",
    );
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                't' => strip_t = true,
                'd' => {
                    let s = o.value.expect("spec requires a value for -d");
                    delim = s.bytes().next().unwrap_or(0u8); // empty -> NUL
                }
                'n' => {
                    let v = o.value.expect("spec requires a value for -n");
                    match num_val(&v, "line count", err, shell, name) {
                        Ok(n) => count = n,
                        Err(()) => return ExecOutcome::Continue(1),
                    }
                }
                's' => {
                    let v = o.value.expect("spec requires a value for -s");
                    match num_val(&v, "line count", err, shell, name) {
                        Ok(n) => skip = n,
                        Err(()) => return ExecOutcome::Continue(1),
                    }
                }
                'O' => {
                    let v = o.value.expect("spec requires a value for -O");
                    match num_val(&v, "array origin", err, shell, name) {
                        Ok(n) => origin = Some(n),
                        Err(()) => return ExecOutcome::Continue(1),
                    }
                }
                // `-u FD` (#511). Same validation and wording as `read -u`:
                // a non-numeric spec is a "specification" error, an unopened
                // one is "Bad file descriptor" — both rc 1, both measured
                // against bash 5.2.21.
                'u' => {
                    let v = o.value.expect("spec requires a value for -u");
                    match v.trim().parse::<std::os::unix::io::RawFd>() {
                        Ok(fd) if fd >= 0 => read_fd = Some(fd),
                        _ => {
                            crate::sh_error_to!(
                                shell,
                                err,
                                None,
                                "{name}: {v}: invalid file descriptor specification"
                            );
                            return ExecOutcome::Continue(1);
                        }
                    }
                }
                'C' => callback = o.value,
                'c' => {
                    let v = o.value.expect("spec requires a value for -c");
                    match num_val(&v, "callback quantum", err, shell, name) {
                        Ok(n) => quantum = n,
                        Err(()) => return ExecOutcome::Continue(1),
                    }
                }
                _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }

    let array_name = args
        .get(g.rest_index())
        .cloned()
        .unwrap_or_else(|| "MAPFILE".to_string());
    if !is_valid_name(&array_name) {
        crate::sh_error_to!(
            shell,
            err,
            None,
            "{name}: `{array_name}': not a valid array name"
        );
        return ExecOutcome::Continue(1);
    }

    // `-u FD`: validate the fd is open BEFORE reading, exactly as `read -u`
    // does (bash checks via fcntl(fd, F_GETFD) == -1), so an unopened fd errors
    // without consuming input.
    if let Some(fd) = read_fd
        && unsafe { libc::fcntl(fd, libc::F_GETFD) } == -1
    {
        crate::sh_error_to!(
            shell,
            err,
            None,
            "{name}: {fd}: invalid file descriptor: Bad file descriptor"
        );
        return ExecOutcome::Continue(1);
    }

    let mut handle = match read_fd {
        Some(fd) => RawFdReader::from_fd(fd),
        None => RawFdReader::new(),
    };
    // Skip the first `skip` records.
    for _ in 0..skip {
        match read_one_record(&mut handle, delim) {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => {
                crate::sh_error_to!(shell, err, None, "{name}: {}", crate::bash_io_error(&e));
                return ExecOutcome::Continue(1);
            }
        }
    }
    // Without `-O`, mapfile REPLACES the array — and does so even when it goes
    // on to read nothing (measured: `A=(x y z); mapfile A </dev/null` leaves
    // A empty). Clearing up front is also what lets a `-C` callback observe the
    // elements assigned so far, which bash's does.
    let base = origin.unwrap_or(0);
    if origin.is_none()
        && shell
            .replace_indexed(&array_name, std::collections::BTreeMap::new())
            .is_err()
    {
        return ExecOutcome::Continue(1);
    }

    // Collect up to `count` (0 = unlimited) records.
    let mut n_read: usize = 0;
    loop {
        if count != 0 && n_read >= count {
            break;
        }
        match read_one_record(&mut handle, delim) {
            Ok(Some((content, had_delim))) => {
                let mut val = content;
                if had_delim && !strip_t {
                    val.push(delim as char);
                }
                // `-C CALLBACK -c QUANTUM` (#511). bash evaluates the STRING
                // `CALLBACK INDEX LINE` — the index and line are appended to
                // the callback text and the whole thing is run as a command
                // line, NOT passed as positional parameters (measured: with
                // `-C 'echo idx=$1'` the `$1` is EMPTY and `1 b` is appended
                // to the output).
                //
                // It fires after every QUANTUM-th record and BEFORE that
                // element is assigned, so the array is one short at that
                // moment — measured with `-c 2` over five lines, the callback
                // saw `${#A[@]}` as 1 then 3, and fired at indices 1 and 3.
                // Hence `(n_read + 1) % quantum`, not `n_read % quantum`:
                // the latter fires at 0, 2, 4 and makes bash's default quantum
                // of 5000 fire immediately on the first record.
                if let Some(cb) = &callback
                    && quantum != 0
                    && (n_read + 1).is_multiple_of(quantum)
                {
                    let idx = base + n_read;
                    let line = val.trim_end_matches(delim as char);
                    let script = format!("{cb} {idx} {line}");
                    let _ = run_sourced_contents_in_sinks(
                        &script,
                        std::path::Path::new("mapfile"),
                        shell,
                    );
                }
                if shell
                    .set_indexed_element(&array_name, base + n_read, val)
                    .is_err()
                {
                    return ExecOutcome::Continue(1);
                }
                n_read += 1;
            }
            Ok(None) => break,
            Err(e) => {
                crate::sh_error_to!(shell, err, None, "{name}: {}", crate::bash_io_error(&e));
                return ExecOutcome::Continue(1);
            }
        }
    }

    // Elements were assigned incrementally above.
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

    let mut g = crate::builtin_opts::Getopt::new(
        "read",
        crate::builtin_opts::ArgView::Plain(args),
        "ersa:d:i:n:N:p:t:u:",
    );
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                'e' => {
                    // Readline editing — huck's `read` has no readline-editing
                    // mode; accepted (matches bash's option spec) and ignored.
                }
                'r' => raw = true,
                's' => silent = true,
                'i' => {
                    // Readline initial text — only meaningful together with
                    // `-e`, itself a no-op here; accepted and ignored.
                    let _ = o.value;
                }
                'p' => prompt = o.value,
                'd' => {
                    let d_val = o.value.expect("spec requires a value for -d");
                    // Empty DELIM means NUL byte.
                    delim = d_val.bytes().next().unwrap_or(0u8);
                }
                'a' => array_name = o.value,
                'u' => {
                    let v = o.value.expect("spec requires a value for -u");
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
                }
                'n' | 'N' => {
                    let upper = o.ch == 'N';
                    let v = o.value.expect("spec requires a value for -n/-N");
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
                }
                't' => {
                    let v = o.value.expect("spec requires a value for -t");
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
                }
                _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }
    let names: Vec<String> = args[g.rest_index()..].to_vec();

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
            // #546: bash's `read_builtin` reports a failed read(2) as
            // `read error: <fd>: <strerror>`, naming the descriptor it was
            // reading from — 0 by default, or the `-u` fd. huck printed the
            // strerror alone, which loses the only clue to WHICH fd failed
            // when a script reads from several.
            let fd = handle.raw_fd();
            crate::sh_error_to!(
                shell,
                err,
                None,
                "read: read error: {fd}: {}",
                crate::bash_io_error(&e)
            );
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
        // `try_set` has already printed bash's `<name>: readonly variable`
        // (as the `-a` path above notes for `replace_indexed`); read adds no
        // second, `read:`-prefixed line — bash prints exactly one.
        if shell.try_set(&name, value).is_err() {
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
    // Parse leading flags: -v VAR.
    let mut v_var: Option<String> = None;
    let mut g =
        crate::builtin_opts::Getopt::new("printf", crate::builtin_opts::ArgView::Plain(args), "v:");
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                'v' => {
                    let target = o.value.expect("spec requires a value for -v");
                    let valid = is_valid_name(&target)
                        || crate::expand::split_name_subscript(&target)
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
                    v_var = Some(target);
                }
                _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }
    let i = g.rest_index();

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
    shell: &mut Shell,
) -> Result<JobsArgs, ExecOutcome> {
    let mut long = false;
    let mut pids_only = false;
    let mut only_new = false;
    let mut only_running = false;
    let mut only_stopped = false;

    // `-x` is deliberately NOT in this spec, even though it's a real bash
    // option (`jobs -x command [args]`: substitutes jobspecs with pids and
    // execs COMMAND in the shell's place). huck has no exec-replace path
    // reachable from a builtin body, and pre-v359 huck already rejected it
    // outright (`-x: invalid option`, rc 2) since the old hand-rolled
    // scanner had no arm for it. This task's first cut accepted it and
    // reported "not supported" (rc 1) instead — worse than either the old
    // behavior OR real bash: bare `jobs -x` exits 0 silently in real bash,
    // so huck went from matching-by-accident to actively wrong (#496 Task 6
    // review, Important). Leaving `x` out of the spec restores the loud
    // rejection via the scanner's own generic invalid-option path — same
    // status/shape as any other unrecognized flag, no bespoke message.
    let mut g = crate::builtin_opts::Getopt::new(
        "jobs",
        crate::builtin_opts::ArgView::Plain(args),
        "lnprs",
    );
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                'l' => long = true,
                'p' => pids_only = true,
                'n' => only_new = true,
                'r' => only_running = true,
                's' => only_stopped = true,
                _ => return Err(ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err))),
            },
            Ok(None) => break,
            Err(code) => return Err(ExecOutcome::Continue(code)),
        }
    }
    let idx = g.rest_index();

    let mut targets = Vec::new();
    for arg in &args[idx..] {
        // #423: the `%` is optional here — `jobs 1` is `jobs %1`.
        targets.push(resolve_job_operand(arg, "jobs", err, shell)?);
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
    // #426: with operands, bash prints one line per OPERAND, in the order
    // given — so `jobs %2 %1` lists 2 then 1, and `jobs %1 %1` lists twice.
    // Without operands it walks the table in job order.
    let order: Vec<u32> = if parsed.targets.is_empty() {
        shell.jobs.iter().map(|j| j.id).collect()
    } else {
        parsed.targets.clone()
    };
    let mut printed_ids: Vec<u32> = Vec::new();
    for id in order {
        let Some(job) = shell.jobs.iter().find(|j| j.id == id) else {
            continue;
        };
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

/// A single positional `wait` target, as classified by `parse_wait_args`.
///
/// #411: classification never fails and never aborts the operand list — bash
/// processes EVERY operand, diagnosing each bad one where it stands, and
/// returns the status of the LAST one. A `%spec` is therefore kept unresolved
/// until the wait loop reaches it, and a word that is neither a pid nor a spec
/// becomes `Bad` rather than an early return.
enum WaitTarget {
    Pid(i32),
    Spec(String),
    Bad(String),
}

/// A `wait -n` target after resolution. `-n` reports and drops everything it
/// cannot resolve up front, so the polling helpers only ever see these two —
/// the type makes the unresolved forms unrepresentable there.
enum LiveTarget {
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
    shell: &mut Shell,
) -> Result<WaitArgs, ExecOutcome> {
    let mut wait_any = false;
    let mut pid_var: Option<String> = None;

    let mut g =
        crate::builtin_opts::Getopt::new("wait", crate::builtin_opts::ArgView::Plain(args), "fnp:");
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                'n' => wait_any = true,
                // #160: "wait for full termination rather than a status change".
                // huck's wait has no return-on-stop path (it already blocks to
                // termination), so accept-and-conform: no state to record.
                'f' => {}
                'p' => pid_var = o.value,
                _ => return Err(ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err))),
            },
            Ok(None) => break,
            Err(code) => return Err(ExecOutcome::Continue(code)),
        }
    }
    let mut idx = g.rest_index();

    // No "-p requires -n" rule: bash has none (#514). `wait -p v` on its own is
    // accepted silently, rc 0, and simply leaves `v` unset — measured, there is
    // no pid to record. huck rejected it with a message bash never emits.

    let mut targets = Vec::with_capacity(args.len() - idx);
    while idx < args.len() {
        let arg = &args[idx];
        // A pid operand must START with a digit (`wait +12` and `wait " 12"`
        // are bad words in bash, though `legal_number` itself would take
        // them); the value then follows `legal_number`, so `12 ` is pid 12.
        // `0` is a legal pid word — it is diagnosed as "not a child" later,
        // never handed to `waitpid`, where it would mean "my process group".
        targets.push(if arg.starts_with('%') {
            WaitTarget::Spec(arg.clone())
        } else if arg.starts_with(|c: char| c.is_ascii_digit()) {
            match parse_legal_number(arg) {
                Some(pid) => WaitTarget::Pid(pid),
                None => WaitTarget::Bad(arg.clone()),
            }
        } else {
            WaitTarget::Bad(arg.clone())
        });
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

    // #224: `-p VAR` on a READONLY variable is refused UP FRONT — bash checks
    // before it waits (measured: the error is instant, not after the child
    // finishes) and returns 1 without reaping anything. The wording is its
    // own: `wait` unsets the variable before assigning, so what it reports is
    // the failed UNSET.
    if let Some(name) = &parsed.pid_var
        && shell.is_readonly(name)
    {
        crate::sh_error_to!(
            shell,
            err,
            None,
            "wait: {name}: cannot unset: readonly variable"
        );
        return ExecOutcome::Continue(1);
    }

    let outcome = match (parsed.wait_any, parsed.targets.len()) {
        (false, 0) => wait_all(shell),
        (false, _) => wait_for_all(parsed.targets, err, shell),
        (true, 0) => wait_any_pending(parsed.pid_var, shell),
        (true, _) => {
            // #411: `-n` has its own error model — EVERY operand that is not a
            // live job or child is reported as "no such job" (whatever its
            // form), and 127 comes back only when none of them was live.
            let mut live = Vec::with_capacity(parsed.targets.len());
            for t in parsed.targets {
                match t {
                    WaitTarget::Spec(spec) => {
                        match crate::job_spec::parse_job_spec(&spec)
                            .ok()
                            .and_then(|s| shell.jobs.resolve(&s).ok())
                        {
                            Some(id) => live.push(LiveTarget::Job(id)),
                            None => {
                                crate::sh_error_to!(shell, err, None, "wait: {spec}: no such job");
                            }
                        }
                    }
                    WaitTarget::Pid(pid) => {
                        if shell.jobs.iter().any(|j| j.pids.contains(&pid)) {
                            live.push(LiveTarget::Pid(pid));
                        } else {
                            crate::sh_error_to!(shell, err, None, "wait: {pid}: no such job");
                        }
                    }
                    WaitTarget::Bad(word) => {
                        crate::sh_error_to!(shell, err, None, "wait: {word}: no such job");
                    }
                }
            }
            if live.is_empty() {
                return ExecOutcome::Continue(127);
            }
            wait_any_of(live, parsed.pid_var, shell)
        }
    };
    mark_signaled_jobs_notified(shell);
    outcome
}

fn wait_all(shell: &mut Shell) -> ExecOutcome {
    while shell.jobs.has_pending() {
        if let Some(o) = crate::executor::check_interrupt(shell) {
            return o;
        }
        // #453: a TRAPPED signal interrupts `wait`. bash runs the action and
        // returns 128+n immediately, leaving the remaining jobs running —
        // it does not resume waiting. An ignored (`trap '' SIG`) or untrapped
        // signal does not interrupt, which `dispatch_pending_traps` encodes by
        // reporting only actions it actually ran.
        if let Some(sig) = crate::traps::dispatch_pending_traps(shell) {
            return ExecOutcome::Continue(128 + sig);
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

/// #418: a job that DIED FROM A SIGNAL and whose status `wait` collected is not
/// announced — `wait` already reported the death through its 128+N status, and
/// bash stays silent for it in both notice forms:
///
/// ```text
/// set -m; sleep 3 & kill -TERM %1; wait      # bash: nothing
/// set -m; sleep 3 & kill -TERM %1; sleep 0.3 # bash: [1]+  Terminated  sleep 3
/// ```
///
/// A job that exited NORMALLY still gets its `[N]+ Done` line after a `wait`.
/// Marking them here rather than in each wait path also removes a race: whether
/// the boundary pass or `wait` reaped first used to decide whether the notice
/// appeared at all.
fn mark_signaled_jobs_notified(shell: &mut Shell) {
    for job in shell.jobs.jobs_mut() {
        if matches!(job.state, crate::jobs::JobState::Signaled(_)) {
            job.notified = true;
        }
    }
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
        // #453: a TRAPPED signal interrupts `wait`. bash runs the action and
        // returns 128+n immediately, leaving the remaining jobs running —
        // it does not resume waiting. An ignored (`trap '' SIG`) or untrapped
        // signal does not interrupt, which `dispatch_pending_traps` encodes by
        // reporting only actions it actually ran.
        if let Some(sig) = crate::traps::dispatch_pending_traps(shell) {
            return ExecOutcome::Continue(128 + sig);
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
        // #453: a TRAPPED signal interrupts `wait`. bash runs the action and
        // returns 128+n immediately, leaving the remaining jobs running —
        // it does not resume waiting. An ignored (`trap '' SIG`) or untrapped
        // signal does not interrupt, which `dispatch_pending_traps` encodes by
        // reporting only actions it actually ran.
        if let Some(sig) = crate::traps::dispatch_pending_traps(shell) {
            return ExecOutcome::Continue(128 + sig);
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
        // #411: each operand is resolved WHERE IT STANDS — a failure is
        // reported and the loop moves on, so `wait %1 %2` diagnoses both. The
        // status of the last operand is the builtin's status: 127 for one that
        // named no job/child, 1 for a word that was no kind of id at all.
        let outcome = match t {
            WaitTarget::Pid(pid) if pid > 0 => wait_for_pid(pid, err, shell),
            WaitTarget::Pid(pid) => {
                crate::sh_error_to!(
                    shell,
                    err,
                    None,
                    "wait: pid {pid} is not a child of this shell"
                );
                ExecOutcome::Continue(127)
            }
            WaitTarget::Spec(spec) => match resolve_spec_or_error(&spec, "wait", err, shell) {
                Ok(id) => wait_for_job(id, shell),
                Err(_) => ExecOutcome::Continue(127),
            },
            WaitTarget::Bad(word) => {
                crate::sh_error_to!(
                    shell,
                    err,
                    None,
                    "wait: `{word}': not a pid or valid job spec"
                );
                ExecOutcome::Continue(1)
            }
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
        // #453: a TRAPPED signal interrupts `wait`. bash runs the action and
        // returns 128+n immediately, leaving the remaining jobs running —
        // it does not resume waiting. An ignored (`trap '' SIG`) or untrapped
        // signal does not interrupt, which `dispatch_pending_traps` encodes by
        // reporting only actions it actually ran.
        if let Some(sig) = crate::traps::dispatch_pending_traps(shell) {
            return ExecOutcome::Continue(128 + sig);
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
    targets: Vec<LiveTarget>,
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
        LiveTarget::Job(id) => shell.jobs.iter().any(|j| j.id == *id),
        LiveTarget::Pid(pid) => {
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
        // #453: a TRAPPED signal interrupts `wait`. bash runs the action and
        // returns 128+n immediately, leaving the remaining jobs running —
        // it does not resume waiting. An ignored (`trap '' SIG`) or untrapped
        // signal does not interrupt, which `dispatch_pending_traps` encodes by
        // reporting only actions it actually ran.
        if let Some(sig) = crate::traps::dispatch_pending_traps(shell) {
            return ExecOutcome::Continue(128 + sig);
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
/// For `LiveTarget::Job(id)` the captured pid is the job's `pgid`. For
/// `LiveTarget::Pid(pid)` the captured pid is the literal PID arg.
fn check_targets_terminal(targets: &[LiveTarget], shell: &Shell) -> Option<(i32, i32)> {
    for t in targets {
        match t {
            LiveTarget::Job(id) => {
                if let Some(job) = shell.jobs.iter().find(|j| j.id == *id) {
                    match job.state {
                        crate::jobs::JobState::Done(c) => return Some((job.pgid, c)),
                        crate::jobs::JobState::Signaled(s) => return Some((job.pgid, 128 + s)),
                        _ => {}
                    }
                }
            }
            LiveTarget::Pid(pid) => {
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
    // #406: bash's option loop swallows ONE leading `-` word after `-l` — it
    // lands in the sigspec slot, which `-l` makes irrelevant, so it is never
    // validated (`kill -l -x 15` prints TERM, `kill -l -TERM` lists). Only the
    // FIRST such word: `kill -l -x -3` still rejects `-3` as an operand.
    let args = match args.first() {
        Some(a) if a.starts_with('-') => &args[1..],
        _ => args,
    };
    if args.is_empty() {
        print_killable_table(out);
        return ExecOutcome::Continue(0);
    }

    for arg in args {
        if let Some(n) = parse_legal_number(arg) {
            let lookup = if n >= 128 { n - 128 } else { n };
            // #405: 0 is EXIT — named by `kill -l 0` but absent from the
            // listing, so it is not in `killable_signals()`. The 128+signo
            // wait-status form does NOT decode to it (`kill -l 128` is an
            // error in bash), hence the `n < 128` guard.
            if lookup == 0 && n < 128 {
                let _ = writeln!(out, "EXIT");
                continue;
            }
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
            // `EXIT` is a name-only pseudo-signal: bash accepts `kill -l exit`
            // but not `kill -l SIGEXIT` (#405).
            if arg.eq_ignore_ascii_case("EXIT") {
                let _ = writeln!(out, "0");
                continue;
            }
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
/// Resolves a job-spec operand for the JOB-ONLY builtins (`fg`, `bg`, `jobs`),
/// where bash's `get_job_spec` makes the leading `%` optional (#423): `1` is
/// job 1 and `foo` is a command-prefix match, exactly as `%1` and `%foo` are.
/// Diagnostics echo the operand as the user wrote it, without the `%`.
///
/// The pid-taking builtins must NOT use this: for `kill`, `wait` and `disown`
/// a bare number is a PID, which is why bash answers `disown: 1: no such job`
/// even when job 1 exists.
fn resolve_job_operand(
    arg: &str,
    builtin: &str,
    err: &mut dyn Write,
    shell: &Shell,
) -> Result<u32, ExecOutcome> {
    let spec = if arg.starts_with('%') {
        arg.to_string()
    } else {
        format!("%{arg}")
    };
    crate::job_spec::parse_job_spec(&spec)
        .ok()
        .and_then(|s| shell.jobs.resolve(&s).ok())
        .ok_or_else(|| {
            crate::sh_error_to!(shell, err, None, "{builtin}: {arg}: no such job");
            ExecOutcome::Continue(1)
        })
}

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
        if first == "--" {
            // Bare `--`: no sigspec, and the `--` is consumed here (so
            // `kill -- --` still reports the SECOND `--` as a bad target).
            (libc::SIGTERM, &args[1..])
        } else if let Some(rest) = first.strip_prefix('-') {
            // -<sig> form. #402: bash has ONE wording for every rejected
            // sigspec, whatever form it took (`sh_invalidsig`).
            let sig = match decode_signal(rest) {
                Some(n) => n,
                None => {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "kill: {rest}: invalid signal specification"
                    );
                    return ExecOutcome::Continue(1);
                }
            };
            (sig, strip_end_of_options(&args[1..]))
        } else {
            // A non-option first word ends option processing immediately, so a
            // later `--` is an ordinary (invalid) target, not a separator.
            (libc::SIGTERM, args)
        }
    } else {
        e!(
            err,
            "kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | jobspec ... or kill -l [sigspec]"
        );
        return ExecOutcome::Continue(2);
    };
    if targets.is_empty() {
        e!(
            err,
            "kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | jobspec ... or kill -l [sigspec]"
        );
        return ExecOutcome::Continue(2);
    }

    send_signal_to_targets(sig, targets, err, shell)
}

/// Drops a single leading `--` from a `kill` target list. bash's `kill.def`
/// breaks out of its option loop on the first `--`, so only ONE is consumed
/// and only at the head of the targets — any later `--` is an ordinary target
/// (and fails as "arguments must be process or job IDs"). Consuming it is what
/// makes the negative-pid process-group form usable with the default signal
/// (`kill -- -$pgid`), since a leading `-<n>` would otherwise be a sigspec.
/// Parses a number the way bash's `legal_number()` does (#402): `strtol(3)`
/// skips leading whitespace (the C `isspace` set), `legal_number` then skips
/// trailing SPACES AND TABS ONLY, and the rest of the string must be consumed.
/// So ` 12`, `12 `, `\t12\t` and ` -99999 ` parse, while `12\n`, `0x10` and
/// `12abc` do not. `kill` runs both its pid targets and its numeric sigspecs
/// through this (an out-of-`i32` value is rejected either way); the caller
/// applies whatever range the position requires.
fn parse_legal_number(s: &str) -> Option<i32> {
    const STRTOL_LEADING: [char; 6] = [' ', '\t', '\n', '\u{b}', '\u{c}', '\r'];
    s.trim_start_matches(STRTOL_LEADING)
        .trim_end_matches([' ', '\t'])
        .parse::<i32>()
        .ok()
}

/// Decodes a sigspec in ANY position — `-SIG`, `-s SIG`, `-n SIG`, `kill -l
/// SIG` — the way bash's single `decode_signal()` does (#405): a NUMBER in
/// `0..=NSIG` (whatever the platform names, valid or not — bash hands it to
/// `kill(2)` and lets the kernel judge), or a NAME with or without the `SIG`
/// prefix, case-insensitively.
///
/// `EXIT` (0) is bash's pseudo-signal for "no signal": accepted as a name here
/// and printed by `kill -l 0`, but deliberately NOT in `killable_signals()` —
/// the `kill -l` listing starts at 1, and `SIGEXIT` is not a name bash knows.
fn decode_signal(spec: &str) -> Option<i32> {
    if let Some(n) = parse_legal_number(spec) {
        return (0..=64).contains(&n).then_some(n);
    }
    if spec.eq_ignore_ascii_case("EXIT") {
        return Some(0);
    }
    signal_by_name(spec)
}

fn strip_end_of_options(targets: &[String]) -> &[String] {
    match targets.first() {
        Some(t) if t == "--" => &targets[1..],
        _ => targets,
    }
}

/// Handles `kill -s SIGNAME [targets...]`. The `-s` token has already
/// been consumed by the dispatcher; `args` is everything after it.
fn kill_with_s_flag(args: &[String], err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let name = match args.first() {
        Some(n) => n,
        None => {
            // bash reports a missing option argument with `sh_needarg` and
            // EXECUTION_FAILURE (1) — not the usage status 2 (#402).
            crate::sh_error_to!(shell, err, None, "kill: -s: option requires an argument");
            return ExecOutcome::Continue(1);
        }
    };
    // #405: `-s` takes a NUMBER as readily as a name — bash decodes every
    // position with the same function.
    let sig = match decode_signal(name) {
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
    let targets = strip_end_of_options(&args[1..]);
    if targets.is_empty() {
        e!(
            err,
            "kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | jobspec ... or kill -l [sigspec]"
        );
        return ExecOutcome::Continue(2);
    }
    send_signal_to_targets(sig, targets, err, shell)
}

/// Handles `kill -n SIGNUM [targets...]`. The `-n` token has already
/// been consumed by the dispatcher; `args` is everything after it.
/// #405: `-n` takes a NAME as readily as a number, and any number bash would
/// hand to `kill(2)` — the old `killable_signals()` membership test rejected
/// signal 0 and everything above the standard table.
fn kill_with_n_flag(args: &[String], err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let num_arg = match args.first() {
        Some(s) => s,
        None => {
            // Missing option argument = status 1, as for `-s` above (#402).
            crate::sh_error_to!(shell, err, None, "kill: -n: option requires an argument");
            return ExecOutcome::Continue(1);
        }
    };
    let n = match decode_signal(num_arg) {
        Some(n) => n,
        None => {
            crate::sh_error_to!(
                shell,
                err,
                None,
                "kill: {num_arg}: invalid signal specification"
            );
            return ExecOutcome::Continue(1);
        }
    };
    let targets = strip_end_of_options(&args[1..]);
    if targets.is_empty() {
        e!(
            err,
            "kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | jobspec ... or kill -l [sigspec]"
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
        if target.is_empty() {
            // #406: bash has a distinct message for the empty word — it is
            // neither a `%`-spec nor a number, and `sh_badjob` names it in
            // backquotes. A whitespace-only word still takes the bad-target
            // path below.
            crate::sh_error_to!(
                shell,
                err,
                None,
                "kill: `{target}': not a pid or valid job spec"
            );
            any_failed = true;
            continue;
        }
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
                let errno = crate::bash_io_error(&std::io::Error::last_os_error());
                crate::sh_error_to!(shell, err, None, "kill: ({target}) - {errno}");
                any_failed = true;
            }
        } else {
            match parse_legal_number(target) {
                Some(pid) => {
                    // #4: bash passes the value straight to `kill(2)`, which
                    // interprets it itself: `>0` a single pid, `0` the caller's
                    // own process group, `<0` the process group `|pid|` (`-1` =
                    // every process the caller may signal). Only a NON-numeric
                    // target is rejected as not-a-process-or-job below.
                    let rc = unsafe { libc::kill(pid, sig) };
                    if rc != 0 {
                        let errno = crate::bash_io_error(&std::io::Error::last_os_error());
                        crate::sh_error_to!(shell, err, None, "kill: ({pid}) - {errno}");
                        any_failed = true;
                    }
                }
                None => {
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
    let mut g = crate::builtin_opts::Getopt::new(
        "disown",
        crate::builtin_opts::ArgView::Plain(args),
        "har",
    );
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                'a' => all = true,
                'r' => running_only = true,
                'h' => mark_nohup = true,
                _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }

    let positional = &args[g.rest_index()..];

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
                    // #406: bash reports EVERY unresolvable `disown` operand
                    // the same way, whether it is a live-looking pid, a word,
                    // or empty — `no such job`, as the pid arm above already
                    // did.
                    _ => {
                        crate::sh_error_to!(shell, err, None, "disown: {arg}: no such job");
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
                crate::sh_error_to!(shell, err, None, "disown: current: no such job");
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
fn leading_invalid_option(args: &[String]) -> Option<u8> {
    let first = args.first()?;
    if first == "--" {
        return None;
    }
    let rest = first.strip_prefix('-')?;
    // The first BYTE — bash's option scan is byte-wise, so a non-ASCII flag
    // name is reported as its leading byte alone (#522).
    rest.as_bytes().first().copied()
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

fn builtin_fg(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    // #518/#416: bash's `fg_bg` refuses OUTRIGHT when the shell has no job
    // control, before it parses options or resolves a job spec — so every
    // form (`%1`, a bad option, no argument at all) reports the same single
    // line. huck used to parse first, reporting `-Q: invalid option` (rc 2)
    // or `%1: no such job` where bash reports this. `job_control_active()`
    // is false in a non-interactive shell without `set -m` AND inside a
    // subshell, which is the other half of #416: one stage of a pipeline
    // has no job control even when the parent does.
    if !shell.job_control_active() {
        crate::sh_error_to!(shell, err, None, "fg: no job control");
        return ExecOutcome::Continue(1);
    }
    // #158: drain pending STOP/CONT before resolving/acting on the job.
    crate::jobs::reap_completed(shell);
    // #161: fg takes no options; a leading-dash argument (other than `--`) is
    // reported as an invalid option before the usage line, matching bash.
    if let Some(c) = leading_invalid_option(args) {
        crate::emit_error_bytes_to(
            shell,
            err,
            None,
            &crate::builtin_opts::invalid_option_body("fg", b'-', c),
        );
        e!(err, "fg: usage: fg [job_spec]");
        return ExecOutcome::Continue(2);
    }
    let id = match args.len() {
        0 => match shell.jobs.current_id() {
            Some(id) => id,
            None => {
                crate::sh_error_to!(shell, err, None, "fg: current: no such job");
                return ExecOutcome::Continue(1);
            }
        },
        // #423: the `%` is optional — `fg 1` is `fg %1`.
        1 => match resolve_job_operand(&args[0], "fg", err, shell) {
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
            crate::sh_error_to!(shell, err, None, "fg: current: no such job");
            return ExecOutcome::Continue(1);
        }
    };

    // #425: bash writes the command it is foregrounding to STDOUT (bg's
    // `[N]+ cmd &` notice, by contrast, really does go to stderr).
    e!(out, "{command}");

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
    // #518/#416: bash's `fg_bg` refuses OUTRIGHT when the shell has no job
    // control, before it parses options or resolves a job spec — so every
    // form (`%1`, a bad option, no argument at all) reports the same single
    // line. huck used to parse first, reporting `-Q: invalid option` (rc 2)
    // or `%1: no such job` where bash reports this. `job_control_active()`
    // is false in a non-interactive shell without `set -m` AND inside a
    // subshell, which is the other half of #416: one stage of a pipeline
    // has no job control even when the parent does.
    if !shell.job_control_active() {
        crate::sh_error_to!(shell, err, None, "bg: no job control");
        return ExecOutcome::Continue(1);
    }
    // #158: drain pending STOP/CONT so `bg` finds a newly-stopped job.
    crate::jobs::reap_completed(shell);
    // #161: bg takes no options; a leading-dash argument (other than `--`) is
    // reported as an invalid option before the usage line, matching bash.
    if let Some(c) = leading_invalid_option(args) {
        crate::emit_error_bytes_to(
            shell,
            err,
            None,
            &crate::builtin_opts::invalid_option_body("bg", b'-', c),
        );
        e!(err, "bg: usage: bg [job_spec ...]");
        return ExecOutcome::Continue(2);
    }
    // #417: bg takes a LIST of job specs, as its own usage line says. Each is
    // resolved and reported in turn; the status is 1 if any operand failed and
    // 0 otherwise (a job that was already running counts as success). With no
    // operand at all there is exactly one implicit operand: the current job.
    // (A spec without its leading `%` is not accepted yet — filed separately.)
    let mut any_failed = false;
    if args.is_empty() {
        if bg_one(None, err, shell).is_err() {
            any_failed = true;
        }
    } else {
        for spec in args {
            if bg_one(Some(spec), err, shell).is_err() {
                any_failed = true;
            }
        }
    }
    ExecOutcome::Continue(if any_failed { 1 } else { 0 })
}

/// Resumes ONE `bg` operand: `Some(spec)` for an explicit `%spec`, `None` for
/// the implicit current job. Diagnostics are printed here so a list keeps
/// going after a bad operand. `Err(())` means this operand failed; a job that
/// was already running is a notice, i.e. `Ok(())`.
fn bg_one(spec: Option<&String>, err: &mut dyn Write, shell: &mut Shell) -> Result<(), ()> {
    // #412: with no operand bash takes the CURRENT job — the most recent
    // stopped one, or, when nothing is stopped, the most recent job full stop
    // (which then reports "already in background"). The spec text names the
    // operand in diagnostics, or `current` when there was none, as `fg` does.
    let (id, spec) = match spec {
        None => match shell
            .jobs
            .current_stopped_id()
            .or_else(|| shell.jobs.current_id())
        {
            Some(id) => (id, "current".to_string()),
            None => {
                crate::sh_error_to!(shell, err, None, "bg: current: no such job");
                return Err(());
            }
        },
        Some(spec) => match resolve_job_operand(spec, "bg", err, shell) {
            Ok(id) => (id, spec.clone()),
            Err(_) => return Err(()),
        },
    };
    // #162: a job the entry-reap already completed is gone — match bash's
    // "no such job" + drop the phantom entry, before the not-stopped check
    // below would misreport it as already in background.
    if job_already_terminal(shell, id) {
        crate::sh_error_to!(shell, err, None, "bg: {spec}: no such job");
        shell.jobs.jobs_mut().retain(|j| j.id != id);
        return Err(());
    }
    // #412: a job that is already running is not an error — bash says so and
    // exits 0, naming the job by its bare id.
    let is_stopped = shell
        .jobs
        .iter()
        .find(|j| j.id == id)
        .map(|j| matches!(j.state, crate::jobs::JobState::Stopped(_)))
        .unwrap_or(false);
    if !is_stopped {
        crate::sh_error_to!(shell, err, None, "bg: job {id} already in background");
        return Ok(());
    }
    let (pgid, command) = {
        if let Some(job) = shell.jobs.jobs_mut().iter_mut().find(|j| j.id == id) {
            job.state = crate::jobs::JobState::Running;
            job.notified = true;
            (job.pgid, job.command.clone())
        } else {
            crate::sh_error_to!(shell, err, None, "bg: {spec}: no such job");
            return Err(());
        }
    };

    unsafe {
        libc::killpg(pgid, libc::SIGCONT);
    }

    e!(err, "[{id}]+ {command} &");
    Ok(())
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

    // Set true only when -c/-d/-w/-r/-a actually ran (NOT for `--` or an
    // unknown option), so the trailing "list all" block can distinguish
    // "no operand, no action" (list all) from "no operand, action already
    // performed" (nothing more to do).
    let mut did_action = false;
    let mut did_clear = false;
    // Deferred rather than acted on immediately (unlike -c/-w/-r/-a below):
    // whether a failure here is reported depends on `did_clear`, which is
    // only known once the WHOLE flag set has been scanned (bash's rule is
    // order-independent — `-cd 1` and `-d1 -c` behave identically; see the
    // comment at the dispatch site).
    let mut delete_op: Option<String> = None;
    // `-a`/`-n`/`-r`/`-w` are mutually exclusive in bash (verified 5.2.21:
    // ANY two of them together — `-aw`, `-wa`, `-rw`, `-ar`, `-an`, `-nr`,
    // ... — error `"cannot use more than one of -anrw"`, rc 1, checked
    // BEFORE any of them runs; `-an` does NOT fall through to -n's "not yet
    // implemented" placeholder). None of them take a getopt value (spec has
    // no `:` on any of the four), so this can only be settled once the
    // whole flag set is known — recorded in encounter order, checked right
    // after the scan.
    let mut anrw_flags: Vec<char> = Vec::new();
    // Populated from `anrw_flags` below once the at-most-one invariant
    // holds. Bash's own syntax (`history -anrw [filename]`) gives whichever
    // ONE of `-a`/`-r`/`-w` was requested the same shared optional trailing
    // filename operand.
    let mut file_ops: Vec<char> = Vec::new();
    // ---- options ----
    let mut g = crate::builtin_opts::Getopt::new(
        "history",
        crate::builtin_opts::ArgView::Plain(args),
        "cd:anrwps",
    );
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                'c' => did_clear = true,
                'd' => delete_op = Some(o.value.expect("spec requires a value for -d")),
                'a' | 'n' | 'r' | 'w' => anrw_flags.push(o.ch),
                'p' | 's' => {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "history: -{}: not yet implemented",
                        o.ch
                    );
                    return ExecOutcome::Continue(1);
                }
                _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }

    if anrw_flags.len() > 1 {
        crate::sh_error_to!(
            shell,
            err,
            None,
            "history: cannot use more than one of -anrw"
        );
        return ExecOutcome::Continue(1);
    }
    if let Some(&flag) = anrw_flags.first() {
        if flag == 'n' {
            // `-n` (read new lines from the history file, not yet consumed
            // by this session) is unimplemented, same placeholder as -p/-s.
            crate::sh_error_to!(shell, err, None, "history: -n: not yet implemented");
            return ExecOutcome::Continue(1);
        }
        file_ops.push(flag);
    }

    // Fixed dispatch order (verified against bash 5.2.21, NOT the order the
    // flags were typed in): -c always runs first. `history -c -w FILE`
    // writes an EMPTY file even when the list was non-empty beforehand, so
    // -w sees the already-cleared list; by the same evidence -d does too.
    if did_clear {
        Rc::make_mut(&mut shell.history).clear();
        did_action = true;
    }
    if let Some(operand) = delete_op {
        did_action = true;
        // Range iff a '-' appears AFTER the first char (so a leading
        // negative sign on a single offset isn't mistaken for a
        // range). `operand.get(1..)` (rather than `operand[1..]`)
        // avoids panicking when `operand` is empty or the byte at
        // index 1 isn't a char boundary; an empty operand simply
        // falls through to the single-offset path below, where
        // `resolve_offset("")` fails and yields the standard
        // out-of-range error.
        let split = operand.get(1..).and_then(|s| s.find('-')).map(|i| i + 1);
        let range = split.map(|i| (&operand[..i], &operand[i + 1..]));
        let ok = if let Some((sa, sb)) = range {
            match (resolve_offset(shell, sa), resolve_offset(shell, sb)) {
                (Some(a), Some(b)) => {
                    Rc::make_mut(&mut shell.history).delete_range(a, b);
                    true
                }
                _ => false,
            }
        } else {
            match resolve_offset(shell, &operand) {
                Some(n) => Rc::make_mut(&mut shell.history).delete(n),
                None => false,
            }
        };
        if !ok {
            // bash 5.2.21, verified: `history -cd 1`, `-cd abc`, `-cd 1-3`,
            // and `-cd 5` against a 2-entry PRELOADED history (offset 5 was
            // out of range even BEFORE the clear) all exit 0 with no
            // output — `-c`'s presence anywhere in the same invocation
            // silently swallows ANY `-d` failure (bad number or out of
            // range alike), order-independent. Standalone `-d 1` against an
            // equally empty history (no `-c`) DOES error, so this is not a
            // general "empty history" exemption — it is specific to the
            // -c-and-d combination.
            if !did_clear {
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

    let rest_after_opts = g.rest_index();
    let mut idx = rest_after_opts;
    if !file_ops.is_empty() {
        // Optional trailing filename operand; else the default histfile.
        // Consuming it here (rather than leaving it for the general
        // trailing-operand handling below) matches bash: once a file op is
        // requested, the operand names ITS file, not a listing count.
        let file: std::path::PathBuf = match args.get(rest_after_opts) {
            Some(f) => {
                idx += 1;
                std::path::PathBuf::from(f)
            }
            // #226: resolve the default from the LIVE $HISTFILE shell
            // variable, not the startup-cached value.
            None => match shell.resolve_histfile_path() {
                Some(p) => p,
                None => {
                    crate::sh_error_to!(shell, err, None, "history: cannot use the history file");
                    return ExecOutcome::Continue(1);
                }
            },
        };
        for op in &file_ops {
            let h = Rc::make_mut(&mut shell.history);
            let res = match op {
                'w' => h.write_all_to(&file),
                'a' => h.append_new_to(&file),
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
        }
        did_action = true;
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
                // v358 (#116): the ONLY builtin error in bash that abandons the
                // rest of the command list. Measured across 15 cases — `cd -Q`,
                // `kill -Q`, `read -Q`, `getopts`, `umask a b`, `history a`,
                // `history -Q`, and even the special builtins `shift a b` and
                // `break 1 2` all continue. Hence its own ErrorKind rather than
                // a general "builtin usage" rule fitted to one data point.
                shell.report_error(crate::error_fatality::ErrorKind::HistoryTooManyArgs);
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

    let mut list_signals = false;
    let mut print_mode = false;
    let mut g =
        crate::builtin_opts::Getopt::new("trap", crate::builtin_opts::ArgView::Plain(args), "lp");
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                'l' => list_signals = true,
                'p' => print_mode = true,
                _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }
    let rest = &args[g.rest_index()..];

    // No operands and no -l: same as `trap -p` (covers bare `trap`, and
    // `trap --`/`trap -p` alone — bash 5.2.21 verified: `trap --` is a
    // silent no-op, not a usage error).
    if rest.is_empty() && !list_signals {
        print_active_traps(out, shell, None);
        return ExecOutcome::Continue(0);
    }

    // -l: list signal name/number pairs. Wins over -p when both are given
    // (bash: `trap -lp`/`trap -pl` both list signals, not active traps).
    // Extra operands are accepted and silently ignored — verified against
    // bash 5.2.21 (`trap -l foo` neither errors nor filters); huck used to
    // reject them ("trap: -l takes no arguments"), which was itself a
    // divergence, not something this conversion needs to preserve.
    if list_signals {
        print_signal_table(out);
        return ExecOutcome::Continue(0);
    }

    // -p [SIGNAL...]: list active traps (optionally filtered).
    if print_mode {
        if rest.is_empty() {
            print_active_traps(out, shell, None);
            return ExecOutcome::Continue(0);
        }
        let mut filter: Vec<TrapSignal> = Vec::new();
        for name in rest {
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

    // `trap - SIGNAL...`: reset each signal. A lone "-" is never an option
    // to the scanner (it's the standard "operand, not a flag cluster" rule),
    // so it always surfaces here as rest[0].
    if rest.first().map(|s| s.as_str()) == Some("-") {
        if rest.len() < 2 {
            // bash: rc 2 (a usage error), not 1 — and `trap` is a POSIX
            // special builtin, so this must be able to exit a posix shell
            // like the scanner's own invalid-option errors do (verified:
            // `set -o posix; trap -; echo SURVIVED` does not print SURVIVED).
            e!(err, "trap: usage: trap [-lp] [[arg] signal_spec ...]");
            shell.builtin_usage_error = Some(2);
            return ExecOutcome::Continue(2);
        }
        for name in &rest[1..] {
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

    // #654: operands that are CONDITIONS rather than an action, i.e. a reset
    // without the `-`. Two independent rules, both measured against bash 5.2.21
    // rather than read off POSIX, because POSIX's wording ("if the first operand
    // is an unsigned decimal integer") is not quite what bash implements:
    //
    //   trap 0            reset EXIT           a lone spec, numeric
    //   trap INT          reset INT            a lone spec, NAME — so it is not
    //                                          only the integer rule
    //   trap 1 2          reset BOTH           first operand is a signal NUMBER
    //   trap 64 2         reset both           64 is in range
    //   trap 65 2         action `65` on INT   65 is NOT, so the integer rule
    //   trap 999 2        action `999` on INT  does not apply and it is an action
    //   trap EXIT INT     action `EXIT` on INT a NAME first operand is an action
    //
    // So the discriminator for the multi-operand form is "a valid signal NUMBER",
    // not "an unsigned integer" and not "a valid spec". A lone operand is
    // separate: any valid spec resets, and anything else falls through to the
    // usage error below (`trap NOSUCHSIG` and `trap 999` are both rc 2).
    let first_is_signal_number = rest
        .first()
        .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
    let first_parses = rest.first().is_some_and(|s| parse_trap_signal(s).is_ok());
    if (first_is_signal_number && first_parses) || (rest.len() == 1 && first_parses) {
        for name in rest {
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
    if rest.len() < 2 {
        // Same rc-2/posix-fatal correction as the reset-usage error above.
        e!(err, "trap: usage: trap [-lp] [[arg] signal_spec ...]");
        shell.builtin_usage_error = Some(2);
        return ExecOutcome::Continue(2);
    }
    let action_text = rest[0].clone();
    let action = if action_text.is_empty() {
        None // empty string → ignore
    } else {
        Some(action_text)
    };
    for name in &rest[1..] {
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

    // Collect entries in (sort-key, signal, action) form. bash walks its trap
    // table by SIGNAL NUMBER — EXIT is signal 0, so it comes first, then the
    // real signals in ascending order — and prints the pseudo-signals it keeps
    // past the end of that table (DEBUG, ERR, RETURN) afterwards, in that
    // order. huck used to group all four pseudo-signals ahead of every real
    // one, which put ERR/DEBUG/RETURN in the wrong place.
    let mut entries: Vec<(i32, TrapSignal, &Option<String>)> = Vec::new();
    for (sig, action) in &shell.traps {
        if let Some(f) = filter
            && !f.contains(sig)
        {
            continue;
        }
        let key = match sig {
            TrapSignal::Exit => 0,
            TrapSignal::Real(n) => *n,
            TrapSignal::Debug => 1000,
            TrapSignal::Err => 1001,
            TrapSignal::Return => 1002,
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
            // bash prints a real signal with its SIG prefix here (`SIGUSR1`),
            // unlike `kill -l`, which lists bare names. The pseudo-signals
            // above never take the prefix.
            TrapSignal::Real(n) => signal_number_to_name(n)
                .map(|nm| format!("SIG{nm}"))
                .unwrap_or_else(|| n.to_string()),
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

    // getopts accepts no options of its own (bash: internal_getopt("")). A
    // leading operand starting with '-' (other than "-" or "--") is an
    // invalid option; a leading "--" is consumed as the option terminator.
    let mut g =
        crate::builtin_opts::Getopt::new("getopts", crate::builtin_opts::ArgView::Plain(args), "");
    match g.next_opt(shell, err) {
        // Empty spec => `accepts` yields nothing, so this is unreached. Routed
        // through the same fallback as every other builtin (#523) rather than
        // `unreachable!`: if the spec ever gains a character, that must be a
        // diagnostic, not a dead shell.
        Ok(Some(o)) => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
        Ok(None) => {}
        Err(code) => return ExecOutcome::Continue(code),
    }
    let args = &args[g.rest_index()..];

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

    // Verbose getopts-internal option diagnostic (suppressed by OPTERR=0),
    // prefixed with $0 (bash sets argv[0] = dollar_vars[0] for sh_getopt).
    // #61: bash emits this BEFORE validating the name variable, so an invalid
    // optstring option AND an invalid name var together print BOTH (this used
    // to sit after the name check, printing only the identifier error).
    if let Some(body) = step.error.as_deref()
        && shell.lookup_var("OPTERR").as_deref() != Some("0")
    {
        e!(err, "{}: {body}", shell.shell_argv0);
    }

    // Validate the name AFTER OPTIND/OPTARG are bound. Invalid identifier is a
    // hard error (bash EXECUTION_FAILURE = 1) with the full builtin prologue.
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
        // #583: on at startup, cleared when the shell commits to being
        // non-interactive — see `ShellOptions`'s initialiser.
        default: true,
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
/// Returns `false` when `name` is not a known `set -o` option name.
pub fn set_o_option_by_name(shell: &mut Shell, name: &str, enable: bool) -> bool {
    match option_set(shell, name, enable) {
        Ok(()) => true,
        Err(OptSetErr::Unknown) => false,
    }
}

/// The `set -o` listing, for the CLI's argument-less `-o` / `+o` (#164).
/// `table` picks the two forms the builtin already has: the `name<TAB>on|off`
/// table (`-o`) or the `set -o name` reinput form (`+o`).
pub fn print_set_o_options(out: &mut dyn Write, shell: &Shell, table: bool) {
    let _ = if table {
        print_options_table(out, shell)
    } else {
        print_options_reinput(out, shell)
    };
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

/// bash's `set` usage line, verbatim from 5.2.21. Currently reached only by
/// the `+r` refusal; huck's unknown-flag path still says "not yet supported"
/// instead of bash's `invalid option` + usage, so when that is aligned it
/// should print this same constant.
const SET_USAGE: &str =
    "set: usage: set [-abefhkmnptuvxBCEHPT] [-o option-name] [--] [-] [arg ...]";

fn builtin_set_inner(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
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
                    // `set -r` restricts a RUNNING shell. Not a startup entry,
                    // so `restricted_at_startup` deliberately stays false —
                    // bash reports `shopt restricted_shell` as `off` here even
                    // though the shell is now fully restricted.
                    b'r' => {
                        shell.policy = crate::policy::Policy::Rbash;
                        shell.apply_restricted_readonly();
                    }
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
                        // v358 (#68): bash reports an unknown `set` flag as an
                        // INVALID OPTION with its usage line, not as something
                        // huck has yet to implement — and, `set` being a POSIX
                        // special builtin, a usage error is fatal to a
                        // non-interactive shell in POSIX mode. Both halves were
                        // wrong: the message claimed a gap that is really a
                        // rejection, and the shell carried on where bash exits.
                        // Raw byte, not `other as char`: bash scans the flag
                        // word byte-wise, so `set -\xC3\xA9` names only the
                        // first byte (#522).
                        crate::emit_error_bytes_to(
                            shell,
                            err,
                            None,
                            &crate::builtin_opts::invalid_option_body("set", b'-', other),
                        );
                        let _ = writeln!(
                            err,
                            "set: usage: set [-abefhkmnptuvxBCEHPT] [-o option-name] [--] [-] [arg ...]"
                        );
                        shell.builtin_usage_error = Some(2);
                        shell.report_error(crate::error_fatality::ErrorKind::SpecialBuiltinUsage {
                            status: 2,
                        });
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
                    // Restriction is one-way. bash does not emit a
                    // restriction-specific refusal here: it routes `+r` through
                    // the ordinary invalid-option path (usage line, rc 1 —
                    // note NOT the rc 2 an unknown flag like `+Z` gets).
                    // Unrestricted, `set +r` is simply accepted at rc 0.
                    b'r' => {
                        if shell.policy.is_restricted() {
                            crate::sh_error_to!(shell, err, None, "set: +r: invalid option");
                            e!(err, "{}", SET_USAGE);
                            return ExecOutcome::Continue(1);
                        }
                    }
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
                        // v358 (#68): bash reports an unknown `set` flag as an
                        // INVALID OPTION with its usage line, not as something
                        // huck has yet to implement — and, `set` being a POSIX
                        // special builtin, a usage error is fatal to a
                        // non-interactive shell in POSIX mode. Both halves were
                        // wrong: the message claimed a gap that is really a
                        // rejection, and the shell carried on where bash exits.
                        // Raw byte, not `other as char`: bash scans the flag
                        // word byte-wise, so `set -\xC3\xA9` names only the
                        // first byte (#522).
                        crate::emit_error_bytes_to(
                            shell,
                            err,
                            None,
                            &crate::builtin_opts::invalid_option_body("set", b'+', other),
                        );
                        let _ = writeln!(
                            err,
                            "set: usage: set [-abefhkmnptuvxBCEHPT] [-o option-name] [--] [-] [arg ...]"
                        );
                        shell.builtin_usage_error = Some(2);
                        shell.report_error(crate::error_fatality::ErrorKind::SpecialBuiltinUsage {
                            status: 2,
                        });
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

/// The one shopt name that is not a stored bit — bash computes it, and it is
/// READ-ONLY: `shopt -s`/`-u` on it are silent no-ops in both directions, so
/// it can neither enter nor escape restriction.
const RESTRICTED_SHELL_OPT: &str = "restricted_shell";

/// Read a shopt bit. `restricted_shell` reports the shell's startup PROVENANCE
/// (see `Shell::restricted_at_startup`), not its current policy: bash says
/// `off` after `set -r` even though that shell is restricted.
fn shopt_get(shell: &Shell, name: &str) -> Option<bool> {
    if name == RESTRICTED_SHELL_OPT {
        return Some(shell.restricted_at_startup);
    }
    shell.shopt_options.get(name)
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
    let mut g = crate::builtin_opts::Getopt::new(
        "shopt",
        crate::builtin_opts::ArgView::Plain(args),
        "pqsuo",
    );
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                's' => set_f = true,
                'u' => unset_f = true,
                'q' => quiet = true,
                'p' => print_f = true,
                'o' => o_bridge = true,
                _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }
    let i = g.rest_index();
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
            let on = shopt_get(shell, opt.name).unwrap_or(false);
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
            if name == RESTRICTED_SHELL_OPT {
                // Silent no-op, rc 0 — `-u` must not lift the restriction and
                // `-s` must not impose one. Verified against bash 5.2.21.
                continue;
            }
            if !shell.shopt_options.set(name, set_f) {
                crate::sh_error_to!(shell, err, None, "shopt: {name}: invalid shell option name");
                rc = 1;
            } else if name == "extdebug" {
                // #264, bash's `shopt_set_debug_mode`:
                //     function_trace_mode = error_trace_mode = <extdebug value>
                // A plain ASSIGNMENT, and extdebug's setter is the only place
                // it happens. That is stronger than "extdebug implies -T/-E",
                // which is how huck used to model it at each read site:
                //
                //   * `$-` and `set -o` REPORT -T and -E afterwards;
                //   * `set +T` afterwards genuinely turns tracing back off;
                //   * `shopt -u extdebug` clears BOTH unconditionally — even a
                //     `-E` the user set explicitly, having never touched
                //     extdebug. Blunt, and verified against bash 5.2.21.
                //
                // The write is ONE-WAY: `set -T` does not turn extdebug on, so
                // the DEBUG skip / return-2 rules stay keyed on extdebug alone.
                shell.shell_options.functrace = set_f;
                shell.shell_options.errtrace = set_f;
            }
        }
        return ExecOutcome::Continue(rc);
    }

    // query mode
    let mut all_set = true;
    for name in names {
        match shopt_get(shell, name) {
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
pub(crate) fn eval_in_sink(args: &[String], shell: &mut Shell) -> ExecOutcome {
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
    let body_newlines = joined.bytes().filter(|&b| b == b'\n').count() as u32;
    shell.eval_frame = Some(shell.current_lineno.max(1) + body_newlines);
    let saved = shell.xtrace_depth;
    shell.xtrace_depth += 1;
    let r = crate::shell::process_line_in_sinks(&joined, shell, true);
    shell.xtrace_depth = saved;
    shell.eval_frame = saved_frame;
    r
}

fn builtin_eval(args: &[String], shell: &mut Shell) -> ExecOutcome {
    eval_in_sink(args, shell)
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
                if crate::arith::should_wrap_expansion_error(&e) {
                    crate::sh_error_to!(
                        shell,
                        err,
                        Some("let"),
                        "{}",
                        crate::arith::render_error_body(a, &e)
                    );
                }
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
    let mut g =
        crate::builtin_opts::Getopt::new("help", crate::builtin_opts::ArgView::Plain(args), "dms");
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                's' => want_synopsis = true,
                'd' => want_description = true,
                'm' => want_man = true,
                _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }
    let names = &args[g.rest_index()..];

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

pub(crate) fn source_in_sink(args: &[String], invoked: &str, shell: &mut Shell) -> ExecOutcome {
    if let Some(path) = args.first()
        && let Err(msg) = shell
            .policy
            .check(crate::policy::Op::SourcePath { invoked, path })
    {
        let mut err = crate::executor::err_writer();
        crate::sh_error_to!(shell, &mut *err, None, "{msg}");
        return ExecOutcome::Continue(1);
    }
    // Materialize the redirect-aware err writer for the early-bail diagnostics
    // below (these don't recurse into the executor, so they must emit here
    // rather than via the thread-local sink — same reasoning as sh_error_to!
    // elsewhere: `sink`/`err_sink` carry the executor's in-memory redirect
    // swap for this `source`/`.` invocation).
    {
        let mut err = crate::executor::err_writer();
        if args.is_empty() {
            // #232: bash emits a "filename argument required" ERROR line FIRST
            // (with the `<prog>: line N:` prologue, prefixed with the INVOKED
            // name `source`/`.`), then the usage line (no prologue). The usage
            // synopsis also uses the invoked name.
            crate::sh_error_to!(
                shell,
                &mut *err,
                Some(invoked),
                "filename argument required"
            );
            e!(
                &mut *err,
                "{invoked}: usage: {invoked} filename [arguments]"
            );
            // POSIX case #1: missing-filename usage error (the not-found case at
            // resolve_source_path below is `SpecialBuiltinOperand`: exits 1
            // in posix under every driver, no `-c` substitution).
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
            let mut err = crate::executor::err_writer();
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
            shell.report_error(crate::error_fatality::ErrorKind::SpecialBuiltinOperand);
            return ExecOutcome::Continue(1);
        }
    };
    let contents = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            let mut err = crate::executor::err_writer();
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
    // #439: DEBUG is scoped at the sourced file's ENTRY the same way it is at
    // a function's — unset for the file's duration without functrace, back
    // afterwards only if the file left DEBUG untrapped. This is where DEBUG
    // and RETURN part company: bash runs an INHERITED RETURN trap for a
    // sourced file with or without functrace (see the #440 note below), but
    // does NOT let the caller's DEBUG fire for the file's commands.
    let saved_debug_trap = crate::traps::take_debug_trap_for_call(shell);
    let result = run_sourced_contents_in_sinks(&contents, &path, shell);
    crate::traps::restore_debug_trap_after_call(shell, saved_debug_trap);
    // #440: a sourced script fires the RETURN trap when it finishes, whether it
    // ran off the end or hit `return N`. Unlike a function call there is NO
    // entry-unset: bash runs an INHERITED trap here with or without functrace
    // (`trap "echo R" RETURN; . f` prints R in bash even without `set -T`).
    //
    shell.call_stack.pop();
    shell.sync_call_arrays();
    shell.source_depth -= 1;
    // Fired AFTER the frame is popped: bash runs this action in the CALLER's
    // context, not the file's — `trap 'echo [${BASH_SOURCE[0]}] ${#FUNCNAME[@]}'
    // RETURN; . f` prints `[] 0` at top level, and inside a function reports
    // that function, not the sourced file. (A FUNCTION's RETURN trap is the
    // other way round — `call_function` fires it with its own frame still on
    // the stack, which is also what bash does.)
    //
    // `$?` is LEFT ALONE — it already holds the last command the file ran,
    // which is what bash's action sees. Installing the file's return value
    // first would diverge after `return N` exactly as #441 did for functions:
    // bash runs the action with `$?` = 0 for `echo body; return 3`, and gives
    // the CALLER 3.
    crate::traps::fire_return_trap(shell);

    if let Some(saved) = saved_positional {
        shell.positional_args = saved;
    }
    result
}

fn builtin_source(
    invoked: &str,
    args: &[String],
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let _ = err; // err writer not used: source_in_sink materializes from sinks
    source_in_sink(args, invoked, shell)
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

/// v348 (#339, R3): unlike the other `LexError` variants surfaced at this
/// driver's skip-and-continue recovery points (which resume parsing at the
/// NEXT physical line), exceeding bash's `HEREDOC_MAX` aborts the whole
/// parse context — verified against real bash: a script whose 17th heredoc
/// is followed by more commands prints exactly ONE error line and runs
/// nothing else (`rc=2`), it does not skip ahead and keep executing.
fn lex_error_is_fatal(le: &crate::lexer::LexError) -> bool {
    matches!(le, crate::lexer::LexError::HeredocMaxExceeded)
}

pub(crate) fn run_sourced_contents_in_sinks(
    contents: &str,
    _path: &std::path::Path,
    shell: &mut crate::shell_state::Shell,
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
    // v325 (#266): a sourced file has its OWN line numbering; the piped-stdin
    // cumulative base (`stdin_line_base`) must not bleed into it via
    // `line_base()`'s stdin fallback (same reset discipline as `eval_frame`).
    let saved_stdin_base = shell.stdin_line_base;
    shell.stdin_line_base = 0;
    let result = run_sourced_contents_in_sinks_inner(contents, _path, shell);
    shell.stdin_line_base = saved_stdin_base;
    shell.eval_frame = saved_eval_frame;
    result
}

fn run_sourced_contents_in_sinks_inner(
    contents: &str,
    _path: &std::path::Path,
    shell: &mut crate::shell_state::Shell,
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
                        let fatal = lex_error_is_fatal(&le);
                        let err = crate::command::ParseError::Lex(Box::new(le));
                        crate::render_syntax_diag(shell, &err, contents, line, iter.error_delim());
                        last_status = 2;
                        if fatal {
                            return ExecOutcome::Continue(2);
                        }
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
                        let mut err = crate::executor::err_writer();
                        let _ = write!(&mut *err, "{}", &contents[prev_end..unit_end_abs]);
                    }
                    prev_end = unit_end_abs;

                    let span = &contents[unit_start_abs..unit_end_abs];
                    let outcome = crate::executor::execute_with_sink(&seq, shell, span);

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
                                && let Some(st) = shell.take_fatal()
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
                        let fatal = lex_error_is_fatal(&le);
                        let err = crate::command::ParseError::Lex(Box::new(le));
                        crate::render_syntax_diag(shell, &err, contents, line, iter.error_delim());
                        last_status = 2;
                        if fatal {
                            return ExecOutcome::Continue(2);
                        }
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
                    // #385: an OPEN-DELIMITER lex error is reported at the line
                    // the delimiter opened on, which is the failing token's
                    // start — `cursor_pos()` is where the failed scan ran to,
                    // i.e. EOF, so `echo "a` on line 3 of a 4-line file was
                    // reported as line 5. Every other lex error keeps the
                    // scan-stop position, which is what bash names for them.
                    let opens_delim = matches!(&e, crate::command::ParseError::Lex(le)
                        if crate::error_emit::lex_error_opens_delim(le));
                    // #617: a NEAR-TOKEN error is reported at the offending
                    // token's own line — bash names `;;` on line 4 of an `if`
                    // that started on line 2, and echoes line 4. huck used the
                    // UNIT's start for every non-lex parse error, which agreed
                    // only while the token sat on the compound's first line.
                    // The failure carries the token's offset already.
                    // Only a failure that FOUND A TOKEN takes this path: an
                    // `Unexpected` whose `found` is Eof is one of the
                    // unexpected-EOF shapes, whose line the renderer derives
                    // itself (the delimiter's opening line, or the EOF line) —
                    // its `pos` is where the scan stopped, which would drag an
                    // unterminated backtick back to the last line.
                    let near_token_off = match &e {
                        crate::command::ParseError::Unexpected(f)
                            if !matches!(f.found, crate::command::Found::Eof) =>
                        {
                            Some(f.pos)
                        }
                        _ => None,
                    };
                    let foff = if is_lex {
                        iter.cursor_pos()
                    } else {
                        unit_start_off
                    };
                    // The RESTART position stays where the scan stopped; only
                    // the reported LINE moves back to the delimiter.
                    let line_off = if let Some(off) = near_token_off {
                        off
                    } else if opens_delim {
                        // The innermost OPEN frame's offset when there is one
                        // (`"…"`, `${…}`, `$((…))`), else where the failing scan
                        // step began (a single-quoted run or a backtick, which
                        // are scanned as one atom with no frame).
                        iter.error_open_start()
                            .unwrap_or_else(|| iter.last_step_start())
                    } else {
                        foff
                    };
                    let line = line_of(start + line_off) as u32;
                    crate::render_syntax_diag(shell, &e, contents, line, iter.error_delim());
                    // #633: a syntax error raised inside a compound assignment
                    // (`v=(`) exits 1 where every other syntax error exits 2 —
                    // measured on `-c`, a script file, `source` and `eval`.
                    //
                    // ⚠️ Status only — do NOT route this through `report_error`.
                    // An ordinary syntax error here deliberately does not, and
                    // for a reason: the classifier's `ExitShell` unwinds past the
                    // CALLER, so a `source`d file would kill the whole shell
                    // instead of ending just that file. Measured — `. ./bad.sh;
                    // echo OUTER=$?` prints `OUTER=1` in bash, and printed
                    // nothing at all while this called `report_error` (#340's
                    // shape).
                    last_status = if iter.error_in_compound_assign() {
                        1
                    } else {
                        2
                    };
                    // #492: a syntax error inside a `$( )` body is bash's one
                    // exception to "a syntax error is status 2" — it exits 127
                    // under `-c`, and it is FATAL to the whole shell even from
                    // a sourced file, where an ordinary syntax error only ends
                    // that file. The classifier already knows both rules; what
                    // was missing was telling the two errors apart, which the
                    // parser now marks (`InDollarCommandSub`).
                    if matches!(e, crate::command::ParseError::InDollarCommandSub(_)) {
                        shell.report_error(crate::error_fatality::ErrorKind::ComsubSyntax {
                            backtick: false,
                        });
                    }
                    // v348 (#339, R3): a HEREDOC_MAX overflow is a lex error
                    // but, unlike other recoverable lex errors here, is FATAL
                    // (see `lex_error_is_fatal`) — falls through to the
                    // abort-the-whole-parse-context `return` below.
                    let recoverable = is_lex
                        && match &e {
                            crate::command::ParseError::Lex(le) => !lex_error_is_fatal(le),
                            _ => false,
                        };
                    if recoverable {
                        start = next_line_start(start + foff);
                        prev_end = start;
                        continue 'outer;
                    }
                    // #340: the here-document limit is bash's one lex error
                    // that is fatal to the WHOLE shell, not just to the parse
                    // context that raised it. At the top level the `return`
                    // below already ends everything with 2, which is what bash
                    // gives there; NESTED (a sourced file) it has to unwind
                    // past the caller, with a plain 1.
                    if is_lex && shell.source_depth > 0 {
                        shell.report_error(crate::error_fatality::ErrorKind::HeredocLimit);
                    }
                    // bash aborts the whole parse-context on a regular syntax
                    // error — it does NOT skip the offending line and resume.
                    // This driver runs only `-c` strings, script files, and
                    // `source`/`.`/rc files; bash aborts every one of them (and a
                    // sourced file's remainder) on a syntax error regardless of
                    // interactivity. Returning here aborts THIS invocation while a
                    // parent driver loop (for a `source`d file) still continues —
                    // reproducing bash's `source bad; echo x` runs `echo x`
                    // (rc 0) with no extra machinery, since each `source` runs its
                    // OWN `run_sourced_contents_in_sinks`. The interactive REPL and
                    // `eval` use `process_line`/`process_line_in_sinks`, not this
                    // driver, so this never affects line-at-a-time recovery there.
                    // (Piped-stdin abort is a separate driver — the REPL
                    // per-line `process_line` loop — tracked in #284.)
                    return ExecOutcome::Continue(2);
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
    run_sourced_contents_in_sinks(contents, path, shell)
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
    let mut want_list = false;
    let mut g =
        crate::builtin_opts::Getopt::new("alias", crate::builtin_opts::ArgView::Plain(args), "p");
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                // #510: `-p` prints the whole table, and does NOT replace the
                // operands — bash does both. `alias -p xx` prints the table and
                // then `xx` again; `alias -p nosuch` prints the table and THEN
                // reports the missing name.
                'p' => want_list = true,
                _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }
    let operands = &args[g.rest_index()..];
    if want_list && !operands.is_empty() {
        let mut names: Vec<&String> = shell.aliases.keys().collect();
        names.sort();
        for name in names {
            let value = &shell.aliases[name];
            let _ = writeln!(out, "alias {}='{}'", name, escape_alias_value(value));
        }
    }
    if operands.is_empty() {
        let mut names: Vec<&String> = shell.aliases.keys().collect();
        names.sort();
        for name in names {
            let value = &shell.aliases[name];
            let _ = writeln!(out, "alias {}='{}'", name, escape_alias_value(value));
        }
        return ExecOutcome::Continue(0);
    }
    let mut any_failed = false;
    for arg in operands {
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
    let mut all = false;
    let mut g =
        crate::builtin_opts::Getopt::new("unalias", crate::builtin_opts::ArgView::Plain(args), "a");
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                'a' => all = true,
                _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }
    if all {
        shell.aliases.clear();
        return ExecOutcome::Continue(0);
    }
    let operands = &args[g.rest_index()..];
    if operands.is_empty() {
        e!(err, "unalias: usage: unalias [-a] name [name ...]");
        return ExecOutcome::Continue(2);
    }
    let mut any_failed = false;
    for name in operands {
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
    /// Found in the command hash table (#655). Reported as
    /// `NAME is hashed (/path)` and it SHADOWS the PATH match — measured:
    /// `hash -p /bin/echo ls; type ls` says `ls is hashed (/bin/echo)`, not
    /// `/usr/bin/ls`. `type -t` still says `file` and `-p`/`command -v` still
    /// print the bare path, so only the long form names it.
    ///
    /// Deliberately NOT produced for `type -a`, which walks PATH and ignores
    /// the table entirely — `hash -p /bin/echo zz; type -a zz` is `not found`
    /// in bash even though plain `type zz` finds it.
    Hashed(std::path::PathBuf),
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
    /// A PATH segment yielded an executable regular file, and the resolved path.
    /// It used to be dropped (the caller re-searched via `execvp` on the bare
    /// name); the command hash table needs it, and so does exec-by-hashed-path
    /// for a name that PATH alone cannot find — `hash -p /bin/echo zz; zz` (#655).
    Executable(std::path::PathBuf),
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
                    return PathClassify::Executable(candidate);
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

/// Resolve a BARE command name for execution, consulting and maintaining the
/// command hash table (#655). Callers handle a name containing `/` themselves —
/// such a name is a path, and neither shell hashes it.
///
/// bash hashes every command it locates by PATH SEARCH, then uses the cached
/// path next time instead of walking PATH again, bumping a hit count that
/// `hash`'s listing shows. `set +h` (`hashall` off) disables the whole
/// mechanism. Before this, huck's table was display-only: nothing populated it
/// and nothing read it, so PATH was re-walked on every single invocation.
///
/// ⚠️ ONE DELIBERATE DIVERGENCE, kept by design (#664): when a cached path no
/// longer exists, huck DISCARDS the entry and re-searches PATH. bash execs the
/// stale path and fails with `No such file or directory` — the classic "I just
/// installed it and the shell still can't find it, run `hash -r`" trap. huck
/// self-heals instead. See docs/bash-divergences.md.
/// Whether a resolution's hash-table WRITES should outlive it.
///
/// Not a policy knob — it mirrors where bash performs the search. A simple
/// command is resolved in the current shell, so its entry and hit count stay.
/// A pipeline stage or a background job is resolved inside the forked child, so
/// bash's entry dies with that child and never reaches the parent's table:
///
///     expr 1 + 1 >/dev/null;             hash   ->   1  /usr/bin/expr
///     expr 1 + 1 >/dev/null | cat;       hash   ->   hash table empty
///     expr 1 + 1 >/dev/null &  wait;     hash   ->   hash table empty
///     ( expr 1 + 1 >/dev/null );         hash   ->   hash table empty
///     { expr 1 + 1 >/dev/null; };        hash   ->   1  /usr/bin/expr
///
/// Reading is always allowed — a child inherits the table, it just cannot send
/// changes back.
pub(crate) enum HashEffect {
    /// Runs in THIS shell: an insert and its hit count persist.
    Persist,
    /// Runs in a forked child: consult the table, never write it.
    Discard,
}

pub(crate) fn resolve_for_exec(name: &str, shell: &mut Shell, effect: HashEffect) -> PathClassify {
    debug_assert!(
        !name.contains('/'),
        "resolve_for_exec is for bare names; a path is never hashed"
    );
    if !shell.shell_options.hashall {
        return classify_path_search(name, shell);
    }
    let persist = matches!(effect, HashEffect::Persist);
    if let Some(cached) = shell.hash_lookup(name) {
        if is_executable_file(&cached) {
            if persist {
                shell.hash_bump(name);
            }
            return PathClassify::Executable(cached);
        }
        // The by-design divergence: forget the stale entry and search again.
        if persist {
            shell.hash_remove(name);
        }
    }
    let found = classify_path_search(name, shell);
    if persist && let PathClassify::Executable(ref path) = found {
        shell.hash_insert(name, path.clone());
        shell.hash_bump(name); // an insert starts at 0; this invocation makes it 1
    }
    found
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
    if let Some(hashed) = shell.hash_lookup(name) {
        return CommandResolution::Hashed(hashed);
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
    if let Some(hashed) = shell.hash_lookup(name) {
        return CommandResolution::Hashed(hashed);
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
            CommandResolution::File(_) | CommandResolution::Hashed(_) => "file",
            CommandResolution::NotFound => return,
        };
        let _ = writeln!(out, "{word}");
        return;
    }
    if path_only {
        if let CommandResolution::File(p) | CommandResolution::Hashed(p) = res {
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
        CommandResolution::Hashed(p) => {
            let _ = writeln!(out, "{name} is hashed ({})", p.display());
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
    let mut g = crate::builtin_opts::Getopt::new(
        "type",
        crate::builtin_opts::ArgView::Plain(args),
        "afptP",
    );
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                'a' => all = true,
                't' => type_only = true,
                'p' => path_only = true,
                'P' => {
                    path_only = true;
                    force_path = true;
                }
                'f' => skip_func = true,
                _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }
    let names = &args[g.rest_index()..];
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
    // bash's `hash_builtin` (builtins/hash.def), whose shape this follows
    // exactly (#509). The flags are NOT a priority ladder over the whole
    // command — they are read in a fixed ORDER at three different places:
    //
    //   1. `-d`/`-t` with no NAMEs is a usage error, checked FIRST — so
    //      `hash -rt` errors and the `-r` flush does NOT happen;
    //   2. `-r` flushes the table, before anything is listed or added;
    //   3. with no NAMEs left, the whole table is listed (`-l` picks the
    //      re-input form) — `-p` and `-l` do not suppress that;
    //   4. per NAME: `-t` (report) wins over `-p` (set) wins over `-d`
    //      (delete) wins over the default PATH search.
    //
    // huck used to run reset > delete > set_path > list, so `hash -dt ls`
    // deleted where bash reports, and `hash -p X -t ls` set where bash
    // reports the OLD entry.
    // #655: `set +h` disables hashing, and then EVERY form of `hash` refuses —
    // measured: `hash`, `hash -r`, `hash -l`, `hash -p X z`, `hash -d ls`,
    // `hash -t ls` and `hash ls` all give this, rc 1. The check precedes option
    // parsing, which is observable: `set +h; hash -Z` is `hashing disabled`
    // (rc 1), NOT the `-Z: invalid option` usage error (rc 2) it gives with
    // hashing on.
    if !shell.shell_options.hashall {
        crate::sh_error_to!(shell, err, None, "hash: hashing disabled");
        return ExecOutcome::Continue(1);
    }

    let mut reset = false;
    let mut delete = false;
    let mut list = false;
    let mut type_only = false;
    let mut explicit_path: Option<String> = None;

    let mut g = crate::builtin_opts::Getopt::new(
        "hash",
        crate::builtin_opts::ArgView::Plain(args),
        "lrp:dt",
    );
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                'r' => reset = true,
                'd' => delete = true,
                'l' => list = true,
                't' => type_only = true,
                'p' => explicit_path = o.value,
                _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }
    let names = &args[g.rest_index()..];

    // (1) bash's `sh_needarg`, status 1 — not huck's invented "at least one
    // name required" at status 2. `-d` names itself when both are set.
    if names.is_empty() && (delete || type_only) {
        let which = if delete { "-d" } else { "-t" };
        crate::sh_error_to!(
            shell,
            err,
            None,
            "hash: {which}: option requires an argument"
        );
        return ExecOutcome::Continue(1);
    }

    // (2) The flush. It empties the table but does NOT un-create it, so a
    // later `hash -d nosuch` still reports `not found`.
    if reset {
        shell.hash_clear();
        // bash keeps `hash -r` SILENT: with no NAMEs left it returns here
        // rather than falling into the listing below, so it does not print
        // `hash: hash table empty` at the table it just emptied.
        if names.is_empty() {
            return ExecOutcome::Continue(0);
        }
    }

    // (3) No NAMEs: list the table. Reached with `-p` and `-l` too — bash's
    // `hash -p /bin/ls` (no name) prints the table rather than erroring.
    if names.is_empty() {
        // #555: bash WALKS its hash table to print, so the order is neither
        // sorted nor insertion order — bucket ascending, newest first within
        // a bucket. `hash_names_in_bash_order` reproduces it.
        let names_ordered = shell.hash_names_in_bash_order();
        if list {
            // Re-input form. An empty table prints NOTHING under `-l`.
            for name in &names_ordered {
                if let Some((path, _)) = shell.command_hash.get(*name) {
                    let _ = writeln!(out, "builtin hash -p {} {name}", path.display());
                }
            }
        } else if names_ordered.is_empty() {
            let _ = writeln!(out, "hash: hash table empty");
        } else {
            let _ = writeln!(out, "hits\tcommand");
            for name in &names_ordered {
                if let Some((path, hits)) = shell.command_hash.get(*name) {
                    let _ = writeln!(out, "{hits:>4}\t{}", path.display());
                }
            }
        }
        return ExecOutcome::Continue(0);
    }

    // (4) The per-name loop, in bash's branch order.
    let mut exit: i32 = 0;
    let multi = names.len() > 1;
    for name in names {
        // `-t` reports from the TABLE, and is the one branch bash runs
        // BEFORE its absolute-program skip: `hash -t /bin/ls` really does
        // report `/bin/ls: not found`.
        if type_only {
            match shell.command_hash.get(name) {
                Some((path, _)) => {
                    if list {
                        let _ = writeln!(out, "builtin hash -p {} {}", path.display(), name);
                    } else if multi {
                        let _ = writeln!(out, "{name}\t{}", path.display());
                    } else {
                        let _ = writeln!(out, "{}", path.display());
                    }
                }
                None => {
                    crate::sh_error_to!(shell, err, None, "hash: {name}: not found");
                    exit = 1;
                }
            }
            continue;
        }
        // bash's `absolute_program()` is "contains a slash", and such a name
        // is SILENTLY skipped — there is nothing to hash. huck used to reject
        // it with an invented `must not contain \`/'` diagnostic.
        if name.contains('/') {
            continue;
        }
        if let Some(path) = &explicit_path {
            shell.hash_insert(name, std::path::PathBuf::from(path));
        } else if delete {
            // A table that has never held anything reports nothing: bash's
            // `phash_remove` returns success when `hashed_filenames` is null.
            if shell.command_hash_created && !shell.hash_remove(name) {
                crate::sh_error_to!(shell, err, None, "hash: {name}: not found");
                exit = 1;
            }
        } else {
            match search_path_for(name, shell) {
                Some(path) => shell.hash_insert(name, path),
                None => {
                    crate::sh_error_to!(shell, err, None, "hash: {name}: not found");
                    exit = 1;
                }
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
    let mut g = crate::builtin_opts::Getopt::new(
        "command",
        crate::builtin_opts::ArgView::Plain(args),
        "pVv",
    );
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                // #508: bash takes the LAST of `-v`/`-V`, bundled or separate
                // (`command -v -V ls` and `command -vV ls` both describe).
                // huck always took `-v`, which agreed only when `-v` came last.
                'v' => {
                    concise = true;
                    verbose = false;
                }
                'V' => {
                    verbose = true;
                    concise = false;
                }
                'p' => {} // accept; introspection uses current $PATH
                _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }
    let names = &args[g.rest_index()..];

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
            // #655: `command -v` prints the bare path exactly as for a PATH
            // find; only the verbose `command -V` names the table.
            CommandResolution::Hashed(path) => {
                if concise {
                    let _ = writeln!(out, "{}", path.display());
                } else {
                    let _ = writeln!(out, "{name} is hashed ({})", path.display());
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

/// bash reports an unrecognized `pushd`/`popd`/`dirs` argument as a bad NUMBER,
/// not a bad option, and echoes the WHOLE token — `dirs -cl` reports `-cl`, not
/// `-c` (#519).
///
/// These three are deliberately NOT on the shared `builtin_opts` scanner and
/// must not be: they take `+N`/`-N` rotation arguments, so bash does not bundle
/// their flags at all. Each argument is matched whole — a known option, `--`,
/// a signed number, or a malformed number. Forcing them through a getopt
/// contract would make them wrong in a new way.
fn invalid_number(name: &str, tok: &str, shell: &mut Shell, err: &mut dyn Write) -> ExecOutcome {
    crate::sh_error_to!(shell, err, None, "{name}: {tok}: invalid number");
    let _ = writeln!(
        err,
        "{name}: usage: {}",
        crate::builtin_opts::usage_for(name)
    );
    shell.builtin_usage_error = Some(2);
    ExecOutcome::Continue(2)
}

/// What bash's `pushd`/`popd` argument loop found.
struct DirStackArgs<'a> {
    /// `-n`: do the stack manipulation, skip the `cd`.
    nflag: bool,
    /// The LAST `+N`/`-N` seen — `pushd +1 +2` rotates by 2, not by 1 and
    /// then 2.
    spec: Option<&'a str>,
    /// Everything from the first token that was neither an option nor a
    /// spec onwards.
    operands: &'a [String],
    /// True when a `--` ended the loop, so a leading `-` in the operand is
    /// part of the directory NAME and must not be re-parsed by `cd`.
    after_ddash: bool,
}

/// bash consumes options and `+N`/`-N` specs from the FRONT in a single loop,
/// in either order, and stops at the first token that is neither — which is why
/// `pushd +1 -n` honours the `-n` but `pushd /var -n` reports "too many
/// arguments" (the loop stopped at `/var`, leaving two operands). `--` ends the
/// loop too, so `pushd -- -n` means the directory named `-n`.
fn scan_dirstack_args<'a>(
    name: &str,
    args: &'a [String],
    shell: &mut Shell,
    err: &mut dyn Write,
) -> Result<DirStackArgs<'a>, ExecOutcome> {
    let mut nflag = false;
    let mut spec: Option<&str> = None;
    let mut after_ddash = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--" {
            after_ddash = true;
            i += 1;
            break;
        }
        if a == "-n" {
            nflag = true;
        } else if is_signed_index_arg(a) {
            spec = Some(a);
        } else if a.starts_with('-') && a.len() > 1 {
            // `-Q`, `-nn`: a bad NUMBER in bash's telling, not a bad option,
            // and it must not reach `cd` (#519).
            return Err(invalid_number(name, a, shell, err));
        } else {
            break;
        }
        i += 1;
    }
    Ok(DirStackArgs {
        nflag,
        spec,
        operands: &args[i..],
        after_ddash,
    })
}

/// Resolve a `+N`/`-N` spec against the stack, with bash's one wrinkle: when
/// there is nothing on the stack but the current directory, an out-of-range
/// index is reported as an EMPTY stack rather than as a bad index
/// (`pushd +9` with nothing pushed says "directory stack empty", but `+0`
/// there is fine).
fn resolve_dirstack_index(
    name: &str,
    spec: &str,
    shell: &mut Shell,
    err: &mut dyn Write,
) -> Result<usize, ExecOutcome> {
    match parse_signed_index(spec, shell.dir_stack.len()) {
        Ok(i) => Ok(i),
        Err(e) => {
            if shell.dir_stack.len() <= 1 && e.ends_with("directory stack index out of range") {
                crate::sh_error_to!(shell, err, None, "{name}: directory stack empty");
            } else {
                crate::sh_error_to!(shell, err, None, "{name}: {e}");
            }
            Err(ExecOutcome::Continue(1))
        }
    }
}

fn builtin_pushd(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    sync_stack_top(shell);

    let scan = match scan_dirstack_args("pushd", args, shell, err) {
        Ok(s) => s,
        Err(o) => return o,
    };

    if let Some(spec) = scan.spec {
        // A rotation. `-n` keeps the rotated list but leaves the current
        // directory where it is, so the entry that would have become $PWD is
        // simply dropped off the top — `dirs` then shows $PWD above the
        // rotated remainder, duplicating an entry, exactly as bash does.
        let idx = match resolve_dirstack_index("pushd", spec, shell, err) {
            Ok(i) => i,
            Err(o) => return o,
        };
        if idx != 0 {
            shell.dir_stack.rotate_left(idx);
            if scan.nflag {
                sync_stack_top(shell);
            } else {
                let target = shell.dir_stack[0].clone();
                let cd_args = vec![target.display().to_string()];
                if let ExecOutcome::Continue(c) = builtin_cd_as("pushd", &cd_args, out, err, shell)
                    && c != 0
                {
                    // Undo rotation on cd failure.
                    shell.dir_stack.rotate_right(idx);
                    return ExecOutcome::Continue(c);
                }
            }
        }
        // A rotation under `-n` prints NOTHING — the one `pushd` form that is
        // silent on success.
        if scan.nflag {
            return ExecOutcome::Continue(0);
        }
        return print_stack(out, shell, true, false, false);
    }

    let Some(dir) = scan.operands.first() else {
        // No directory and no spec. `-n` makes this a silent no-op — not even
        // the "no other directory" complaint on an empty stack.
        if scan.nflag {
            return ExecOutcome::Continue(0);
        }
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
    };

    if scan.nflag {
        // The directory goes in BELOW the current one, unresolved and
        // unvalidated: with no `cd` there is nothing to fail, so
        // `pushd -n /nonexistent` succeeds and a relative path is stored as
        // typed. Trailing operands are ignored here, where the chdir form
        // rejects them.
        shell.dir_stack.insert(1, std::path::PathBuf::from(dir));
        return print_stack(out, shell, true, false, false);
    }

    if scan.operands.len() > 1 {
        crate::sh_error_to!(shell, err, None, "pushd: too many arguments");
        return ExecOutcome::Continue(1);
    }

    // pushd DIR. After a `--` the name is handed to `cd` behind its own `--`,
    // so `pushd -- -n` reports `pushd: -n: No such file or directory` instead
    // of letting `cd` read it as an option.
    let cd_args = if scan.after_ddash {
        vec!["--".to_string(), dir.clone()]
    } else {
        vec![dir.clone()]
    };
    if let ExecOutcome::Continue(c) = builtin_cd_as("pushd", &cd_args, out, err, shell)
        && c != 0
    {
        return ExecOutcome::Continue(c);
    }
    let new_cwd = shell
        .lookup_var("PWD")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| std::path::PathBuf::from(dir));
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

    // Argument validity is decided BEFORE the stack-empty check: bash reports
    // `popd -Q` as an invalid number, and `popd /usr` as an invalid ARGUMENT
    // (with the usage line and status 2), even on an empty stack, where huck
    // used to report "directory stack empty" and never look at the argument
    // (#519, #530).
    let scan = match scan_dirstack_args("popd", args, shell, err) {
        Ok(s) => s,
        Err(o) => return o,
    };
    if let Some(bad) = scan.operands.first() {
        crate::sh_error_to!(shell, err, None, "popd: {bad}: invalid argument");
        let _ = writeln!(
            err,
            "popd: usage: {}",
            crate::builtin_opts::usage_for("popd")
        );
        shell.builtin_usage_error = Some(2);
        return ExecOutcome::Continue(2);
    }

    if shell.dir_stack.len() <= 1 {
        crate::sh_error_to!(shell, err, None, "popd: directory stack empty");
        return ExecOutcome::Continue(1);
    }

    let mut idx = match scan.spec {
        None => 0,
        Some(spec) => match resolve_dirstack_index("popd", spec, shell, err) {
            Ok(i) => i,
            Err(o) => return o,
        },
    };

    if scan.nflag {
        // Without a chdir the current directory cannot be popped, so the top
        // of the LIST — the entry below $PWD — goes instead. That is why
        // `popd -n`, `popd -n +0` and `popd -n +1` all remove the same entry.
        idx = idx.max(1);
        shell.dir_stack.remove(idx);
        return print_stack(out, shell, true, false, false);
    }

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
            // `--` ends option processing; `dirs` takes no operands after it,
            // so nothing reads `i` again and it is simply dropped.
            "--" => break,
            s if s.starts_with('-') && s.len() > 1 => {
                return invalid_number("dirs", s, shell, err);
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
    let mut g =
        crate::builtin_opts::Getopt::new("umask", crate::builtin_opts::ArgView::Plain(args), "pS");
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                'S' => symbolic = true,
                'p' => posix = true,
                _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }
    let idx = g.rest_index();
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
    Some(v / res.mult)
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
    let mut want_soft = false;
    let mut want_hard = false;
    let mut show_all = false;
    let mut letters: Vec<char> = Vec::new();
    // Built from ULIMIT_TABLE rather than bash's full "SHabcdefiklmnpqrstuvxPRT"
    // literal: `b`/`k`/`P`/`T` name resource limits bash itself only accepts
    // on platforms it was built with support for, and this project's bash
    // target (ubuntu-24.04, bash 5.2.21) rejects all four itself (verified:
    // `bash -c 'ulimit -b'` -> `invalid option`, same for -k/-P/-T) — so
    // excluding them here is a MATCH with the target bash, not a gap. `R`
    // (RLIMIT_RTTIME) is the one real, bash-accepted-on-Linux letter huck
    // does not implement (no ULIMIT_TABLE entry); pre-v359 huck already
    // rejected it too, so this keeps that pending divergence rather than
    // widening into an unimplemented resource (#496 Task 6's mapfile
    // lesson: don't parse what isn't backed by real behavior).
    let spec: String = format!(
        "SHap{}",
        ULIMIT_TABLE.iter().map(|r| r.letter).collect::<String>()
    );
    let mut g = crate::builtin_opts::Getopt::new(
        "ulimit",
        crate::builtin_opts::ArgView::Plain(args),
        &spec,
    );
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                'S' => want_soft = true,
                'H' => want_hard = true,
                'a' => show_all = true,
                'p' => letters.push('p'),
                other => {
                    debug_assert!(
                        ulimit_lookup(other).is_some(),
                        "spec is built from ULIMIT_TABLE, so every non-fixed char resolves"
                    );
                    letters.push(other);
                }
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }
    let value_arg: Option<&String> = args.get(g.rest_index());

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
        // Neither flag given => set both.
        let set_soft = want_soft || !want_hard;
        let set_hard = want_hard || !want_soft;
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

/// bash's `no_options()` (builtins/common.c): a builtin that takes NO options
/// still runs the option scanner, so a leading `-x` is rejected with the
/// standard two-line diagnostic at status 2 and a `--` is consumed. Returns
/// the index of the first operand, or `Err(2)` once the diagnostic is out.
///
/// Used by `times` (which then IGNORES its operands — `times x` runs) and by
/// `caller` (whose leading-dash argument is an INVALID OPTION, not an invalid
/// number) — #520.
fn no_options(
    name: &'static str,
    args: &[String],
    shell: &mut Shell,
    err: &mut dyn Write,
) -> Result<usize, i32> {
    let mut g =
        crate::builtin_opts::Getopt::new(name, crate::builtin_opts::ArgView::Plain(args), "");
    match g.next_opt(shell, err) {
        // An empty spec accepts nothing, so a leading `-x` always fails here.
        Ok(Some(o)) => Err(g.reject_unhandled(o.ch, shell, err)),
        Ok(None) => Ok(g.rest_index()),
        Err(code) => Err(code),
    }
}

fn builtin_times(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    // bash's `times` takes no options — and ignores any operands, so
    // `times x` prints the times while `times -Q` is a usage error. huck ran
    // regardless, which made a bad option silently do the work.
    if let Err(code) = no_options("times", args, shell, err) {
        return ExecOutcome::Continue(code);
    }
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
    let mut disable = false; // -n
    let mut all = false; // -a
    let mut special = false; // -s
    // `-d`/`-f filename` (bash's "dynamic loading" pair: load/unload a
    // builtin from a shared object) are deliberately NOT in this spec.
    // huck has no dlopen-based builtin-loading mechanism at all, so
    // accepting `-f` would silently swallow a filename argument and do
    // nothing — the same "parses, does nothing, wrong result with no
    // error" shape #496 Task 6 flagged as strictly worse than rejecting.
    // Pre-v359 huck already rejected both outright; this keeps that
    // (`enable -d`/`enable -f x` -> `invalid option`, rc 2). Filed as a
    // follow-up divergence, not implemented here.
    let mut g = crate::builtin_opts::Getopt::new(
        "enable",
        crate::builtin_opts::ArgView::Plain(args),
        "anps",
    );
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => match o.ch {
                'n' => disable = true,
                'a' => all = true,
                's' => special = true,
                'p' => {} // print format — the listing default
                _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
            },
            Ok(None) => break,
            Err(code) => return ExecOutcome::Continue(code),
        }
    }
    let names = &args[g.rest_index()..];

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

/// `caller [expr]` — report the LINE, [FUNCNAME,] and FILE of a call-stack
/// frame, reading the same `shell.call_stack` that already backs
/// `FUNCNAME`/`BASH_LINENO`/`BASH_SOURCE`. No arg reports the immediate
/// caller (rc 1, no output, if there isn't one); `expr` walks `expr` frames
/// further up (rc 1 out of range). Extra args are ignored.
fn builtin_caller(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    // bash returns 1 SILENTLY when there is no call frame at all — its
    // `caller_builtin` bails on a missing FUNCNAME/BASH_SOURCE/BASH_LINENO
    // before it looks at the arguments, so `caller -Q` at the top level is
    // rc 1 with no diagnostic, not an option error (#520).
    let in_fn_or_source = shell.call_stack.iter().any(|f| {
        matches!(
            f.kind,
            crate::shell_state::FrameKind::Function | crate::shell_state::FrameKind::Source
        )
    });
    if !in_fn_or_source {
        return ExecOutcome::Continue(1);
    }
    // Then bash's `no_options`: a leading dash is an INVALID OPTION (`-1` and
    // `-Q` alike), and `--` is consumed so `caller --` is the no-expr form.
    let rest = match no_options("caller", args, shell, err) {
        Ok(i) => i,
        Err(code) => return ExecOutcome::Continue(code),
    };
    let args = &args[rest..];
    let n = shell.call_stack.len();
    match args.first() {
        None => {
            // The no-expr form reports the line the CURRENT frame was called
            // from plus the caller's source file. bash prints the literal
            // `NULL` when that source does not exist — a function called from
            // a `-c` string, or the top level of a sourced file — rather than
            // failing, which is what huck did by demanding two frames (#559).
            if n >= 1 {
                let line = shell.call_stack[n - 1].call_line;
                let file = if n >= 2 {
                    shell.call_stack[n - 2].source.clone()
                } else {
                    String::new()
                };
                let file = if file.is_empty() { "NULL" } else { &file };
                let _ = writeln!(out, "{line} {file}");
                ExecOutcome::Continue(0)
            } else {
                ExecOutcome::Continue(1)
            }
        }
        Some(a) => {
            let k: usize = match a.parse::<u64>() {
                Ok(v) => v as usize,
                Err(_) => {
                    crate::sh_error_to!(shell, err, Some("caller"), "{a}: invalid number");
                    e!(err, "caller: usage: caller [expr]");
                    return ExecOutcome::Continue(2);
                }
            };
            if n >= k + 2 {
                let line = shell.call_stack[n - 1 - k].call_line;
                let func = shell.call_stack[n - 2 - k].funcname.clone();
                let file = shell.call_stack[n - 2 - k].source.clone();
                let _ = writeln!(out, "{line} {func} {file}");
                ExecOutcome::Continue(0)
            } else {
                ExecOutcome::Continue(1)
            }
        }
    }
}

#[cfg(test)]
mod ulimit_tests;

#[cfg(test)]
mod enable_tests;

#[cfg(test)]
mod normalize_logical_tests;
