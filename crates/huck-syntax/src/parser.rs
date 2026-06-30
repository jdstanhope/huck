//! Parser-driven front-end (Phase C). Consumes the stack-mode lexer's atoms and
//! builds the existing AST (`WordPart`/`Word`). DORMANT in v241: reached only by
//! tests; production still uses the lexer's pre-built Words + command.rs.
#![allow(dead_code, unused_imports)]

use crate::command::{Command, Sequence, Pipeline, SimpleCommand, ExecCommand, Assignment, Connector, ParseError};
use crate::lexer::{
    CaseDirection, Lexer, Mode, Operator, ParamModifier, ParamOpKind, SubstAnchor, SubstKind,
    SubscriptKind, TokenKind, TransformOp, Word, WordPart,
};

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
        // ── Bare close: ${name}, ${#name}, ${!name}, ${name[sub]}, ${@}, ${*}, ${}
        Some(TokenKind::ParamClose) => {
            if indirect && matches!(subscript, Some(SubscriptKind::All) | Some(SubscriptKind::Star)) {
                // `${!arr[@]}` / `${!arr[*]}` — indirect array-keys form.
                // Mirrors the production: IndirectKeys modifier, indirect:false.
                WordPart::ParamExpansion {
                    name,
                    modifier: ParamModifier::IndirectKeys,
                    quoted,
                    subscript,
                    indirect: false,
                }
            } else if subscript.is_some() {
                // `${a[i]}` / `${a[@]}` / `${a[*]}` — bare subscripted reference.
                WordPart::ParamExpansion {
                    name,
                    modifier: ParamModifier::None,
                    quoted,
                    subscript,
                    indirect,
                }
            } else if indirect {
                // `${!name}` — indirect scalar expansion with no modifier.
                WordPart::ParamExpansion {
                    name,
                    modifier: ParamModifier::None,
                    quoted,
                    subscript: None,
                    indirect: true,
                }
            } else if length_form {
                // `${#name}` — length.
                WordPart::ParamExpansion {
                    name,
                    modifier: ParamModifier::Length,
                    quoted,
                    subscript: None,
                    indirect: false,
                }
            } else if name == "@" {
                // `${@}` — all positional args (joined=false).
                // Mirrors `scan_braced_param_expansion`'s `Some('@')` early-return.
                WordPart::AllArgs { quoted, joined: false }
            } else if name == "*" {
                // `${*}` — all positional args (joined=true).
                WordPart::AllArgs { quoted, joined: true }
            } else if name.is_empty() {
                // `${}` — bad substitution at runtime.
                WordPart::ParamExpansion {
                    name: String::new(),
                    modifier: ParamModifier::BadSubst { raw: "${}".to_string() },
                    quoted,
                    subscript: None,
                    indirect: false,
                }
            } else {
                // `${name}` — plain variable reference.
                WordPart::Var { name, quoted }
            }
        }

        // ── Operator: pattern removal, substitute, case, transform, substring
        Some(TokenKind::ParamOp(op_kind)) => {
            // Macro: push a mode, parse_word, pop mode. On error pops ParamExpansion too.
            // NOTE: macros are scoped to this function.
            macro_rules! word_in_mode {
                ($mode:expr, $wquoted:expr) => {{
                    iter.push_mode($mode);
                    match parse_word(iter, $wquoted) {
                        Ok(w) => {
                            iter.pop_mode();
                            w
                        }
                        Err(e) => {
                            iter.pop_mode(); // the operand mode
                            iter.pop_mode(); // ParamExpansion
                            return Err(e);
                        }
                    }
                }};
            }
            // Macro: consume ParamClose; on failure pop ParamExpansion and return Err.
            macro_rules! expect_close {
                () => {
                    match iter.next_kind()? {
                        Some(TokenKind::ParamClose) => {}
                        _ => {
                            iter.pop_mode(); // ParamExpansion
                            return Err(ParseError::UnsupportedExpansion);
                        }
                    }
                };
            }

            match op_kind {
                // ── Value family: UseDefault / AssignDefault / ErrorIfUnset / UseAlternate
                // Production: `modifier_with_operand(chars, quoted/false, ...)`.
                // `ErrorIfUnset` uses `enclosing_dquote=false`; others use `quoted`.
                ParamOpKind::UseDefault(colon) => {
                    let word = word_in_mode!(Mode::ParamWordOperand { in_dquote: false }, quoted);
                    expect_close!();
                    WordPart::ParamExpansion {
                        name, modifier: ParamModifier::UseDefault { word, colon },
                        quoted, subscript, indirect,
                    }
                }
                ParamOpKind::AssignDefault(colon) => {
                    let word = word_in_mode!(Mode::ParamWordOperand { in_dquote: false }, quoted);
                    expect_close!();
                    WordPart::ParamExpansion {
                        name, modifier: ParamModifier::AssignDefault { word, colon },
                        quoted, subscript, indirect,
                    }
                }
                ParamOpKind::ErrorIfUnset(colon) => {
                    // Production: `modifier_with_operand(chars, false, ...)` — NOT `quoted`.
                    let word = word_in_mode!(Mode::ParamWordOperand { in_dquote: false }, false);
                    expect_close!();
                    WordPart::ParamExpansion {
                        name, modifier: ParamModifier::ErrorIfUnset { word, colon },
                        quoted, subscript, indirect,
                    }
                }
                ParamOpKind::UseAlternate(colon) => {
                    let word = word_in_mode!(Mode::ParamWordOperand { in_dquote: false }, quoted);
                    expect_close!();
                    WordPart::ParamExpansion {
                        name, modifier: ParamModifier::UseAlternate { word, colon },
                        quoted, subscript, indirect,
                    }
                }

                // ── Pattern removal: RemovePrefix / RemoveSuffix
                // Production: `modifier_with_operand(chars, false, ...)` — enclosing_dquote=false.
                ParamOpKind::RemovePrefix(longest) => {
                    let pattern = word_in_mode!(Mode::ParamWordOperand { in_dquote: false }, false);
                    expect_close!();
                    WordPart::ParamExpansion {
                        name, modifier: ParamModifier::RemovePrefix { pattern, longest },
                        quoted, subscript, indirect,
                    }
                }
                ParamOpKind::RemoveSuffix(longest) => {
                    let pattern = word_in_mode!(Mode::ParamWordOperand { in_dquote: false }, false);
                    expect_close!();
                    WordPart::ParamExpansion {
                        name, modifier: ParamModifier::RemoveSuffix { pattern, longest },
                        quoted, subscript, indirect,
                    }
                }

                // ── Substitute: ${var/pat/repl} / ${var//…} / ${var/#…} / ${var/%…}
                // Pattern in ParamSubstPatternOperand (sep=/); replacement in ParamWordOperand.
                // Both operands: enclosing_dquote=false (mirrors scan_substitution_operand).
                // Absent replacement (no ParamSep) → empty Word, matching bash ${var/pat}.
                ParamOpKind::Substitute(subst_kind) => {
                    let (anchor, all) = match subst_kind {
                        SubstKind::First  => (SubstAnchor::None,   false),
                        SubstKind::All    => (SubstAnchor::None,   true),
                        SubstKind::Prefix => (SubstAnchor::Prefix, false),
                        SubstKind::Suffix => (SubstAnchor::Suffix, false),
                    };

                    // Pattern in subst-pattern mode (sep = `/`).
                    iter.push_mode(Mode::ParamSubstPatternOperand { in_dquote: false });
                    let pattern = match parse_word(iter, false) {
                        Ok(w) => { iter.pop_mode(); w }
                        Err(e) => { iter.pop_mode(); iter.pop_mode(); return Err(e); }
                    };

                    // Optional `/replacement`.
                    let replacement =
                        if matches!(iter.peek_kind()?, Some(TokenKind::ParamSep)) {
                            iter.next_kind()?; // consume `/`
                            word_in_mode!(Mode::ParamWordOperand { in_dquote: false }, false)
                        } else {
                            Word(vec![])
                        };

                    expect_close!();
                    WordPart::ParamExpansion {
                        name,
                        modifier: ParamModifier::Substitute { pattern, replacement, anchor, all },
                        quoted, subscript, indirect,
                    }
                }

                // ── Case conversion: ${var^pat} / ${var^^} / ${var,pat} / ${var,,}
                // Production: `scan_optional_braced_operand` — empty body → None.
                ParamOpKind::Case(direction, all) => {
                    let word = word_in_mode!(Mode::ParamWordOperand { in_dquote: false }, false);
                    expect_close!();
                    let pattern = if word.0.is_empty() { None } else { Some(word) };
                    WordPart::ParamExpansion {
                        name, modifier: ParamModifier::Case { direction, all, pattern },
                        quoted, subscript, indirect,
                    }
                }

                // ── Transform: ${var@Q} / ${var@U} / etc.
                // No operand: the operator letter was already consumed by the head mode.
                // Only a ParamClose follows.
                ParamOpKind::Transform(op) => {
                    expect_close!();
                    WordPart::ParamExpansion {
                        name, modifier: ParamModifier::Transform { op },
                        quoted, subscript, indirect,
                    }
                }

                // ── Substring: ${var:offset} / ${var:offset:length}
                // Offset in ParamSubstringOffsetOperand (sep = `:`); length in ParamWordOperand.
                // Empty offset (${var:}) → BadSubst, matching dispatch_braced_modifier's
                // `Some(':') / Some('}') → recover_bad_subst` branch.
                ParamOpKind::Substring => {
                    // Offset in substring-offset mode (sep = `:`).
                    iter.push_mode(Mode::ParamSubstringOffsetOperand { in_dquote: false });
                    let offset = match parse_word(iter, false) {
                        Ok(w) => { iter.pop_mode(); w }
                        Err(e) => { iter.pop_mode(); iter.pop_mode(); return Err(e); }
                    };

                    // Optional `:length`.
                    let length =
                        if matches!(iter.peek_kind()?, Some(TokenKind::ParamSep)) {
                            iter.next_kind()?; // consume `:`
                            Some(word_in_mode!(Mode::ParamWordOperand { in_dquote: false }, false))
                        } else {
                            None
                        };

                    expect_close!();

                    if offset.0.is_empty() {
                        // `${x:}` — bad substitution at runtime.  Raw reconstructed from name.
                        let raw = format!("${{{name}:}}");
                        WordPart::ParamExpansion {
                            name: String::new(),
                            modifier: ParamModifier::BadSubst { raw },
                            quoted,
                            subscript: None,
                            indirect: false,
                        }
                    } else {
                        WordPart::ParamExpansion {
                            name,
                            modifier: ParamModifier::Substring { offset, length },
                            quoted, subscript, indirect,
                        }
                    }
                }
            }
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

pub(crate) fn parse_sequence(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError> {
    let _ = iter;
    unimplemented!("parse_sequence: Task 2")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{
        tokenize_with_opts, CaseDirection, Lexer, LexerOptions, Mode, ParamModifier,
        ParamOpKind, SubstAnchor, SubstKind, SubscriptKind, TokenKind, TransformOp, Word,
        WordPart,
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

    // ── T5 tests ─────────────────────────────────────────────────────────────

    #[test]
    fn diff_removal_and_case() {
        for s in [
            "${x#p}", "${x##p}", "${x%p}", "${x%%p}",
            "${x^p}", "${x^^}", "${x,p}", "${x,,}",
            "${x#$a}", "${x##${p}}",
        ] {
            diff_ok(s);
        }
    }

    #[test]
    fn diff_substitute() {
        for s in [
            "${x/p/r}", "${x//p/r}", "${x/#p/r}", "${x/%p/r}",
            "${x/p}", "${x//p}", "${x/$a/$b}", "${x/p/}",
        ] {
            diff_ok(s);
        }
    }

    #[test]
    fn diff_substring() {
        for s in [
            "${x:1}", "${x:1:2}", "${x:$o}", "${x:$o:$l}", "${x: -1}",
        ] {
            diff_ok(s);
        }
    }

    #[test]
    fn diff_transform() {
        for s in [
            "${x@Q}", "${x@P}", "${x@U}", "${x@L}", "${x@u}",
            "${x@E}", "${x@A}", "${x@K}", "${x@k}", "${x@a}",
        ] {
            diff_ok(s);
        }
    }

    #[test]
    fn diff_indirect_and_special() {
        // NOTE: `${!pre*}` / `${!pre@}` (PrefixNames) are NOT tested here because
        // the head mode's post-name path for unrecognised chars (`*`, `@` when not
        // a valid Transform letter) consumes to `}` and emits ParamClose — making
        // `${!pre*}` atom-identical to `${!pre}`.  This is a T2 head-mode
        // limitation; fixing it requires the head mode to emit a distinct marker
        // for `*`/`@` in indirect-prefix context.  Deferred to a follow-up task.
        for s in [
            "${!x}", "${!x[@]}", "${!x[*]}",
            "${@}", "${*}", "${#}", "${?}", "${$}", "${!}", "${-}",
        ] {
            diff_ok(s);
        }
    }

    #[test]
    fn diff_badsubst() {
        // `${x@}` is NOT tested here: the head mode's `@` arm (post-name) emits
        // ParamClose after consuming `@+}` on the bad-op path, making the token
        // stream for `${x@}` identical to `${x}`.  The parser cannot distinguish
        // them without a dedicated bad-subst atom from the head mode.
        // Deferred to a T2/T3 head-mode fix.
        assert_eq!(new_part("${}", false), old_part("${}", false), "badsubst ${{}}");
        assert_eq!(new_part("${x:}", false), old_part("${x:}", false), "badsubst ${{x:}}");
    }

    #[test]
    fn diff_dquote_operands() {
        // T3 fix: double-quoted operands tokenize FLAT (per-frame in_dquote). A simple
        // `"…"` is one quoted Lit (`}` stays literal); a `"…"` with a nested `${}` recurses.
        // These MUST match the production lexer's flat WordPart::Literal{quoted:true}
        // (no Quoted wrapper — verified at parse_braced_operand_opts lexer.rs:3735).
        for s in [
            "${x:-\"a}b\"}",
            "${x:-\"a${y}b\"}",
            "${x:-\"$v\"}",
            "${x:-pre\"mid\"post}",
            "${x#\"$p\"}",
            "${x/\"a/b\"/c}",
        ] {
            diff_ok(s);
        }
    }

    #[test]
    fn diff_deferred_returns_unsupported() {
        use crate::lexer::{Lexer, LexerOptions};
        // $(…)/arith/backtick remain deferred even INSIDE a double-quoted operand.
        for s in [
            "${x:-$(cmd)}", "${x:-$((1+1))}", "${x:-`cmd`}", "${x:-\"$(cmd)\"}",
        ] {
            let mut lx = Lexer::new_live(s, &Default::default(), LexerOptions::default());
            assert!(
                matches!(
                    parse_param_expansion(&mut lx, false),
                    Err(crate::command::ParseError::UnsupportedExpansion)
                ),
                "expected UnsupportedExpansion for {s}"
            );
        }
    }

    // ── v242 differential harness ────────────────────────────────────────────

    fn old_seq(s: &str) -> Result<Option<Sequence>, ParseError> {
        let toks = tokenize_with_opts(s, LexerOptions::default()).expect("lex");
        crate::command::parse(&mut Lexer::from_tokens(toks))
    }
    fn new_seq(s: &str) -> Result<Option<Sequence>, ParseError> {
        let toks = tokenize_with_opts(s, LexerOptions::default()).expect("lex");
        parse_sequence(&mut Lexer::from_tokens(toks))
    }
    /// In-scope: the new parser must produce the SAME AST as command.rs (the oracle).
    fn diff_cmd(s: &str) {
        assert_eq!(new_seq(s).unwrap(), old_seq(s).unwrap(), "command AST mismatch for {s:?}");
    }
    /// Deferred: the new parser must return UnsupportedCommand.
    fn diff_unsupported(s: &str) {
        assert!(matches!(new_seq(s), Err(ParseError::UnsupportedCommand)),
                "expected UnsupportedCommand for {s:?}, got {:?}", new_seq(s));
    }
    // tests added in later tasks

    #[test]
    fn v242_scaffolding_exists() {
        let _ = crate::command::ParseError::UnsupportedCommand;
        // harness compiles + the entry is callable
        let _ = old_seq("echo a");
    }
}
