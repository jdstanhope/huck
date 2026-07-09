// src/prompt.rs
//
// PS1/PS2 prompt-template expansion. Tier A escapes + $VAR
// interpolation. See docs/superpowers/specs/2026-05-31-huck-ps1-design.md.

use crate::shell_state::Shell;

/// Expands a bash-style prompt template into the byte string
/// passed to rustyline. Handles the Tier-A escape set:
/// \u \h \H \w \W \$ \n \r \\ \? \j \! \# \e \033 \a \[ \] and
/// $VAR / ${VAR} interpolation. Unknown \X passes through
/// literally.
pub fn expand_prompt(template: &str, shell: &mut Shell) -> String {
    let mut out = String::new();
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Fast path: copy any run of non-special bytes.
        let mut j = i;
        while j < bytes.len() && bytes[j] != b'\\' && bytes[j] != b'$' && bytes[j] != b'`' {
            j += 1;
        }
        if j > i {
            out.push_str(&template[i..j]);
            i = j;
            if i >= bytes.len() {
                break;
            }
        }

        if bytes[i] == b'`' {
            // Backtick command substitution.
            match scan_backtick_close(bytes, i + 1) {
                Some(close) => {
                    let body = template[i + 1..close].to_string();
                    out.push_str(&run_prompt_cmdsub(&body, shell));
                    i = close + 1;
                }
                None => {
                    out.push_str(&template[i..]);
                    break;
                }
            }
        } else if bytes[i] == b'\\' {
            // Escape sequence.
            if i + 1 >= bytes.len() {
                // Trailing backslash — keep literal.
                out.push('\\');
                i += 1;
                continue;
            }
            let next = bytes[i + 1];
            // \033 special-case (alias for \e).
            if next == b'0' && i + 3 < bytes.len() && bytes[i + 2] == b'3' && bytes[i + 3] == b'3' {
                out.push('\x1B');
                i += 4;
                continue;
            }
            match next {
                b'u' => out.push_str(&user()),
                b'h' => out.push_str(&host_short()),
                b'H' => out.push_str(&host_full()),
                b'w' => out.push_str(&cwd_tilde(shell)),
                b'W' => out.push_str(&cwd_basename(shell)),
                b'$' => out.push(if is_root() { '#' } else { '$' }),
                b'n' => out.push('\n'),
                b'r' => out.push('\r'),
                b'\\' => out.push('\\'),
                b'?' => out.push_str(&shell.last_status().to_string()),
                b'j' => out.push_str(&shell.jobs.iter().count().to_string()),
                b'!' | b'#' => out.push_str(&next_history_number(shell).to_string()),
                b'e' => out.push('\x1B'),
                b'a' => out.push('\x07'),
                b'[' => out.push('\x01'),
                b']' => out.push('\x02'),
                other => {
                    // Unknown: preserve backslash + char.
                    out.push('\\');
                    out.push(other as char);
                }
            }
            i += 2;
        } else {
            // bytes[i] == b'$' — variable interpolation.
            if i + 1 >= bytes.len() {
                out.push('$');
                i += 1;
                continue;
            }
            // $((...)) arithmetic.
            if bytes[i + 1] == b'(' && i + 2 < bytes.len() && bytes[i + 2] == b'(' {
                match scan_arith_close(bytes, i + 3) {
                    Some((body_end, next_i)) => {
                        let body = &template[i + 3..body_end];
                        if let Ok(expr) = crate::arith::parse(body)
                            && let Ok(n) = crate::arith::eval(&expr, shell)
                        {
                            out.push_str(&n.to_string());
                        }
                        i = next_i;
                        continue;
                    }
                    None => {
                        out.push_str(&template[i..]);
                        break;
                    }
                }
            }
            // $(...) command substitution.
            if bytes[i + 1] == b'(' {
                match scan_cmdsub_close(bytes, i + 2) {
                    Some((body_end, next_i)) => {
                        let body = template[i + 2..body_end].to_string();
                        out.push_str(&run_prompt_cmdsub(&body, shell));
                        i = next_i;
                        continue;
                    }
                    None => {
                        out.push_str(&template[i..]);
                        break;
                    }
                }
            }
            if bytes[i + 1] == b'{' {
                // ${NAME}
                let start = i + 2;
                let mut end = start;
                while end < bytes.len() && bytes[end] != b'}' {
                    end += 1;
                }
                if end >= bytes.len() {
                    // Unterminated; emit literal "${...".
                    out.push_str(&template[i..]);
                    break;
                }
                let name = &template[start..end];
                if !name.is_empty() {
                    if let Some(v) = shell.lookup_var(name) {
                        out.push_str(&v);
                    }
                } else {
                    out.push_str("${}");
                }
                i = end + 1;
            } else if bytes[i + 1] == b'_' || bytes[i + 1].is_ascii_alphabetic() {
                // $NAME
                let start = i + 1;
                let mut end = start;
                while end < bytes.len()
                    && (bytes[end] == b'_' || bytes[end].is_ascii_alphanumeric())
                {
                    end += 1;
                }
                let name = &template[start..end];
                if let Some(v) = shell.lookup_var(name) {
                    out.push_str(&v);
                }
                i = end;
            } else {
                // $ followed by non-identifier; pass through.
                out.push('$');
                i += 1;
            }
        }
    }
    out
}

