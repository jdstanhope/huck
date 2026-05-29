use std::env;
use std::io::Write;
use std::path::Path;

use crate::shell_state::Shell;

/// The result of running a command — either the shell continues (carrying the
/// command's exit status) or the shell should terminate with a code.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ExecOutcome {
    Continue(i32),
    Exit(i32),
    LoopBreak,
    LoopContinue,
    FunctionReturn(i32),
}

pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return", "trap", "alias", "unalias",
    "set", "shift", ".", "source", "local",
    ":", "true", "false", "command",
    "readonly", "read",
];

pub fn is_builtin(name: &str) -> bool {
    BUILTIN_NAMES.contains(&name)
}

/// True for POSIX "special builtins" (2.14). Inline assignments preceding a
/// special builtin persist in the shell; assignments preceding a regular
/// builtin or external command are scoped to the command. The set is huck's
/// existing builtins intersected with the POSIX special list; expand here as
/// huck adds `eval`/`exec`/`:`/`readonly`.
pub fn is_special_builtin(name: &str) -> bool {
    matches!(name,
        ":" | "." | "break" | "continue" | "exit" | "export" | "readonly" | "return"
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
    shell: &mut Shell,
) -> ExecOutcome {
    match name {
        "cd" => builtin_cd(args, shell),
        "pwd" => builtin_pwd(out),
        "echo" => builtin_echo(args, out),
        "exit" => builtin_exit(args),
        "export" => builtin_export(args, out, shell),
        "unset" => builtin_unset(args, shell),
        "local" => builtin_local(args, shell),
        "jobs" => builtin_jobs(args, out, shell),
        "wait" => builtin_wait(args, out, shell),
        "fg" => builtin_fg(args, shell),
        "bg" => builtin_bg(args, out, shell),
        "kill" => builtin_kill(args, out, shell),
        "disown" => builtin_disown(args, shell),
        "history" => builtin_history(args, out, shell),
        "trap" => builtin_trap(args, out, shell),
        "set" => builtin_set(args, out, shell),
        "shift" => builtin_shift(args, shell),
        "." | "source" => builtin_source(args, shell),
        "alias" => builtin_alias(args, out, shell),
        "unalias" => builtin_unalias(args, shell),
        ":" => builtin_colon(args, shell),
        "true" => builtin_true(args, shell),
        "false" => builtin_false(args, shell),
        "command" => builtin_command(args, out, shell),
        "readonly" => builtin_readonly(args, out, shell),
        "read" => builtin_read(args, out, shell),
        "test" | "[" => builtin_test(name, args),
        "break" => ExecOutcome::LoopBreak,
        "continue" => ExecOutcome::LoopContinue,
        "return" => {
            let code = match args.first() {
                Some(s) => s.parse::<i32>().unwrap_or_else(|_| shell.last_status()),
                None => shell.last_status(),
            };
            ExecOutcome::FunctionReturn(code)
        }
        _ => unreachable!("run_builtin called with non-builtin: {name}"),
    }
}

fn builtin_cd(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if args.len() > 1 {
        eprintln!("huck: cd: too many arguments");
        return ExecOutcome::Continue(1);
    }
    let target = match args.first() {
        Some(dir) => dir.clone(),
        None => match shell.get("HOME") {
            Some(home) => home.to_string(),
            None => {
                eprintln!("huck: cd: HOME not set");
                return ExecOutcome::Continue(1);
            }
        },
    };
    if let Err(e) = env::set_current_dir(Path::new(&target)) {
        eprintln!("huck: cd: {target}: {e}");
        return ExecOutcome::Continue(1);
    }
    // chdir succeeded — maintain PWD/OLDPWD.
    let prev_pwd = shell.get("PWD").map(str::to_string);
    match env::current_dir() {
        Ok(new_pwd) => {
            if let Some(prev) = prev_pwd {
                shell.export_set("OLDPWD", prev);
            }
            shell.export_set("PWD", new_pwd.to_string_lossy().to_string());
        }
        Err(e) => {
            // chdir succeeded but we can't read it back — warn but
            // don't fail the command.
            eprintln!("huck: cd: warning: could not read current dir: {e}");
        }
    }
    ExecOutcome::Continue(0)
}

fn builtin_pwd(out: &mut dyn Write) -> ExecOutcome {
    match env::current_dir() {
        Ok(path) => {
            if let Err(e) = writeln!(out, "{}", path.display()) {
                eprintln!("huck: pwd: {e}");
                return ExecOutcome::Continue(1);
            }
            ExecOutcome::Continue(0)
        }
        Err(e) => {
            eprintln!("huck: pwd: {e}");
            ExecOutcome::Continue(1)
        }
    }
}

fn builtin_echo(args: &[String], out: &mut dyn Write) -> ExecOutcome {
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
        eprintln!("huck: echo: {e}");
        return ExecOutcome::Continue(1);
    }
    if !suppress_newline
        && let Err(e) = out.write_all(b"\n")
    {
        eprintln!("huck: echo: {e}");
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

fn builtin_exit(args: &[String]) -> ExecOutcome {
    match args.first() {
        None => ExecOutcome::Exit(0),
        Some(code_str) => match code_str.parse::<i32>() {
            Ok(code) => ExecOutcome::Exit(code.rem_euclid(256)),
            Err(_) => {
                eprintln!("huck: exit: {code_str}: numeric argument required");
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
                eprintln!("huck: export: {e}");
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
                    eprintln!("huck: export: '{arg}': not a valid identifier");
                    any_error = true;
                    continue;
                }
                if shell.is_readonly(name) {
                    eprintln!("huck: export: {name}: readonly variable");
                    any_error = true;
                    continue;
                }
                shell.export_set(name, value.to_string());
            }
            None => {
                if !is_valid_name(arg) {
                    eprintln!("huck: export: '{arg}': not a valid identifier");
                    any_error = true;
                    continue;
                }
                // Bare `export NAME` (no `=`) is exempt from the
                // readonly check: bash allows flipping the export flag
                // on a readonly variable without changing its value.
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
            eprintln!("huck: unset: '{arg}': not a valid identifier");
            any_error = true;
            continue;
        }
        if shell.is_readonly(arg) {
            eprintln!("huck: unset: {arg}: readonly variable");
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

fn builtin_local(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if shell.local_scopes.is_empty() {
        eprintln!("huck: local: can only be used in a function");
        return ExecOutcome::Continue(1);
    }
    let mut exit: i32 = 0;
    for arg in args {
        let (name, value): (&str, Option<String>) = match arg.find('=') {
            Some(eq) => (&arg[..eq], Some(arg[eq + 1..].to_string())),
            None => (arg.as_str(), None),
        };
        if !is_valid_name(name) {
            eprintln!("huck: local: `{arg}': not a valid identifier");
            exit = 1;
            continue;
        }
        // Refuse to shadow a readonly variable. Do NOT snapshot or
        // set; the outer (readonly) binding stays live.
        if shell.is_readonly(name) {
            eprintln!("huck: local: {name}: readonly variable");
            exit = 1;
            continue;
        }
        // Snapshot pre-local state only if NAME is not already saved
        // in this frame. Compute the snapshot via shell.snapshot_var
        // BEFORE taking the mutable borrow on local_scopes.
        let already_saved = shell
            .local_scopes
            .last()
            .map(|f| f.contains_key(name))
            .unwrap_or(false);
        if !already_saved {
            let snap = shell.snapshot_var(name);
            shell
                .local_scopes
                .last_mut()
                .unwrap()
                .insert(name.to_string(), snap);
        }
        shell.set(name, value.unwrap_or_default());
    }
    ExecOutcome::Continue(exit)
}

/// `readonly [-p] [NAME[=VALUE] ...]`. POSIX special builtin. With no
/// names (or with `-p`), lists every readonly variable in
/// `readonly NAME='value'` form (using the existing single-quote
/// escape). For each NAME=VALUE arg, sets the value and marks readonly;
/// for each bare NAME arg, marks readonly (creating an empty var if
/// unset). Refuses to overwrite an already-readonly variable. Invalid
/// identifiers → status 1 (other args still processed).
fn builtin_readonly(
    args: &[String],
    out: &mut dyn std::io::Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let mut want_list = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-p" => {
                want_list = true;
                i += 1;
            }
            "--" => {
                i += 1;
                break;
            }
            s if s.starts_with('-') && s.len() > 1 => {
                eprintln!("huck: readonly: {s}: invalid option");
                return ExecOutcome::Continue(2);
            }
            _ => break,
        }
    }
    let names = &args[i..];

    if names.is_empty() || want_list {
        for name in shell.readonly_names() {
            let value = shell.lookup_var(&name).unwrap_or_default();
            if let Err(e) = writeln!(
                out,
                "readonly {name}='{}'",
                escape_alias_value(&value)
            ) {
                eprintln!("huck: readonly: {e}");
                return ExecOutcome::Continue(1);
            }
        }
        return ExecOutcome::Continue(0);
    }

    let mut exit = 0;
    for arg in names {
        let (name, value): (&str, Option<String>) = match arg.find('=') {
            Some(eq) => (&arg[..eq], Some(arg[eq + 1..].to_string())),
            None => (arg.as_str(), None),
        };
        if !is_valid_name(name) {
            eprintln!("huck: readonly: `{arg}': not a valid identifier");
            exit = 1;
            continue;
        }
        if let Some(v) = value {
            // Refuse to overwrite an already-readonly variable.
            if shell.is_readonly(name) {
                eprintln!("huck: readonly: {name}: readonly variable");
                exit = 1;
                continue;
            }
            shell.set(name, v);
        }
        shell.mark_readonly(name);
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
fn read_one_line<R: std::io::BufRead>(
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

/// `read [-r] [-p PROMPT] [-s] [-d DELIM] [NAME ...]`. Regular
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
    shell: &mut Shell,
) -> ExecOutcome {
    let mut raw = false;
    let mut silent = false;
    let mut prompt: Option<String> = None;
    let mut delim: u8 = b'\n';
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
                            eprintln!("huck: read: -p: option requires an argument");
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
                            eprintln!("huck: read: -d: option requires an argument");
                            return ExecOutcome::Continue(2);
                        }
                        args[i].clone()
                    };
                    // Empty DELIM means NUL byte.
                    delim = d_val.bytes().next().unwrap_or(0u8);
                    break;
                }
                c => {
                    eprintln!("huck: read: -{}: invalid option", c as char);
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
            eprintln!("huck: read: `{name}': not a valid identifier");
            return ExecOutcome::Continue(1);
        }
    }

    // Prompt — only when stdin is a tty (matches bash).
    if let Some(p) = &prompt {
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() {
            eprint!("{p}");
            let _ = std::io::Write::flush(&mut std::io::stderr());
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

    let stdin = std::io::stdin();
    let mut handle = stdin.lock();
    let line_opt = match read_one_line(&mut handle, raw, delim) {
        Ok(opt) => opt,
        Err(e) => {
            eprintln!("huck: read: {e}");
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
        eprintln!();
    }

    let line = match line_opt {
        Some(l) => l,
        None => return ExecOutcome::Continue(1), // EOF, nothing read
    };

    // Assignment.
    let ifs = shell
        .lookup_var("IFS")
        .unwrap_or_else(|| " \t\n".to_string());
    let assignments: Vec<(String, String)> = if names.is_empty() {
        vec![("REPLY".to_string(), line)]
    } else {
        split_into_names(&line, &names, &ifs)
    };

    let mut exit = 0;
    for (name, value) in assignments {
        if shell.try_set(&name, value).is_err() {
            eprintln!("huck: read: {name}: readonly variable");
            exit = 1;
        }
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
fn parse_jobs_args(args: &[String], shell: &Shell) -> Result<JobsArgs, ExecOutcome> {
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
                        eprintln!("huck: jobs: -{c}: invalid option");
                        eprintln!("huck: jobs: usage: jobs [-lpnrs] [%spec ...]");
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
            eprintln!("huck: jobs: {arg}: no such job");
            return Err(ExecOutcome::Continue(1));
        }
        let id = resolve_spec_or_error(arg, "jobs", shell)?;
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

fn builtin_jobs(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let parsed = match parse_jobs_args(args, shell) {
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
            eprintln!("huck: jobs: {e}");
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
fn parse_wait_args(args: &[String], shell: &Shell) -> Result<WaitArgs, ExecOutcome> {
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
                    eprintln!("huck: wait: -p: option requires a variable name");
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
                eprintln!("huck: wait: {s}: invalid option");
                eprintln!("huck: wait: usage: wait [-n] [-p var] [id ...]");
                return Err(ExecOutcome::Continue(2));
            }
            _ => break,
        }
    }

    if pid_var.is_some() && !wait_any {
        eprintln!("huck: wait: -p: option requires -n");
        return Err(ExecOutcome::Continue(2));
    }

    let mut targets = Vec::with_capacity(args.len() - idx);
    while idx < args.len() {
        let arg = &args[idx];
        if arg.starts_with('%') {
            let id = resolve_spec_or_error(arg, "wait", shell)?;
            targets.push(WaitTarget::Job(id));
        } else {
            match arg.parse::<i32>() {
                Ok(pid) if pid > 0 => targets.push(WaitTarget::Pid(pid)),
                _ => {
                    eprintln!("huck: wait: {arg}: not a pid or valid job spec");
                    return Err(ExecOutcome::Continue(2));
                }
            }
        }
        idx += 1;
    }

    Ok(WaitArgs { wait_any, pid_var, targets })
}

fn builtin_wait(args: &[String], _out: &mut dyn std::io::Write, shell: &mut Shell) -> ExecOutcome {
    let parsed = match parse_wait_args(args, shell) {
        Ok(p) => p,
        Err(outcome) => return outcome,
    };

    match (parsed.wait_any, parsed.targets.len()) {
        (false, 0) => wait_all(shell),
        (false, 1) => match &parsed.targets[0] {
            WaitTarget::Job(id) => wait_for_job(*id, shell),
            WaitTarget::Pid(pid) => wait_for_pid(*pid, shell),
        },
        (false, _) => wait_for_all(parsed.targets, shell),
        (true, 0) => wait_any_pending(parsed.pid_var, shell),
        (true, _) => wait_any_of(parsed.targets, parsed.pid_var, shell),
    }
}

fn wait_all(shell: &mut Shell) -> ExecOutcome {
    while shell.jobs.has_pending() {
        if check_sigint(shell) { return ExecOutcome::Continue(130); }
        let mut status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if r > 0 {
            shell.jobs.reap(r, status);
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
        if check_sigint(shell) { return ExecOutcome::Continue(130); }
        let mut status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if r > 0 {
            shell.jobs.reap(r, status);
        } else {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

fn wait_for_pid(pid: i32, shell: &mut Shell) -> ExecOutcome {
    let mut first = true;
    loop {
        if check_sigint(shell) { return ExecOutcome::Continue(130); }
        let mut status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if r > 0 {
            shell.jobs.reap(r, status);
            if libc::WIFSTOPPED(status) {
                // Still alive; keep polling.
                first = false;
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
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
                eprintln!("huck: wait: pid {pid} is not a child of this shell");
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
fn wait_for_all(targets: Vec<WaitTarget>, shell: &mut Shell) -> ExecOutcome {
    let mut last = 0;
    for t in targets {
        let outcome = match t {
            WaitTarget::Job(id) => wait_for_job(id, shell),
            WaitTarget::Pid(pid) => wait_for_pid(pid, shell),
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

        if check_sigint(shell) {
            return ExecOutcome::Continue(130);
        }
        let mut status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if r > 0 {
            shell.jobs.reap(r, status);
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

    let any_active = targets.iter().any(|t| match t {
        WaitTarget::Job(id) => shell.jobs.iter().any(|j| j.id == *id),
        WaitTarget::Pid(pid) => {
            let mut s: libc::c_int = 0;
            let r = unsafe { libc::waitpid(*pid, &mut s, libc::WNOHANG | libc::WUNTRACED) };
            if r > 0 {
                shell.jobs.reap(r, s);
                true
            } else {
                r == 0
            }
        }
    });
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
        if check_sigint(shell) {
            return ExecOutcome::Continue(130);
        }
        let mut status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if r > 0 {
            shell.jobs.reap(r, status);
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

fn check_sigint(shell: &Shell) -> bool {
    if shell.sigint_flag
        .compare_exchange(
            true,
            false,
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
        )
        .is_ok()
    {
        eprintln!();
        true
    } else {
        false
    }
}

fn print_killable_table(out: &mut dyn Write) {
    let table = crate::traps::killable_signals();
    let mut sorted: Vec<&(&str, i32)> = table.iter().collect();
    sorted.sort_by_key(|(_, n)| *n);
    let cols = 4;
    for chunk in sorted.chunks(cols) {
        let mut line = String::new();
        for (i, (name, num)) in chunk.iter().enumerate() {
            if i > 0 { line.push(' '); }
            line.push_str(&format!("{num:>2}) {name:<5}"));
        }
        let _ = writeln!(out, "{line}");
    }
}

fn handle_kill_l(args: &[String], out: &mut dyn Write) -> ExecOutcome {
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
                    eprintln!("huck: kill: {arg}: invalid signal specification");
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
                    eprintln!("huck: kill: {arg}: invalid signal specification");
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
    shell: &Shell,
) -> Result<u32, ExecOutcome> {
    let spec = crate::job_spec::parse_job_spec(arg).map_err(|_| {
        eprintln!("huck: {builtin}: {arg}: bad job spec");
        ExecOutcome::Continue(1)
    })?;
    match shell.jobs.resolve(&spec) {
        Ok(id) => Ok(id),
        Err(crate::jobs::JobSpecResolveError::NotFound) => {
            eprintln!("huck: {builtin}: {arg}: no such job");
            Err(ExecOutcome::Continue(1))
        }
        Err(crate::jobs::JobSpecResolveError::Ambiguous) => {
            eprintln!("huck: {builtin}: {arg}: ambiguous job spec");
            Err(ExecOutcome::Continue(1))
        }
    }
}

fn builtin_kill(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if matches!(args.first().map(|s| s.as_str()), Some("-l")) {
        return handle_kill_l(&args[1..], out);
    }
    match args.first().map(|s| s.as_str()) {
        Some("-s") => return kill_with_s_flag(&args[1..], shell),
        Some("-n") => return kill_with_n_flag(&args[1..], shell),
        _ => {}
    }
    let (sig, targets) = if let Some(first) = args.first() {
        if let Some(rest) = first.strip_prefix('-') {
            // -<sig> form
            let sig = match rest.parse::<i32>() {
                Ok(n) if (0..=64).contains(&n) => n,
                Ok(_) => {
                    eprintln!("huck: kill: {rest}: invalid signal number");
                    return ExecOutcome::Continue(1);
                }
                Err(_) => match signal_by_name(rest) {
                    Some(n) => n,
                    None => {
                        eprintln!("huck: kill: {rest}: invalid signal");
                        return ExecOutcome::Continue(1);
                    }
                },
            };
            if args.len() < 2 {
                eprintln!("huck: kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | %job ...");
                return ExecOutcome::Continue(2);
            }
            (sig, &args[1..])
        } else {
            (libc::SIGTERM, args)
        }
    } else {
        eprintln!("huck: kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | %job ...");
        return ExecOutcome::Continue(2);
    };

    send_signal_to_targets(sig, targets, shell)
}

/// Handles `kill -s SIGNAME [targets...]`. The `-s` token has already
/// been consumed by the dispatcher; `args` is everything after it.
fn kill_with_s_flag(args: &[String], shell: &mut Shell) -> ExecOutcome {
    let name = match args.first() {
        Some(n) => n,
        None => {
            eprintln!("huck: kill: -s: option requires an argument");
            return ExecOutcome::Continue(2);
        }
    };
    let sig = match signal_by_name(name) {
        Some(n) => n,
        None => {
            eprintln!("huck: kill: {name}: invalid signal specification");
            return ExecOutcome::Continue(1);
        }
    };
    let targets = &args[1..];
    if targets.is_empty() {
        eprintln!("huck: kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | %job ...");
        return ExecOutcome::Continue(2);
    }
    send_signal_to_targets(sig, targets, shell)
}

/// Handles `kill -n SIGNUM [targets...]`. The `-n` token has already
/// been consumed by the dispatcher; `args` is everything after it.
/// Number must be in `killable_signals()` (matching `kill -l`'s set).
fn kill_with_n_flag(args: &[String], shell: &mut Shell) -> ExecOutcome {
    let num_arg = match args.first() {
        Some(s) => s,
        None => {
            eprintln!("huck: kill: -n: option requires an argument");
            return ExecOutcome::Continue(2);
        }
    };
    let n = match num_arg.parse::<i32>() {
        Ok(n) if (1..=64).contains(&n) => n,
        _ => {
            eprintln!("huck: kill: {num_arg}: invalid signal specification");
            return ExecOutcome::Continue(1);
        }
    };
    if !crate::traps::killable_signals()
        .iter()
        .any(|(_, num)| *num == n)
    {
        eprintln!("huck: kill: {num_arg}: invalid signal specification");
        return ExecOutcome::Continue(1);
    }
    let targets = &args[1..];
    if targets.is_empty() {
        eprintln!("huck: kill: usage: kill [-s sigspec | -n signum | -sigspec] pid | %job ...");
        return ExecOutcome::Continue(2);
    }
    send_signal_to_targets(n, targets, shell)
}

/// Sends `sig` to each target (`%spec` or PID). Returns `Continue(1)`
/// if any send failed (with errors already on stderr), `Continue(0)`
/// otherwise. Shared between every kill dispatch arm.
fn send_signal_to_targets(
    sig: i32,
    targets: &[String],
    shell: &mut Shell,
) -> ExecOutcome {
    let mut any_failed = false;
    for target in targets {
        if let Some(_rest) = target.strip_prefix('%') {
            let id = match resolve_spec_or_error(target, "kill", shell) {
                Ok(id) => id,
                Err(_) => {
                    any_failed = true;
                    continue;
                }
            };
            let pgid = match shell.jobs.iter().find(|j| j.id == id) {
                Some(j) => j.pgid,
                None => {
                    eprintln!("huck: kill: {target}: no such job");
                    any_failed = true;
                    continue;
                }
            };
            let rc = unsafe { libc::killpg(pgid, sig) };
            if rc != 0 {
                let errno = std::io::Error::last_os_error();
                eprintln!("huck: kill: ({target}) - {errno}");
                any_failed = true;
            }
        } else {
            match target.parse::<i32>() {
                Ok(pid) if pid > 0 => {
                    let rc = unsafe { libc::kill(pid, sig) };
                    if rc != 0 {
                        let errno = std::io::Error::last_os_error();
                        eprintln!("huck: kill: ({pid}) - {errno}");
                        any_failed = true;
                    }
                }
                _ => {
                    eprintln!("huck: kill: {target}: arguments must be process or job IDs");
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

fn builtin_disown(args: &[String], shell: &mut Shell) -> ExecOutcome {
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
                        eprintln!("huck: disown: -{c}: invalid option");
                        eprintln!("huck: disown: usage: disown [-ahr] [%job ...]");
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
                match resolve_spec_or_error(arg, "disown", shell) {
                    Ok(id) => ids.push(id),
                    Err(outcome) => return outcome,
                }
            } else {
                match arg.parse::<i32>() {
                    Ok(pid) if pid > 0 => {
                        match shell.jobs.iter().find(|j| j.pids.contains(&pid)) {
                            Some(job) => ids.push(job.id),
                            None => {
                                eprintln!("huck: disown: {arg}: no such job");
                                return ExecOutcome::Continue(1);
                            }
                        }
                    }
                    _ => {
                        eprintln!("huck: disown: {arg}: not a valid job spec");
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
                eprintln!("huck: disown: no current job");
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

fn builtin_fg(args: &[String], shell: &mut Shell) -> ExecOutcome {
    let id = match args.len() {
        0 => match shell.jobs.current_id() {
            Some(id) => id,
            None => {
                eprintln!("huck: fg: no current job");
                return ExecOutcome::Continue(1);
            }
        },
        1 if args[0].starts_with('%') => match resolve_spec_or_error(&args[0], "fg", shell) {
            Ok(id) => id,
            Err(outcome) => return outcome,
        },
        _ => {
            eprintln!("huck: fg: usage: fg [%job]");
            return ExecOutcome::Continue(2);
        }
    };
    let (pgid, pids, command) = {
        if let Some(job) = shell.jobs.jobs_mut().iter_mut().find(|j| j.id == id) {
            job.state = crate::jobs::JobState::Running;
            job.notified = true;
            (job.pgid, job.pids.clone(), job.command.clone())
        } else {
            eprintln!("huck: fg: no current job");
            return ExecOutcome::Continue(1);
        }
    };

    eprintln!("{command}");

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
        eprintln!("\n{line}");
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

fn builtin_bg(args: &[String], _out: &mut dyn std::io::Write, shell: &mut Shell) -> ExecOutcome {
    let id = match args.len() {
        0 => match shell.jobs.current_stopped_id() {
            Some(id) => id,
            None => {
                eprintln!("huck: bg: no current job");
                return ExecOutcome::Continue(1);
            }
        },
        1 if args[0].starts_with('%') => {
            let id = match resolve_spec_or_error(&args[0], "bg", shell) {
                Ok(id) => id,
                Err(outcome) => return outcome,
            };
            // Verify the resolved job is actually Stopped.
            let is_stopped = shell.jobs.iter()
                .find(|j| j.id == id)
                .map(|j| matches!(j.state, crate::jobs::JobState::Stopped(_)))
                .unwrap_or(false);
            if !is_stopped {
                eprintln!("huck: bg: job %{id} already running");
                return ExecOutcome::Continue(1);
            }
            id
        }
        _ => {
            eprintln!("huck: bg: usage: bg [%job]");
            return ExecOutcome::Continue(2);
        }
    };
    let (pgid, command) = {
        if let Some(job) = shell.jobs.jobs_mut().iter_mut().find(|j| j.id == id) {
            job.state = crate::jobs::JobState::Running;
            job.notified = true;
            (job.pgid, job.command.clone())
        } else {
            eprintln!("huck: bg: no current job");
            return ExecOutcome::Continue(1);
        }
    };

    unsafe { libc::killpg(pgid, libc::SIGCONT); }

    eprintln!("[{id}]+ {command} &");
    ExecOutcome::Continue(0)
}

fn builtin_history(
    args: &[String],
    out: &mut dyn Write,
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
            shell.history.clear();
            ExecOutcome::Continue(0)
        }
        Some(other) => {
            eprintln!("huck: history: {other}: invalid option");
            ExecOutcome::Continue(1)
        }
    }
}

fn builtin_trap(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    use crate::traps::{TrapSignal, install, reset, parse_trap_signal};

    // No args: same as `trap -p`.
    if args.is_empty() {
        print_active_traps(out, shell, None);
        return ExecOutcome::Continue(0);
    }

    // -l: list signal name/number pairs.
    if args[0] == "-l" {
        if args.len() != 1 {
            eprintln!("huck: trap: -l takes no arguments");
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
                    eprintln!("huck: trap: {msg}");
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
            eprintln!("huck: trap: usage: trap [-lp] [[arg] signal_spec ...]");
            return ExecOutcome::Continue(1);
        }
        for name in &args[1..] {
            let sig = match parse_trap_signal(name) {
                Ok(s) => s,
                Err(msg) => {
                    eprintln!("huck: trap: {msg}");
                    return ExecOutcome::Continue(1);
                }
            };
            if let Err(msg) = reset(shell, sig) {
                eprintln!("huck: trap: {msg}");
                return ExecOutcome::Continue(1);
            }
        }
        return ExecOutcome::Continue(0);
    }

    // `trap ACTION SIGNAL...`: install action for each signal.
    if args.len() < 2 {
        eprintln!("huck: trap: usage: trap [-lp] [[arg] signal_spec ...]");
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
                eprintln!("huck: trap: {msg}");
                return ExecOutcome::Continue(1);
            }
        };
        if let Err(msg) = install(shell, sig, action.clone()) {
            eprintln!("huck: trap: {msg}");
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
    use crate::traps::name_table;
    let table = name_table();
    // Sort by signal number for the listing.
    let mut sorted: Vec<&(&str, i32)> = table.iter().collect();
    sorted.sort_by_key(|(_, n)| *n);
    let cols = 4;
    for chunk in sorted.chunks(cols) {
        let mut line = String::new();
        for (i, (name, num)) in chunk.iter().enumerate() {
            if i > 0 { line.push(' '); }
            line.push_str(&format!("{num:>2}) {name:<5}"));
        }
        let _ = writeln!(out, "{line}");
    }
}

/// Returns the canonical name (no SIG prefix) for `signum`, or None
/// if `signum` is not in the trappable table.
fn signal_number_to_name(signum: i32) -> Option<String> {
    crate::traps::name_table().iter().find_map(|(name, n)| {
        if *n == signum { Some(name.to_string()) } else { None }
    })
}

fn builtin_shift(args: &[String], shell: &mut Shell) -> ExecOutcome {
    let n: usize = match args.first() {
        None => 1,
        Some(s) => match s.parse::<usize>() {
            Ok(n) => n,
            Err(_) => {
                eprintln!("huck: shift: {s}: numeric argument required");
                return ExecOutcome::Continue(1);
            }
        },
    };
    if n > shell.positional_args.len() {
        eprintln!("huck: shift: shift count out of range");
        return ExecOutcome::Continue(1);
    }
    shell.positional_args.drain(0..n);
    ExecOutcome::Continue(0)
}

fn builtin_set(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
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

    let first = &args[0];
    if first == "--" {
        shell.positional_args = args[1..].to_vec();
        return ExecOutcome::Continue(0);
    }
    if (first.starts_with('-') || first.starts_with('+')) && first.len() > 1 {
        eprintln!("huck: set: {first}: options not yet supported in this version");
        return ExecOutcome::Continue(2);
    }
    // No leading -- or option flag — replace positional with all args.
    shell.positional_args = args.to_vec();
    ExecOutcome::Continue(0)
}

fn set_escape_value(v: &str) -> String {
    format!("'{}'", v.replace('\'', r#"'\''"#))
}

fn builtin_source(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if args.is_empty() {
        eprintln!("huck: .: usage: . filename [arguments]");
        return ExecOutcome::Continue(2);
    }
    if shell.source_depth >= 64 {
        eprintln!("huck: .: maximum source depth (64) exceeded");
        return ExecOutcome::Continue(1);
    }
    let filename = &args[0];
    let path = match resolve_source_path(filename, shell) {
        Some(p) => p,
        None => {
            eprintln!("huck: .: {filename}: file not found");
            return ExecOutcome::Continue(1);
        }
    };
    let contents = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("huck: .: {}: {e}", path.display());
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
    let result = run_sourced_contents(&contents, &path, shell);
    shell.source_depth -= 1;

    if let Some(saved) = saved_positional {
        shell.positional_args = saved;
    }
    result
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

fn run_sourced_contents(
    contents: &str,
    path: &std::path::Path,
    shell: &mut crate::shell_state::Shell,
) -> ExecOutcome {
    use crate::continuation::{classify, Completeness};
    let mut last_status = shell.last_status();
    let mut buf = String::new();
    for line in contents.lines() {
        buf.push_str(line);
        buf.push('\n');
        if let Completeness::Incomplete(_) = classify(&buf) {
            continue;
        }
        let tokens = match crate::lexer::tokenize(&buf) {
            Ok(t) if t.is_empty() => {
                buf.clear();
                continue;
            }
            Ok(t) => t,
            Err(e) => {
                eprintln!(
                    "huck: {}: syntax error{}",
                    path.display(),
                    crate::shell::lex_error_message(e)
                );
                last_status = 2;
                buf.clear();
                continue;
            }
        };
        match crate::command::parse(tokens) {
            Ok(Some(seq)) => {
                let outcome = crate::executor::execute(&seq, shell, &buf);
                buf.clear();
                match outcome {
                    ExecOutcome::Continue(c) => last_status = c,
                    ExecOutcome::Exit(n) => return ExecOutcome::Exit(n),
                    ExecOutcome::FunctionReturn(n) => {
                        return ExecOutcome::Continue(n);
                    }
                    ExecOutcome::LoopBreak | ExecOutcome::LoopContinue => {
                        last_status = 0;
                    }
                }
            }
            Ok(None) => buf.clear(),
            Err(e) => {
                eprintln!(
                    "huck: {}: syntax error: {}",
                    path.display(),
                    crate::shell::parse_error_message(e)
                );
                last_status = 2;
                buf.clear();
            }
        }
    }
    ExecOutcome::Continue(last_status)
}

fn is_valid_alias_name(s: &str) -> bool {
    !s.is_empty()
        && !s.contains('=')
        && s.chars().all(|c| !c.is_whitespace() && !"|&;<>()$`\\\"'*?[]#~{}".contains(c))
}

fn escape_alias_value(v: &str) -> String {
    // Bash format: alias name='value' with single quotes inside
    // the value rewritten as '\''.
    v.replace('\'', r#"'\''"#)
}

fn builtin_alias(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if args.is_empty() {
        let mut names: Vec<&String> = shell.aliases.keys().collect();
        names.sort();
        for name in names {
            let value = &shell.aliases[name];
            let _ = writeln!(out, "alias {}='{}'", name, escape_alias_value(value));
        }
        return ExecOutcome::Continue(0);
    }
    let mut any_err = false;
    for arg in args {
        if let Some(eq) = arg.find('=') {
            let name = &arg[..eq];
            let value = &arg[eq + 1..];
            if !is_valid_alias_name(name) {
                eprintln!("huck: alias: `{name}': invalid alias name");
                any_err = true;
                continue;
            }
            shell.aliases.insert(name.to_string(), value.to_string());
        } else {
            match shell.aliases.get(arg) {
                Some(v) => {
                    let _ = writeln!(out, "alias {}='{}'", arg, escape_alias_value(v));
                }
                None => {
                    eprintln!("huck: alias: {arg}: not found");
                    any_err = true;
                }
            }
        }
    }
    ExecOutcome::Continue(if any_err { 1 } else { 0 })
}

fn builtin_unalias(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if args.is_empty() {
        eprintln!("huck: unalias: usage: unalias [-a] name [name ...]");
        return ExecOutcome::Continue(2);
    }
    if args[0] == "-a" {
        shell.aliases.clear();
        return ExecOutcome::Continue(0);
    }
    let mut any_err = false;
    for name in args {
        if shell.aliases.remove(name).is_none() {
            eprintln!("huck: unalias: {name}: not found");
            any_err = true;
        }
    }
    ExecOutcome::Continue(if any_err { 1 } else { 0 })
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
        | "for" | "in"
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

fn search_path_for(name: &str, shell: &Shell) -> Option<std::path::PathBuf> {
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

fn builtin_command(
    args: &[String],
    out: &mut dyn std::io::Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let mut concise = false;
    let mut verbose = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-v" => { concise = true; i += 1; }
            "-V" => { verbose = true; i += 1; }
            "--" => { i += 1; break; }
            s if s.starts_with('-') && s.len() > 1 => {
                eprintln!("huck: command: {s}: invalid option");
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
        eprintln!(
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
                    eprintln!("huck: command: {name}: not found");
                }
            }
        }
    }
    ExecOutcome::Continue(if any_not_found { 1 } else { 0 })
}

fn builtin_test(name: &str, args: &[String]) -> ExecOutcome {
    let eval_args: &[String] = if name == "[" {
        match args.last() {
            Some(last) if last == "]" => &args[..args.len() - 1],
            _ => {
                eprintln!("huck: [: missing ']'");
                return ExecOutcome::Continue(2);
            }
        }
    } else {
        args
    };
    match crate::test_builtin::evaluate(eval_args) {
        Ok(true) => ExecOutcome::Continue(0),
        Ok(false) => ExecOutcome::Continue(1),
        Err(msg) => {
            eprintln!("huck: {name}: {msg}");
            ExecOutcome::Continue(2)
        }
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
    fn exit_masks_value_greater_than_255() {
        assert!(matches!(
            builtin_exit(&["300".to_string()]),
            ExecOutcome::Exit(44)
        ));
    }

    #[test]
    fn exit_masks_negative_value() {
        assert!(matches!(
            builtin_exit(&["-1".to_string()]),
            ExecOutcome::Exit(255)
        ));
    }

    #[test]
    fn exit_masks_exact_256_to_zero() {
        assert!(matches!(
            builtin_exit(&["256".to_string()]),
            ExecOutcome::Exit(0)
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
    fn echo_n_suppresses_trailing_newline() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-n".to_string(), "hello".to_string()], &mut out);
        assert_eq!(out, b"hello");
    }

    #[test]
    fn echo_n_alone_writes_nothing() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-n".to_string()], &mut out);
        assert_eq!(out, b"");
    }

    #[test]
    fn echo_e_processes_basic_escapes() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-e".to_string(), r"a\tb\nc".to_string()], &mut out);
        assert_eq!(out, b"a\tb\nc\n");
    }

    #[test]
    fn echo_capital_e_keeps_backslashes_literal() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-E".to_string(), r"a\tb".to_string()], &mut out);
        assert_eq!(out, b"a\\tb\n");
    }

    #[test]
    fn echo_default_keeps_backslashes_literal() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&[r"a\tb".to_string()], &mut out);
        assert_eq!(out, b"a\\tb\n");
    }

    #[test]
    fn echo_combined_ne_flag() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-ne".to_string(), r"a\tb".to_string()], &mut out);
        assert_eq!(out, b"a\tb");
    }

    #[test]
    fn echo_e_then_capital_e_disables_escapes() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-eE".to_string(), r"a\tb".to_string()], &mut out);
        assert_eq!(out, b"a\\tb\n");
    }

    #[test]
    fn echo_non_flag_arg_stops_flag_parsing() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(
            &["-n".to_string(), "foo".to_string(), "-n".to_string(), "bar".to_string()],
            &mut out,
        );
        assert_eq!(out, b"foo -n bar");
    }

    #[test]
    fn echo_unknown_flag_is_literal() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-x".to_string(), "foo".to_string()], &mut out);
        assert_eq!(out, b"-x foo\n");
    }

    #[test]
    fn echo_single_dash_is_literal() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-".to_string()], &mut out);
        assert_eq!(out, b"-\n");
    }

    #[test]
    fn echo_double_dash_is_literal() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["--".to_string(), "foo".to_string()], &mut out);
        assert_eq!(out, b"-- foo\n");
    }

    #[test]
    fn echo_e_c_escape_terminates_output() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-e".to_string(), r"abc\cdef".to_string()], &mut out);
        assert_eq!(out, b"abc");
    }

    #[test]
    fn echo_e_octal_escape() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-e".to_string(), r"\0101".to_string()], &mut out);
        assert_eq!(out, b"A\n");
    }

    #[test]
    fn echo_e_hex_escape() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-e".to_string(), r"\x41".to_string()], &mut out);
        assert_eq!(out, b"A\n");
    }

    #[test]
    fn echo_e_unknown_escape_keeps_backslash() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&["-e".to_string(), r"\z".to_string()], &mut out);
        assert_eq!(out, b"\\z\n");
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
        shell.set("HUCK_EXP", "v".to_string());
        let mut out = Vec::new();
        let outcome = builtin_export(&["HUCK_EXP".to_string()], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let in_exported = shell.exported_env().any(|(k, _)| k == "HUCK_EXP");
        assert!(in_exported);
    }

    #[test]
    fn export_name_only_creates_empty_exported() {
        let mut shell = Shell::new();
        let mut out = Vec::new();
        let outcome = builtin_export(&["HUCK_NEW_VAR".to_string()], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.get("HUCK_NEW_VAR"), Some(""));
        assert!(shell.exported_env().any(|(k, _)| k == "HUCK_NEW_VAR"));
    }

    #[test]
    fn export_sets_and_exports() {
        let mut shell = Shell::new();
        let mut out = Vec::new();
        let outcome = builtin_export(&["HUCK_EXP2=hello".to_string()], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.get("HUCK_EXP2"), Some("hello"));
        let in_exported = shell.exported_env().any(|(k, _)| k == "HUCK_EXP2");
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
        shell.set("HUCK_RM", "v".to_string());
        let outcome = builtin_unset(&["HUCK_RM".to_string()], &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.get("HUCK_RM"), None);
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
        let outcome = builtin_unset(&["NEVER_SET_HUCK_XYZ".to_string()], &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn jobs_with_empty_table_prints_nothing_and_returns_zero() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_jobs(&[], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(out.is_empty());
    }

    #[test]
    fn jobs_lists_synthetic_done_entry() {
        let mut shell = Shell::new();
        let _ = shell.jobs.add_synthetic_done("echo hi".to_string(), 0);
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_jobs(&[], &mut out, &mut shell);
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
        let outcome = run_builtin("jobs", &[], &mut buf, &mut shell);
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
        let outcome = run_builtin("jobs", &["-l".to_string()], &mut buf, &mut shell);
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
        let outcome = run_builtin("jobs", &["-l".to_string()], &mut buf, &mut shell);
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
        let outcome = run_builtin("jobs", &["-p".to_string()], &mut buf, &mut shell);
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
        let outcome = run_builtin("jobs", &["-r".to_string()], &mut buf, &mut shell);
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
        let outcome = run_builtin("jobs", &["-s".to_string()], &mut buf, &mut shell);
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
        let outcome = run_builtin("jobs", &["-n".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("[1]"), "first call should show [1]: {out:?}");
        assert!(out.contains("[2]"), "first call should show [2]: {out:?}");

        // Second call: both jobs are now marked notified -> empty output.
        let mut buf2: Vec<u8> = Vec::new();
        let outcome2 = run_builtin("jobs", &["-n".to_string()], &mut buf2, &mut shell);
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
        let outcome = run_builtin("jobs", &["%2".to_string()], &mut buf, &mut shell);
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
        let outcome = run_builtin("jobs", &["-x".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn jobs_p_overrides_l() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "sleep".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &["-lp".to_string()], &mut buf, &mut shell);
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
        let outcome = builtin_wait(&[], &mut out, &mut shell);
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
        let outcome = run_builtin("test", &args, &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn builtin_test_false_expression() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let args = vec!["-z".to_string(), "x".to_string()];
        let outcome = run_builtin("test", &args, &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn builtin_test_usage_error() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let args = vec!["3".to_string(), "-eq".to_string(), "abc".to_string()];
        let outcome = run_builtin("test", &args, &mut out, &mut shell);
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
        let outcome = run_builtin("[", &args, &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn builtin_bracket_missing_close_is_error() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let args = vec!["-n".to_string(), "x".to_string()];
        let outcome = run_builtin("[", &args, &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn builtin_bracket_empty_is_error() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let outcome = run_builtin("[", &[], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn builtin_break_returns_loop_break() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let outcome = run_builtin("break", &[], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::LoopBreak));
    }

    #[test]
    fn builtin_continue_returns_loop_continue() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let outcome = run_builtin("continue", &[], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::LoopContinue));
    }

    #[test]
    fn builtin_return_with_arg_returns_function_return() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            run_builtin("return", &["7".to_string()], &mut out, &mut shell),
            ExecOutcome::FunctionReturn(7)
        );
    }

    #[test]
    fn builtin_return_no_arg_returns_last_status() {
        let mut shell = Shell::new();
        shell.set_last_status(42);
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            run_builtin("return", &[], &mut out, &mut shell),
            ExecOutcome::FunctionReturn(42)
        );
    }

    #[test]
    fn builtin_return_invalid_arg_falls_back_to_last_status() {
        let mut shell = Shell::new();
        shell.set_last_status(13);
        let mut out: Vec<u8> = Vec::new();
        assert_eq!(
            run_builtin("return", &["not-a-num".to_string()], &mut out, &mut shell),
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
            &mut buf,
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
            &mut buf,
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
            &mut buf,
            &mut shell,
        );
        // Then reset.
        let outcome = run_builtin(
            "trap",
            &["-".to_string(), "EXIT".to_string()],
            &mut buf,
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
            &mut buf,
            &mut shell,
        );
        // Clear the buffer (the install printed nothing, but be defensive).
        buf.clear();
        // List.
        let outcome = run_builtin(
            "trap",
            &["-p".to_string()],
            &mut buf,
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
            &mut buf,
            &mut shell,
        );
        buf.clear();
        let outcome = run_builtin("trap", &[], &mut buf, &mut shell);
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
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("2) INT"), "stdout: {out}");
        assert!(out.contains("15) TERM"), "stdout: {out}");
    }

    #[test]
    fn trap_unknown_signal_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["echo bye".to_string(), "NOPE".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn trap_kill_signal_errors_uncatchable() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["echo nope".to_string(), "KILL".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn trap_no_signals_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "trap",
            &["echo bye".to_string()],
            &mut buf,
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
        let outcome = run_builtin("fg", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn bg_with_no_jobs_errors() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn fg_with_percent_spec_arg_and_no_job_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("fg", &["%1".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn bg_with_percent_spec_arg_and_no_job_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &["%1".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn bg_on_running_job_returns_no_current_job() {
        let mut shell = Shell::new();
        shell.jobs.add(4242, vec![4242], "sleep 100".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &[], &mut buf, &mut shell);
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
        let outcome = run_builtin("fg", &["%abc".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn fg_with_no_such_job_spec_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("fg", &["%99".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn fg_with_non_percent_arg_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("fg", &["1".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn fg_with_multiple_args_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "fg",
            &["%1".to_string(), "%2".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn bg_with_bad_job_spec_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &["%abc".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn bg_with_no_such_job_spec_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &["%99".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn bg_with_running_spec_errors_already_running() {
        let mut shell = Shell::new();
        shell.jobs.add(4242, vec![4242], "sleep 100".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &["%1".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn bg_with_multiple_args_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "bg",
            &["%1".to_string(), "%2".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn wait_with_bad_spec_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["%abc".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn wait_with_no_such_spec_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["%99".to_string()], &mut buf, &mut shell);
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
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn wait_with_unparseable_pid_arg_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["abc".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn wait_with_done_spec_returns_decoded_status_immediately() {
        let mut shell = Shell::new();
        // Synthetic Done job — wait should see it's already terminal and
        // return decode(0) → 0 without blocking.
        shell.jobs.add_synthetic_done("echo hi".to_string(), 0);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["%1".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn wait_with_done_spec_returns_nonzero_for_exit_n() {
        let mut shell = Shell::new();
        shell.jobs.add_synthetic_done("false".to_string(), 1);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["%1".to_string()], &mut buf, &mut shell);
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
            &mut buf,
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
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn wait_n_with_no_jobs_returns_127() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["-n".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(127)));
    }

    #[test]
    fn wait_n_with_only_done_jobs_returns_127() {
        let mut shell = Shell::new();
        shell.jobs.add_synthetic_done("true".to_string(), 0);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["-n".to_string()], &mut buf, &mut shell);
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
            &mut buf,
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
            &mut buf,
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
            &mut buf,
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
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn wait_invalid_flag_is_usage_error() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["-x".to_string()], &mut buf, &mut shell);
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
        let outcome = run_builtin("kill", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn kill_sig_flag_with_no_targets_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &["-TERM".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn kill_invalid_signal_name_returns_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-ABC".to_string(), "%1".to_string()],
            &mut buf,
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
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn kill_unparseable_target_returns_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &["abc".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn kill_no_such_job_spec_returns_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &["%99".to_string()], &mut buf, &mut shell);
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
        let outcome = run_builtin("kill", &["-0".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)),
            "kill -0 (no targets) should reach usage check, not signal check");
    }

    #[test]
    fn kill_l_no_args_lists_all_16_signals() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &["-l".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s.matches(')').count(), 16, "output: {s}");
        assert!(s.contains("KILL"), "output missing KILL: {s}");
        assert!(s.contains("TERM"), "output missing TERM: {s}");
        assert!(s.contains("WINCH"), "output missing WINCH: {s}");
    }

    #[test]
    fn kill_l_with_name_returns_number() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-l".to_string(), "TERM".to_string()],
            &mut buf,
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
            &mut buf,
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
            &mut buf,
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
            &mut buf,
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
            &mut buf,
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
            &mut buf,
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
            &mut buf,
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
            &mut buf,
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
            &mut buf,
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
            &mut buf,
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
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn kill_s_missing_arg_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &["-s".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn kill_s_invalid_name_returns_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-s".to_string(), "BOGUS".to_string(), "99999".to_string()],
            &mut buf,
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
            &mut buf,
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
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn kill_n_missing_arg_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &["-n".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn kill_n_invalid_number_returns_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-n".to_string(), "99".to_string(), "12345".to_string()],
            &mut buf,
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
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }
}

#[cfg(test)]
mod cd_pwd_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn cd_sets_pwd_to_target_directory() {
        let mut shell = Shell::new();
        let prev = std::env::current_dir().unwrap();
        let outcome = builtin_cd(&["/tmp".to_string()], &mut shell);
        // Restore for any other tests.
        let _ = std::env::set_current_dir(&prev);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.get("PWD"), Some("/tmp"));
        assert!(shell.exported_env().any(|(k, _)| k == "PWD"));
    }

    #[test]
    fn cd_sets_oldpwd_to_previous_pwd() {
        let mut shell = Shell::new();
        shell.export_set("PWD", "/var".to_string());
        let prev = std::env::current_dir().unwrap();
        let outcome = builtin_cd(&["/tmp".to_string()], &mut shell);
        let _ = std::env::set_current_dir(&prev);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.get("OLDPWD"), Some("/var"));
        assert!(shell.exported_env().any(|(k, _)| k == "OLDPWD"));
    }

    #[test]
    fn cd_with_pwd_initially_unset_does_not_set_oldpwd() {
        let mut shell = Shell::new();
        shell.unset("PWD");
        shell.unset("OLDPWD");
        let prev = std::env::current_dir().unwrap();
        let outcome = builtin_cd(&["/tmp".to_string()], &mut shell);
        let _ = std::env::set_current_dir(&prev);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.get("OLDPWD"), None);
        assert_eq!(shell.get("PWD"), Some("/tmp"));
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
        let outcome = run_builtin("disown", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn disown_no_args_removes_current_job() {
        let mut shell = Shell::new();
        shell.jobs.add(4242, vec![4242], "sleep 100".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.jobs.iter().count(), 0);
    }

    #[test]
    fn disown_with_spec_removes_specified_job() {
        let mut shell = Shell::new();
        shell.jobs.add(100, vec![100], "a".to_string());
        shell.jobs.add(200, vec![200], "b".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["%1".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let remaining: Vec<u32> = shell.jobs.iter().map(|j| j.id).collect();
        assert_eq!(remaining, vec![2]);
    }

    #[test]
    fn disown_with_bad_spec_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["%abc".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn disown_with_non_percent_arg_returns_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["1".to_string()], &mut buf, &mut shell);
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
        let outcome = run_builtin("disown", &["%1".to_string()], &mut buf, &mut shell);
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
        let outcome = run_builtin("disown", &["-a".to_string()], &mut buf, &mut shell);
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
        let outcome = run_builtin("disown", &["-r".to_string()], &mut buf, &mut shell);
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
            &mut buf,
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
            &mut buf,
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
        let outcome = run_builtin("disown", &["-ah".to_string()], &mut buf, &mut shell);
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
        let outcome = run_builtin("disown", &["-ar".to_string()], &mut buf, &mut shell);
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
        let outcome = run_builtin("disown", &["-arh".to_string()], &mut buf, &mut shell);
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
        let outcome = run_builtin("disown", &["-x".to_string()], &mut buf, &mut shell);
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
            &mut buf,
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
        let outcome = run_builtin("disown", &["1234".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.jobs.iter().count(), 0);
    }

    #[test]
    fn disown_bare_pid_matches_pipeline_stage() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234, 1235, 1236], "a | b | c".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["1235".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.jobs.iter().count(), 0);
    }

    #[test]
    fn disown_unknown_pid_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["99999".to_string()], &mut buf, &mut shell);
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
            &mut buf,
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
        shell.history.add("first cmd".to_string());
        shell.history.add("second cmd".to_string());
        let mut out: Vec<u8> = Vec::new();
        let outcome = run_builtin("history", &[], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("first cmd"), "output: {text}");
        assert!(text.contains("second cmd"), "output: {text}");
        assert!(text.contains("1"), "output should have numbers: {text}");
    }

    #[test]
    fn history_dash_c_clears() {
        let mut shell = Shell::new();
        shell.history.add("doomed".to_string());
        let mut out: Vec<u8> = Vec::new();
        let outcome = run_builtin("history", &["-c".to_string()], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.history.last(), None);
    }

    #[test]
    fn history_invalid_option_errors() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let outcome = run_builtin("history", &["--bogus".to_string()], &mut out, &mut shell);
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
            &mut buf,
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
            &mut buf,
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
            &mut buf,
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
                &mut buf,
                &mut shell,
            );
        }
        buf.clear();
        let outcome = run_builtin("trap", &["-p".to_string()], &mut buf, &mut shell);
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
        let outcome = run_builtin("alias", &[], &mut buf, &mut shell);
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
        let outcome = run_builtin("alias", &[], &mut buf, &mut shell);
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
            &mut buf,
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
        let outcome = run_builtin("alias", &["ll".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out, "alias ll='ls -l'\n");
    }

    #[test]
    fn alias_lookup_missing_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("alias", &["xyz".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn unalias_removes_existing() {
        let mut shell = Shell::new();
        shell.aliases.insert("ll".to_string(), "ls -l".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("unalias", &["ll".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(!shell.aliases.contains_key("ll"));
    }

    #[test]
    fn unalias_missing_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("unalias", &["xyz".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn unalias_dash_a_clears_all() {
        let mut shell = Shell::new();
        shell.aliases.insert("ll".to_string(), "ls -l".to_string());
        shell.aliases.insert("la".to_string(), "ls -A".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("unalias", &["-a".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(shell.aliases.is_empty());
    }

    #[test]
    fn unalias_no_args_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("unalias", &[], &mut buf, &mut shell);
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
        let outcome = run_builtin("shift", &[], &mut buf, &mut shell);
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
        let outcome = run_builtin("shift", &["2".to_string()], &mut buf, &mut shell);
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
        let _ = run_builtin("shift", &[], &mut buf, &mut shell_a);
        let _ = run_builtin("shift", &["1".to_string()], &mut buf, &mut shell_b);

        assert_eq!(shell_a.positional_args, shell_b.positional_args);
        assert_eq!(shell_a.positional_args, vec!["y"]);
    }

    #[test]
    fn shift_too_large_errors_status_1() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("shift", &["5".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        // Positional unchanged after error.
        assert_eq!(shell.positional_args, vec!["a"]);
    }

    #[test]
    fn shift_zero_is_noop() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "b".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("shift", &["0".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.positional_args, vec!["a", "b"]);
    }

    #[test]
    fn shift_non_numeric_errors_status_1() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("shift", &["abc".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        assert_eq!(shell.positional_args, vec!["a"]);
    }

    #[test]
    fn shift_negative_errors_status_1() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "b".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        // `-1` fails parse::<usize>() because usize can't be negative.
        let outcome = run_builtin("shift", &["-1".to_string()], &mut buf, &mut shell);
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
        let outcome = run_builtin("set", &[], &mut buf, &mut shell);
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
        let outcome = run_builtin("set", &["--".to_string()], &mut buf, &mut shell);
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
            &mut buf,
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
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.positional_args, vec!["one", "two", "three"]);
    }

    #[test]
    fn set_dash_e_rejects_with_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("set", &["-e".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn set_plus_x_rejects_with_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("set", &["+x".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
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
        let outcome = run_builtin(".", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn source_missing_file_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            ".",
            &["/nonexistent/file/path/huck-v51-test".to_string()],
            &mut buf,
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
            &mut buf,
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
        let outcome = run_builtin(
            "local",
            &["X=hi".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn local_with_value_sets_and_records_snapshot() {
        let mut shell = Shell::new();
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "local",
            &["XYZ_LOCAL_T1=hi".to_string()],
            &mut buf,
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
    fn local_without_value_sets_empty() {
        let mut shell = Shell::new();
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "local",
            &["XYZ_LOCAL_T2".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("XYZ_LOCAL_T2").as_deref(), Some(""));
    }

    #[test]
    fn local_snapshots_existing_var() {
        let mut shell = Shell::new();
        shell.set("XYZ_LOCAL_T3", "outer".to_string());
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "local",
            &["XYZ_LOCAL_T3=inner".to_string()],
            &mut buf,
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
        assert_eq!(v.value, "outer");
    }

    #[test]
    fn local_idempotent_in_same_frame() {
        let mut shell = Shell::new();
        shell.set("XYZ_LOCAL_T4", "outer".to_string());
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        // First `local`: snapshot the outer value.
        let _ = run_builtin(
            "local",
            &["XYZ_LOCAL_T4=first".to_string()],
            &mut buf,
            &mut shell,
        );
        // Second `local` for the same name in the same frame: must NOT
        // re-snapshot (otherwise it would overwrite the outer snapshot
        // with "first").
        let _ = run_builtin(
            "local",
            &["XYZ_LOCAL_T4=second".to_string()],
            &mut buf,
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
        assert_eq!(v.value, "outer");
    }

    #[test]
    fn local_invalid_identifier_errors() {
        let mut shell = Shell::new();
        shell.local_scopes.push(std::collections::HashMap::new());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "local",
            &["1foo=bar".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
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
        let outcome = run_builtin(":", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn colon_with_args_exits_zero() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["one".to_string(), "two".to_string()];
        let outcome = run_builtin(":", &args, &mut buf, &mut shell);
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
        let outcome = run_builtin("true", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn false_exits_one() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("false", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn true_and_false_ignore_args() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["ignored".to_string()];
        let t = run_builtin("true", &args, &mut buf, &mut shell);
        let f = run_builtin("false", &args, &mut buf, &mut shell);
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
        let outcome = run_builtin("command", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn command_bare_form_errors() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["echo".to_string(), "hi".to_string()];
        let outcome = run_builtin("command", &args, &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn command_dash_v_builtin_concise() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["-v".to_string(), "echo".to_string()];
        let outcome = run_builtin("command", &args, &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out.trim_end(), "echo");
    }

    #[test]
    fn command_dash_v_notfound_silent_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["-v".to_string(), "__no_such_cmd_xyzzy__".to_string()];
        let outcome = run_builtin("command", &args, &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.is_empty(), "expected silent stdout, got: {out:?}");
    }

    #[test]
    fn command_dash_v_builtin_verbose() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["-V".to_string(), "echo".to_string()];
        let outcome = run_builtin("command", &args, &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out.trim_end(), "echo is a shell builtin");
    }

    #[test]
    fn command_dash_v_keyword_verbose() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["-V".to_string(), "if".to_string()];
        let outcome = run_builtin("command", &args, &mut buf, &mut shell);
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
            crate::command::SimpleCommand::Assign(vec![]),
        ));
        shell.functions.insert("myfn".to_string(), body);
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["-v".to_string(), "myfn".to_string()];
        let outcome = run_builtin("command", &args, &mut buf, &mut shell);
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
        let outcome = run_builtin("command", &args, &mut buf, &mut shell);
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
        let outcome = run_builtin("readonly", &args, &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X").as_deref(), Some("hi"));
        assert!(shell.is_readonly("X"));
    }

    #[test]
    fn readonly_no_value_creates_empty_and_locks() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["X".to_string()];
        let outcome = run_builtin("readonly", &args, &mut buf, &mut shell);
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
        let outcome = run_builtin("readonly", &args, &mut buf, &mut shell);
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
        let outcome = run_builtin("readonly", &args, &mut buf, &mut shell);
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
        let outcome = run_builtin("readonly", &args, &mut buf, &mut shell);
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
        let outcome = run_builtin("readonly", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        // Sorted; POSIX-escape format.
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines.contains(&"readonly X='v'"));
        assert!(lines.contains(&"readonly Y='w'"));
    }

    #[test]
    fn readonly_dash_p_same_as_no_args() {
        let mut shell = Shell::new();
        shell.set("X", "v".to_string());
        shell.mark_readonly("X");
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("readonly", &["-p".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.lines().any(|l| l == "readonly X='v'"));
    }

    #[test]
    fn readonly_overwrite_existing_readonly_errors() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        run_builtin("readonly", &["X=first".to_string()], &mut buf, &mut shell);
        let outcome = run_builtin(
            "readonly",
            &["X=second".to_string()],
            &mut buf,
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
        let outcome = run_builtin("unset", &["X".to_string()], &mut buf, &mut shell);
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
        let bad = run_builtin(
            "export",
            &["X=newval".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(bad, ExecOutcome::Continue(1)));
        assert_eq!(shell.lookup_var("X").as_deref(), Some("v"));
        // `export X` (bare) should succeed and flip the export flag.
        let bare = run_builtin("export", &["X".to_string()], &mut buf, &mut shell);
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
}
