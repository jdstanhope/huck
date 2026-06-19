//! Readline key-sequence and function-name parsing for the `bind` builtin.
//!
//! This is the ONLY rustyline-coupled module added in v161. It maps readline
//! key-sequence notation (e.g. `"\C-x"`, `"\M-f"`, `"\e[A"`) into a rustyline
//! [`Event`], and readline function names (e.g. `beginning-of-line`) into
//! rustyline [`Cmd`]s. Everything here is pure and unit-tested; the `bind`
//! builtin (Task 3) consumes only the `bool` validators so it stays
//! rustyline-free.

use rustyline::{Anchor, At, Cmd, KeyCode, KeyEvent, Modifiers, Movement, Word};
use rustyline::Event;

/// Parses a readline key-sequence string into a rustyline `Event`.
/// Handles: optional surrounding double-quotes; `\C-x` (control),
/// `\M-x`/`\e` (meta/escape), well-known escape sequences (`\e[A`/`\e[B`/
/// `\e[C`/`\e[D` arrows, `\e[H`/`\e[F` home/end), `\t`/`\r`/`\n`/`\b`,
/// octal `\nnn`, hex `\xHH`, and literal characters. Returns `None` on an
/// unparseable/unsupported sequence (conservative — never mis-bind).
pub fn parse_keyseq(seq: &str) -> Option<Event> {
    let s = seq
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(seq);
    if s.is_empty() {
        return None;
    }
    // whole well-known escape sequences -> named keys
    match s {
        "\\e[A" | "\\eOA" => return Some(KeyEvent(KeyCode::Up, Modifiers::NONE).into()),
        "\\e[B" | "\\eOB" => return Some(KeyEvent(KeyCode::Down, Modifiers::NONE).into()),
        "\\e[C" | "\\eOC" => return Some(KeyEvent(KeyCode::Right, Modifiers::NONE).into()),
        "\\e[D" | "\\eOD" => return Some(KeyEvent(KeyCode::Left, Modifiers::NONE).into()),
        "\\e[H" | "\\eOH" => return Some(KeyEvent(KeyCode::Home, Modifiers::NONE).into()),
        "\\e[F" | "\\eOF" => return Some(KeyEvent(KeyCode::End, Modifiers::NONE).into()),
        _ => {}
    }
    let mut mods = Modifiers::NONE;
    let mut rest = s;
    loop {
        if let Some(r) = rest.strip_prefix("\\C-") {
            mods |= Modifiers::CTRL;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("\\M-") {
            mods |= Modifiers::ALT;
            rest = r;
        } else {
            break;
        }
    }
    let ch = match rest {
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
    // Normalize so the produced KeyEvent matches what rustyline delivers when
    // the key is pressed: e.g. `\C-a` -> Char('A')+CTRL (rustyline upper-cases
    // control letters and folds control chars into named keys). Without this,
    // bind_sequence would register an event the terminal never produces.
    // Readline treats \C-i / \C-m / \C-h (and \t / \r / \b / DEL) as the named
    // Tab / Enter / Backspace keys; the terminal delivers them that way, so a
    // binding must register the named KeyCode (not Char+CTRL) or it never fires.
    let is_ctrl = mods.contains(Modifiers::CTRL);
    if matches!(ch, '\t') || (is_ctrl && matches!(ch, 'i' | 'I')) {
        return Some(KeyEvent(KeyCode::Tab, Modifiers::NONE).into());
    }
    if matches!(ch, '\r' | '\n') || (is_ctrl && matches!(ch, 'm' | 'M')) {
        return Some(KeyEvent(KeyCode::Enter, Modifiers::NONE).into());
    }
    if matches!(ch, '\x08' | '\x7f') || (is_ctrl && matches!(ch, 'h' | 'H')) {
        return Some(KeyEvent(KeyCode::Backspace, Modifiers::NONE).into());
    }
    Some(KeyEvent::normalize(KeyEvent::new(ch, mods)).into())
}

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

/// Maps a readline function name to the rustyline `Cmd` that implements it.
/// Returns `None` for unknown or unsupported function names.
pub fn function_to_cmd(name: &str) -> Option<Cmd> {
    Some(match name {
        "beginning-of-line" => Cmd::Move(Movement::BeginningOfLine),
        "end-of-line" => Cmd::Move(Movement::EndOfLine),
        "forward-char" => Cmd::Move(Movement::ForwardChar(1)),
        "backward-char" => Cmd::Move(Movement::BackwardChar(1)),
        "forward-word" => Cmd::Move(Movement::ForwardWord(1, At::AfterEnd, Word::Emacs)),
        "backward-word" => Cmd::Move(Movement::BackwardWord(1, Word::Emacs)),
        "kill-line" => Cmd::Kill(Movement::EndOfLine),
        "backward-kill-line" => Cmd::Kill(Movement::BeginningOfLine),
        "unix-line-discard" => Cmd::Kill(Movement::BeginningOfLine),
        "kill-word" => Cmd::Kill(Movement::ForwardWord(1, At::AfterEnd, Word::Emacs)),
        "backward-kill-word" => Cmd::Kill(Movement::BackwardWord(1, Word::Emacs)),
        "unix-word-rubout" => Cmd::Kill(Movement::BackwardWord(1, Word::Big)),
        "clear-screen" => Cmd::ClearScreen,
        "accept-line" => Cmd::AcceptLine,
        "previous-history" => Cmd::PreviousHistory,
        "next-history" => Cmd::NextHistory,
        "beginning-of-history" => Cmd::BeginningOfHistory,
        "end-of-history" => Cmd::EndOfHistory,
        "history-search-backward" => Cmd::HistorySearchBackward,
        "history-search-forward" => Cmd::HistorySearchForward,
        "reverse-search-history" => Cmd::ReverseSearchHistory,
        "forward-search-history" => Cmd::ForwardSearchHistory,
        "complete" => Cmd::Complete,
        "upcase-word" => Cmd::UpcaseWord,
        "downcase-word" => Cmd::DowncaseWord,
        "capitalize-word" => Cmd::CapitalizeWord,
        "transpose-chars" => Cmd::TransposeChars,
        "transpose-words" => Cmd::TransposeWords(1),
        "undo" => Cmd::Undo(1),
        "yank" => Cmd::Yank(1, Anchor::Before),
        "delete-char" => Cmd::Kill(Movement::ForwardChar(1)),
        "backward-delete-char" => Cmd::Kill(Movement::BackwardChar(1)),
        "abort" => Cmd::Abort,
        _ => return None,
    })
}

/// Whether `name` is a readline function that huck knows how to bind.
pub fn is_known_function(name: &str) -> bool {
    function_to_cmd(name).is_some()
}

/// Whether `seq` parses as a bindable key sequence.
pub fn keyseq_is_valid(seq: &str) -> bool {
    parse_keyseq(seq).is_some()
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
    fn parse_basic_keyseqs() {
        assert!(parse_keyseq("\\C-a").is_some());
        assert!(parse_keyseq("\"\\C-a\"").is_some());
        assert!(parse_keyseq("\\M-f").is_some());
        assert!(parse_keyseq("\\e[A").is_some());
        assert!(parse_keyseq("a").is_some());
        assert!(parse_keyseq("\\C-").is_none());
        assert!(parse_keyseq("").is_none());
    }

    #[test]
    fn parse_keyseq_produces_correct_events() {
        use rustyline::{KeyCode, KeyEvent, Modifiers};
        assert_eq!(parse_keyseq("\\C-w"), Some(KeyEvent(KeyCode::Char('W'), Modifiers::CTRL).into()));
        assert_eq!(parse_keyseq("\\C-i"), Some(KeyEvent(KeyCode::Tab, Modifiers::NONE).into()));
        assert_eq!(parse_keyseq("\\C-m"), Some(KeyEvent(KeyCode::Enter, Modifiers::NONE).into()));
        assert_eq!(parse_keyseq("\\C-h"), Some(KeyEvent(KeyCode::Backspace, Modifiers::NONE).into()));
        assert_eq!(parse_keyseq("\\e[A"), Some(KeyEvent(KeyCode::Up, Modifiers::NONE).into()));
        assert_eq!(parse_keyseq("\\M-f"), Some(KeyEvent(KeyCode::Char('f'), Modifiers::ALT).into()));
    }

    #[test]
    fn function_map_and_names() {
        assert!(function_to_cmd("beginning-of-line").is_some());
        assert!(function_to_cmd("kill-line").is_some());
        assert!(function_to_cmd("accept-line").is_some());
        assert!(function_to_cmd("no-such-function").is_none());
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
