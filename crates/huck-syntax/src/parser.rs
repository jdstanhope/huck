//! Parser-driven front-end (Phase C). Consumes the stack-mode lexer's atoms and
//! builds the existing AST (`WordPart`/`Word`). DORMANT in v241: reached only by
//! tests; production still uses the lexer's pre-built Words + command.rs.
#![allow(dead_code, unused_imports)]

use crate::command::ParseError;
use crate::lexer::{Lexer, Mode, ParamModifier, ParamOpKind, SubscriptKind, TokenKind, Word, WordPart};

/// Assemble a `Word` (Vec<WordPart>) from atoms in the CURRENT mode, stopping
/// at a boundary atom (`ParamClose` / `ParamSep` / `RBracket`).  Does NOT
/// consume the boundary token; callers consume it themselves.
///
/// `quoted` is the enclosing quoted context (e.g. the outer `"…"` wrapping the
/// whole `${…}`).  Each `Lit{..., quoted:atom_q}` atom's flag is OR-ed with
/// `quoted`.  Nested `${…}` expansions inherit the effective quoted value.
pub(crate) fn parse_word(iter: &mut Lexer, quoted: bool) -> Result<Word, ParseError> {
    let mut parts = Vec::new();

    loop {
        // ── boundary: stop without consuming ─────────────────────────────────
        if matches!(
            iter.peek_kind()?,
            None | Some(
                TokenKind::ParamClose | TokenKind::RBracket | TokenKind::ParamSep,
            )
        ) {
            break;
        }

        // ── nested `${…}` ────────────────────────────────────────────────────
        // `parse_param_expansion` owns its push/pop and consumes the buffered
        // `ParamOpen` token — we must NOT consume it here first.
        if matches!(iter.peek_kind()?, Some(TokenKind::ParamOpen { .. })) {
            // The atom carries its own `quoted` context (set by the lexer from
            // its per-frame `in_dquote` flag); OR with the enclosing `quoted`.
            let atom_q = match iter.peek_kind()? {
                Some(TokenKind::ParamOpen { quoted: q }) => *q,
                _ => unreachable!(),
            };
            let eff = quoted || atom_q;
            let p = parse_param_expansion(iter, eff)?;
            parts.push(p);
            continue;
        }

        // ── all other atoms ───────────────────────────────────────────────────
        let kind = iter.next_kind()?.expect("non-boundary atom after peek");
        match kind {
            TokenKind::Lit { text, quoted: atom_q } => {
                parts.push(WordPart::Literal { text, quoted: atom_q || quoted });
            }
            TokenKind::DollarName { name: n, quoted: atom_q } => {
                // The atom carries its own `quoted` context (set by the lexer
                // from its per-frame `in_dquote` flag); OR with enclosing.
                let eff = quoted || atom_q;
                let part = match n.as_str() {
                    "@" => WordPart::AllArgs   { quoted: eff, joined: false },
                    "*" => WordPart::AllArgs   { quoted: eff, joined: true  },
                    "?" => WordPart::LastStatus { quoted: eff },
                    _   => WordPart::Var       { name: n, quoted: eff },
                };
                parts.push(part);
            }
            TokenKind::DeferredExpansion => {
                // `$(…)` / `$((…))` / backtick inside an operand — v241 boundary.
                return Err(ParseError::UnsupportedExpansion);
            }
            _ => {
                // Unexpected atom in operand context.
                return Err(ParseError::UnsupportedExpansion);
            }
        }
    }
    Ok(Word(parts))
}

/// Convert the subscript word assembled by `parse_word` into a `SubscriptKind`.
/// A bare unquoted `@` or `*` literal maps to `All` / `Star` respectively;
/// anything else becomes `Index(word)`.  Mirrors `scan_param_subscript` in the
/// production lexer.
fn subscript_kind_from(w: Word) -> SubscriptKind {
    match w.0.as_slice() {
        [WordPart::Literal { text, quoted: false }] if text == "@" => SubscriptKind::All,
        [WordPart::Literal { text, quoted: false }] if text == "*" => SubscriptKind::Star,
        _ => SubscriptKind::Index(w),
    }
}

