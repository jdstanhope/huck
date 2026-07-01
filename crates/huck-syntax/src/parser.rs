//! Parser-driven front-end (Phase C). Consumes the stack-mode lexer's atoms and
//! builds the existing AST (`WordPart`/`Word`). DORMANT in v241: reached only by
//! tests; production still uses the lexer's pre-built Words + command.rs.
#![allow(dead_code, unused_imports)]

use crate::command::{
    Command, Sequence, Pipeline, SimpleCommand, ExecCommand, Assignment, Connector, ParseError,
    Redirection, RedirFd, RedirOp, FileMode, word_literal_text, IfClause, ElifBranch, WhileClause,
};
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

/// Skip over any `Newline` tokens without consuming anything else.
/// Mirrors `skip_newlines` in `command.rs`.
fn skip_newlines(iter: &mut Lexer) -> Result<(), ParseError> {
    while matches!(iter.peek_kind()?, Some(TokenKind::Newline)) {
        iter.next_kind()?;
    }
    Ok(())
}

/// Returns `true` if the token is a standalone `!` word (pipeline negation).
/// Mirrors `is_bang_word` in `command.rs`.
fn is_bang_word(tok: &TokenKind) -> bool {
    match tok {
        TokenKind::Word(w) => word_literal_text(w) == Some("!"),
        _ => false,
    }
}

/// Reserved-word kinds.  Mirrors `command.rs`'s `Keyword` exactly so that
/// Tasks 2–7 can share the same stop-at sets and function signatures.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum Keyword {
    If, Then, Elif, Else, Fi,
    While, Until, Do, Done,
    For, In, Case, Esac,
    LBrace, RBrace,
    DoubleBracketOpen,   // `[[`
    DoubleBracketClose,  // `]]`
    Function, Select, Coproc,
}

impl Keyword {
    fn name(self) -> &'static str {
        match self {
            Keyword::If       => "if",
            Keyword::Then     => "then",
            Keyword::Elif     => "elif",
            Keyword::Else     => "else",
            Keyword::Fi       => "fi",
            Keyword::While    => "while",
            Keyword::Until    => "until",
            Keyword::Do       => "do",
            Keyword::Done     => "done",
            Keyword::For      => "for",
            Keyword::In       => "in",
            Keyword::Case     => "case",
            Keyword::Esac     => "esac",
            Keyword::LBrace   => "{",
            Keyword::RBrace   => "}",
            Keyword::DoubleBracketOpen  => "[[",
            Keyword::DoubleBracketClose => "]]",
            Keyword::Function => "function",
            Keyword::Select   => "select",
            Keyword::Coproc   => "coproc",
        }
    }
}

/// Returns the keyword a `TokenKind` represents, or `None`.  A token is a
/// keyword only when it is a `Word` of exactly one part — an *unquoted*
/// `Literal` whose text equals the keyword.  Mirrors `keyword_of` in
/// `command.rs`.
pub(crate) fn keyword_kind(token: &TokenKind) -> Option<Keyword> {
    let TokenKind::Word(Word(parts)) = token else { return None };
    if parts.len() != 1 { return None; }
    let WordPart::Literal { text, quoted: false } = &parts[0] else { return None; };
    match text.as_str() {
        "if"       => Some(Keyword::If),
        "then"     => Some(Keyword::Then),
        "elif"     => Some(Keyword::Elif),
        "else"     => Some(Keyword::Else),
        "fi"       => Some(Keyword::Fi),
        "while"    => Some(Keyword::While),
        "until"    => Some(Keyword::Until),
        "do"       => Some(Keyword::Do),
        "done"     => Some(Keyword::Done),
        "for"      => Some(Keyword::For),
        "in"       => Some(Keyword::In),
        "case"     => Some(Keyword::Case),
        "esac"     => Some(Keyword::Esac),
        "{"        => Some(Keyword::LBrace),
        "}"        => Some(Keyword::RBrace),
        "[["       => Some(Keyword::DoubleBracketOpen),
        "]]"       => Some(Keyword::DoubleBracketClose),
        "function" => Some(Keyword::Function),
        "select"   => Some(Keyword::Select),
        "coproc"   => Some(Keyword::Coproc),
        _          => None,
    }
}

/// Returns `true` if `token` is a reserved word (keyword).  Delegates to
/// `keyword_kind` so there is ONE keyword table.  `time` is NOT a keyword
/// (see `cmd_time_is_plain_command`).
fn keyword_of_tok(token: &TokenKind) -> bool {
    keyword_kind(token).is_some()
}


