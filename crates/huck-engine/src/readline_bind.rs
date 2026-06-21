//! Readline key-sequence and function-name data for the `bind` builtin.
//!
//! The rustyline-coupled mappers (`parse_keyseq` -> `rustyline::Event`,
//! `function_to_cmd` -> `rustyline::Cmd`) live in `huck-cli`
//! (`readline_apply`). This module keeps the rustyline-free data the `bind`
//! builtin + `Shell` need: the default emacs binds, the function-name list,
//! and the pure `bool` validators (`is_known_function`, `keyseq_is_valid`).
//! `keyseq_is_valid` mirrors the cli `parse_keyseq` control flow exactly,
//! emitting `Some(())` where the cli builds a `rustyline::Event` — so the
//! accept/reject decision is byte-identical, without naming a rustyline type.

/// huck's default emacs key bindings — the standard emacs keys rustyline honors
/// for huck's supported functions, in bash's `bind -p` keyseq spelling. Each
/// entry is verified to appear in bash's own default `bind -p` (the harness
/// enforces this subset relation), so huck never reports a binding bash lacks.
/// Functions in the honored set with no entry here render as `# … (not bound)`.
pub const DEFAULT_EMACS_BINDS: &[(&str, &str)] = &[
    ("\\C-a", "beginning-of-line"), ("\\C-e", "end-of-line"),
    ("\\C-f", "forward-char"), ("\\C-b", "backward-char"),
    ("\\ef", "forward-word"), ("\\eb", "backward-word"),
    ("\\C-k", "kill-line"), ("\\C-u", "unix-line-discard"),
    ("\\C-w", "unix-word-rubout"), ("\\ed", "kill-word"),
    ("\\e\\C-?", "backward-kill-word"),
    ("\\C-l", "clear-screen"), ("\\C-g", "abort"),
    ("\\C-j", "accept-line"), ("\\C-m", "accept-line"),
    ("\\C-p", "previous-history"), ("\\C-n", "next-history"),
    ("\\e<", "beginning-of-history"), ("\\e>", "end-of-history"),
    ("\\C-r", "reverse-search-history"), ("\\C-s", "forward-search-history"),
    ("\\C-i", "complete"),
    ("\\eu", "upcase-word"), ("\\el", "downcase-word"),
    ("\\ec", "capitalize-word"), ("\\C-t", "transpose-chars"),
    ("\\et", "transpose-words"), ("\\C-_", "undo"),
    ("\\C-y", "yank"), ("\\C-d", "delete-char"),
    ("\\C-?", "backward-delete-char"),
];

/// Pure validity mirror of the cli `parse_keyseq`: returns `Some(())` exactly
/// where `parse_keyseq` returns `Some(event)`, and `None` exactly where it
/// returns `None`. Used by `keyseq_is_valid` so the engine validates key
/// sequences without depending on rustyline.
fn parse_keyseq_valid(seq: &str) -> Option<()> {
    let s = seq
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(seq);
    if s.is_empty() {
        return None;
    }
    // whole well-known escape sequences -> named keys
    match s {
        "\\e[A" | "\\eOA" => return Some(()),
        "\\e[B" | "\\eOB" => return Some(()),
        "\\e[C" | "\\eOC" => return Some(()),
        "\\e[D" | "\\eOD" => return Some(()),
        "\\e[H" | "\\eOH" => return Some(()),
        "\\e[F" | "\\eOF" => return Some(()),
        _ => {}
    }
    let mut rest = s;
    loop {
        if let Some(r) = rest.strip_prefix("\\C-") {
            rest = r;
        } else if let Some(r) = rest.strip_prefix("\\M-") {
            rest = r;
        } else {
            break;
        }
    }
    let _ch = match rest {
        "" => return None,
        "\\e" => '\x1b',
        "\\t" => '\t',
        "\\r" => '\r',
        "\\n" => '\n',
        "\\b" => '\x08',
        _ if rest.starts_with("\\x") => {
            let h = &rest[2..];
            if h.len() != 2 {
                return None;
            }
            char::from_u32(u32::from_str_radix(h, 16).ok()?)?
        }
        _ if rest.starts_with('\\') && rest.len() > 1 => {
            let body = &rest[1..];
            if body.chars().all(|c| c.is_digit(8)) {
                char::from_u32(u32::from_str_radix(body, 8).ok()?)?
            } else {
                let mut cs = body.chars();
                let c = cs.next()?;
                if cs.next().is_some() {
                    return None;
                }
                c
            }
        }
        _ => {
            let mut cs = rest.chars();
            let c = cs.next()?;
            if cs.next().is_some() {
                return None;
            }
            c
        }
    };
    Some(())
}

/// Whether `name` is a readline function that huck knows how to bind.
/// Equivalent to (and the rustyline-free counterpart of) the cli
/// `function_to_cmd(name).is_some()`: the set of bindable function names is
/// exactly `readline_function_names()`.
pub fn is_known_function(name: &str) -> bool {
    readline_function_names().contains(&name)
}

/// Whether `seq` parses as a bindable key sequence.
pub fn keyseq_is_valid(seq: &str) -> bool {
    parse_keyseq_valid(seq).is_some()
}

/// The static list of readline function names for `bind -l`.
pub fn readline_function_names() -> &'static [&'static str] {
    &[
        "abort",
        "accept-line",
        "backward-char",
        "backward-delete-char",
        "backward-kill-line",
        "backward-kill-word",
        "backward-word",
        "beginning-of-history",
        "beginning-of-line",
        "capitalize-word",
        "clear-screen",
        "complete",
        "delete-char",
        "downcase-word",
        "end-of-history",
        "end-of-line",
        "forward-char",
        "forward-search-history",
        "forward-word",
        "history-search-backward",
        "history-search-forward",
        "kill-line",
        "kill-word",
        "next-history",
        "previous-history",
        "reverse-search-history",
        "transpose-chars",
        "transpose-words",
        "undo",
        "unix-line-discard",
        "unix-word-rubout",
        "upcase-word",
        "yank",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyseq_validity_matches_basic_cases() {
        assert!(keyseq_is_valid("\\C-a"));
        assert!(keyseq_is_valid("\"\\C-a\""));
        assert!(keyseq_is_valid("\\M-f"));
        assert!(keyseq_is_valid("\\e[A"));
        assert!(keyseq_is_valid("a"));
        assert!(!keyseq_is_valid("\\C-"));
        assert!(!keyseq_is_valid(""));
    }

    #[test]
    fn function_map_and_names() {
        assert!(is_known_function("beginning-of-line"));
        assert!(is_known_function("kill-line"));
        assert!(is_known_function("accept-line"));
        assert!(!is_known_function("no-such-function"));
        assert!(is_known_function("clear-screen"));
        assert!(!is_known_function("totally-bogus"));
        assert!(readline_function_names().contains(&"accept-line"));
    }

    #[test]
    fn default_emacs_binds_only_reference_honored_functions() {
        assert!(!DEFAULT_EMACS_BINDS.is_empty());
        for (seq, func) in DEFAULT_EMACS_BINDS {
            assert!(is_known_function(func), "default binds a function huck can't honor: {func}");
            assert!(!seq.is_empty());
        }
    }
}
