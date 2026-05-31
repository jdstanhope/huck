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
pub fn expand_prompt(template: &str, shell: &Shell) -> String {
    let mut out = String::new();
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Fast path: copy any run of non-special bytes.
        let mut j = i;
        while j < bytes.len() && bytes[j] != b'\\' && bytes[j] != b'$' {
            j += 1;
        }
        if j > i {
            out.push_str(&template[i..j]);
            i = j;
            if i >= bytes.len() {
                break;
            }
        }

        if bytes[i] == b'\\' {
            // Escape sequence.
            if i + 1 >= bytes.len() {
                // Trailing backslash — keep literal.
                out.push('\\');
                i += 1;
                continue;
            }
            let next = bytes[i + 1];
            // \033 special-case (alias for \e).
            if next == b'0'
                && i + 3 < bytes.len()
                && bytes[i + 2] == b'3'
                && bytes[i + 3] == b'3'
            {
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
                b'!' => out.push_str(&next_history_number(shell).to_string()),
                b'#' => out.push_str(&next_history_number(shell).to_string()),
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
    // Find the NUL terminator.
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

fn cwd_tilde(shell: &Shell) -> String {
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
    if !home.is_empty() && cwd == home {
        return "~".to_string();
    }
    match cwd.rsplit_once('/') {
        Some((_, base)) if !base.is_empty() => base.to_string(),
        _ => cwd,
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
        let shell = Shell::new();
        assert_eq!(expand_prompt("hello ", &shell), "hello ");
    }

    #[test]
    fn expand_user() {
        let shell = Shell::new();
        let out = expand_prompt("\\u", &shell);
        assert!(!out.is_empty(), "\\u should resolve to something");
    }

    #[test]
    fn expand_hostname_short() {
        let shell = Shell::new();
        let out = expand_prompt("\\h", &shell);
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
        assert_eq!(expand_prompt("\\w", &shell), "~/x");
    }

    #[test]
    fn expand_cwd_basename() {
        let mut shell = Shell::new();
        shell.set("PWD", "/a/b/c".to_string());
        assert_eq!(expand_prompt("\\W", &shell), "c");
    }

    #[test]
    fn expand_dollar_user_vs_root() {
        let shell = Shell::new();
        let out = expand_prompt("\\$", &shell);
        let expected = if unsafe { libc::geteuid() } == 0 {
            "#"
        } else {
            "$"
        };
        assert_eq!(out, expected);
    }

    #[test]
    fn expand_n_r_backslash() {
        let shell = Shell::new();
        assert_eq!(expand_prompt("\\n", &shell), "\n");
        assert_eq!(expand_prompt("\\r", &shell), "\r");
        assert_eq!(expand_prompt("\\\\", &shell), "\\");
    }

    #[test]
    fn expand_status() {
        let mut shell = Shell::new();
        shell.set_last_status(42);
        assert_eq!(expand_prompt("\\?", &shell), "42");
    }

    #[test]
    fn expand_jobs_count_zero() {
        let shell = Shell::new();
        assert_eq!(expand_prompt("\\j", &shell), "0");
    }

    #[test]
    fn expand_escape_e_and_033() {
        let shell = Shell::new();
        assert_eq!(expand_prompt("\\e", &shell), "\x1B");
        assert_eq!(expand_prompt("\\033", &shell), "\x1B");
    }

    #[test]
    fn expand_bell() {
        let shell = Shell::new();
        assert_eq!(expand_prompt("\\a", &shell), "\x07");
    }

    #[test]
    fn expand_bracket_markers() {
        let shell = Shell::new();
        assert_eq!(expand_prompt("\\[X\\]", &shell), "\x01X\x02");
    }

    #[test]
    fn expand_dollar_var_with_braces() {
        let mut shell = Shell::new();
        shell.set("XYZ_PROMPT", "hi".to_string());
        assert_eq!(expand_prompt("${XYZ_PROMPT}", &shell), "hi");
    }

    #[test]
    fn expand_dollar_var_bare() {
        let mut shell = Shell::new();
        shell.set("XYZ_PROMPT", "hi".to_string());
        assert_eq!(expand_prompt("$XYZ_PROMPT ", &shell), "hi ");
    }

    #[test]
    fn expand_unknown_escape_preserved() {
        let shell = Shell::new();
        assert_eq!(expand_prompt("\\z", &shell), "\\z");
    }

    #[test]
    fn expand_undefined_var_empty() {
        let shell = Shell::new();
        assert_eq!(
            expand_prompt("${___DEFINITELY_UNSET_PROMPT___}>", &shell),
            ">"
        );
    }
}