/// Finds the matching `))` for a `$((…))` whose `$((` ends at `start`.
/// Returns `(body_end_exclusive, next_index_after_closing)`. Mirrors the
/// lexer's `scan_arith_block` close rule (a `)` at depth 0 followed by `)`).
fn scan_arith_close(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    let mut k = start;
    let mut depth: i32 = 0;
    while k < bytes.len() {
        match bytes[k] {
            b'(' => depth += 1,
            b')' => {
                if depth == 0 && k + 1 < bytes.len() && bytes[k + 1] == b')' {
                    return Some((k, k + 2));
                }
                depth -= 1;
            }
            _ => {}
        }
        k += 1;
    }
    None
}

/// Finds the matching `)` for a `$(…)` whose `$(` ends at `start` (depth starts
/// at 1). Quote-aware so a `)` inside `'…'`/`"…"` does not close early.
/// Returns `(body_end_exclusive, next_index_after_close)`.
fn scan_cmdsub_close(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    let mut k = start;
    let mut depth: i32 = 1;
    let mut in_single = false;
    let mut in_double = false;
    while k < bytes.len() {
        let b = bytes[k];
        if in_single {
            if b == b'\'' {
                in_single = false;
            }
        } else if in_double {
            if b == b'"' {
                in_double = false;
            }
        } else {
            match b {
                b'\'' => in_single = true,
                b'"' => in_double = true,
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some((k, k + 1));
                    }
                }
                _ => {}
            }
        }
        k += 1;
    }
    None
}

/// Finds the next unescaped backtick after `start`. Returns its index.
fn scan_backtick_close(bytes: &[u8], start: usize) -> Option<usize> {
    let mut k = start;
    while k < bytes.len() {
        match bytes[k] {
            b'\\' => k += 2,
            b'`' => return Some(k),
            _ => k += 1,
        }
    }
    None
}

/// Parses `body` as a command and runs it as a command substitution, returning
/// its output (trailing newlines already stripped by `run_substitution`). On a
/// lex/parse error returns an empty string.
fn run_prompt_cmdsub(body: &str, shell: &mut Shell) -> String {
    let raw = match crate::parser::parse_sequence(&mut crate::lexer::Lexer::new(
        body,
        &Default::default(),
        crate::lexer::LexerOptions::default(),
    )) {
        Ok(Some(seq)) => crate::expand::run_substitution(&seq, shell),
        _ => String::new(),
    };
    convert_prompt_markers(&raw)
}

/// Converts readline non-printing markers `\[`/`\]` emitted INSIDE a prompt
/// command substitution (e.g. by oh-my-posh's `$(_omp_get_primary)`) into
/// rustyline's zero-width delimiters `\x01`/`\x02`, so the wrapped ANSI escape
/// bytes are excluded from the prompt-width calculation. Without this, the line
/// editor counts the ANSI bytes as visible columns and pushes the cursor right.
/// Matches bash, which honors `\[ \]` from a cmdsub-expanded PS1. Other
/// backslash escapes in the output are left untouched (bash does not re-decode
/// `\w`/`\h`/etc. in a cmdsub-expanded prompt).
fn convert_prompt_markers(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    out.push('\x01');
                    continue;
                }
                Some(']') => {
                    chars.next();
                    out.push('\x02');
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
    }
    out
}