/// Parses a SINGLE redirect token group (optional `RedirFd` prefix + redirect
/// operator + target word) from `iter`.  Mirrors one iteration of
/// `parse_trailing_redirects` in `command.rs`.
///
/// Returns `UnsupportedCommand` for heredocs and here-strings (deferred).
fn parse_one_redirect(iter: &mut Lexer) -> Result<Vec<Redirection>, ParseError> {
    // Optional explicit fd-prefix (`3>`, `{fd}>`).
    let fd_prefix = if let Some(TokenKind::RedirFd(_)) = iter.peek_kind()? {
        let Some(TokenKind::RedirFd(fd)) = iter.next_kind()? else {
            unreachable!("peek confirmed RedirFd")
        };
        Some(fd)
    } else {
        None
    };

    match iter.peek_kind()? {
        Some(TokenKind::Heredoc { .. }) => {
            // Heredoc — deferred to a future task.
            Err(ParseError::UnsupportedCommand)
        }
        Some(TokenKind::Op(op)) if crate::command::is_redirect_op(op) => {
            let op = *op;
            iter.next_kind()?; // consume the redirect operator
            // HereString (`<<<`) — deferred.
            if matches!(op, Operator::HereString) {
                return Err(ParseError::UnsupportedCommand);
            }
            let target = match iter.next_kind()? {
                Some(TokenKind::Word(word)) => word,
                Some(TokenKind::Op(_)) => return Err(ParseError::RedirectTargetIsOperator),
                Some(TokenKind::Newline) | None => return Err(ParseError::MissingRedirectTarget),
                Some(TokenKind::Heredoc { .. }) => return Err(ParseError::RedirectTargetIsOperator),
                Some(TokenKind::RedirFd(_)) => return Err(ParseError::RedirectTargetIsOperator),
                Some(TokenKind::ArithBlock(..)) => return Err(ParseError::RedirectTargetIsOperator),
                // Phase C atom variants (dormant — never emitted in Command mode)
                Some(_) => return Err(ParseError::RedirectTargetIsOperator),
            };
            Ok(crate::command::build_redirections(op, target, fd_prefix))
        }
        _ => {
            // A bare fd-prefix with no following redirect operator: defensively
            // guard (the lexer only emits RedirFd glued to an op, but be safe).
            if fd_prefix.is_some() {
                return Err(ParseError::MissingRedirectTarget);
            }
            // Should not be reached (caller checks next_is_redirect first).
            Err(ParseError::UnsupportedCommand)
        }
    }
}

/// Parses a simple command (program + args + redirects, with optional leading
/// assignments) from a flat token stream.  Mirrors `parse_simple_stage` +
/// `finalize_stage` in `command.rs`.
///
/// Stops — without consuming — at any stage/list terminator:
/// `|`, `;`, `&&`, `||`, `&`, `)`, `;;`, `;&`, `;;&`, newline, or EOF.
///
/// Redirects are parsed in source order and interleaved with words — a
/// redirect may appear before, between, or after words.  Heredocs and
/// here-strings return `UnsupportedCommand` (deferred).
///
/// Leading `NAME=value` words (and `NAME+=value` / `NAME[i]=value` forms)
/// become `inline_assignments`.  A line of ONLY assignments with NO redirects
/// produces `Command::Simple(SimpleCommand::Assign(…))`.  A command with
/// redirects but no program word produces an empty-program `ExecCommand`
/// (mirrors `finalize_stage`'s empty-remaining + redirects branch).
fn parse_simple(iter: &mut Lexer) -> Result<Command, ParseError> {
    let line = iter.current_line()?;
    let mut all_words: Vec<Word> = Vec::new();
    let mut redirects: Vec<Redirection> = Vec::new();

    loop {
        let Some(token) = iter.peek_kind()? else { break };
        // Stage/list terminators — stop without consuming.
        if matches!(
            token,
            TokenKind::Op(
                Operator::Pipe
                    | Operator::Semi
                    | Operator::And
                    | Operator::Or
                    | Operator::Background
                    | Operator::RParen
                    | Operator::DoubleSemi
                    | Operator::SemiAmp
                    | Operator::DoubleSemiAmp
            ) | TokenKind::Newline
        ) {
            break;
        }
        // Redirect tokens — parse in source order, extending the redirects
        // list.  Mirrors the `next_is_redirect` + `parse_trailing_redirects`
        // delegation in `parse_simple_stage`.
        if crate::command::next_is_redirect(iter)? {
            redirects.extend(parse_one_redirect(iter)?);
            continue;
        }
        // Consume the token.
        let kind = iter.next_kind()?.unwrap();
        match kind {
            TokenKind::Word(word) => all_words.push(word),
            _ => return Err(ParseError::UnsupportedCommand),
        }
    }

    if all_words.is_empty() && redirects.is_empty() {
        return Err(ParseError::MissingCommand);
    }

    // Peel leading assignments from the front — mirrors `finalize_stage`.
    // Uses `is_assignment_word` (cheap peek) then `try_split_assignment`
    // (consuming move) to match the oracle's assignment-detection exactly.
    let mut inline_assignments: Vec<Assignment> = Vec::new();
    let mut word_iter = all_words.into_iter().peekable();
    while let Some(w) = word_iter.peek() {
        if !crate::command::is_assignment_word(w) {
            break;
        }
        let owned = word_iter.next().expect("just peeked Some");
        match crate::command::try_split_assignment(owned) {
            Ok(a) => inline_assignments.push(a),
            Err(_) => unreachable!("is_assignment_word confirmed assignment shape"),
        }
    }
    let remaining: Vec<Word> = word_iter.collect();

    // Bare-assign line: all words were assignments, no program word follows,
    // AND no redirects.  Mirrors `finalize_stage`'s guard exactly:
    //   `remaining.is_empty() && redirects.is_empty() && !inline.is_empty()`.
    if remaining.is_empty() && redirects.is_empty() && !inline_assignments.is_empty() {
        return Ok(Command::Simple(SimpleCommand::Assign(inline_assignments, line)));
    }

    // Empty-program case: redirect-only or assignment+redirect commands
    // (e.g. `>out`, `2>err`, `A=1 >out`).  Mirrors `finalize_stage`'s second
    // empty-remaining branch and the `program=None+redirects` early-return in
    // `parse_simple_stage`.
    if remaining.is_empty() {
        return Ok(Command::Simple(SimpleCommand::Exec(ExecCommand {
            inline_assignments,
            program: Word(Vec::new()),
            args: Vec::new(),
            redirects,
            line,
        })));
    }

    let mut remaining_iter = remaining.into_iter();
    let program = remaining_iter.next().expect("non-empty remaining");
    let args: Vec<Word> = remaining_iter.collect();

    Ok(Command::Simple(SimpleCommand::Exec(ExecCommand {
        inline_assignments,
        program,
        args,
        redirects,
        line,
    })))
}

