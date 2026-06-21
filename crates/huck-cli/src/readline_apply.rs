//! rustyline-coupled readline mappers for the `bind` builtin's apply path.
//!
//! Maps readline key-sequence notation (e.g. `"\C-x"`, `"\M-f"`, `"\e[A"`)
//! into a rustyline [`Event`], and readline function names (e.g.
//! `beginning-of-line`) into rustyline [`Cmd`]s. The rustyline-free data
//! (`DEFAULT_EMACS_BINDS`, `readline_function_names`, the `bool` validators)
//! lives in `huck_engine::readline_bind`.

use rustyline::Event;
use rustyline::{Anchor, At, Cmd, KeyCode, KeyEvent, Modifiers, Movement, Word};

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
    fn parse_keyseq_matches_engine_validity() {
        // The engine's rustyline-free `keyseq_is_valid` (via parse_keyseq_valid)
        // must agree with the cli's real `parse_keyseq` on accept/reject for
        // every input — they are hand-kept in sync across the crate split.
        let cases = [
            "\\C-a", "\\M-f", "\\e[A", "\\C-", "", "a", "\\x41", "\\x4",
            "\\101", "\\C-\\M-a", "\"\\C-a\"", "\\e", "\\t", "\\r", "\\n",
            "\\b", "\\C-i", "\\C-m", "\\C-h", "\\x7f", "\\xZZ", "abc",
            "\\C-x", "\\M-\\C-a", "\\0", "\\377",
        ];
        for s in cases {
            assert_eq!(
                huck_engine::readline_bind::keyseq_is_valid(s),
                super::parse_keyseq(s).is_some(),
                "validity divergence on {s:?}",
            );
        }
    }

    #[test]
    fn function_map() {
        assert!(function_to_cmd("beginning-of-line").is_some());
        assert!(function_to_cmd("kill-line").is_some());
        assert!(function_to_cmd("accept-line").is_some());
        assert!(function_to_cmd("no-such-function").is_none());
    }
}