/// The "raw" (visible-only) form of an expanded prompt: everything inside a
/// `\x01`…`\x02` non-printing span (and the marker chars themselves) is removed,
/// leaving just the text/glyphs the terminal actually renders.
///
/// rustyline 18 measures prompt width from the `Prompt::raw()` string and ignores
/// the `\x01`/`\x02` markers, so a prompt whose colors / OSC sequences are wrapped
/// in markers (e.g. oh-my-posh) is over-measured (it counts OSC-8 hyperlink URLs and
/// the OSC-0 window title as visible). Passing `(prompt_raw(styled), styled)` to
/// `readline` makes rustyline measure the visible width while still displaying the
/// full styled prompt (B-01). An unbalanced `\x01` (no closing `\x02`) skips to the
/// end; a stray `\x02` is dropped. A prompt with no markers is returned unchanged.
pub fn prompt_raw(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut skip = false;
    for c in s.chars() {
        match c {
            '\x01' => skip = true,
            '\x02' => skip = false,
            _ if skip => {}
            _ => out.push(c),
        }
    }
    out
}

// ── Per-escape helpers ─────────────────────────────────────────

fn user() -> String {
    if let Ok(u) = std::env::var("USER") {
        return u;
    }
    // Fallback: getpwuid(getuid()).
    unsafe {
        let uid = libc::getuid();
        let pw = libc::getpwuid(uid);
        if !pw.is_null() {
            let name = std::ffi::CStr::from_ptr((*pw).pw_name);
            if let Ok(s) = name.to_str() {
                return s.to_string();
            }
        }
    }
    String::new()
}

fn host_full() -> String {
    let mut buf = [0u8; 256];
    unsafe {
        let rc = libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len());
        if rc != 0 {
            return String::new();
        }
    }
    // Force a sentinel at the last byte in case the OS filled the
    // buffer without a NUL terminator (POSIX allows that when the
    // hostname is longer than buf.len()).
    buf[255] = 0;
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..len]).into_owned()
}

fn host_short() -> String {
    let full = host_full();
    match full.find('.') {
        Some(idx) => full[..idx].to_string(),
        None => full,
    }
}

/// Returns `(cwd, home)` by checking the shell's `PWD`/`HOME` variables,
/// falling back to the OS environment. Both values default to an empty string
/// if unset.
fn resolve_cwd_home(shell: &Shell) -> (String, String) {
    let cwd = shell
        .lookup_var("PWD")
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.display().to_string())
        })
        .unwrap_or_default();
    let home = shell
        .lookup_var("HOME")
        .or_else(|| std::env::var("HOME").ok())
        .unwrap_or_default();
    (cwd, home)
}

fn cwd_tilde(shell: &Shell) -> String {
    let (cwd, home) = resolve_cwd_home(shell);
    if !home.is_empty() && cwd == home {
        return "~".to_string();
    }
    if !home.is_empty() {
        let prefix = format!("{home}/");
        if let Some(rest) = cwd.strip_prefix(&prefix) {
            return format!("~/{rest}");
        }
    }
    cwd
}

fn cwd_basename(shell: &Shell) -> String {
    let (cwd, home) = resolve_cwd_home(shell);
    if !home.is_empty() && cwd == home {
        return "~".to_string();
    }
    // Strip trailing slashes (except the root `/` itself) so
    // `PWD=/foo/` produces `foo`, matching bash.
    let trimmed: &str = if cwd.len() > 1 && cwd.ends_with('/') {
        cwd.trim_end_matches('/')
    } else {
        &cwd
    };
    match trimmed.rsplit_once('/') {
        Some((_, base)) if !base.is_empty() => base.to_string(),
        _ => trimmed.to_string(),
    }
}

fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