/// Parses a single command stage (dispatch).  Mirrors the dispatch logic of
/// `parse_command_inner` in `command.rs`.
///
/// Returns the BARE stage command — `parse_pipeline` decides Pipeline wrapping.
///
/// Compound commands currently supported:
///   `{` → `parse_brace_group` (Task 1)
/// All other compound-opening keywords → `UnsupportedCommand` (Tasks 2–7).
fn parse_command(iter: &mut Lexer) -> Result<Command, ParseError> {
    // Skip leading newlines (mirrors `parse_command_inner` command.rs:1019).
    skip_newlines(iter)?;
    // EOF with no token.
    if iter.peek_kind()?.is_none() {
        return Err(ParseError::MissingCommand);
    }
    // `(( expr ))` at command position.
    if matches!(iter.peek_kind()?, Some(TokenKind::ArithBlock(..))) {
        return Err(ParseError::UnsupportedCommand);
    }
    // Bare `(` → subshell.
    if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen))) {
        return parse_subshell(iter);
    }
    // Heredoc / `<<<` at command position.
    if matches!(
        iter.peek_kind()?,
        Some(TokenKind::Heredoc { .. }) | Some(TokenKind::Op(Operator::HereString))
    ) {
        return Err(ParseError::UnsupportedCommand);
    }
    // Reserved word (keyword): `{` dispatches to brace group; all others defer.
    if let Some(tok) = iter.peek_kind()? {
        match keyword_kind(tok) {
            Some(Keyword::LBrace) => return parse_brace_group(iter),
            Some(Keyword::If)     => return parse_if(iter),
            Some(Keyword::While) | Some(Keyword::Until) => return parse_while(iter),
            Some(_) => return Err(ParseError::UnsupportedCommand),
            None => {}
        }
    }
    // Function definition `name() compound` — two-token lookahead (deferred).
    if matches!(iter.peek_kind()?, Some(TokenKind::Word(_)))
        && matches!(iter.peek2_kind()?, Some(TokenKind::Op(Operator::LParen)))
    {
        return Err(ParseError::UnsupportedCommand);
    }
    // Simple command: parse and return BARE.  `parse_pipeline` wraps it.
    parse_simple(iter)
}

/// Parses a pipeline: an optional leading run of `!` words (odd count →
/// negate), then command stages joined by `|`.  Mirrors
/// `parse_command` (bang handling) + `parse_command_then_pipeline` +
/// `parse_next_stage` in `command.rs`.
///
/// Wrapping behaviour mirrors the oracle exactly:
/// - Simple commands without `|`: wrapped in `Command::Pipeline(…)` (oracle:
///   `parse_pipeline_with_first` always returns Pipeline).
/// - Compound commands without `|`: returned as-is, or Pipeline-wrapped only
///   when `!` negate applies (oracle: `parse_command_then_pipeline` returns raw).
/// - Any command with `|`: all stages collected into `Command::Pipeline(…)`.
fn parse_pipeline(iter: &mut Lexer) -> Result<Command, ParseError> {
    // Count leading `!` words (each one flips the negate flag).
    let mut bangs = 0usize;
    while iter.peek_kind()?.map(is_bang_word).unwrap_or(false) {
        iter.next_kind()?; // consume `!`
        bangs += 1;
    }
    let negate = bangs % 2 == 1;

    // Parse the first stage command (may be simple or compound).
    let first = parse_command(iter)?;

    // No `|` follows — wrapping decision mirrors the oracle:
    //   simple  → always wrap in Pipeline (oracle: parse_pipeline_with_first)
    //   compound, no negate → return as-is (oracle: parse_command_then_pipeline)
    //   compound, negate    → wrap so the Pipeline carries the negate flag
    if !matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::Pipe))) {
        return Ok(match first {
            Command::Simple(_) => Command::Pipeline(Pipeline { negate, commands: vec![first] }),
            cmd if negate      => Command::Pipeline(Pipeline { negate: true, commands: vec![cmd] }),
            cmd                => cmd,
        });
    }

    // A `|` follows — collect all stages into a Pipeline.
    let mut stages = vec![first];
    iter.next_kind()?; // consume `|`
    skip_newlines(iter)?;

    loop {
        stages.push(parse_command(iter)?);
        if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::Pipe))) {
            iter.next_kind()?; // consume `|`
            skip_newlines(iter)?;
        } else {
            break;
        }
    }

    Ok(Command::Pipeline(Pipeline { negate, commands: stages }))
}