/// Assemble a single `WordPart` for a `${…}` expansion.  Pushes
/// `Mode::ParamExpansion` itself, so callers must position the lexer at `${`
/// (under any mode — the push ensures `${` is scanned as atoms rather than a
/// pre-built Word token).
///
/// Owns the full push/pop lifecycle of its `ParamExpansion` frame and consumes
/// the `ParamOpen` token at entry.
pub(crate) fn parse_param_expansion(iter: &mut Lexer, quoted: bool) -> Result<WordPart, ParseError> {
    // 1. Push the mode and consume the `ParamOpen` (`${`) token.
    iter.push_mode(Mode::ParamExpansion { seen_name: false });
    match iter.next_kind()? {
        Some(TokenKind::ParamOpen { .. }) => {}
        _ => {
            iter.pop_mode();
            return Err(ParseError::UnsupportedExpansion);
        }
    }

    // 2. Optional length prefix (`${#name}`) or indirect prefix (`${!name}`).
    let mut length_form = false;
    let mut indirect = false;
    if matches!(iter.peek_kind()?, Some(TokenKind::ParamLengthPrefix)) {
        iter.next_kind()?;
        length_form = true;
    } else if matches!(iter.peek_kind()?, Some(TokenKind::ParamIndirect)) {
        iter.next_kind()?;
        indirect = true;
    }

    // 3. The parameter name (always present; may be "" for bad-subst).
    let name = match iter.next_kind()? {
        Some(TokenKind::ParamName(n)) => n,
        _ => {
            iter.pop_mode();
            return Err(ParseError::UnsupportedExpansion);
        }
    };

    // 4. Optional subscript `[…]`.
    let mut subscript: Option<SubscriptKind> = None;
    if matches!(iter.peek_kind()?, Some(TokenKind::LBracket)) {
        iter.next_kind()?; // consume LBracket
        iter.push_mode(Mode::ParamSubscriptOperand { in_dquote: false });
        let sub_word = match parse_word(iter, false) {
            Ok(w) => w,
            Err(e) => {
                iter.pop_mode(); // ParamSubscriptOperand
                iter.pop_mode(); // ParamExpansion
                return Err(e);
            }
        };
        match iter.next_kind()? {
            Some(TokenKind::RBracket) => {}
            _ => {
                iter.pop_mode(); // ParamSubscriptOperand
                iter.pop_mode(); // ParamExpansion
                return Err(ParseError::UnsupportedExpansion);
            }
        }
        iter.pop_mode(); // ParamSubscriptOperand
        subscript = Some(subscript_kind_from(sub_word));
    }

    // 5. Dispatch on `ParamClose` (bare/length/indirect/subscript) or `ParamOp`.
    let result = match iter.next_kind()? {
        Some(TokenKind::ParamClose) => {
            if subscript.is_some() {
                // `${a[i]}` / `${a[@]}` / `${a[*]}`
                WordPart::ParamExpansion {
                    name,
                    modifier: ParamModifier::None,
                    quoted,
                    subscript,
                    indirect,
                }
            } else if indirect {
                // `${!name}` — indirect scalar expansion.
                WordPart::ParamExpansion {
                    name,
                    modifier: ParamModifier::None,
                    quoted,
                    subscript: None,
                    indirect: true,
                }
            } else if length_form {
                // `${#name}` — string/array length.
                WordPart::ParamExpansion {
                    name,
                    modifier: ParamModifier::Length,
                    quoted,
                    subscript: None,
                    indirect: false,
                }
            } else {
                // `${name}` — plain variable reference.
                // Matches production's bare `Var` for unmodified braced references.
                WordPart::Var { name, quoted }
            }
        }

        Some(TokenKind::ParamOp(op_kind)) => {
            // Production passes `enclosing_dquote=false` specifically for
            // ErrorIfUnset; all other value-family ops use the outer `quoted`.
            let op_quoted = match op_kind {
                ParamOpKind::ErrorIfUnset(_) => false,
                _ => quoted,
            };
            iter.push_mode(Mode::ParamWordOperand { in_dquote: false });
            let word = match parse_word(iter, op_quoted) {
                Ok(w) => w,
                Err(e) => {
                    iter.pop_mode(); // ParamWordOperand
                    iter.pop_mode(); // ParamExpansion
                    return Err(e);
                }
            };
            iter.pop_mode(); // ParamWordOperand
            // Expect closing `}`.
            match iter.next_kind()? {
                Some(TokenKind::ParamClose) => {}
                _ => {
                    iter.pop_mode(); // ParamExpansion
                    return Err(ParseError::UnsupportedExpansion);
                }
            }
            let modifier = match op_kind {
                ParamOpKind::UseDefault(colon)    => ParamModifier::UseDefault    { word, colon },
                ParamOpKind::AssignDefault(colon) => ParamModifier::AssignDefault { word, colon },
                ParamOpKind::ErrorIfUnset(colon)  => ParamModifier::ErrorIfUnset  { word, colon },
                ParamOpKind::UseAlternate(colon)  => ParamModifier::UseAlternate  { word, colon },
                _ => {
                    // Task 5 fills the remaining operators (pattern, substring, case, transform).
                    iter.pop_mode(); // ParamExpansion
                    unimplemented!("parse_param_expansion: operator not yet implemented (Task 5)")
                }
            };
            WordPart::ParamExpansion { name, modifier, quoted, subscript, indirect }
        }

        _ => {
            iter.pop_mode(); // ParamExpansion
            return Err(ParseError::UnsupportedExpansion);
        }
    };

    // 6. Pop the ParamExpansion frame.
    iter.pop_mode();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{
        tokenize_with_opts, Lexer, LexerOptions, Mode, ParamModifier, ParamOpKind,
        SubstKind, SubscriptKind, TokenKind, Word, WordPart,
    };
    use crate::command::ParseError;

    // ── Differential helpers ─────────────────────────────────────────────────
    //
    // THE PRODUCTION LEXER IS THE ORACLE.  When `new_part` ≠ `old_part`, fix
    // the new path to match — never weaken or skip the comparison.

    /// Recursively find the first expansion `WordPart` in a slice.
    /// Looks for `ParamExpansion`, `Var`, `AllArgs`, and `LastStatus` (all the
    /// forms the production lexer emits for `${…}` inputs), and descends into
    /// `Quoted` wrappers.
    fn find_expansion(parts: &[WordPart]) -> Option<WordPart> {
        for p in parts {
            match p {
                WordPart::ParamExpansion { .. }
                | WordPart::Var { .. }
                | WordPart::AllArgs { .. }
                | WordPart::LastStatus { .. } => return Some(p.clone()),
                WordPart::Quoted { parts, .. } => {
                    if let Some(x) = find_expansion(parts) {
                        return Some(x);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Build the expected `WordPart` using the PRODUCTION lexer (oracle).
    /// Wraps `s` in `"…"` when `quoted=true` to simulate a double-quoted context.
    fn old_part(s: &str, quoted: bool) -> WordPart {
        let src = if quoted { format!("\"{s}\"") } else { s.to_string() };
        let toks = tokenize_with_opts(&src, LexerOptions::default()).expect("old lex");
        match &toks[0].kind {
            TokenKind::Word(w) => find_expansion(&w.0).expect("no param part in production token"),
            _ => panic!("production token is not a Word for {src:?}"),
        }
    }

    /// Build the expected `WordPart` using the NEW parser-driven path.
    fn new_part(s: &str, quoted: bool) -> WordPart {
        let mut lx = Lexer::new_live(s, &Default::default(), LexerOptions::default());
        parse_param_expansion(&mut lx, quoted).expect("new parse")
    }

    /// Assert that the new and old paths produce identical results for both
    /// unquoted and quoted contexts.
    fn diff_ok(s: &str) {
        assert_eq!(new_part(s, false), old_part(s, false), "unquoted {s:?}");
        assert_eq!(new_part(s, true),  old_part(s, true),  "quoted   {s:?}");
    }

    // ── Tests ────────────────────────────────────────────────────────────────

    #[test]
    fn scaffolding_types_exist() {
        let _ = TokenKind::ParamOpen { quoted: false };
        let _ = TokenKind::Lit { text: "x".into(), quoted: false };
        let _ = ParamOpKind::Substitute(SubstKind::All);
        let _ = Mode::ParamWordOperand { in_dquote: false };
        let _ = ParseError::UnsupportedExpansion;
    }

    #[test]
    fn diff_core_forms() {
        for s in [
            "${x}",
            "${x:-d}",
            "${x-d}",
            "${x:=d}",
            "${x:?m}",
            "${x:+a}",
            "${x:-a b}",
            "${x:-${y}}",
            "${#x}",
            "${!x}",
            "${a[1]}",
            "${a[@]}",
            "${a[*]}",
            "${a[$i]}",
        ] {
            diff_ok(s);
        }
    }

    #[test]
    fn diff_dquote_operand() {
        // Confirm T3 flattening: `"a${y}b"` inside an operand produces
        // [Literal{"a",q:true}, Var{name:"y",q:true}, Literal{"b",q:true}].
        // The atom now carries `quoted` directly, so the nested `${y}` is
        // assembled with `quoted:true` without any heuristic.
        diff_ok("${x:-\"a${y}b\"}");
    }

    #[test]
    fn diff_dquote_expansion_first() {
        // dquote operand starting with the expansion (no leading literal) — the
        // heuristic got this wrong; carrying quoted on the atom fixes it.
        diff_ok("${x:-\"${y}c\"}");
        diff_ok("${x:-\"$v${y}\"}");
    }
}