fn next_history_number(shell: &Shell) -> usize {
    shell.history.last_number().map(|n| n + 1).unwrap_or(1)
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn literal_text_passes_through() {
        let mut shell = Shell::new();
        assert_eq!(expand_prompt("hello ", &mut shell), "hello ");
    }

    #[test]
    fn prompt_raw_strips_marker_spans() {
        // \x01 <ANSI> \x02 X \x01 <ANSI> \x02  ->  "X"
        assert_eq!(prompt_raw("\x01\x1b[31m\x02X\x01\x1b[0m\x02"), "X");
    }

    #[test]
    fn prompt_raw_strips_osc_inside_markers() {
        // An OSC-8 hyperlink and OSC-0 title wrapped in markers vanish; the
        // visible glyph between them survives.
        let styled = "\x01\x1b]0;title\x07\x02G\x01\x1b]8;;http://x\x1b\\\x02";
        assert_eq!(prompt_raw(styled), "G");
    }

    #[test]
    fn prompt_raw_keeps_marker_free_prompt() {
        assert_eq!(prompt_raw("huck> "), "huck> ");
        // Visible powerline glyph + text outside any markers is kept verbatim.
        assert_eq!(prompt_raw("\u{e0b6} john "), "\u{e0b6} john ");
    }

    #[test]
    fn prompt_raw_unbalanced_start_skips_to_end() {
        assert_eq!(prompt_raw("ab\x01rest with no close"), "ab");
    }

    #[test]
    fn prompt_raw_bare_end_marker_dropped() {
        assert_eq!(prompt_raw("a\x02b"), "ab");
    }

    #[test]
    fn prompt_raw_output_has_no_control_or_escape() {
        let styled = "\x01\x1b[38;2;1;2;3m\x02\u{e0b0} john \x01\x1b]8;;u\x1b\\\x02";
        let raw = prompt_raw(styled);
        assert!(!raw.contains('\x01') && !raw.contains('\x02'));
        assert!(!raw.contains('\x1b'));
        assert_eq!(raw, "\u{e0b0} john ");
    }

    #[test]
    fn expand_user() {
        let mut shell = Shell::new();
        let out = expand_prompt("\\u", &mut shell);
        assert!(!out.is_empty(), "\\u should resolve to something");
    }

    #[test]
    fn expand_hostname_short() {
        let mut shell = Shell::new();
        let out = expand_prompt("\\h", &mut shell);
        assert!(
            !out.contains('.'),
            "short hostname must not contain '.': {out:?}"
        );
    }

    #[test]
    fn expand_cwd_with_home_collapse() {
        let mut shell = Shell::new();
        shell.set("HOME", "/h/me".to_string());
        shell.set("PWD", "/h/me/x".to_string());
        assert_eq!(expand_prompt("\\w", &mut shell), "~/x");
    }

    #[test]
    fn expand_cwd_basename() {
        let mut shell = Shell::new();
        shell.set("PWD", "/a/b/c".to_string());
        assert_eq!(expand_prompt("\\W", &mut shell), "c");
    }

    #[test]
    fn expand_cwd_basename_trailing_slash() {
        // Regression: bash strips trailing slashes from PWD before
        // taking the basename. `PWD=/foo/` → `\W` = `foo`.
        let mut shell = Shell::new();
        shell.set("PWD", "/foo/".to_string());
        assert_eq!(expand_prompt("\\W", &mut shell), "foo");
    }

    #[test]
    fn expand_cwd_basename_root_is_slash() {
        // PWD=/ → \W is `/` (the root itself; no basename).
        let mut shell = Shell::new();
        shell.set("PWD", "/".to_string());
        assert_eq!(expand_prompt("\\W", &mut shell), "/");
    }

    #[test]
    fn expand_dollar_user_vs_root() {
        let mut shell = Shell::new();
        let out = expand_prompt("\\$", &mut shell);
        let expected = if unsafe { libc::geteuid() } == 0 {
            "#"
        } else {
            "$"
        };
        assert_eq!(out, expected);
    }

    #[test]
    fn expand_n_r_backslash() {
        let mut shell = Shell::new();
        assert_eq!(expand_prompt("\\n", &mut shell), "\n");
        assert_eq!(expand_prompt("\\r", &mut shell), "\r");
        assert_eq!(expand_prompt("\\\\", &mut shell), "\\");
    }

    #[test]
    fn expand_status() {
        let mut shell = Shell::new();
        shell.set_last_status(42);
        assert_eq!(expand_prompt("\\?", &mut shell), "42");
    }

    #[test]
    fn expand_jobs_count_zero() {
        let mut shell = Shell::new();
        assert_eq!(expand_prompt("\\j", &mut shell), "0");
    }

    #[test]
    fn expand_escape_e_and_033() {
        let mut shell = Shell::new();
        assert_eq!(expand_prompt("\\e", &mut shell), "\x1B");
        assert_eq!(expand_prompt("\\033", &mut shell), "\x1B");
    }

    #[test]
    fn expand_bell() {
        let mut shell = Shell::new();
        assert_eq!(expand_prompt("\\a", &mut shell), "\x07");
    }

    #[test]
    fn expand_bracket_markers() {
        let mut shell = Shell::new();
        assert_eq!(expand_prompt("\\[X\\]", &mut shell), "\x01X\x02");
    }

    #[test]
    fn expand_dollar_var_with_braces() {
        let mut shell = Shell::new();
        shell.set("XYZ_PROMPT", "hi".to_string());
        assert_eq!(expand_prompt("${XYZ_PROMPT}", &mut shell), "hi");
    }

    #[test]
    fn expand_dollar_var_bare() {
        let mut shell = Shell::new();
        shell.set("XYZ_PROMPT", "hi".to_string());
        assert_eq!(expand_prompt("$XYZ_PROMPT ", &mut shell), "hi ");
    }

    #[test]
    fn expand_unknown_escape_preserved() {
        let mut shell = Shell::new();
        assert_eq!(expand_prompt("\\z", &mut shell), "\\z");
    }

    #[test]
    fn expand_undefined_var_empty() {
        let mut shell = Shell::new();
        assert_eq!(
            expand_prompt("${___DEFINITELY_UNSET_PROMPT___}>", &mut shell),
            ">"
        );
    }

    #[test]
    fn expand_arith_simple() {
        let mut shell = Shell::new();
        assert_eq!(expand_prompt("$((40+2))", &mut shell), "42");
    }
    #[test]
    fn expand_arith_nested_parens() {
        let mut shell = Shell::new();
        assert_eq!(expand_prompt("[$(( (1+2)*3 ))]", &mut shell), "[9]");
    }
    #[test]
    fn expand_arith_unterminated_is_literal() {
        let mut shell = Shell::new();
        assert_eq!(expand_prompt("$((1+2", &mut shell), "$((1+2");
    }

    #[test]
    fn expand_cmdsub_simple() {
        let mut shell = Shell::new();
        assert_eq!(expand_prompt("$(echo hi)", &mut shell), "hi");
    }
    #[test]
    fn expand_cmdsub_nested_and_mixed() {
        let mut shell = Shell::new();
        assert_eq!(expand_prompt("a$(echo $(echo x))b", &mut shell), "axb");
    }
    #[test]
    fn expand_cmdsub_strips_trailing_newlines() {
        let mut shell = Shell::new();
        assert_eq!(expand_prompt("$(printf 'a\\n\\n')", &mut shell), "a");
    }
    #[test]
    fn expand_cmdsub_paren_in_quotes() {
        let mut shell = Shell::new();
        assert_eq!(expand_prompt("$(echo \")\")", &mut shell), ")");
    }
    #[test]
    fn expand_backtick() {
        let mut shell = Shell::new();
        assert_eq!(expand_prompt("`echo y`", &mut shell), "y");
    }
    #[test]
    fn expand_cmdsub_unterminated_is_literal() {
        let mut shell = Shell::new();
        assert_eq!(expand_prompt("$(echo", &mut shell), "$(echo");
    }
    #[test]
    fn expand_cmdsub_passes_ansi_and_markers_through() {
        // \[ \] -> \x01 \x02, ANSI escape literal, cmdsub in the middle.
        let mut shell = Shell::new();
        assert_eq!(
            expand_prompt("\\[\\e[31m\\]$(echo R)\\[\\e[0m\\]", &mut shell),
            "\x01\x1b[31m\x02R\x01\x1b[0m\x02"
        );
    }

    #[test]
    fn cmdsub_output_readline_markers_converted() {
        // The oh-my-posh case: `\[ \]` markers come OUT of the prompt cmdsub and
        // must convert to \x01/\x02 so the wrapped ANSI is excluded from width.
        let mut shell = Shell::new();
        let out = expand_prompt("$(printf '%s' '\\[X\\]')", &mut shell);
        assert_eq!(out, "\x01X\x02", "{out:?}");
    }

    #[test]
    fn cmdsub_output_markers_with_ansi_excluded_from_width() {
        // `\[<ESC>...m\]john\[<ESC>0m\]` from a cmdsub -> markers around ANSI.
        let mut shell = Shell::new();
        let out = expand_prompt("$(printf '%s' '\\[\\e[31m\\]john\\[\\e[0m\\]')", &mut shell);
        // printf '%s' keeps the literal text; only the readline \[ \] convert.
        assert_eq!(out, "\x01\\e[31m\x02john\x01\\e[0m\x02", "{out:?}");
    }
}