/// Mirrors `parse_command_then_pipeline` in `command.rs`.
/// Delegates to `parse_pipeline` (which handles `!` + `|` stages).
fn parse_command_then_pipeline(iter: &mut Lexer) -> Result<Command, ParseError> {
    parse_pipeline(iter)
}

/// Parses the full flat and-or list (the complete `Sequence`).  Mirrors
/// `parse_sequence_opts` in `command.rs` with `stop_at_top_newline = false`
/// — the `parse` (non-unit) contract where a top-level `Newline` is a
/// Semi-like continue connector, NOT a unit terminator.
///
/// `stop_at` is a set of keywords that terminate the list WITHOUT consuming
/// the terminator token — used by compound commands to stop before their
/// closing keyword (e.g. `stop_at = &[Keyword::RBrace]` inside `{ … }`).
/// Three stop checks mirror `parse_sequence_opts` ~890/917/958.
///
/// After the first pipeline, loops consuming:
/// - `Op(Semi)` / `Newline` → `Connector::Semi`
/// - `Op(And)` → `Connector::And`
/// - `Op(Or)` → `Connector::Or`
/// - `Op(Background)`:
///   - trailing (nothing meaningful follows) → sets `background = true`
///   - `& &` → `Err(UnexpectedBackground)`
///   - `& cmd` → `Connector::Amp` separator
///
/// Stops (without consuming) at EOF, case terminators (`;;`/`;&`/`;;&`),
/// or a `stop_at` keyword.
pub(crate) fn parse_and_or(iter: &mut Lexer, stop_at: &[Keyword]) -> Result<Sequence, ParseError> {
    let first = parse_command_then_pipeline(iter)?;
    let mut rest: Vec<(Connector, Command)> = Vec::new();
    let mut background = false;

    loop {
        // ── Stop check 1: before consuming any connector (mirrors ~890) ──────
        match iter.peek_kind()? {
            // EOF — end of list.
            None => break,
            // Case-clause terminators — break without consuming.
            Some(TokenKind::Op(
                Operator::DoubleSemi | Operator::SemiAmp | Operator::DoubleSemiAmp,
            )) => break,
            Some(tok) => {
                if let Some(kw) = keyword_kind(tok) {
                    if stop_at.contains(&kw) {
                        break;
                    }
                }
            }
        }

        // Consume the connector token.
        let token = iter.next_kind()?.unwrap();
        match token {
            // ── `&` — background / Amp separator ────────────────────────────
            TokenKind::Op(Operator::Background) => {
                // Skip any newlines emitted after heredoc bodies (mirrors oracle).
                skip_newlines(iter)?;
                match iter.peek_kind()? {
                    // Nothing follows → trailing `&`: background the whole sequence.
                    None => {
                        background = true;
                        break;
                    }
                    // ── Stop check 2: stop_at keyword after `&` (~917) ────────
                    Some(tok)
                        if keyword_kind(tok)
                            .map(|k| stop_at.contains(&k))
                            .unwrap_or(false) =>
                    {
                        background = true;
                        break;
                    }
                    // A case-clause terminator → trailing `&` for the last group.
                    Some(TokenKind::Op(
                        Operator::DoubleSemi | Operator::SemiAmp | Operator::DoubleSemiAmp,
                    )) => {
                        background = true;
                        break;
                    }
                    // Another `&` with no preceding command → `cmd & &` is invalid.
                    Some(TokenKind::Op(Operator::Background)) => {
                        return Err(ParseError::UnexpectedBackground);
                    }
                    // A command follows → `&` is a separator.
                    Some(_) => {
                        rest.push((Connector::Amp, parse_command_then_pipeline(iter)?));
                    }
                }
            }

            // ── `;` or newline — semi-like connector ─────────────────────────
            TokenKind::Op(Operator::Semi) | TokenKind::Newline => {
                skip_newlines(iter)?;
                // ── Stop check 3: stop_at keyword after `;`/newline (~958) ───
                match iter.peek_kind()? {
                    None => break,
                    Some(TokenKind::Op(
                        Operator::DoubleSemi | Operator::SemiAmp | Operator::DoubleSemiAmp,
                    )) => break,
                    Some(tok) => {
                        if keyword_kind(tok).map(|k| stop_at.contains(&k)).unwrap_or(false) {
                            break;
                        }
                    }
                }
                rest.push((Connector::Semi, parse_command_then_pipeline(iter)?));
            }

            // ── `&&` — and connector ─────────────────────────────────────────
            TokenKind::Op(Operator::And) => {
                skip_newlines(iter)?;
                rest.push((Connector::And, parse_command_then_pipeline(iter)?));
            }

            // ── `||` — or connector ──────────────────────────────────────────
            TokenKind::Op(Operator::Or) => {
                skip_newlines(iter)?;
                rest.push((Connector::Or, parse_command_then_pipeline(iter)?));
            }

            // ── anything else (e.g. stray word / `|` after a closed block) ──
            other => {
                if let Some(kw) = keyword_kind(&other) {
                    return Err(ParseError::UnexpectedKeyword(kw.name().to_string()));
                }
                return Err(ParseError::UnexpectedToken);
            }
        }
    }

    Ok(Sequence { first, rest, background })
}

/// Entry point for the new flat command-list parser.  Mirrors `parse` /
/// `parse_cursor` in `command.rs`.
///
/// Returns `Ok(None)` on empty input (newlines only or EOF).
pub(crate) fn parse_sequence(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError> {
    // Skip leading newlines (mirrors `parse_cursor` → `skip_newlines`).
    while matches!(iter.peek_kind()?, Some(TokenKind::Newline)) {
        iter.next_kind()?;
    }
    if iter.peek_kind()?.is_none() {
        return Ok(None);
    }
    let seq = parse_and_or(iter, &[])?;
    // Mirror `parse_cursor`: a stray terminator (`;;`/`;&`/`;;&`) left after
    // the top-level sequence → `UnexpectedToken`.
    if iter.peek_kind()?.is_some() {
        return Err(ParseError::UnexpectedToken);
    }
    Ok(Some(seq))
}

/// Expects a specific keyword token; returns `on_missing` if the next token
/// is not the expected keyword.  Mirrors `expect_keyword` in `command.rs`.
fn expect_keyword(
    iter: &mut Lexer,
    expected: Keyword,
    on_missing: ParseError,
) -> Result<(), ParseError> {
    match iter.next_kind()? {
        Some(ref t) if keyword_kind(t) == Some(expected) => Ok(()),
        _ => Err(on_missing),
    }
}

/// Parses a compound command's body (`LIST` terminated by a keyword in
/// `stop_at`).  If the body is empty AND the iterator is exhausted, the
/// compound is unterminated — return `unterminated` instead of
/// `MissingCommand`.  Mirrors `parse_compound_section` in `command.rs`.
pub(crate) fn parse_compound_section(
    iter: &mut Lexer,
    stop_at: &[Keyword],
    unterminated: ParseError,
) -> Result<Sequence, ParseError> {
    match parse_and_or(iter, stop_at) {
        Err(ParseError::MissingCommand) if iter.peek_kind()?.is_none() => Err(unterminated),
        other => other,
    }
}

/// Wraps a freshly-parsed compound command in `Command::Redirected` when one
/// or more redirects immediately follow its terminator; otherwise returns the
/// command unchanged.  Mirrors `maybe_wrap_redirects` in `command.rs`.
pub(crate) fn maybe_wrap_redirects(
    cmd: Command,
    iter: &mut Lexer,
) -> Result<Command, ParseError> {
    let mut redirects: Vec<Redirection> = Vec::new();
    while crate::command::next_is_redirect(iter)? {
        redirects.extend(parse_one_redirect(iter)?);
    }
    if !redirects.is_empty() {
        Ok(Command::Redirected { inner: Box::new(cmd), redirects })
    } else {
        Ok(cmd)
    }
}

/// Parses `{ LIST }`.  Expects the `{` keyword, a compound section stopping
/// at `}`, then the `}` keyword.  Trailing redirects are handled by
/// `maybe_wrap_redirects`.  Mirrors `parse_brace_group` in `command.rs`
/// (with `maybe_wrap_redirects` inlined from the caller, since
/// `parse_command` returns `Command` rather than a bare `Sequence`).
fn parse_brace_group(iter: &mut Lexer) -> Result<Command, ParseError> {
    expect_keyword(iter, Keyword::LBrace, ParseError::UnterminatedBrace)?;
    let body =
        parse_compound_section(iter, &[Keyword::RBrace], ParseError::UnterminatedBrace)?;
    expect_keyword(iter, Keyword::RBrace, ParseError::UnterminatedBrace)?;
    maybe_wrap_redirects(Command::BraceGroup(Box::new(body)), iter)
}

/// Parses `if COND then BODY [elif COND then BODY]* [else BODY] fi`.
/// Mirrors `parse_if` (~1282) in `command.rs`:
/// - `expect` `if`; condition stops at `then`; `expect` `then`.
/// - then_body stops at `elif`/`else`/`fi`.
/// - loop while next keyword is `elif`: condition stops at `then`; `expect` `then`; body stops at `elif`/`else`/`fi`.
/// - optional `else`: body stops at `fi`.
/// - `expect` `fi`.  Trailing redirects handled by `maybe_wrap_redirects`.
fn parse_if(iter: &mut Lexer) -> Result<Command, ParseError> {
    expect_keyword(iter, Keyword::If, ParseError::UnterminatedIf)?;
    let condition = parse_compound_section(iter, &[Keyword::Then], ParseError::UnterminatedIf)?;
    expect_keyword(iter, Keyword::Then, ParseError::UnterminatedIf)?;
    let then_body = parse_compound_section(
        iter,
        &[Keyword::Elif, Keyword::Else, Keyword::Fi],
        ParseError::UnterminatedIf,
    )?;

    let mut elif_branches = Vec::new();
    while iter.peek_kind()?.and_then(|t| keyword_kind(t)) == Some(Keyword::Elif) {
        iter.next_kind()?; // consume `elif`
        let condition =
            parse_compound_section(iter, &[Keyword::Then], ParseError::UnterminatedIf)?;
        expect_keyword(iter, Keyword::Then, ParseError::UnterminatedIf)?;
        let body = parse_compound_section(
            iter,
            &[Keyword::Elif, Keyword::Else, Keyword::Fi],
            ParseError::UnterminatedIf,
        )?;
        elif_branches.push(ElifBranch { condition, body });
    }

    let else_body = if iter.peek_kind()?.and_then(|t| keyword_kind(t)) == Some(Keyword::Else) {
        iter.next_kind()?; // consume `else`
        Some(parse_compound_section(iter, &[Keyword::Fi], ParseError::UnterminatedIf)?)
    } else {
        None
    };

    expect_keyword(iter, Keyword::Fi, ParseError::UnterminatedIf)?;
    let clause = IfClause { condition, then_body, elif_branches, else_body };
    maybe_wrap_redirects(Command::If(Box::new(clause)), iter)
}

/// Parses `while LIST; do LIST; done` or `until LIST; do LIST; done`.
/// Mirrors `parse_while` (~1886) in `command.rs`: consume the opener keyword
/// (setting `until`), then condition stops at `do`, then body stops at `done`.
/// Trailing redirects are handled by `maybe_wrap_redirects`.
fn parse_while(iter: &mut Lexer) -> Result<Command, ParseError> {
    let until = match iter.next_kind()?.as_ref().and_then(|t| keyword_kind(t)) {
        Some(Keyword::While) => false,
        Some(Keyword::Until) => true,
        _ => unreachable!("parse_command guarantees a while/until keyword here"),
    };
    let condition = parse_compound_section(iter, &[Keyword::Do], ParseError::UnterminatedLoop)?;
    expect_keyword(iter, Keyword::Do, ParseError::UnterminatedLoop)?;
    let body = parse_compound_section(iter, &[Keyword::Done], ParseError::UnterminatedLoop)?;
    expect_keyword(iter, Keyword::Done, ParseError::UnterminatedLoop)?;
    maybe_wrap_redirects(Command::While(Box::new(WhileClause { condition, body, until })), iter)
}

/// Parses a `( LIST )` subshell.  Mirrors `parse_subshell` (~1780) in
/// `command.rs`:
/// - Consumes the leading `(`.
/// - `()` (immediate `)`) → `Err(EmptySubshell)`.
/// - No tokens → `Err(UnterminatedSubshell)`.
/// - Otherwise delegates to `parse_subshell_sequence` (bespoke connector
///   loop that terminates on `)`, NOT on a keyword).
/// - Wraps trailing redirects via `maybe_wrap_redirects`.
fn parse_subshell(iter: &mut Lexer) -> Result<Command, ParseError> {
    // Consume `(`.
    iter.next_kind()?;

    // Empty subshell `()` — immediately hit `)` with no commands inside.
    if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::RParen))) {
        iter.next_kind()?; // consume `)`
        return Err(ParseError::EmptySubshell);
    }

    // No tokens at all → unterminated.
    if iter.peek_kind()?.is_none() {
        return Err(ParseError::UnterminatedSubshell);
    }

    let body = parse_subshell_sequence(iter)?;
    maybe_wrap_redirects(Command::Subshell { body: Box::new(body) }, iter)
}

/// Parses a sequence of commands terminated by `)`.  Mirrors
/// `parse_subshell_sequence` (~1807) in `command.rs`:
/// - Breaks on `Op(RParen)` (consuming it) instead of on a keyword.
/// - Returns `Err(UnterminatedSubshell)` if the token stream ends before `)`.
///
/// This is a BESPOKE loop — it does NOT use `parse_and_or(stop_at)` because
/// the subshell stops on `)` (an operator token), not on a keyword.
fn parse_subshell_sequence(iter: &mut Lexer) -> Result<Sequence, ParseError> {
    // Parse the first command (may itself be a subshell, compound, etc.)
    // and — if followed by `|` — the rest of the pipeline.
    let first = parse_command_then_pipeline(iter)?;

    let mut rest = Vec::new();
    loop {
        match iter.peek_kind()? {
            // End of tokens before `)` → unterminated.
            None => return Err(ParseError::UnterminatedSubshell),
            // `)` terminates the subshell body — consume and return.
            Some(TokenKind::Op(Operator::RParen)) => {
                iter.next_kind()?;
                break;
            }
            Some(TokenKind::Op(Operator::Semi)) | Some(TokenKind::Newline) => {
                iter.next_kind()?; // consume `;` or newline
                skip_newlines(iter)?;
                // Trailing `;` or newline before `)` — break cleanly.
                if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::RParen))) {
                    iter.next_kind()?; // consume `)`
                    break;
                }
                if iter.peek_kind()?.is_none() {
                    return Err(ParseError::UnterminatedSubshell);
                }
                let cmd = parse_command_then_pipeline(iter)?;
                rest.push((Connector::Semi, cmd));
            }
            Some(TokenKind::Op(Operator::Background)) => {
                iter.next_kind()?; // consume `&`
                // `&` inside a subshell body backgrounds the preceding command
                // and acts as a separator.  Skip any redundant `;` or newlines
                // that follow (`&;` is equivalent to `&` in bash).
                while matches!(
                    iter.peek_kind()?,
                    Some(TokenKind::Op(Operator::Semi)) | Some(TokenKind::Newline)
                ) {
                    iter.next_kind()?;
                }
                skip_newlines(iter)?;
                // If `)` follows (or stream ends), this `&` terminates the
                // whole body as a backgrounded sequence.
                if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::RParen))) {
                    iter.next_kind()?; // consume `)`
                    return Ok(Sequence { first, rest, background: true });
                }
                if iter.peek_kind()?.is_none() {
                    return Err(ParseError::UnterminatedSubshell);
                }
                // More commands follow (`(cmd1 & cmd2)` pattern): parse the
                // next command and continue.
                let cmd = parse_command_then_pipeline(iter)?;
                rest.push((Connector::Amp, cmd));
            }
            Some(TokenKind::Op(Operator::And)) => {
                iter.next_kind()?;
                skip_newlines(iter)?;
                rest.push((Connector::And, parse_command_then_pipeline(iter)?));
            }
            Some(TokenKind::Op(Operator::Or)) => {
                iter.next_kind()?;
                skip_newlines(iter)?;
                rest.push((Connector::Or, parse_command_then_pipeline(iter)?));
            }
            // Any other token (stray keyword, another `(`, etc.) after a
            // complete command and before `)` is unexpected.
            Some(_) => return Err(ParseError::UnterminatedSubshell),
        }
    }

    Ok(Sequence { first, rest, background: false })
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
    /// Error parity: the new parser must return the SAME error as the oracle.
    fn diff_err(s: &str) {
        assert_eq!(new_seq(s), old_seq(s), "error mismatch for {s:?}");
    }
    // v243 T2 tests

    #[test]
    fn cmd_subshell() {
        diff_cmd("( a )");
        diff_cmd("( a; b )");
        diff_cmd("( a | b )");
        diff_cmd("( a && b || c )");
        diff_cmd("( a; b; )");             // trailing ;
        diff_cmd("( (a) )");               // nested subshell
        diff_cmd("( { a; } )");            // brace group inside subshell
        diff_cmd("{ ( a ); }");            // subshell inside brace group
        diff_cmd("( a ) >f");              // trailing redirect
        diff_cmd("( a ) | b");             // subshell as pipeline stage
        diff_err("()");                    // EmptySubshell parity
        diff_err("( a");                   // unterminated parity
    }

    // v243 T3 tests

    #[test]
    fn cmd_if() {
        diff_cmd("if x; then y; fi");
        diff_cmd("if x; then y; else z; fi");
        diff_cmd("if a; then b; elif c; then d; fi");
        diff_cmd("if a; then b; elif c; then d; else e; fi");
        diff_cmd("if a; then b; elif c; then d; elif e; then f; fi");   // multi-elif
        diff_cmd("if x; then if y; then z; fi; fi");                    // nested if
        diff_cmd("if x; then a; b; c; fi");                             // multi-command body
        diff_cmd("if x | y; then z; fi");                               // pipeline condition
        diff_cmd("if x; then y; fi | cat");                             // if as pipeline stage
        diff_cmd("if x; then y; fi >f");                               // trailing redirect
        diff_err("if x; then y");                                       // UnterminatedIf parity
    }

    // v243 T4 tests

    #[test]
    fn cmd_while_until() {
        diff_cmd("while x; do y; done");
        diff_cmd("until x; do y; done");
        diff_cmd("while x; do a; b; done");
        diff_cmd("while x | y; do z; done");                       // pipeline condition
        diff_cmd("while x; do if y; then z; fi; done");            // nested if in body
        diff_cmd("while x; do while y; do z; done; done");         // nested loop
        diff_cmd("until x; do ( a ); done");                       // subshell in body
        diff_cmd("while x; do y; done | cat");                     // as pipeline stage
        diff_cmd("while x; do y; done <f");                        // trailing redirect
        diff_err("while x; do y");                                  // UnterminatedLoop parity
    }

    // v242 T2 tests

    #[test]
    fn cmd_single_simple() {
        diff_cmd("echo");
        diff_cmd("echo a");
        diff_cmd("echo a b c");
        diff_cmd("echo \"$x\" 'y' z");
        assert_eq!(new_seq("").unwrap(), None);       // empty input
        assert_eq!(new_seq("\n\n").unwrap(), None);   // only newlines
    }

    #[test]
    fn cmd_deferred_boundary() {
        // `{ a; }` removed: brace groups are now in-scope (Task 1).
        // `( a )` removed: subshells are now in-scope (Task 2).
        // `while x; do y; done` removed: while/until are now in-scope (Task 4).
        for s in ["(( 1+2 ))",
                  "for i in a; do x; done", "case x in y) z;; esac",
                  "[[ -n x ]]", "f() { x; }", "coproc x"] {
            diff_unsupported(s);
        }
    }

    // T1 tests

    #[test]
    fn cmd_brace_group() {
        diff_cmd("{ a; }");
        diff_cmd("{ a; b; }");
        diff_cmd("{ a; b; c; }");
        diff_cmd("{ echo hi; }");
        diff_cmd("{ { a; } }");            // nested
        diff_cmd("{ a; } >f");             // trailing redirect -> Command::Redirected
        diff_cmd("{ a; } >f 2>&1");
        diff_cmd("{ a; } | cat");          // brace as pipeline stage
        diff_cmd("a | { b; }");
        diff_cmd("{ a; }; { b; }");        // two brace groups in a sequence
        diff_err("{ a");                   // UnterminatedBrace parity
    }

    // T3 tests

    #[test]
    fn cmd_assignments() {
        diff_cmd("A=1 cmd");
        diff_cmd("A=1 B=2 cmd x y");
        diff_cmd("A=1");                 // bare assign -> SimpleCommand::Assign
        diff_cmd("A=1 B=2");             // bare multi-assign
        diff_cmd("A=$x cmd");
        diff_cmd("A+=v cmd");            // append
        diff_cmd("arr[0]=v cmd");        // subscripted (AssignPrefix)
        diff_cmd("PATH=/x:/y cmd");
    }

    // tests added in later tasks

    #[test]
    fn v242_scaffolding_exists() {
        let _ = crate::command::ParseError::UnsupportedCommand;
        // harness compiles + the entry is callable
        let _ = old_seq("echo a");
    }

    // T4 tests

    #[test]
    fn cmd_redirects() {
        diff_cmd("cmd >out");
        diff_cmd("cmd >>out");
        diff_cmd("cmd <in");
        diff_cmd("cmd 2>err");
        diff_cmd("cmd >out 2>&1");
        diff_cmd("cmd 2>&1 >out");       // order matters
        diff_cmd(">out cmd");            // leading redirect
        diff_cmd("cmd a >o b <i c");     // interleaved
        diff_cmd("3>f cmd");             // RedirFd prefix
        diff_cmd("cmd >|f");             // clobber
        diff_cmd("cmd <>f");             // read-write
        diff_cmd("cmd <&3");             // dup-in
        diff_cmd("cmd &>f");             // and-redirect
        diff_cmd("cmd >&2");             // dup-out to stderr
        diff_cmd("cmd 2>&-");            // close fd 2
    }

    #[test]
    fn cmd_heredoc_deferred() {
        diff_unsupported("cat <<<word");
        // (heredoc body cases need a newline; keep to here-string for the dispatch test)
    }

    // T5 tests

    #[test]
    fn cmd_pipelines() {
        diff_cmd("a | b");
        diff_cmd("a | b | c");
        diff_cmd("! a");
        diff_cmd("! a | b");
        diff_cmd("echo x | grep y | wc -l");
        diff_cmd("A=1 cmd | other");
        diff_cmd("cmd >o | other");
        diff_cmd("! ! a");                 // double-bang cancels (negate=false)
        diff_cmd("!\ncmd");                // newline after `!` is skipped (M1: parse_command top skip_newlines)
    }


    // T6 tests

    #[test]
    fn cmd_and_or_lists() {
        diff_cmd("a; b");
        diff_cmd("a; b; c");
        diff_cmd("x && y");
        diff_cmd("x || y");
        diff_cmd("x && y || z");
        diff_cmd("a | b && c | d");
        diff_cmd("p &");                  // trailing background
        diff_cmd("p & q");                // & as separator (Connector::Amp)
        diff_cmd("a\nb");                 // newline as connector (parse contract)
        diff_cmd("a; b &");
        diff_cmd("! a | b && c");
    }

    #[test]
    fn cmd_invalid_double_background() {
        // `cmd & &` → command.rs returns UnexpectedBackground; match it exactly.
        assert_eq!(new_seq("cmd & &"), old_seq("cmd & &"));
    }

    #[test]
    fn cmd_time_is_plain_command() {
        // `command.rs` has NO special `time` handling — it parses `time …` as a
        // plain command named `time`. The new parser MUST match the oracle (not
        // defer), so these are diff_cmd. (When huck later adds a `Timed` AST node,
        // both parsers change together; until then `time` is just a command word.)
        diff_cmd("time cmd");
        diff_cmd("time -p cmd");
    }
}
