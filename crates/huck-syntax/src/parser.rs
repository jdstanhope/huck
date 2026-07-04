//! Parser-driven front-end (Phase C). Consumes the stack-mode lexer's atoms and
//! builds the existing AST (`WordPart`/`Word`). DORMANT in v241: reached only by
//! tests; production still uses the lexer's pre-built Words + command.rs.
#![allow(dead_code, unused_imports)]

use crate::command::{
    Command, Sequence, Pipeline, SimpleCommand, ExecCommand, Assignment, Connector, ParseError,
    Redirection, RedirFd, RedirOp, FileMode, word_literal_text, IfClause, ElifBranch, WhileClause,
    ForClause, SelectClause, CaseClause, CaseItem, CaseTerminator, ArithForClause,
    TestExpr, TestUnaryOp, TestBinaryOp, try_unary_op, skip_test_newlines,
};
use crate::lexer::{
    brace_expand_parts, ArrayLiteralElement, CaseDirection, Lexer, Mode, Operator, ParamModifier,
    ParamOpKind, ProcDir, QuoteStyle, SubstAnchor, SubstKind, SubscriptKind, TokenKind, TransformOp,
    Word, WordPart,
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
            TokenKind::CmdSubOpen => {
                // v244 T4: `$(cmd)` signal from scan_step_param_operand.
                // The cursor is at `$(` — parse_command_sub pushes Mode::CommandSub
                // and scan_step_command_sub(false) owns consuming `$(`.
                let cs = parse_command_sub(iter, quoted)?;
                parts.push(cs);
            }
            TokenKind::BeginBacktick => {
                // v245 T6: `` `cmd` `` signal from scan_step_param_operand.
                // The cursor is at `` ` `` — parse_backtick_sub pushes Mode::Backtick
                // and scan_step_backtick(depth=0) owns consuming the opening `` ` ``.
                let bt = parse_backtick_sub(iter, quoted)?;
                parts.push(bt);
            }
            TokenKind::ArithOpen => {
                // v246 T6: `$((…))` signal from scan_step_param_operand.
                // Cursor is at `$((` — parse_arith_expansion pushes Mode::Arith
                // whose first scan consumes `$((`.
                let a = parse_arith_expansion(iter, quoted)?;
                parts.push(a);
            }
            TokenKind::DeferredExpansion => {
                // `$(cmd)` / `$((…))` inside a nested `"…"` operand span — still
                // deferred (see the `DeferredExpansion` doc comment).
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

/// Assemble a `Word` (Vec<WordPart>) from atoms in `Mode::Command` under the
/// v247 atom-command scanner (`scan_step_command_atoms`), stopping at a
/// boundary atom (`Blank` / `Newline` / `Op(_)` / EOF) WITHOUT consuming it —
/// callers (`parse_simple` et al.) own blank-skipping and the boundary token.
///
/// `quoted` is the enclosing quoted context (always `false` for a bare
/// command-position word in T2; later tasks may thread a non-`false` value in
/// through nested contexts). Each `Lit { quoted: atom_q }` atom's flag is
/// OR-ed with `quoted`, mirroring `parse_word`. `QuoteRun` atoms wrap into
/// `WordPart::Quoted { style, parts: vec![Literal { quoted: true }] }` —
/// reproducing the oracle's `scan_step_command` quote-wrapping (see the
/// `QuoteRun` doc comment in lexer.rs for why a flat `Literal` can't do this).
fn parse_word_command(iter: &mut Lexer, quoted: bool) -> Result<Word, ParseError> {
    let mut parts = Vec::new();
    // Pending coalescible literal chunk (adjacent `Lit` atoms with the SAME
    // `quoted` flag merge into one, mirroring the oracle's single literal
    // buffer). A `DollarLit` is a BARRIER: it flushes this and pushes `$`
    // standalone, matching the oracle flushing its buffer and starting fresh.
    let mut acc: Option<(String, bool)> = None;
    loop {
        match iter.peek_kind()? {
            None
            | Some(TokenKind::Blank)
            | Some(TokenKind::Newline)
            // v252 T3 (BUG-2 fix): break WITHOUT consuming on `ArrayClose` too, so
            // an EMPTY subscripted value immediately before `)` (`a=([0]=)`) yields
            // `Word([])` (normalized to an empty literal by the caller) instead of
            // consuming the `)` and erroring `UnexpectedToken`. `ArrayClose` is
            // emitted ONLY inside `Mode::ArrayLiteral`, so no other caller is
            // affected, and the enclosing `parse_array_literal` loop consumes the
            // `ArrayClose` on its next iteration.
            | Some(TokenKind::ArrayClose)
            | Some(TokenKind::Op(_)) => break,
            Some(TokenKind::Lit { .. }) => {
                if let Some(TokenKind::Lit { text, quoted: q }) = iter.next_kind()? {
                    push_lit(&mut acc, &mut parts, text, q || quoted);
                }
            }
            Some(TokenKind::DollarLit { .. }) => {
                if let Some(TokenKind::DollarLit { quoted: q }) = iter.next_kind()? {
                    flush_lit(&mut acc, &mut parts);
                    parts.push(WordPart::Literal { text: "$".into(), quoted: q || quoted });
                }
            }
            Some(TokenKind::QuoteRun { .. }) => {
                if let Some(TokenKind::QuoteRun { style, text }) = iter.next_kind()? {
                    flush_lit(&mut acc, &mut parts);
                    parts.push(WordPart::Quoted {
                        style,
                        parts: vec![WordPart::Literal { text, quoted: true }],
                    });
                }
            }
            // ── v247 T3: command-position expansions ──────────────────────────
            // `parse_param_expansion` owns its push/pop and consumes the buffered
            // `ParamOpen`, so it is dispatched on a PEEK (not consumed here first).
            Some(TokenKind::ParamOpen { .. }) => {
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_param_expansion(iter, quoted)?);
            }
            // The zero-width `CmdSubOpen`/`BeginBacktick`/`ArithOpen` signals must
            // be discarded via `next_kind()` BEFORE dispatching, so the sub-parser's
            // own pushed mode re-scans the real opener (mirrors `parse_word`).
            Some(TokenKind::CmdSubOpen) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_command_sub(iter, quoted)?);
            }
            // v252: compound array RHS. The prefix part (Literal "name=" or
            // AssignPrefix) is already accumulated; glue the ArrayLiteral after it.
            Some(TokenKind::ArrayOpen) => {
                iter.next_kind()?;            // discard the signal (cursor on `(`)
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_array_literal(iter)?);
            }
            // `<`/`>` are POSIX operator characters — unlike `$(`/`` ` ``, a
            // `<(`/`>(` process substitution ALWAYS ends any word already in
            // progress (oracle: `x<(y)` is TWO words, program "x" + arg
            // "<(y)", not one glued word). Only dispatch here when this atom
            // is a fresh word start (nothing accumulated yet); otherwise
            // break WITHOUT consuming so the caller's word-start dispatch
            // re-enters for a standalone procsub word. A procsub CAN still
            // have trailing content glued after its close (`<(y)z`), since
            // once inside this word there is no more `<`/`>` in the way —
            // that's why the loop continues rather than breaking below.
            Some(TokenKind::ProcSubOpen { .. }) if !(parts.is_empty() && acc.is_none()) => break,
            Some(TokenKind::ProcSubOpen { dir }) => {
                let dir = dir.clone();
                iter.next_kind()?;            // discard the signal (cursor stays on `(`)
                parts.push(parse_process_sub(iter, dir)?);
            }
            Some(TokenKind::BeginBacktick) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_backtick_sub(iter, quoted)?);
            }
            Some(TokenKind::ArithOpen) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_arith_expansion(iter, quoted)?);
            }
            Some(TokenKind::DollarName { .. }) => {
                if let Some(TokenKind::DollarName { name, quoted: q }) = iter.next_kind()? {
                    let eff = q || quoted;
                    flush_lit(&mut acc, &mut parts);
                    parts.push(match name.as_str() {
                        "@" => WordPart::AllArgs { quoted: eff, joined: false },
                        "*" => WordPart::AllArgs { quoted: eff, joined: true },
                        "?" => WordPart::LastStatus { quoted: eff },
                        _   => WordPart::Var { name, quoted: eff },
                    });
                }
            }
            Some(TokenKind::Tilde(_)) => {
                if let Some(TokenKind::Tilde(spec)) = iter.next_kind()? {
                    flush_lit(&mut acc, &mut parts);
                    parts.push(WordPart::Tilde(spec));
                }
            }
            // v247 T4: an assignment-prefix atom (`name+=` / `name[sub]=` /
            // `name[sub]+=`). Carried into the Word unchanged as the leading
            // `WordPart::AssignPrefix`; `try_split_assignment` consumes it later.
            Some(TokenKind::AssignPrefix { .. }) => {
                if let Some(TokenKind::AssignPrefix { target, append }) = iter.next_kind()? {
                    flush_lit(&mut acc, &mut parts);
                    parts.push(WordPart::AssignPrefix { target, append });
                }
            }
            // `"…"` — parser-driven double-quote mode. `parse_dquote` consumes the
            // zero-width `BeginDquote` signal, pushes `Mode::DoubleQuote`, collects
            // the inner parts, and pops.
            Some(TokenKind::BeginDquote) => {
                iter.next_kind()?; // discard the zero-width open signal
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_dquote(iter, quoted)?);
            }
            // Legacy Word-mode token: a COMPLETE word (the Word-lexer coalesces a
            // whole word into one `Word`). Reached when `parse_word_command` runs
            // on the Word-lexer path — command-sub bodies (`new_live`), for/select
            // in-lists, case patterns, and redirect targets. Consume exactly ONE
            // and stop: adjacent `Word` tokens are SEPARATE words in Word mode, so
            // gluing them would be wrong. (Without this arm the catch-all `break`
            // returns an empty Word WITHOUT advancing → the caller loop spins.)
            Some(TokenKind::Word(_)) => {
                if let Some(TokenKind::Word(w)) = iter.next_kind()? {
                    flush_lit(&mut acc, &mut parts);
                    parts.extend(w.0);
                }
                break;
            }
            _ => {
                // A non-word-part atom (`RedirFd`/`Heredoc`/`ArithBlock`/…). If we
                // have already assembled part of a word, this is a trailing
                // boundary — stop and let the caller handle the token. But if we
                // have accumulated NOTHING, the caller entered here on a token that
                // is not a word start (e.g. a redirect/heredoc atom sitting in a
                // for/select `in`-list or a case pattern position). Breaking would
                // return an EMPTY Word WITHOUT consuming, and the caller's loop —
                // which pushes `parse_word_command(..)?` for any non-`Op` token —
                // would re-peek the identical token and spin forever. Consume it and
                // error instead. (The oracle hits an analogous `unreachable!()` /
                // UnexpectedToken on the same malformed input, but always consumes
                // first, so it panics rather than hangs — a clean error is strictly
                // better.)
                if parts.is_empty() && acc.is_none() {
                    iter.next_kind()?;
                    return Err(ParseError::UnexpectedToken);
                }
                break;
            }
        }
    }
    flush_lit(&mut acc, &mut parts);
    Ok(Word(parts))
}

/// v254: assemble the `=~` regex pattern operand. The parser has just consumed
/// the `=~` operator word; push `Mode::Regex` and pull pattern atoms until the
/// zero-width `RegexEnd` (the lexer pops the mode when it emits `RegexEnd`).
/// Mirrors `parse_word_command`'s part-assembly arms — the lexer emitted the
/// regex metacharacters as `Lit` (not `Op`), so the same arms apply.
fn parse_regex_operand(iter: &mut Lexer) -> Result<Word, ParseError> {
    iter.push_mode(Mode::Regex { paren_depth: 0, body_started: false });
    // Drop any already-buffered leading Blank/Newline (the `next_is_test_binary_
    // operator_atom` peek2 buffered exactly one boundary atom after `=~`). The
    // lexer's own leading-skip handles the rest (spaces after a newline, `\<NL>`).
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank) | Some(TokenKind::Newline)) {
        iter.next_kind()?;
    }
    let mut parts: Vec<WordPart> = Vec::new();
    let mut acc: Option<(String, bool)> = None;
    loop {
        match iter.peek_kind()? {
            Some(TokenKind::RegexEnd) => { iter.next_kind()?; break; }
            Some(TokenKind::Lit { .. }) => {
                if let Some(TokenKind::Lit { text, quoted: q }) = iter.next_kind()? { push_lit(&mut acc, &mut parts, text, q); }
            }
            Some(TokenKind::DollarLit { .. }) => {
                if let Some(TokenKind::DollarLit { quoted: q }) = iter.next_kind()? { flush_lit(&mut acc, &mut parts); parts.push(WordPart::Literal { text: "$".into(), quoted: q }); }
            }
            Some(TokenKind::QuoteRun { .. }) => {
                if let Some(TokenKind::QuoteRun { style, text }) = iter.next_kind()? {
                    flush_lit(&mut acc, &mut parts);
                    // The oracle (`scan_regex_operand`) inlines a SINGLE-quoted run
                    // as a bare `Literal{quoted:true}` (no `Quoted` wrapper), but a
                    // `$'…'` ANSI-C run keeps its `Quoted{AnsiC,…}` wrapper (via
                    // `scan_dollar_expansion`'s `$'` arm). Backslash/Double never
                    // reach here (handled in the lexer's literal run / BeginDquote).
                    match style {
                        QuoteStyle::Single => parts.push(WordPart::Literal { text, quoted: true }),
                        _ => parts.push(WordPart::Quoted { style, parts: vec![WordPart::Literal { text, quoted: true }] }),
                    }
                }
            }
            Some(TokenKind::ParamOpen { .. }) => { flush_lit(&mut acc, &mut parts); parts.push(parse_param_expansion(iter, false)?); }
            Some(TokenKind::CmdSubOpen) => { iter.next_kind()?; flush_lit(&mut acc, &mut parts); parts.push(parse_command_sub(iter, false)?); }
            Some(TokenKind::ArithOpen) => { iter.next_kind()?; flush_lit(&mut acc, &mut parts); parts.push(parse_arith_expansion(iter, false)?); }
            Some(TokenKind::BeginBacktick) => { iter.next_kind()?; flush_lit(&mut acc, &mut parts); parts.push(parse_backtick_sub(iter, false)?); }
            // The oracle inlines the `"…"` body parts FLAT (each quoted:true) — it
            // calls `scan_dquote_expansion_body`, which pushes directly into the
            // operand's part list, NOT a `Quoted{Double,…}` wrapper, and whose
            // `flush_literal` pushes NOTHING for an EMPTY body. Unwrap
            // `parse_dquote`'s wrapper; but DROP the injected empty-`""` marker
            // (`[Literal{"",true}]`) so an empty `""` contributes no part — that is
            // what leaves the operand "unstarted" (via `set_regex_body_started`
            // below) so the pattern becomes the literal `]]` → the `=~` arm guard
            // reproduces the oracle's `Err(TestExprMissingOperand)` for
            // `[[ $x =~ "" ]]`, and drops the middle part in `a""b`.
            Some(TokenKind::BeginDquote) => {
                iter.next_kind()?; flush_lit(&mut acc, &mut parts);
                match parse_dquote(iter, false)? {
                    WordPart::Quoted { parts: inner, .. } => {
                        let is_empty_marker = inner.len() == 1
                            && matches!(&inner[0], WordPart::Literal { text, quoted: true } if text.is_empty());
                        if !is_empty_marker { parts.extend(inner); }
                    }
                    other => parts.push(other),
                }
            }
            Some(TokenKind::DollarName { .. }) => {
                if let Some(TokenKind::DollarName { name, quoted: q }) = iter.next_kind()? {
                    flush_lit(&mut acc, &mut parts);
                    parts.push(match name.as_str() {
                        "@" => WordPart::AllArgs { quoted: q, joined: false },
                        "*" => WordPart::AllArgs { quoted: q, joined: true },
                        "?" => WordPart::LastStatus { quoted: q },
                        _   => WordPart::Var { name, quoted: q },
                    });
                }
            }
            // `$[expr]` inside a regex defers everywhere on the atom path (v247).
            Some(TokenKind::DeferredExpansion) => return Err(ParseError::UnsupportedCommand),
            other => return Err(ParseError::TestExprBadOperator(format!("regex operand: {other:?}"))),
        }
        // v254 T1 fix: after every NON-`RegexEnd` atom, tell the lexer whether the
        // operand has produced any content yet — `Mode::Regex.body_started` then
        // mirrors the oracle's `!(lit.is_empty() && parts.is_empty())`, so an empty
        // `""` (which added no part) keeps the operand "unstarted" and the lexer's
        // leading-ws skip swallows the trailing space (→ oracle's Err). Skipped on
        // `RegexEnd` (the `break` above), where the lexer has already popped the mode.
        iter.set_regex_body_started(!(parts.is_empty() && acc.is_none()));
    }
    flush_lit(&mut acc, &mut parts);
    Ok(Word(parts))
}

/// Append a `Lit` atom into the pending coalescible chunk `acc`, matching the
/// oracle's single-buffer literal accumulation (`flush_literal`): adjacent
/// literals with the SAME `quoted` flag merge into one. Needed e.g. for a
/// trailing unescaped `\` at EOF (its own `Lit` atom, folded into the
/// surrounding literal by the oracle), a failed-tilde `~` continuing its run,
/// and a `"…"` body of mixed backslash-escapes + plain text (one `qbuf`).
fn push_lit(acc: &mut Option<(String, bool)>, out: &mut Vec<WordPart>, text: String, quoted: bool) {
    match acc {
        Some((buf, aq)) if *aq == quoted => buf.push_str(&text),
        _ => {
            flush_lit(acc, out);
            *acc = Some((text, quoted));
        }
    }
}

/// Flush the pending coalescible literal chunk into `out`, then clear it.
/// Pushes a single `WordPart::Literal` only if the chunk is non-empty — the
/// oracle never emits an empty `Literal`, so an empty chunk is dropped.
fn flush_lit(acc: &mut Option<(String, bool)>, out: &mut Vec<WordPart>) {
    if let Some((text, quoted)) = acc.take() {
        if !text.is_empty() {
            out.push(WordPart::Literal { text, quoted });
        }
    }
}

/// v247 T3: assemble a `WordPart::Quoted { style: Double, parts }` for a `"…"`
/// span. The caller has already consumed the zero-width `BeginDquote` signal;
/// this pushes `Mode::DoubleQuote` (whose first scan consumes the opening `"`),
/// collects the inner parts until `EndDquote`, pops the mode, and coalesces
/// adjacent literals. Every inner part is `quoted: true`; nested `$(…)`/`` `…`
/// ``/`$((…))` recurse through their own sub-parsers (parser-owned recursion).
/// Owns the full push/pop lifecycle of its `DoubleQuote` frame; pops on ALL
/// exit paths.
fn parse_dquote(iter: &mut Lexer, _outer_quoted: bool) -> Result<WordPart, ParseError> {
    iter.push_mode(Mode::DoubleQuote { body_started: false });
    let result = (|| -> Result<Vec<WordPart>, ParseError> {
        let mut parts: Vec<WordPart> = Vec::new();
        // Pending coalescible literal chunk (see `push_lit`/`flush_lit`); a
        // `DollarLit` is a barrier that flushes it and pushes `$` standalone.
        let mut acc: Option<(String, bool)> = None;
        loop {
            match iter.peek_kind()? {
                // Closing `"` — consume and finish.
                Some(TokenKind::EndDquote) => { iter.next_kind()?; break; }
                // EOF before the closing `"` — unterminated (matches the oracle,
                // whose fat dquote scanner errors on EOF).
                None => return Err(ParseError::UnterminatedSubshell),
                Some(TokenKind::ParamOpen { .. }) => {
                    flush_lit(&mut acc, &mut parts);
                    parts.push(parse_param_expansion(iter, true)?);
                }
                Some(TokenKind::CmdSubOpen) => {
                    iter.next_kind()?;
                    flush_lit(&mut acc, &mut parts);
                    parts.push(parse_command_sub(iter, true)?);
                }
                Some(TokenKind::BeginBacktick) => {
                    iter.next_kind()?;
                    flush_lit(&mut acc, &mut parts);
                    parts.push(parse_backtick_sub(iter, true)?);
                }
                Some(TokenKind::ArithOpen) => {
                    iter.next_kind()?;
                    flush_lit(&mut acc, &mut parts);
                    parts.push(parse_arith_expansion(iter, true)?);
                }
                Some(TokenKind::DollarName { .. }) => {
                    if let Some(TokenKind::DollarName { name, quoted: _ }) = iter.next_kind()? {
                        flush_lit(&mut acc, &mut parts);
                        parts.push(match name.as_str() {
                            "@" => WordPart::AllArgs { quoted: true, joined: false },
                            "*" => WordPart::AllArgs { quoted: true, joined: true },
                            "?" => WordPart::LastStatus { quoted: true },
                            _   => WordPart::Var { name, quoted: true },
                        });
                    }
                }
                Some(TokenKind::DollarLit { .. }) => {
                    if let Some(TokenKind::DollarLit { quoted: _ }) = iter.next_kind()? {
                        flush_lit(&mut acc, &mut parts);
                        parts.push(WordPart::Literal { text: "$".into(), quoted: true });
                    }
                }
                Some(TokenKind::Lit { .. }) => {
                    if let Some(TokenKind::Lit { text, quoted: _ }) = iter.next_kind()? {
                        push_lit(&mut acc, &mut parts, text, true);
                    }
                }
                _ => return Err(ParseError::UnsupportedExpansion),
            }
        }
        flush_lit(&mut acc, &mut parts);
        Ok(parts)
    })();
    iter.pop_mode();
    let mut parts = result?;
    if parts.is_empty() {
        // Empty `""` — preserve the empty-token contract (matches the oracle).
        parts.push(WordPart::Literal { text: String::new(), quoted: true });
    }
    Ok(WordPart::Quoted { style: crate::lexer::QuoteStyle::Double, parts })
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

/// Recursively zero all source-line fields (`ExecCommand.line` /
/// `SimpleCommand::Assign(_, line)`) in a `Sequence`.
///
/// The production oracle (`parse_substitution_body`) resets all token spans
/// to `line = 0` before parsing, so every command-AST node inside a `$(…)`
/// body carries `line: 0` ("unknown").  The new parser-driven path parses
/// in-situ with the live cursor, so lines are script-relative by default.
/// Calling this helper after `parse_subshell_sequence` aligns the two paths.
fn zero_lines_in_sequence(seq: &mut Sequence) {
    zero_lines_in_command(&mut seq.first);
    for (_, cmd) in &mut seq.rest {
        zero_lines_in_command(cmd);
    }
}

fn zero_lines_in_command(cmd: &mut Command) {
    match cmd {
        Command::Simple(sc) => zero_lines_in_simple(sc),
        Command::Pipeline(p) => {
            for c in &mut p.commands { zero_lines_in_command(c); }
        }
        Command::BraceGroup(seq) => zero_lines_in_sequence(seq),
        Command::Subshell { body } => zero_lines_in_sequence(body),
        Command::If(clause) => {
            zero_lines_in_sequence(&mut clause.condition);
            zero_lines_in_sequence(&mut clause.then_body);
            for branch in &mut clause.elif_branches {
                zero_lines_in_sequence(&mut branch.condition);
                zero_lines_in_sequence(&mut branch.body);
            }
            if let Some(b) = &mut clause.else_body { zero_lines_in_sequence(b); }
        }
        Command::While(clause) => {
            zero_lines_in_sequence(&mut clause.condition);
            zero_lines_in_sequence(&mut clause.body);
        }
        Command::For(clause) => zero_lines_in_sequence(&mut clause.body),
        Command::Select(clause) => zero_lines_in_sequence(&mut clause.body),
        Command::Case(clause) => {
            for item in &mut clause.items {
                if let Some(b) = &mut item.body { zero_lines_in_sequence(b); }
            }
        }
        Command::FunctionDef { body, .. } => zero_lines_in_command(body),
        Command::DoubleBracket { .. } => {} // no line field in TestExpr
        Command::Arith(_) => {}
        Command::ArithFor(clause) => zero_lines_in_sequence(&mut clause.body),
        Command::Redirected { inner, .. } => zero_lines_in_command(inner),
        Command::Coproc { body, .. } => zero_lines_in_command(body),
    }
}

fn zero_lines_in_simple(sc: &mut SimpleCommand) {
    match sc {
        SimpleCommand::Assign(_, line) => *line = 0,
        SimpleCommand::Exec(e) => e.line = 0,
    }
}

/// Assemble a `WordPart::CommandSub` for a `$(…)` expansion. Pushes
/// `Mode::CommandSub` itself, so callers must position the lexer at `$(`
/// (under any mode — the push ensures `$(` is scanned as atoms rather than
/// a pre-built Word token).
///
/// Owns the full push/pop lifecycle of its `CommandSub` frame and consumes
/// the opening `CmdSubOpen` atom plus (via `parse_subshell_sequence`) the
/// closing `)` token.
pub(crate) fn parse_command_sub(iter: &mut Lexer, quoted: bool) -> Result<WordPart, ParseError> {
    // 1. Push the mode and pull the opening atom.
    iter.push_mode(Mode::CommandSub { body_started: false });
    match iter.next_kind()? {
        Some(TokenKind::DeferredExpansion) => {
            // `$((` — arithmetic; defer to runtime.
            iter.pop_mode();
            return Err(ParseError::UnsupportedExpansion);
        }
        Some(TokenKind::CmdSubOpen) => {} // continue
        _ => {
            iter.pop_mode();
            return Err(ParseError::UnsupportedExpansion);
        }
    }

    // 2. Dispatch: empty body or non-empty body.
    let sequence = if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::RParen))) {
        // Empty body `$()` — consume `)` and construct the same Sequence the
        // production oracle yields via `parse_substitution_body("")` →
        // `unwrap_or_else(empty_sequence)`.
        iter.next_kind()?; // consume `)`
        Sequence {
            first: Command::Pipeline(Pipeline { negate: false, commands: Vec::new() }),
            rest: Vec::new(),
            background: false,
        }
    } else {
        // Non-empty body: delegate to parse_subshell_sequence (which consumes `)`).
        match parse_subshell_sequence(iter) {
            Ok(mut seq) => {
                // Zero all source-line fields to match the production oracle, which
                // parses the body in isolation after zeroing all token spans.
                zero_lines_in_sequence(&mut seq);
                seq
            }
            Err(e) => {
                // Pop the CommandSub frame before propagating.  Map UnsupportedCommand
                // (body-deferred constructs: `[[`, function-def, coproc, …) to
                // UnsupportedExpansion so parse_command_sub has a consistent return
                // type for all deferrals.
                iter.pop_mode();
                let mapped = match e {
                    ParseError::UnsupportedCommand => ParseError::UnsupportedExpansion,
                    other => other,
                };
                return Err(mapped);
            }
        }
    };

    // 3. Pop the CommandSub frame.
    iter.pop_mode();
    Ok(WordPart::CommandSub { sequence, quoted })
}

/// v251: assemble a `WordPart::ProcessSub` for a `<(…)`/`>(…)` process
/// substitution. Mirrors `parse_command_sub`: the body is a paren-delimited
/// command sequence lexed under `Mode::CommandSub` (the lexer's bare-`(` opener
/// path; the word-mode `ProcSubOpen` signal was already consumed by the caller).
/// `dir` comes from that signal.
pub(crate) fn parse_process_sub(iter: &mut Lexer, dir: ProcDir) -> Result<WordPart, ParseError> {
    iter.push_mode(Mode::CommandSub { body_started: false });
    match iter.next_kind()? {
        Some(TokenKind::CmdSubOpen) => {} // the real opener, scanned under CommandSub mode
        _ => { iter.pop_mode(); return Err(ParseError::UnsupportedExpansion); }
    }
    let sequence = if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::RParen))) {
        iter.next_kind()?; // consume `)`
        Sequence {
            first: Command::Pipeline(Pipeline { negate: false, commands: Vec::new() }),
            rest: Vec::new(),
            background: false,
        }
    } else {
        match parse_subshell_sequence(iter) {
            Ok(mut seq) => { zero_lines_in_sequence(&mut seq); seq }
            Err(e) => {
                iter.pop_mode();
                let mapped = match e {
                    ParseError::UnsupportedCommand => ParseError::UnsupportedExpansion,
                    other => other,
                };
                return Err(mapped);
            }
        }
    };
    iter.pop_mode();
    Ok(WordPart::ProcessSub { sequence, dir })
}

/// v252 T1: assemble `WordPart::ArrayLiteral` from atoms under
/// `Mode::ArrayLiteral`. The caller (`parse_word_command`'s `ArrayOpen` arm)
/// has already discarded the zero-width `ArrayOpen` signal; the cursor is on
/// `(`. Positional values (bare elements, brace-expanded via
/// `brace_expand_parts`) and — v252 T3 — explicit `[expr]=value` subscripted
/// elements (single value, NO brace expansion). Owns the full push/pop
/// lifecycle of its `ArrayLiteral` frame; pops on every exit path.
pub(crate) fn parse_array_literal(iter: &mut Lexer) -> Result<WordPart, ParseError> {
    iter.push_mode(Mode::ArrayLiteral { body_started: false, expect_subscript_eq: false, at_element_start: true });
    let mut elements: Vec<ArrayLiteralElement> = Vec::new();
    loop {
        match iter.peek_kind()? {
            Some(TokenKind::Blank) | Some(TokenKind::Newline) => { iter.next_kind()?; }
            Some(TokenKind::ArrayClose) => { iter.next_kind()?; break; }
            // v252 T3: an explicit `[expr]=value` element. The lexer emitted a
            // zero-width `LBracket` at element start; assemble the subscript Word
            // under `Mode::ParamSubscriptOperand` (identical to the `${a[i]}`
            // reader at parse_param_expansion), then the lexer consumes the
            // required `=` (or errors `ArrayLiteralMissingEquals`) as we scan the
            // value. Subscripted values keep single-value semantics — NO brace
            // expansion (matches `scan_array_literal`).
            Some(TokenKind::LBracket) => {
                iter.next_kind()?; // consume LBracket
                iter.push_mode(Mode::ParamSubscriptOperand { in_dquote: false });
                let sub_word = match parse_word(iter, false) {
                    Ok(w) => w,
                    Err(e) => { iter.pop_mode(); iter.pop_mode(); return Err(e); }
                };
                match iter.next_kind() {
                    Ok(Some(TokenKind::RBracket)) => {}
                    Ok(_) => { iter.pop_mode(); iter.pop_mode(); return Err(ParseError::UnsupportedExpansion); }
                    Err(e) => { iter.pop_mode(); iter.pop_mode(); return Err(ParseError::Lex(Box::new(e))); }
                }
                iter.pop_mode(); // ParamSubscriptOperand
                // The lexer consumes the required `=` (or errors
                // ArrayLiteralMissingEquals) as `parse_word_command` scans on.
                let value = match parse_word_command(iter, false) {
                    Ok(v) => v,
                    Err(e) => { iter.pop_mode(); return Err(e); }
                };
                // An empty value (`[i]= ` / `[i]=)`) re-tokenizes to a single
                // empty literal in the oracle (`scan_array_element_word`'s
                // `words.is_empty()` fallback), NOT an empty Word.
                let value = if value.0.is_empty() {
                    Word(vec![WordPart::Literal { text: String::new(), quoted: false }])
                } else {
                    value
                };
                elements.push(ArrayLiteralElement { subscript: Some(sub_word), value });
            }
            Some(_) => {
                // A positional value: parse_word_command stops at the next
                // Blank/Newline/ArrayClose (its catch-all arm breaks WITHOUT
                // consuming once something has been accumulated — see its doc
                // comment). Then brace-expand (bare elements only).
                let value = match parse_word_command(iter, false) {
                    Ok(v) => v,
                    Err(e) => { iter.pop_mode(); return Err(e); }
                };
                match brace_expand_parts(value.0) {
                    Ok(expansions) => {
                        for p in expansions {
                            elements.push(ArrayLiteralElement { subscript: None, value: Word(p) });
                        }
                    }
                    Err(e) => { iter.pop_mode(); return Err(ParseError::Lex(Box::new(e))); }
                }
            }
            // Unreachable: `scan_step_array_literal` errors with
            // `LexError::UnterminatedArrayLiteral` on EOF before ever handing
            // back a bare `None` token, so `peek_kind()` surfaces that as an
            // `Err` (caught by the `?` above) rather than `Ok(None)`.
            None => unreachable!("Mode::ArrayLiteral never yields None; EOF errors first"),
        }
    }
    iter.pop_mode();
    Ok(WordPart::ArrayLiteral(elements))
}

/// Parse the body of a backtick substitution: a `Sequence` of commands
/// terminated by `EndBacktick`.  Mirrors `parse_subshell_sequence` but stops
/// on `TokenKind::EndBacktick` instead of `Op(RParen)`.  Consumes the
/// `EndBacktick` token before returning.
fn parse_backtick_body_sequence(iter: &mut Lexer) -> Result<Sequence, ParseError> {
    let first = parse_command_then_pipeline(iter)?;

    let mut rest = Vec::new();
    loop {
        match iter.peek_kind()? {
            // EOF before EndBacktick → unterminated.
            None => return Err(ParseError::UnterminatedSubshell),
            // EndBacktick terminates the body — consume and return.
            Some(TokenKind::EndBacktick) => {
                iter.next_kind()?; // consume EndBacktick
                break;
            }
            Some(TokenKind::Op(Operator::Semi)) | Some(TokenKind::Newline) => {
                iter.next_kind()?; // consume `;` or newline
                skip_newlines(iter)?;
                // Trailing `;` or newline before the closing backtick — break cleanly.
                if matches!(iter.peek_kind()?, Some(TokenKind::EndBacktick)) {
                    iter.next_kind()?; // consume EndBacktick
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
                while matches!(
                    iter.peek_kind()?,
                    Some(TokenKind::Op(Operator::Semi)) | Some(TokenKind::Newline)
                ) {
                    iter.next_kind()?;
                }
                skip_newlines(iter)?;
                if matches!(iter.peek_kind()?, Some(TokenKind::EndBacktick)) {
                    iter.next_kind()?; // consume EndBacktick
                    return Ok(Sequence { first, rest, background: true });
                }
                if iter.peek_kind()?.is_none() {
                    return Err(ParseError::UnterminatedSubshell);
                }
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
            // Unexpected token after a complete command.
            Some(_) => return Err(ParseError::UnterminatedSubshell),
        }
    }

    Ok(Sequence { first, rest, background: false })
}

/// Assemble a `WordPart::CommandSub` for a `` `…` `` backtick substitution.
///
/// **OUTER call** (top mode is not `Backtick`): pushes `Mode::Backtick { depth: 0 }`
/// itself, so callers must position the lexer at the opening backtick (the push
/// ensures the backtick is scanned as atoms rather than a pre-built Word token).
///
/// **NESTED recursion** (top mode is already `Backtick`, i.e. this is a `` \` ``
/// child inside a backtick body): the LEXER already owns the SINGLE depth counter
/// and has emitted the child's `BeginBacktick` (incrementing depth in place), so
/// this call must NOT push another frame — it consumes the buffered `BeginBacktick`
/// and parses the child body under the same continuous depth.  The child's
/// matching `EndBacktick` (lexer depth −1) terminates it.
///
/// Owns the push/pop lifecycle of its `Backtick` frame on the OUTER path and
/// pops on ALL outer exit paths (Ok / empty / error).  Nested recursion neither
/// pushes nor pops (the single frame is owned by the outer call).
pub(crate) fn parse_backtick_sub(iter: &mut Lexer, quoted: bool) -> Result<WordPart, ParseError> {
    // Detect nested recursion: a `` \` `` child is entered while the top mode is
    // already `Backtick` (the lexer has flipped the single frame's depth up and
    // emitted the child's `BeginBacktick`).  Only the OUTER call owns push/pop.
    let pushed = !matches!(iter.current_mode(), Mode::Backtick { .. });

    // 1. Push the mode (outer only).  The fallible body runs inside an
    //    immediately-invoked closure so that EVERY exit path — including a
    //    `LexError` surfaced by `?` on a body pull — flows through the single
    //    `pop_mode` below (outer only).  Nested recursion neither pushes nor pops.
    if pushed {
        iter.push_mode(Mode::Backtick { depth: 0 });
    }
    let result = (|| -> Result<Sequence, ParseError> {
        // Pull the opening BeginBacktick atom.
        match iter.next_kind()? {
            Some(TokenKind::BeginBacktick) => {} // continue
            _ => return Err(ParseError::UnsupportedExpansion),
        }

        // Dispatch: empty body or non-empty body.
        if matches!(iter.peek_kind()?, Some(TokenKind::EndBacktick)) {
            // Empty body `` `` `` — consume EndBacktick and return the same Sequence
            // that the production oracle yields via `parse_substitution_body("")`.
            iter.next_kind()?; // consume EndBacktick
            Ok(Sequence {
                first: Command::Pipeline(Pipeline { negate: false, commands: Vec::new() }),
                rest: Vec::new(),
                background: false,
            })
        } else {
            // Non-empty body: parse_backtick_body_sequence consumes EndBacktick.
            let mut seq = parse_backtick_body_sequence(iter).map_err(|e| match e {
                ParseError::UnsupportedCommand => ParseError::UnsupportedExpansion,
                other => other,
            })?;
            // Zero all source-line fields to match the production oracle.
            zero_lines_in_sequence(&mut seq);
            Ok(seq)
        }
    })();

    // 2. Pop the Backtick frame (outer only) on EVERY path, then propagate.
    if pushed { iter.pop_mode(); }
    let sequence = result?;
    Ok(WordPart::CommandSub { sequence, quoted })
}

/// Assemble a `WordPart::Arith` for a `$(( … ))` arithmetic expansion.
///
/// Pushes `Mode::Arith { paren_depth: 0, in_dquote: quoted, body_started: false, for_header: false }`;
/// the mode's first scan consumes the opening `$((` and emits `ArithOpen`.  The
/// parser assembles the body `Word` (literal runs + embedded expansions), stops on
/// `ArithClose`, and on `ArithBail` rewinds to the `$((` start and re-drives as a
/// command substitution of a subshell (`$( (…) )`).  Owns the push/pop lifecycle;
/// pops the `Arith` frame on ALL exit paths.
enum ArithBodyOutcome { Closed(Word), Bail }

/// Assemble the arith body `Word` by pulling atoms until `ArithClose` (→ `Closed`)
/// or `ArithBail` (→ `Bail`, consumed here so the parser can rewind cleanly).
///
/// `parse_param_expansion` consumes its OWN `ParamOpen` token (which the lexer
/// already emits WITH `${` consumed — not zero-width, mirrors the
/// `scan_step_param_operand` precedent), so it's dispatched on a PEEK (not a
/// consume) exactly like `parse_word`'s `ParamOpen` arm.
///
/// `parse_command_sub`/`parse_backtick_sub`, by contrast, expect to consume a
/// FRESH `CmdSubOpen`/`BeginBacktick` scanned under their OWN pushed mode: the
/// signal atom `scan_step_arith` emits for `$(`/`` ` `` is zero-width (mirrors
/// `scan_step_param_operand`'s `$(cmd)` signal — cursor stays at `$`/`` ` ``), so
/// it must be discarded here via `next_kind()` BEFORE calling the sub-parser —
/// otherwise `push_mode` + the sub-parser's own `next_kind()` would just replay
/// the stale zero-width signal instead of triggering a real scan that consumes
/// `$(`/`` ` ``. This mirrors `parse_word`'s `CmdSubOpen`/`BeginBacktick` arms,
/// which consume via the generic `next_kind()` before dispatching.
///
/// Every embedded expansion inside an arith body is `quoted: true` — this matches
/// the production oracle `arith_string_to_word` (lexer.rs), which hardcodes `true`
/// for every recursive `scan_dollar_expansion`/backtick call regardless of the
/// outer `$((…))`'s own quoted flag (arithmetic contexts never word-split, so
/// nested parts behave as if quoted). Hence `true` is passed to the sub-parsers
/// here, not `_in_dquote` (which is the OUTER `$((…))`'s own quoted flag, only
/// used for the resulting `WordPart::Arith { quoted, .. }` in
/// `parse_arith_expansion`, not for what's inside the body).
fn parse_arith_body(iter: &mut Lexer, _in_dquote: bool) -> Result<ArithBodyOutcome, ParseError> {
    let mut parts: Vec<WordPart> = Vec::new();
    loop {
        match iter.peek_kind()? {
            Some(TokenKind::ArithClose) => { iter.next_kind()?; return Ok(ArithBodyOutcome::Closed(Word(parts))); }
            Some(TokenKind::ArithBail)  => { return Ok(ArithBodyOutcome::Bail); } // Task 5 consumes/rewinds
            Some(TokenKind::ParamOpen { .. })  => { parts.push(parse_param_expansion(iter, true)?); }
            Some(TokenKind::CmdSubOpen)        => { iter.next_kind()?; parts.push(parse_command_sub(iter, true)?); }
            Some(TokenKind::BeginBacktick)     => { iter.next_kind()?; parts.push(parse_backtick_sub(iter, true)?); }
            // Nested `$((` — mirrors the `CmdSubOpen`/`BeginBacktick` arms above: the
            // atom peeked here is the zero-width SIGNAL `scan_step_arith` emits without
            // consuming `$((` (cursor stays at `$`), so it must be discarded via
            // `next_kind()` BEFORE calling `parse_arith_expansion` — otherwise that
            // function's own `push_mode` + `next_kind()` would just replay the stale
            // signal instead of triggering a real scan that consumes `$((` under the
            // NEW frame (leading to a spurious extra recursion once the real `$((`
            // consumption is later mis-peeked as another nested open).
            // `true`, not `_in_dquote`: every embedded expansion inside an arith body is
            // `quoted: true` regardless of the outer body's own quoted flag (see the
            // doc comment above this function; matches the production oracle
            // `arith_string_to_word`/`scan_dollar_expansion`, which hardcodes `true`).
            Some(TokenKind::ArithOpen)         => { iter.next_kind()?; parts.push(parse_arith_expansion(iter, true)?); }
            Some(TokenKind::Lit { .. })        => {
                if let Some(TokenKind::Lit { text, quoted }) = iter.next_kind()? {
                    parts.push(WordPart::Literal { text, quoted });
                }
            }
            Some(TokenKind::DollarName { .. }) => {
                if let Some(TokenKind::DollarName { name, quoted }) = iter.next_kind()? {
                    let part = match name.as_str() {
                        "@" => WordPart::AllArgs { quoted, joined: false },
                        "*" => WordPart::AllArgs { quoted, joined: true },
                        "?" => WordPart::LastStatus { quoted },
                        _   => WordPart::Var { name, quoted },
                    };
                    parts.push(part);
                }
            }
            _ => return Err(ParseError::UnsupportedExpansion),
        }
    }
}

pub(crate) fn parse_arith_expansion(iter: &mut Lexer, quoted: bool) -> Result<WordPart, ParseError> {
    // Mark BEFORE pushing the Arith mode / consuming the `$((` opener, so an
    // `ArithBail` rewind returns to the `$((` start with the pre-push mode stack
    // (mark captures `self.modes`). `parse_arith_expansion` is always called at a
    // pull boundary (the parser dispatches on a peeked opener), so mark/rewind's
    // pull-boundary assert holds.
    let mark = iter.mark();
    iter.push_mode(Mode::Arith { paren_depth: 0, in_dquote: quoted, body_started: false, for_header: false });
    let result = (|| -> Result<ArithBodyOutcome, ParseError> {
        match iter.next_kind()? {
            Some(TokenKind::ArithOpen) => {}
            _ => return Err(ParseError::UnsupportedExpansion),
        }
        parse_arith_body(iter, quoted)
    })();
    iter.pop_mode();
    match result? {
        ArithBodyOutcome::Closed(body) => Ok(WordPart::Arith { body, quoted }),
        ArithBodyOutcome::Bail => {
            // The `$((` was really `$( (…) )` (a command-sub whose body starts with
            // a subshell): a depth-0 `)` not followed by `)` bailed the arith scan.
            // Rewind to the `$((` start, tell the lexer to tokenize that `$((` as
            // `$(` + `(`, and re-drive as a command substitution.
            iter.rewind(&mark);
            iter.set_retokenize_arith_as_cmdsub();
            parse_command_sub(iter, quoted)
        }
    }
}

/// v255: assemble a standalone `(( expr ))` arithmetic command at command
/// position. The atom scanner emits glued `((` as two `Op(LParen)` atoms and the
/// caller (`parse_command`) has already peeked both. Speculatively delimit the
/// body as arith (reusing v246's `Mode::Arith` + `parse_arith_body`): on the
/// matching `))` (`ArithClose`) build `Command::Arith(body)` and wrap trailing
/// redirects; on `ArithBail` (a depth-0 `)` not followed by `)`, e.g. `((cmd);
/// c2)`) rewind to before `((` and reparse as a nested subshell `( (…) )`
/// (matching bash's arith-command backoff). Mirrors `parse_arith_expansion`'s
/// mark/push/pop lifecycle; the `mark` is taken BEFORE consuming/pushing so a
/// bail rewind returns to the pre-`((` position with the pre-push mode stack.
///
/// No lexer change: consuming the two buffered `Op(LParen)` first, then pushing
/// `Mode::Arith { body_started: true }`, makes the next pull enter
/// `scan_step_arith`'s body loop directly — the `$((`-opener branch (and its
/// `$`-assert) is never reached.
///
/// `mark()` is taken here, AFTER `parse_command`'s `peek_kind`/`peek2_kind` `((`
/// lookahead — unlike the v248 mark-after-peek hazard (which involved
/// non-idempotent word-content scanning that could leave scanner flags mutated
/// from prior state), this is safe: the two peeked atoms are always
/// `Op(LParen)`, whose operator scan unconditionally ends in `boundary_reset()`
/// (`cmd_at_word_start = true`, etc.) regardless of what came before, and
/// `parse_command` is only ever entered at a genuine command-word boundary. So
/// the scanner flags that this `mark()` snapshots are identical to what a
/// fresh scan at the pre-`((` position would produce, and the `ArithBail`
/// `rewind` below re-scans correctly.
fn parse_arith_command(iter: &mut Lexer) -> Result<Command, ParseError> {
    let mark = iter.mark();
    iter.next_kind()?; // consume first `(` (buffered Op(LParen))
    iter.next_kind()?; // consume second `(`
    iter.push_mode(Mode::Arith { paren_depth: 0, in_dquote: false, body_started: true, for_header: false });
    let result = parse_arith_body(iter, false);
    iter.pop_mode();
    match result? {
        ArithBodyOutcome::Closed(body) => maybe_wrap_redirects(Command::Arith(body), iter),
        ArithBodyOutcome::Bail => {
            iter.rewind(&mark);
            parse_subshell(iter)
        }
    }
}

/// Skip over any `Newline` or `Blank` tokens without consuming anything else.
/// Mirrors `skip_newlines` in `command.rs`. The oracle's Word lexer never emits
/// `Blank`, so also skipping `Blank` here only affects the atom path — where a
/// `Blank` is exactly the inter-word/boundary whitespace the oracle folds away.
/// This makes every command-boundary caller (`&&`/`||`/`;`/newline connectors,
/// pipe stages, `parse_command` entry, and all compound-body boundaries) skip
/// the atom-path `Blank`s the oracle never sees.
///
/// v250 T3: this is ALSO the single lowest choke point for consuming a
/// `Newline` on the atom path (every caller either loops here directly, or
/// consumes one `Newline` itself as a connector token and then calls this
/// immediately after — see `parse_and_or`/`parse_backtick_body_sequence`/
/// `parse_subshell_sequence`). The lexer emits any pending heredoc-body groups
/// (`HeredocBodyBegin`…`End`) as atoms immediately following the line's
/// `Newline`, so this loop also drains those groups (via
/// `iter.push_heredoc_body`) wherever they appear interleaved with
/// `Newline`/`Blank` — otherwise the next `peek_kind`/`next_kind` call would
/// try to parse a stray `HeredocBodyBegin` as a command and error (or, worse,
/// spin if some caller loops on a non-progressing match).
fn skip_newlines(iter: &mut Lexer) -> Result<(), ParseError> {
    loop {
        match iter.peek_kind()? {
            Some(TokenKind::Newline | TokenKind::Blank) => { iter.next_kind()?; }
            Some(TokenKind::HeredocBodyBegin { .. }) => {
                let body = parse_heredoc_body(iter)?;
                iter.push_heredoc_body(body);
            }
            _ => break,
        }
    }
    Ok(())
}

/// v250 T3: after consuming a `Newline` OUTSIDE of `skip_newlines` (a few
/// call-sites consume their own connector token directly), drain any heredoc
/// body groups the lexer emitted for that line. Thin wrapper so those
/// call-sites read the same as the ones that go through `skip_newlines`.
/// (Most direct-`Newline`-consuming call-sites call `skip_newlines` themselves
/// immediately afterward, which already drains via the loop above; this
/// exists for the couple of sites that don't.)
fn collect_heredoc_bodies_after_newline(iter: &mut Lexer) -> Result<(), ParseError> {
    while matches!(iter.peek_kind()?, Some(TokenKind::HeredocBodyBegin { .. })) {
        let body = parse_heredoc_body(iter)?;
        iter.push_heredoc_body(body);
    }
    Ok(())
}

/// v250 T3: assemble one heredoc body atom group (`HeredocBodyBegin` … each
/// body Lit … `HeredocBodyEnd`) into a body `Word`. Task 3 handles LITERAL
/// bodies (the lexer emits a single already-merged `Lit{quoted:true}` atom
/// spanning every content line, or none at all for an empty body); Task 4
/// adds the expansion atom arms (`DollarName`/`ParamOpen`/`CmdSubOpen`/
/// `BeginBacktick`/`ArithOpen`) for expanding heredoc bodies.
///
/// IMPORTANT divergence from the brief's straw-man (verified against
/// `old_seq`, the oracle): the oracle's `collect_one_heredoc_body` does NOT
/// coalesce a literal heredoc body into one `Literal` — it pushes ONE
/// `Literal{line_content, quoted:true}` PLUS a SEPARATE
/// `Literal{"\n", quoted:true}` per content line (even when a line's content
/// is empty), so a 2-line body is a 4-element `Word`, not a 1-element one.
/// The atom lexer instead emits the WHOLE multi-line body as a single merged
/// `Lit` token (one Rust `String` with embedded `\n`s). So instead of
/// `push_lit`/`flush_lit` coalescing (which would collapse everything back
/// into one `Literal` and mismatch the oracle), `push_heredoc_literal_lines`
/// SPLITS that merged text back into the oracle's per-line
/// (content, "\n") `Literal` pairs.
fn parse_heredoc_body(iter: &mut Lexer) -> Result<Word, ParseError> {
    let expand = match iter.next_kind()? {
        Some(TokenKind::HeredocBodyBegin { expand }) => expand,
        _ => unreachable!("lexer emits a complete heredoc body group beginning with HeredocBodyBegin"),
    };
    if expand {
        parse_heredoc_body_expanding(iter)
    } else {
        parse_heredoc_body_literal(iter)
    }
}

/// v250 T3: assemble a LITERAL heredoc body. The lexer emits at most one merged
/// `Lit{quoted:true}` spanning every content line (with embedded `\n`s), which
/// `push_heredoc_literal_lines` SPLITS into the oracle's per-line
/// `(content, "\n")` `Literal` pairs (deliberately NOT coalesced).
fn parse_heredoc_body_literal(iter: &mut Lexer) -> Result<Word, ParseError> {
    let mut parts: Vec<WordPart> = Vec::new();
    loop {
        match iter.peek_kind()? {
            Some(TokenKind::HeredocBodyEnd) => { iter.next_kind()?; break; }
            Some(TokenKind::Lit { .. }) => {
                if let Some(TokenKind::Lit { text, quoted }) = iter.next_kind()? {
                    push_heredoc_literal_lines(&mut parts, &text, quoted);
                }
            }
            _ => unreachable!("lexer emits a complete literal heredoc body group (one Lit then End)"),
        }
    }
    Ok(Word(parts))
}

/// v250 T4: assemble an EXPANDING heredoc body from its per-atom stream. Mirrors
/// `parse_dquote`'s arms (the body is a line-oriented `"…"`-like context): the
/// expansion openers recurse through the SAME sub-parsers (parser-owned
/// recursion), and each pushes a sub-mode that scans the nested structure from
/// the cursor. `quoted:false` literal runs coalesce (like the oracle's `current`
/// buffer); `quoted:true` `Lit`s (escaped chars + the per-line `"\n"` separator)
/// are standalone parts (the oracle never merges them — verified against
/// `scan_expanding_body_line`/`collect_one_heredoc_body`).
fn parse_heredoc_body_expanding(iter: &mut Lexer) -> Result<Word, ParseError> {
    let mut parts: Vec<WordPart> = Vec::new();
    // Pending coalescible chunk — used ONLY for `quoted:false` literal runs.
    let mut acc: Option<(String, bool)> = None;
    loop {
        match iter.peek_kind()? {
            Some(TokenKind::HeredocBodyEnd) => { iter.next_kind()?; break; }
            Some(TokenKind::Lit { quoted: false, .. }) => {
                if let Some(TokenKind::Lit { text, .. }) = iter.next_kind()? {
                    push_lit(&mut acc, &mut parts, text, false);
                }
            }
            // A `quoted:true` `Lit` is an escaped char (`\$`/`` \` ``/`\\`) or the
            // per-line `"\n"` separator — the oracle pushes each as its OWN part.
            Some(TokenKind::Lit { quoted: true, .. }) => {
                if let Some(TokenKind::Lit { text, .. }) = iter.next_kind()? {
                    flush_lit(&mut acc, &mut parts);
                    parts.push(WordPart::Literal { text, quoted: true });
                }
            }
            Some(TokenKind::ParamOpen { .. }) => {
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_param_expansion(iter, true)?);
            }
            Some(TokenKind::CmdSubOpen) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_command_sub(iter, true)?);
            }
            Some(TokenKind::BeginBacktick) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_backtick_sub(iter, true)?);
            }
            Some(TokenKind::ArithOpen) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_arith_expansion(iter, true)?);
            }
            Some(TokenKind::DollarName { .. }) => {
                if let Some(TokenKind::DollarName { name, quoted: _ }) = iter.next_kind()? {
                    flush_lit(&mut acc, &mut parts);
                    parts.push(match name.as_str() {
                        "@" => WordPart::AllArgs { quoted: true, joined: false },
                        "*" => WordPart::AllArgs { quoted: true, joined: true },
                        "?" => WordPart::LastStatus { quoted: true },
                        _   => WordPart::Var { name, quoted: true },
                    });
                }
            }
            Some(TokenKind::DollarLit { .. }) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                parts.push(WordPart::Literal { text: "$".into(), quoted: true });
            }
            // `$[expr]` legacy arith inside the body — still deferred (Stage 2),
            // matching the dquote path's `DeferredExpansion` deferral.
            Some(TokenKind::DeferredExpansion) => return Err(ParseError::UnsupportedExpansion),
            _ => unreachable!("lexer emits only body-part atoms between HeredocBodyBegin and HeredocBodyEnd"),
        }
    }
    flush_lit(&mut acc, &mut parts);
    Ok(Word(parts))
}

/// v250 T3: split one accumulated heredoc-body `Lit` atom's text on embedded
/// `\n` into the oracle's per-line `Literal` PAIR shape: the line's content
/// (pushed even when empty) followed by a separate `"\n"` literal — mirroring
/// `collect_one_heredoc_body`'s two unconditional pushes per content line
/// (lexer.rs). These are deliberately NOT coalesced (the oracle never merges
/// them), so this must NOT reuse `push_lit`/`flush_lit`.
fn push_heredoc_literal_lines(parts: &mut Vec<WordPart>, text: &str, quoted: bool) {
    let mut rest = text;
    while let Some(idx) = rest.find('\n') {
        let (line, tail) = rest.split_at(idx);
        parts.push(WordPart::Literal { text: line.to_string(), quoted });
        parts.push(WordPart::Literal { text: "\n".to_string(), quoted });
        rest = &tail[1..];
    }
    // A trailing fragment with no newline shouldn't occur for a literal
    // heredoc body (the lexer always appends '\n' after every content line),
    // but guard defensively rather than silently drop trailing text.
    if !rest.is_empty() {
        parts.push(WordPart::Literal { text: rest.to_string(), quoted });
    }
}

/// Returns `true` if the token is a standalone `!` word (pipeline negation).
/// Mirrors `is_bang_word` in `command.rs`.
fn is_bang_word(tok: &TokenKind) -> bool {
    match tok {
        TokenKind::Word(w) => word_literal_text(w) == Some("!"),
        // v247 T5: under the atom command scanner a standalone `!` is a single
        // unquoted `Lit` atom (`! cmd` → `Lit "!"`, then a `Blank`). A glued
        // `!foo` is `Lit "!foo"` (text != "!") — correctly NOT a bang.
        TokenKind::Lit { text, quoted: false } => text == "!",
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
    keyword_from_str(text)
}

/// The SINGLE keyword table: maps a reserved-word text to its `Keyword`.  Both
/// the Word-token recognizer (`keyword_kind`) and the atom-stream recognizer
/// (`peek_leading_keyword`) delegate here so there is exactly one source of
/// truth.  Mirrors `command.rs`'s `keyword_of` text match.
pub(crate) fn keyword_from_str(text: &str) -> Option<Keyword> {
    match text {
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

/// Atom-stream keyword recognition WITHOUT consuming word content.  Skips any
/// leading inter-token `Blank`s (pure whitespace boundaries, never content),
/// then returns the keyword the NEXT command word represents — but ONLY when
/// that word is a single, BARE, unquoted `Lit` atom whose text is a reserved
/// word AND the atom immediately after it is a WORD BOUNDARY
/// (`Blank`/`Newline`/`Op`/`RedirFd`/`Heredoc`/EOF).  A non-boundary follower
/// (`QuoteRun`/`DollarName`/another `Lit`/`AssignPrefix`/`Tilde`/…) means the
/// word has more parts and is NOT a bare keyword (`iffy`, `if''`, `i$x`).
///
/// A legacy single `TokenKind::Word` token (non-atom callers) is recognized via
/// `keyword_kind`.
fn peek_leading_keyword(iter: &mut Lexer) -> Result<Option<Keyword>, ParseError> {
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
        iter.next_kind()?;
    }
    let kw = match iter.peek_kind()? {
        // Legacy Word token: delegate directly (it already carries its parts).
        Some(w @ TokenKind::Word(_)) => return Ok(keyword_kind(w)),
        Some(TokenKind::Lit { text, quoted: false }) => keyword_from_str(text),
        _ => None,
    };
    let Some(kw) = kw else { return Ok(None) };
    let boundary = matches!(
        iter.peek2_kind()?,
        None | Some(TokenKind::Blank)
            | Some(TokenKind::Newline)
            | Some(TokenKind::Op(_))
            | Some(TokenKind::RedirFd(_))
            | Some(TokenKind::Heredoc { .. })
    );
    Ok(if boundary { Some(kw) } else { None })
}

/// Consume the next command word, assembling it from atoms via
/// `parse_word_command` (atom path) or taking a legacy `TokenKind::Word` token
/// whole (non-atom path).  Callers that expect a keyword here must have already
/// verified it via `peek_leading_keyword` (which also skips leading blanks).
fn consume_command_word(iter: &mut Lexer) -> Result<Word, ParseError> {
    if matches!(iter.peek_kind()?, Some(TokenKind::Word(_))) {
        match iter.next_kind()? {
            Some(TokenKind::Word(w)) => Ok(w),
            _ => unreachable!("peek confirmed Word"),
        }
    } else {
        parse_word_command(iter, false)
    }
}

/// The keyword an already-CONSUMED token represents (for error reporting in the
/// connector loop).  Handles both a legacy `Word` token and a bare atom `Lit`.
fn keyword_of_consumed(token: &TokenKind) -> Option<Keyword> {
    match token {
        TokenKind::Word(_) => keyword_kind(token),
        TokenKind::Lit { text, quoted: false } => keyword_from_str(text),
        _ => None,
    }
}

/// Extract a `for`/`select` loop-variable name from an assembled `Word`: it must
/// be a single unquoted `Literal`.  Mirrors `for_variable_name`'s rule.
fn for_variable_name_word(w: &Word) -> Option<String> {
    if w.0.len() != 1 { return None; }
    let WordPart::Literal { text, quoted: false } = &w.0[0] else { return None; };
    if text.is_empty() { return None; }
    Some(text.clone())
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
/// Returns `UnsupportedCommand` for heredocs (deferred).
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
            // v250 T3/T4: heredoc (LITERAL `expand:false` quoted/escaped delimiter,
            // or EXPANDING `expand:true` bare delimiter). Consume the opener; the
            // body arrives as atoms after the line's newline and is attached in
            // source order by the final `attach_heredoc_bodies` walk
            // (`parse_sequence`). Build a provisional empty-body redirect now.
            let (expand, strip_tabs) = match iter.next_kind()? {
                Some(TokenKind::Heredoc { expand, strip_tabs, .. }) => (expand, strip_tabs),
                _ => unreachable!("peek confirmed Heredoc"),
            };
            Ok(vec![Redirection {
                fd: fd_prefix.unwrap_or(crate::command::RedirFd::Number(0)),
                op: crate::command::RedirOp::Heredoc { body: Word(vec![]), expand, strip_tabs },
            }])
        }
        Some(TokenKind::Op(op)) if crate::command::is_redirect_op(op) => {
            let op = *op;
            iter.next_kind()?; // consume the redirect operator
            // NOTE (v251 T3): a `<(`/`>(` procsub-defer guard used to live here
            // (pre-v251 T1, when a glued `<(`/`>(` still surfaced as
            // `Op(RedirIn|RedirOut)` immediately followed by `Op(LParen)`).
            // Since v251 T1 the lexer emits a dedicated `ProcSubOpen` atom for
            // a glued `<(`/`>(` and NEVER produces `Op(RedirIn|RedirOut)` when
            // the very next source char is `(` (see `scan_command_operator_atom`'s
            // `<`/`>` arms) — so that sequence is now unreachable here; removed
            // (confirmed via a temporary panic probe run across the full
            // `huck-syntax --lib` suite: never hit).
            //
            // The redirect target may be separated from the operator by an
            // inter-token `Blank` in the atom stream (`> out`); skip it. Then
            // ASSEMBLE the target from word atoms via `parse_word_command` (the
            // atom scanner emits `Lit`/quote/expansion atoms, never a single
            // `Word` token). A legacy `Word` token is still accepted for the
            // Word-mode path. Non-word tokens are the same errors as the oracle.
            if matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
                iter.next_kind()?;
            }
            let target = match iter.peek_kind()? {
                Some(TokenKind::Op(_)) => return Err(ParseError::RedirectTargetIsOperator),
                Some(TokenKind::Newline) | None => return Err(ParseError::MissingRedirectTarget),
                Some(TokenKind::Heredoc { .. }) => return Err(ParseError::RedirectTargetIsOperator),
                Some(TokenKind::RedirFd(_)) => return Err(ParseError::RedirectTargetIsOperator),
                Some(TokenKind::ArithBlock(..)) => return Err(ParseError::RedirectTargetIsOperator),
                Some(TokenKind::Word(_)) => match iter.next_kind()? {
                    Some(TokenKind::Word(word)) => word,
                    _ => unreachable!("peek confirmed Word"),
                },
                Some(_) => parse_word_command(iter, false)?,
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
/// redirect may appear before, between, or after words.  Heredocs return
/// `UnsupportedCommand` (deferred); here-strings (`<<<`) are handled.
///
/// Leading `NAME=value` words (and `NAME+=value` / `NAME[i]=value` forms)
/// become `inline_assignments`.  A line of ONLY assignments with NO redirects
/// produces `Command::Simple(SimpleCommand::Assign(…))`.  A command with
/// redirects but no program word produces an empty-program `ExecCommand`
/// (mirrors `finalize_stage`'s empty-remaining + redirects branch).
fn parse_simple(iter: &mut Lexer) -> Result<Command, ParseError> {
    let line = iter.current_line()?;
    parse_simple_with_leading_word(iter, line, None)
}

/// `parse_simple`, optionally seeded with an ALREADY-CONSUMED leading word
/// (used by the `name()` funcdef lookahead in `parse_command`: it must consume
/// the leading word to see whether `(` follows, and — when it does NOT —
/// needs to hand that already-consumed word to the ordinary simple-command
/// path rather than re-lexing it. Re-lexing would require a `mark`/`rewind`
/// spanning tokens already buffered by `parse_command`'s earlier peeks
/// (`ArithBlock`/`LParen`/heredoc/keyword checks all peek the same leading
/// token first), and `rewind` truncates history back to the mark's `pos` —
/// discarding that pre-existing lookahead and forcing a genuine re-scan under
/// whatever scanner flags (e.g. `cmd_at_word_start`) happen to hold at
/// rewind time, which are the POST-word-production flags, not the ones that
/// were live when the word was first scanned. For most words that merely
/// reproduces the same tokens, but the `name[...]`-non-assignment literal
/// fallback (v247 T4) sets `cmd_at_word_start = false` as it swallows the
/// bracket region, so a rewind-and-re-scan of e.g. `arr[$i]` loses the
/// swallow and re-lexes it as separate atoms — an oracle-parity regression.
/// Seeding this function with the already-consumed word sidesteps that
/// hazard entirely: the word is assembled exactly once, matching the
/// oracle's own one-pass "consume word, then check for `(`" shape.
///
/// `line` must be the line captured BEFORE `leading_word` was consumed (the
/// oracle's `ExecCommand::line` is the command's start line, not wherever the
/// cursor sits after the leading word has already been eaten).
fn parse_simple_with_leading_word(
    iter: &mut Lexer,
    line: u32,
    leading_word: Option<Word>,
) -> Result<Command, ParseError> {
    let mut all_words: Vec<Word> = Vec::new();
    let mut redirects: Vec<Redirection> = Vec::new();
    if let Some(w) = leading_word {
        all_words.push(w);
    }

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
            // `EndBacktick` terminates the body of a `` `…` `` substitution.
            | TokenKind::EndBacktick
        ) {
            break;
        }
        // Nested `` \` `` backtick child inside a backtick BODY — the lexer has
        // emitted a REAL `BeginBacktick` child-open token (single-frame depth
        // already incremented), cursor already past the `` ` ``. Recurse to
        // assemble a standalone Word carrying its `WordPart::CommandSub`.
        // (Glued adjacency `` a\`b\`c `` — one word with literal + CommandSub parts
        // — is not yet handled; deferred, untested at this level.)
        //
        // Guarded to `Mode::Backtick`: at TOP LEVEL (Command/DoubleQuote mode) a
        // leading `` ` `` is instead a ZERO-WIDTH signal (v247 T3), handled by
        // `parse_word_command`'s BeginBacktick arm below (which pre-consumes the
        // signal so `parse_backtick_sub` re-scans the real opening `` ` ``).
        if matches!(token, TokenKind::BeginBacktick)
            && matches!(iter.current_mode(), Mode::Backtick { .. })
        {
            let part = parse_backtick_sub(iter, false)?;
            all_words.push(Word(vec![part]));
            continue;
        }
        // Redirect tokens — parse in source order, extending the redirects
        // list.  Mirrors the `next_is_redirect` + `parse_trailing_redirects`
        // delegation in `parse_simple_stage`.
        if crate::command::next_is_redirect(iter)? {
            redirects.extend(parse_one_redirect(iter)?);
            continue;
        }
        // Skip inter-word blanks in the atom stream (v247 T2: the atom-command
        // scanner emits `Blank` for whitespace instead of folding it into word
        // boundaries the way the legacy fat Word-Lexer does).
        if matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
            iter.next_kind()?;
            continue;
        }
        // v253 T3: an inline-assignment PREFIX immediately followed by `[[`
        // (`FOO=1 [[ … ]]`, `A=1 B=2 [[ … ]]`) routes to `Command::DoubleBracket`
        // with the peeled assignments as `inline_assignments`, mirroring the
        // oracle's assignment-peel → `parse_double_bracket_with_assigns`
        // dispatch (command.rs `parse_command_inner`). Forward-only and
        // rewind-free: `peek_leading_keyword` skips blanks and PEEKS the `[[`
        // keyword (consuming nothing), so on a match we dispatch straight into
        // `parse_double_bracket`, which re-reads the still-pending `[[` — we
        // never rewind over the already-assembled assignment words (heeding the
        // mark-after-peek hazard). Guarded to a PURE assignment prefix with no
        // redirects collected yet: the oracle only peels consecutive assignment
        // WORDS, so a redirect (or a non-assignment word) before `[[` breaks the
        // peel and this command falls through to the ordinary simple path.
        if !all_words.is_empty()
            && redirects.is_empty()
            && all_words.iter().all(crate::command::is_assignment_word)
            && peek_leading_keyword(iter)? == Some(Keyword::DoubleBracketOpen)
        {
            let mut assigns: Vec<Assignment> = Vec::new();
            for w in all_words {
                match crate::command::try_split_assignment(w) {
                    Ok(a) => assigns.push(a),
                    Err(_) => unreachable!("is_assignment_word confirmed assignment shape"),
                }
            }
            return parse_double_bracket(iter, assigns);
        }
        // A command word begins here — assemble it from atoms (v247 T3: plain/
        // quoted/glued literals + command-position expansions `$x`/`${…}`/`$(…)`/
        // `$((…))`/`~`/`"…"`). A LEADING `` ` `` is handled by the BeginBacktick
        // arm above (standalone word); `parse_word_command`'s BeginBacktick arm
        // handles only backticks glued after earlier word content.
        if matches!(
            iter.peek_kind()?,
            Some(
                TokenKind::Lit { .. }
                    | TokenKind::DollarLit { .. }
                    | TokenKind::QuoteRun { .. }
                    | TokenKind::DollarName { .. }
                    | TokenKind::ParamOpen { .. }
                    | TokenKind::CmdSubOpen
                    | TokenKind::ProcSubOpen { .. }
                    | TokenKind::BeginBacktick
                    | TokenKind::ArithOpen
                    | TokenKind::Tilde(_)
                    | TokenKind::BeginDquote
                    | TokenKind::AssignPrefix { .. }
            )
        ) {
            all_words.push(parse_word_command(iter, false)?);
            continue;
        }
        // Consume the token.
        let kind = iter.next_kind()?.unwrap();
        match kind {
            // Legacy Word token (Word-mode path, still used by non-atom
            // callers — `old_seq`/production do NOT reach this arm via
            // `parse_sequence`, but it keeps `parse_simple` total).
            TokenKind::Word(word) => all_words.push(word),
            // v251 T3: a stray `Op(LParen)` reaching a word-expected position
            // (e.g. `echo \<(x)` — the backslash escapes `<` to a plain
            // literal, so the following `(` is never folded into a
            // `ProcSubOpen`/redirect target and surfaces bare here) is the
            // oracle's own generic "unexpected token where a word was
            // expected" outcome, NOT a deferred construct — the production
            // parser returns `UnexpectedToken` for a bare trailing `LParen`
            // after words (command.rs, e.g. `parse(vec![w_tok("echo"),
            // w_tok("hi"), Op(LParen)])` asserts `Err(UnexpectedToken)`).
            // Match it for parity instead of the generic deferral below —
            // UNLESS the last word is assignment-shaped (`name=`/`name+=`).
            // NOTE (v252 T1): a `(` glued RIGHT AFTER `name=`/`name+=` (the
            // real array-literal case, `a=(1 2 3)`) never reaches this arm at
            // all any more — the assignment-prefix scan already emits the
            // zero-width `ArrayOpen` signal there, which `parse_word_command`
            // glues into the SAME word (see its `ArrayOpen` arm and
            // `parse_array_literal`), so it never surfaces as a standalone
            // `Op(LParen)` token here. What DOES still reach this branch is a
            // bare `(` separated by whitespace/boundary from an
            // assignment-shaped word — e.g. `a=b () { :; }` (see
            // `atoms_function_assignment_name_divergence`) — which stays a
            // deliberate `UnsupportedCommand` deferral (unrelated to array
            // literals; the oracle's `(` funcdef-name check fires before its
            // assignment check for `a=b`, a shape the atom path doesn't
            // reconcile).
            TokenKind::Op(Operator::LParen) => {
                if all_words.last().is_some_and(crate::command::is_assignment_word) {
                    return Err(ParseError::UnsupportedCommand);
                }
                return Err(ParseError::UnexpectedToken);
            }
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
///   `{` → `parse_brace_group` (Task 1), `(` → `parse_subshell` (Task 2),
///   `if` (Task 3), `while`/`until` (Task 4), `for`/`select` (Task 5),
///   `case` (Task 6).
/// All other compound-opening keywords → `UnsupportedCommand`.
fn parse_command(iter: &mut Lexer) -> Result<Command, ParseError> {
    // Skip leading newlines (mirrors `parse_command_inner` command.rs:1019).
    skip_newlines(iter)?;
    // EOF with no token.
    if iter.peek_kind()?.is_none() {
        return Err(ParseError::MissingCommand);
    }
    // `(( expr ))` at command position.  The Word-lexer emits a single
    // `ArithBlock`; the atom scanner instead emits two GLUED `Op(LParen)` atoms
    // (no `Blank` between) — v255 handles those via `parse_arith_command`
    // (speculative arith with an `ArithBail`→nested-subshell backoff).  A SPACED
    // `( (` keeps a `Blank` between the two `(`, so it never matches here and
    // flows to the single-`(` subshell arm below (never arith).
    if matches!(iter.peek_kind()?, Some(TokenKind::ArithBlock(..))) {
        return Err(ParseError::UnsupportedCommand);
    }
    if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen)))
        && matches!(iter.peek2_kind()?, Some(TokenKind::Op(Operator::LParen)))
    {
        return parse_arith_command(iter);
    }
    // Bare `(` → subshell.
    if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen))) {
        return parse_subshell(iter);
    }
    // A leading heredoc (`<<EOF`/`<<'EOF'`/`<<-EOF`) at command position is NOT
    // deferred (v250 T3): like `<<<` (v249), it flows to `parse_simple` as a
    // leading redirect on an empty-words command reading stdin from the
    // heredoc body, matching the oracle (which falls through to
    // `parse_pipeline` → `parse_simple_stage`).
    // Reserved word (keyword): dispatch to compound parsers; unknown → defer.
    // Atom-aware: the leading command word is a SEQUENCE OF ATOMS
    // (`Lit("if")` + boundary), never a single `Word` token, so recognition
    // goes through `peek_leading_keyword` (which handles both paths).
    match peek_leading_keyword(iter)? {
        Some(Keyword::LBrace) => return parse_brace_group(iter),
        Some(Keyword::If)     => return parse_if(iter),
        Some(Keyword::While) | Some(Keyword::Until) => return parse_while(iter),
        Some(Keyword::For)    => return parse_for(iter),
        Some(Keyword::Select) => return parse_select(iter),
        Some(Keyword::Case)   => return parse_case(iter),
        Some(Keyword::Function) => return parse_function_keyword_def(iter),
        // v253 T3-fix: wrap a trailing redirect on a command-position
        // `[[ … ]]` (`[[ -f a ]] >out` → `Redirected{DoubleBracket, [>out]}`),
        // exactly like every other atom-path compound (brace/if/while/for/
        // select/case/subshell) and the oracle (command.rs:1050-1053:
        // `let cmd = parse_double_bracket(iter)?; maybe_wrap_redirects(cmd, iter)`).
        // NOTE: the wrap lives HERE, not inside `parse_double_bracket`, because
        // the inline-assignment dispatch site (`FOO=hi [[ … ]]` in
        // `parse_simple_with_leading_word`) must stay UNWRAPPED to match the
        // oracle's unwrapped `parse_double_bracket_with_assigns` (command.rs:1111).
        Some(Keyword::DoubleBracketOpen) => {
            let cmd = parse_double_bracket(iter, Vec::new())?;
            return maybe_wrap_redirects(cmd, iter);
        }
        Some(_) => return Err(ParseError::UnsupportedCommand),
        None => {}
    }
    // Function definition `name() compound` (POSIX form). The oracle consumes
    // the leading word then checks for `(`; the Word-lexer ate any space, so
    // `f()` and `f ()` both reach it with `(` next. The atom stream keeps the
    // `Blank` explicit, so mirror the oracle via consume-name/skip-Blank/
    // check-`(`. Only a bare word (`Lit`/legacy `Word`) can start a name.
    //
    // NOT a `mark`/`rewind` speculation: by the time we get here, the earlier
    // `ArithBlock`/`LParen`/heredoc/keyword peeks in this same function have
    // already scanned (and buffered) this leading token, mutating scanner
    // state (`cmd_at_word_start`) as a side effect of producing it. A
    // `rewind` back to a mark taken here would truncate that pre-existing
    // buffered lookahead and force a genuine re-scan under the CURRENT
    // (post-production) flags rather than the ones live when the token was
    // first produced — for the `name[...]`-non-assignment literal fallback
    // (v247 T4) that flips `cmd_at_word_start` to `false`, so a rewind-driven
    // re-scan of e.g. `arr[$i]` would re-lex it as separate atoms instead of
    // the swallowed single literal (oracle-parity regression). So: commit to
    // consuming the word exactly once, and when it is NOT a funcdef, hand the
    // already-assembled word straight to `parse_simple_with_leading_word`
    // instead of rewinding and re-parsing it.
    // v252 T4: an ASSIGNMENT-PREFIX leading atom (`a+=`/`a[i]=`/`a[i]+=`, emitted
    // by the lexer as a zero-width `AssignPrefix` atom, unlike the plain `a=`
    // form which is a `Lit("a=")`) must ALSO reach this funcdef-lookahead path,
    // NOT fall through to `parse_simple`. Otherwise `a+=(1)(2)` / `a[0]=(1)(2)`
    // (a CLOSED array literal glued before a second `(`) would reach
    // `parse_simple`'s trailing-`Op(LParen)` arm → `UnsupportedCommand`, while
    // the oracle attempts `parse_function_def` on the whole assembled word and
    // gets `FunctionName` (a multi-part / non-Literal word is not a valid
    // function name). Admitting `AssignPrefix` here routes those through the
    // SAME `consume_command_word` + funcdef-attempt logic below for parity. This
    // does NOT affect ordinary array/scalar assignments (`a+=(1 2)`, `a[0]=(1 2)`,
    // `a[i]=x`) — they have no following second `(`, so the funcdef attempt is
    // never entered and they fall through to `parse_simple_with_leading_word`.
    if matches!(
        iter.peek_kind()?,
        Some(TokenKind::Word(_))
            | Some(TokenKind::Lit { quoted: false, .. })
            | Some(TokenKind::AssignPrefix { .. })
    ) {
        let line = iter.current_line()?;
        let name_word = consume_command_word(iter)?;
        while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) { iter.next_kind()?; }
        // A `(` glued right after `name=`/`name+=` (the real array-literal
        // case, `a=(1 2 3)`) never reaches this check at all — the
        // assignment-prefix scan already emitted the zero-width `ArrayOpen`
        // signal there, which `parse_word_command` glues into the SAME word
        // (see its `ArrayOpen` arm and `parse_array_literal`), so `peek_kind()`
        // sees `ArrayOpen`, not `Op(LParen)`, here regardless of any guard.
        //
        // What DOES reach this point with `peek_kind() == Op(LParen)` is a
        // standalone `(` that is NOT array-literal glue: either (a) a `(`
        // after a CLOSED array literal (`a=(one)(two)`, `a+=(1)(2)`,
        // `a[0]=(1)(2)`) or a subshell after an empty/plain scalar value
        // (`a= (subshell)`, `a=b (subshell)`, `a+= (subshell)`), or (b) the
        // funcdef form `name ()`. The oracle (command.rs) attempts
        // `parse_function_def` UNCONDITIONALLY whenever the leading word is
        // followed by `(` — its own `valid_function_name_text` is what rejects
        // non-funcdef shapes, and it accepts a word ONLY when it is a SINGLE
        // unquoted `Literal` that is not a keyword. So for EVERY other assembled
        // word — a multi-part word (`[Literal("a="), ArrayLiteral(..)]`,
        // `[AssignPrefix, ArrayLiteral(..)]`) OR a single-part `AssignPrefix`
        // word (`[AssignPrefix]`, which is NOT a `Literal`) — the oracle's
        // `parse_function_def` correctly falls through to `FunctionName`, so
        // attempting it on the atom path is SAFE and CORRECT (converges on the
        // same `FunctionName` error).
        //
        // The ONE shape that must still be diverted away from
        // `parse_function_def` is a single-part unquoted-`Literal` assignment
        // word (`a=b`, `a=`): `valid_function_name_text` accepts it as a name
        // (it does NOT special-case the `=`-containing spelling), so the oracle
        // actually ACCEPTS `a=b () {…}` as `FunctionDef{name:"a=b"}` — a KNOWN,
        // PINNED v248 divergence (see `atoms_function_assignment_name_divergence`):
        // the atom path deliberately keeps deferring that one case (bash itself
        // rejects it as a syntax error, so the divergence is judged closer to
        // bash). The skip condition is therefore precisely "an assignment word
        // the oracle WOULD accept as a function name" =
        // `is_assignment_word(&w) && valid_function_name_text(&w).is_some()`,
        // which is true ONLY for the single-Literal assignment shape and false
        // for the array/subscript (`ArrayLiteral`) and `AssignPrefix` shapes —
        // so those correctly reach the funcdef attempt and get `FunctionName`
        // parity. `parse_simple_with_leading_word` below is what actually
        // assembles ordinary assignments (`a=(1 2 3)`, `a+=(1 2)`, `a[i]=x`),
        // which have no following `(` and never enter the funcdef attempt.
        let oracle_accepts_as_name = crate::command::is_assignment_word(&name_word)
            && crate::command::valid_function_name_text(&name_word).is_some();
        if !oracle_accepts_as_name
            && matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen)))
        {
            return parse_function_def(name_word, iter);
        }
        return parse_simple_with_leading_word(iter, line, Some(name_word));
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
    // Count leading `!` words (each one flips the negate flag). Under the atom
    // scanner successive bangs are separated by `Blank` atoms (`! ! a`), so skip
    // any inter-token blanks after each bang before checking for the next one.
    let mut bangs = 0usize;
    while iter.peek_kind()?.map(is_bang_word).unwrap_or(false) {
        iter.next_kind()?; // consume `!`
        bangs += 1;
        while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
            iter.next_kind()?;
        }
    }
    let negate = bangs % 2 == 1;

    // Parse the first stage command (may be simple or compound).
    let first = parse_command(iter)?;

    // A trailing inter-token `Blank` may sit between a compound command's
    // terminator (e.g. `fi`, `}`, `)`) and a following `|` (the atom scanner
    // emits it; simple commands already swallow it in `parse_simple`).
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
        iter.next_kind()?;
    }

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
        while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
            iter.next_kind()?;
        }
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
        // Inter-token `Blank` (atom path) sitting between a command and its
        // connector/terminator is not content — skip it so the stop checks and
        // connector dispatch below see the real token (the oracle's Word lexer
        // never emits a `Blank` here).
        while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
            iter.next_kind()?;
        }
        // ── Stop check 1: before consuming any connector (mirrors ~890) ──────
        // Atom-aware keyword recognition (a bare `Lit` keyword, not a `Word`).
        if peek_leading_keyword(iter)?.map(|k| stop_at.contains(&k)).unwrap_or(false) {
            break;
        }
        match iter.peek_kind()? {
            // EOF — end of list.
            None => break,
            // Case-clause terminators — break without consuming.
            Some(TokenKind::Op(
                Operator::DoubleSemi | Operator::SemiAmp | Operator::DoubleSemiAmp,
            )) => break,
            _ => {}
        }

        // Consume the connector token.
        let token = iter.next_kind()?.unwrap();
        match token {
            // ── `&` — background / Amp separator ────────────────────────────
            TokenKind::Op(Operator::Background) => {
                // Skip any newlines emitted after heredoc bodies (mirrors oracle),
                // plus inter-token `Blank`s (v247 T5: the atom scanner emits a
                // `Blank` between `&` and what follows — `cmd & &`, `p & q` — where
                // the oracle's Word lexer had none). Skipping both lets the
                // trailing-`&` / `& &` / `& cmd` decision below see the real token.
                // `skip_newlines` (v250 T3) also drains any heredoc-body atom
                // groups the lexer emitted after a `Newline` consumed here.
                skip_newlines(iter)?;
                // ── Stop check 2: stop_at keyword after `&` (~917) ────────
                // Atom-aware: compute before the borrow-holding match below.
                let stop_kw =
                    peek_leading_keyword(iter)?.map(|k| stop_at.contains(&k)).unwrap_or(false);
                match iter.peek_kind()? {
                    // Nothing follows → trailing `&`: background the whole sequence.
                    None => {
                        background = true;
                        break;
                    }
                    _ if stop_kw => {
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
                if peek_leading_keyword(iter)?.map(|k| stop_at.contains(&k)).unwrap_or(false) {
                    break;
                }
                match iter.peek_kind()? {
                    None => break,
                    Some(TokenKind::Op(
                        Operator::DoubleSemi | Operator::SemiAmp | Operator::DoubleSemiAmp,
                    )) => break,
                    _ => {}
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
                if let Some(kw) = keyword_of_consumed(&other) {
                    return Err(ParseError::UnexpectedKeyword(kw.name().to_string()));
                }
                return Err(ParseError::UnexpectedToken);
            }
        }
    }

    Ok(Sequence { first, rest, background })
}

/// v250 T3: fill every still-empty `RedirOp::Heredoc { body }` in `redirects`
/// (in source order) from `bodies`. A body is "still empty" (`Word(vec![])`)
/// exactly when `parse_one_redirect` built it as a provisional placeholder;
/// an ALREADY-filled one (can't happen on the atom path today, but keeps this
/// idempotent) is left alone.
fn fill_redirects(redirects: &mut [Redirection], bodies: &mut impl Iterator<Item = Word>) {
    for r in redirects.iter_mut() {
        if let RedirOp::Heredoc { body, .. } = &mut r.op {
            if body.0.is_empty() {
                if let Some(next) = bodies.next() {
                    *body = next;
                }
            }
        }
    }
}

/// v250 T3: fill every still-empty heredoc body reachable from `cmd`, in a
/// left-to-right pre-order that matches the lexer's emission order (each
/// heredoc's body atoms are emitted right after the `Newline` ending the line
/// its opener appeared on, and commands/redirects are parsed in that same
/// source order). EXHAUSTIVE over every `Command` variant — no `_ =>`
/// wildcard, so a future variant can't silently drop a body.
fn fill_command(cmd: &mut Command, bodies: &mut impl Iterator<Item = Word>) {
    match cmd {
        Command::Simple(SimpleCommand::Assign(_, _)) => {
            // No redirects possible on a bare-assignment stage (parse_simple's
            // `Assign` arm is only reached when `redirects.is_empty()`).
        }
        Command::Simple(SimpleCommand::Exec(exec)) => {
            fill_redirects(&mut exec.redirects, bodies);
        }
        Command::Pipeline(pipeline) => {
            for stage in pipeline.commands.iter_mut() {
                fill_command(stage, bodies);
            }
        }
        Command::If(clause) => {
            fill_sequence(&mut clause.condition, bodies);
            fill_sequence(&mut clause.then_body, bodies);
            for elif in clause.elif_branches.iter_mut() {
                fill_sequence(&mut elif.condition, bodies);
                fill_sequence(&mut elif.body, bodies);
            }
            if let Some(else_body) = clause.else_body.as_mut() {
                fill_sequence(else_body, bodies);
            }
        }
        Command::While(clause) => {
            fill_sequence(&mut clause.condition, bodies);
            fill_sequence(&mut clause.body, bodies);
        }
        Command::For(clause) => {
            fill_sequence(&mut clause.body, bodies);
        }
        Command::Case(clause) => {
            for item in clause.items.iter_mut() {
                if let Some(body) = item.body.as_mut() {
                    fill_sequence(body, bodies);
                }
            }
        }
        Command::BraceGroup(seq) => fill_sequence(seq, bodies),
        Command::Subshell { body } => fill_sequence(body, bodies),
        Command::FunctionDef { body, .. } => fill_command(body, bodies),
        // `[[ … ]]` carries no nested `Sequence`/redirect list of its own —
        // any trailing redirects on it are on the enclosing `Redirected`
        // wrapper, handled there. Not reachable from the atom parser today
        // (still deferred), but exhaustiveness must not skip it.
        Command::DoubleBracket { .. } => {}
        // `((expr))` is a bare arithmetic `Word`, no redirect list of its own.
        // Not reachable from the atom parser today (still deferred).
        Command::Arith(_) => {}
        // C-style `for ((init;cond;step))`: only `body` is a `Sequence`; the
        // header sections are bare `Word`s, not redirect-bearing. Not
        // reachable from the atom parser today (still deferred).
        Command::ArithFor(clause) => {
            fill_sequence(&mut clause.body, bodies);
        }
        Command::Select(clause) => {
            fill_sequence(&mut clause.body, bodies);
        }
        Command::Redirected { inner, redirects } => {
            // Source order: the wrapped command's own (possibly nested)
            // heredocs appear before the compound's OWN trailing redirects
            // (`{ …<<A…; } <<B` — A's body precedes B's in the source).
            fill_command(inner, bodies);
            fill_redirects(redirects, bodies);
        }
        // `coproc [NAME] command`: recurse into the wrapped command. Not
        // reachable from the atom parser today (still deferred).
        Command::Coproc { body, .. } => fill_command(body, bodies),
    }
}

/// v250 T3: `fill_command` over every command in a `Sequence` (the first
/// pipeline/command plus each connector-joined one), in source order.
fn fill_sequence(seq: &mut Sequence, bodies: &mut impl Iterator<Item = Word>) {
    fill_command(&mut seq.first, bodies);
    for (_, cmd) in seq.rest.iter_mut() {
        fill_command(cmd, bodies);
    }
}

/// Entry point for the new flat command-list parser.  Mirrors `parse` /
/// `parse_cursor` in `command.rs`.
///
/// Returns `Ok(None)` on empty input (newlines only or EOF).
pub(crate) fn parse_sequence(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError> {
    // Skip leading newlines AND inter-token blanks (mirrors `parse_cursor` →
    // `skip_newlines`). The atom scanner emits a `Blank` where the oracle folds
    // whitespace, so a blank-only / blank+comment line must reduce to `Ok(None)`
    // exactly as the oracle does (which never sees a `Blank`).
    skip_newlines(iter)?;
    if iter.peek_kind()?.is_none() {
        return Ok(None);
    }
    let mut seq = parse_and_or(iter, &[])?;
    // A leftover trailing `Blank` (atom path only — e.g. `"a; "`) is NOT content;
    // skip it so the stray-terminator check below matches the oracle.
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
        iter.next_kind()?;
    }
    // Mirror `parse_cursor`: a stray terminator (`;;`/`;&`/`;;&`) left after
    // the top-level sequence → `UnexpectedToken`.
    if iter.peek_kind()?.is_some() {
        return Err(ParseError::UnexpectedToken);
    }
    // v250 T3: attach every heredoc body collected along the way (in source
    // order == emission order) to its still-empty placeholder.
    let mut bodies = iter.take_heredoc_bodies().into_iter();
    fill_sequence(&mut seq, &mut bodies);
    Ok(Some(seq))
}

/// Expects a specific keyword token; returns `on_missing` if the next token
/// is not the expected keyword.  Mirrors `expect_keyword` in `command.rs`.
fn expect_keyword(
    iter: &mut Lexer,
    expected: Keyword,
    on_missing: ParseError,
) -> Result<(), ParseError> {
    // Atom-aware: `peek_leading_keyword` skips leading blanks and matches a bare
    // `Lit` keyword (or a legacy `Word` token).  On a match, CONSUME the whole
    // keyword word via `consume_command_word` (a single `Lit` atom or a `Word`).
    if peek_leading_keyword(iter)? != Some(expected) {
        return Err(on_missing);
    }
    consume_command_word(iter)?;
    Ok(())
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
    loop {
        // A trailing inter-token `Blank` may sit between the compound's
        // terminator and its redirect (`fi >f`); `next_is_redirect` does not
        // see through it, so skip blanks before each probe.
        while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
            iter.next_kind()?;
        }
        if !crate::command::next_is_redirect(iter)? {
            break;
        }
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
    while peek_leading_keyword(iter)? == Some(Keyword::Elif) {
        consume_command_word(iter)?; // consume `elif`
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

    let else_body = if peek_leading_keyword(iter)? == Some(Keyword::Else) {
        consume_command_word(iter)?; // consume `else`
        Some(parse_compound_section(iter, &[Keyword::Fi], ParseError::UnterminatedIf)?)
    } else {
        None
    };

    expect_keyword(iter, Keyword::Fi, ParseError::UnterminatedIf)?;
    let clause = IfClause { condition, then_body, elif_branches, else_body };
    maybe_wrap_redirects(Command::If(Box::new(clause)), iter)
}

/// Validates a `for`/`select` loop variable name token.  Mirrors
/// `for_variable_name` in `command.rs`: must be an unquoted single-literal Word.
fn for_variable_name(token: &TokenKind) -> Option<String> {
    let TokenKind::Word(w) = token else { return None };
    if w.0.len() != 1 { return None; }
    let WordPart::Literal { text, quoted: false } = &w.0[0] else { return None; };
    if text.is_empty() { return None; }
    Some(text.clone())
}

/// Skips `;`/newline separators before `do`, then consumes `do`, the loop body,
/// and `done`.  Returns the parsed body `Sequence`.  Shared by `parse_for` and
/// `parse_select`.  Mirrors `parse_do_body_done` (~1522) in `command.rs`.
fn parse_do_body_done(iter: &mut Lexer) -> Result<Sequence, ParseError> {
    loop {
        match iter.peek_kind()? {
            Some(TokenKind::Op(Operator::Semi)) => { iter.next_kind()?; }
            // v250 T3: a `Newline` consumed here may be immediately followed
            // by a heredoc-body atom group the lexer emitted for the line —
            // drain it before continuing, or the next `peek_kind` would see a
            // stray `HeredocBodyBegin`.
            Some(TokenKind::Newline) => {
                iter.next_kind()?;
                collect_heredoc_bodies_after_newline(iter)?;
            }
            _ => break,
        }
    }
    expect_keyword(iter, Keyword::Do, ParseError::UnterminatedLoop)?;
    let body = parse_compound_section(iter, &[Keyword::Done], ParseError::UnterminatedLoop)?;
    expect_keyword(iter, Keyword::Done, ParseError::UnterminatedLoop)?;
    Ok(body)
}

/// Trim a for-header section `Word` to match the oracle's `s.trim()` + empty-⇒-`None`.
/// Trims leading whitespace from the first `Literal` part and trailing whitespace from
/// the last `Literal` part (dropping parts that become empty). No parts ⇒ `None`.
fn trim_section(word: &Word) -> Option<Word> {
    let mut parts: Vec<WordPart> = word.0.clone();
    // Trim the leading Literal.
    if let Some(WordPart::Literal { text, quoted }) = parts.first().cloned() {
        let trimmed = text.trim_start().to_string();
        if trimmed.is_empty() { parts.remove(0); } else { parts[0] = WordPart::Literal { text: trimmed, quoted }; }
    }
    // Trim the trailing Literal.
    if let Some(WordPart::Literal { text, quoted }) = parts.last().cloned() {
        let trimmed = text.trim_end().to_string();
        let last = parts.len() - 1;
        if trimmed.is_empty() { parts.pop(); } else { parts[last] = WordPart::Literal { text: trimmed, quoted }; }
    }
    if parts.is_empty() { None } else { Some(Word(parts)) }
}

/// Assemble the for-header sections by pulling atoms until `ArithClose`, splitting on
/// `ArithSemi`. Mirrors `parse_arith_body`'s part arms (all `quoted:true`). Returns the
/// section `Word`s (≥1). `ArithBail` (a depth-0 `)` not followed by `)`) ⇒ the header
/// never closed ⇒ `UnterminatedLoop` (matching the oracle's `for ((` fallback).
fn parse_arith_for_body(iter: &mut Lexer) -> Result<Vec<Word>, ParseError> {
    let mut sections: Vec<Word> = Vec::new();
    let mut cur: Vec<WordPart> = Vec::new();
    loop {
        match iter.peek_kind()? {
            Some(TokenKind::ArithClose) => { iter.next_kind()?; sections.push(Word(cur)); return Ok(sections); }
            Some(TokenKind::ArithSemi)  => { iter.next_kind()?; sections.push(Word(std::mem::take(&mut cur))); }
            Some(TokenKind::ArithBail)  => { return Err(ParseError::UnterminatedLoop); }
            Some(TokenKind::ParamOpen { .. }) => { cur.push(parse_param_expansion(iter, true)?); }
            Some(TokenKind::CmdSubOpen)       => { iter.next_kind()?; cur.push(parse_command_sub(iter, true)?); }
            Some(TokenKind::BeginBacktick)    => { iter.next_kind()?; cur.push(parse_backtick_sub(iter, true)?); }
            Some(TokenKind::ArithOpen)        => { iter.next_kind()?; cur.push(parse_arith_expansion(iter, true)?); }
            Some(TokenKind::Lit { .. }) => {
                if let Some(TokenKind::Lit { text, quoted }) = iter.next_kind()? {
                    cur.push(WordPart::Literal { text, quoted });
                }
            }
            Some(TokenKind::DollarName { .. }) => {
                if let Some(TokenKind::DollarName { name, quoted }) = iter.next_kind()? {
                    let part = match name.as_str() {
                        "@" => WordPart::AllArgs { quoted, joined: false },
                        "*" => WordPart::AllArgs { quoted, joined: true },
                        "?" => WordPart::LastStatus { quoted },
                        _   => WordPart::Var { name, quoted },
                    };
                    cur.push(part);
                }
            }
            _ => return Err(ParseError::UnsupportedExpansion),
        }
    }
}

/// Parse a C-style `for (( init; cond; step )); do … done`. The caller (`parse_for`)
/// has verified two glued `Op(LParen)`. Delimits the header via `Mode::Arith`
/// (`for_header: true`, reusing v255/v246), splits into three sections, and reuses
/// `parse_do_body_done` for the body. A non-closing header surfaces as the
/// `Mode::Arith` unterminated-arith lex error (EOF before `))`) ⇒ `UnterminatedLoop`,
/// matching the oracle's `for ((` fallback.
fn parse_arith_for_clause(iter: &mut Lexer) -> Result<Command, ParseError> {
    iter.next_kind()?; // first `(`
    iter.next_kind()?; // second `(`
    iter.push_mode(Mode::Arith { paren_depth: 0, in_dquote: false, body_started: true, for_header: true });
    let result = parse_arith_for_body(iter);
    iter.pop_mode();
    let sections = match result {
        Ok(s) => s,
        Err(ParseError::Lex(_)) => return Err(ParseError::UnterminatedLoop),
        Err(e) => return Err(e),
    };
    if sections.len() != 3 {
        return Err(ParseError::ArithForHeader(format!(
            "expected 3 sections separated by `;`, got {}", sections.len())));
    }
    let init = trim_section(&sections[0]);
    let cond = trim_section(&sections[1]);
    let step = trim_section(&sections[2]);
    let body = parse_do_body_done(iter)?;
    maybe_wrap_redirects(Command::ArithFor(Box::new(ArithForClause { init, cond, step, body })), iter)
}

/// Parses `for NAME [in WORD...]; do LIST; done`.  Mirrors
/// `parse_for_command`/`parse_for_after_keyword` (~1487/1537) in `command.rs`.
/// C-style `for ((...))` (ArithFor) is parsed via `parse_arith_for_clause`.
fn parse_for(iter: &mut Lexer) -> Result<Command, ParseError> {
    expect_keyword(iter, Keyword::For, ParseError::UnterminatedLoop)?;

    // Skip inter-token blanks + newlines so `for ((...))` / `for\n((...))` are
    // recognized identically (the atom scanner emits a `Blank` after `for`).
    // `skip_newlines` (v250 T3) also drains any heredoc-body atom groups.
    skip_newlines(iter)?;

    // v255 carry: the Word-lexer emits a single `ArithBlock` for a COMPLETE
    // legacy-mode header; the atom scanner never produces `ArithBlock` (kept
    // for parity with any pre-atoms fixture that might still route here).
    if matches!(iter.peek_kind()?, Some(TokenKind::ArithBlock(..))) {
        return Err(ParseError::UnsupportedCommand);
    }
    // v256: C-style `for ((init;cond;step)); do … done`. The atom scanner emits the
    // glued `((` as two `Op(LParen)`; a spaced `( (` after `for` is not valid, so
    // (like the oracle) two glued `(` here is always an arith-for header.
    if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen)))
        && matches!(iter.peek2_kind()?, Some(TokenKind::Op(Operator::LParen)))
    {
        return parse_arith_for_clause(iter);
    }

    // Read the loop variable name (assembled from atoms).
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
        iter.next_kind()?;
    }
    if iter.peek_kind()?.is_none() {
        return Err(ParseError::UnterminatedLoop);
    }
    let var_word = consume_command_word(iter)?;
    let var = for_variable_name_word(&var_word).ok_or(ParseError::ForVariable)?;

    // POSIX allows a linebreak between the variable and `in`.
    skip_newlines(iter)?;

    // Optional `in` plus the word list.
    let mut words: Vec<Word> = Vec::new();
    let has_in = if peek_leading_keyword(iter)? == Some(Keyword::In) {
        consume_command_word(iter)?; // consume `in`
        loop {
            while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
                iter.next_kind()?;
            }
            let is_do = peek_leading_keyword(iter)? == Some(Keyword::Do);
            let stop = is_do
                || matches!(
                    iter.peek_kind()?,
                    None | Some(TokenKind::Newline) | Some(TokenKind::Op(Operator::Semi))
                );
            if stop { break; }
            match iter.peek_kind()? {
                Some(TokenKind::Op(_)) => return Err(ParseError::UnexpectedToken),
                _ => words.push(parse_word_command(iter, false)?),
            }
        }
        true
    } else {
        false
    };

    let body = parse_do_body_done(iter)?;
    maybe_wrap_redirects(Command::For(Box::new(ForClause { var, words, has_in, body })), iter)
}

/// Parses `select NAME [in WORD...]; do LIST; done`.  Mirrors
/// `parse_select_command` (~1583) in `command.rs`.  Like `parse_for` but uses
/// `words: Option<Vec<Word>>` to distinguish the no-`in` form (`None`) from an
/// explicit `in` clause (`Some`, possibly empty).
fn parse_select(iter: &mut Lexer) -> Result<Command, ParseError> {
    expect_keyword(iter, Keyword::Select, ParseError::UnterminatedLoop)?;

    // Read the loop variable name (assembled from atoms).
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
        iter.next_kind()?;
    }
    if iter.peek_kind()?.is_none() {
        return Err(ParseError::UnterminatedLoop);
    }
    let var_word = consume_command_word(iter)?;
    let var = for_variable_name_word(&var_word).ok_or(ParseError::ForVariable)?;

    // POSIX allows a linebreak between the variable and `in`.
    skip_newlines(iter)?;

    // Optional `in` plus the word list.
    let words: Option<Vec<Word>> = if peek_leading_keyword(iter)? == Some(Keyword::In) {
        consume_command_word(iter)?; // consume `in`
        let mut list: Vec<Word> = Vec::new();
        loop {
            while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
                iter.next_kind()?;
            }
            let is_do = peek_leading_keyword(iter)? == Some(Keyword::Do);
            let stop = is_do
                || matches!(
                    iter.peek_kind()?,
                    None | Some(TokenKind::Newline) | Some(TokenKind::Op(Operator::Semi))
                );
            if stop { break; }
            match iter.peek_kind()? {
                Some(TokenKind::Op(_)) => return Err(ParseError::UnexpectedToken),
                _ => list.push(parse_word_command(iter, false)?),
            }
        }
        Some(list)
    } else {
        None
    };

    let body = parse_do_body_done(iter)?;
    maybe_wrap_redirects(Command::Select(Box::new(SelectClause { var, words, body })), iter)
}

/// Parses `case WORD in [clause]... esac`.  Mirrors `parse_case` (~1673) in
/// `command.rs`.  Returns `Command::Case(Box::new(CaseClause{subject, items}))`.
fn parse_case(iter: &mut Lexer) -> Result<Command, ParseError> {
    expect_keyword(iter, Keyword::Case, ParseError::UnterminatedCase)?;
    skip_newlines(iter)?;

    // Subject word (assembled from atoms — e.g. `$x`, `x`).
    let subject = match iter.peek_kind()? {
        None => return Err(ParseError::UnterminatedCase),
        Some(TokenKind::Op(_)) => return Err(ParseError::UnexpectedToken),
        _ => consume_command_word(iter)?,
    };

    skip_newlines(iter)?;
    expect_keyword(iter, Keyword::In, ParseError::UnterminatedCase)?;
    skip_newlines(iter)?;

    let mut items: Vec<CaseItem> = Vec::new();
    loop {
        skip_newlines(iter)?;
        if peek_leading_keyword(iter)? == Some(Keyword::Esac) {
            break;
        }
        if iter.peek_kind()?.is_none() {
            return Err(ParseError::UnterminatedCase);
        }
        items.push(parse_case_item(iter)?);
    }
    expect_keyword(iter, Keyword::Esac, ParseError::UnterminatedCase)?;
    maybe_wrap_redirects(Command::Case(Box::new(CaseClause { subject, items })), iter)
}

/// Parses one `[(] pattern [| pattern]... ) [body] [terminator]` clause.
/// Mirrors `parse_case_item` (~1702) in `command.rs`.
fn parse_case_item(iter: &mut Lexer) -> Result<CaseItem, ParseError> {
    // Optional leading `(`.
    if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen))) {
        iter.next_kind()?;
    }

    // Pattern list — Word (`|` Word)* `)`, non-empty.  Pattern words are
    // assembled from atoms (`a`, `*`, `$x`, …); `|`/`)`/`(` are `Op` atoms.
    let mut patterns: Vec<Word> = Vec::new();
    loop {
        skip_newlines(iter)?;
        match iter.peek_kind()? {
            None => return Err(ParseError::UnterminatedCase),
            Some(TokenKind::Op(_)) => return Err(ParseError::UnexpectedToken),
            _ => patterns.push(parse_word_command(iter, false)?),
        }
        while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
            iter.next_kind()?;
        }
        match iter.peek_kind()? {
            None => return Err(ParseError::UnterminatedCase),
            Some(TokenKind::Op(Operator::Pipe)) => {
                iter.next_kind()?;
            }
            Some(TokenKind::Op(Operator::RParen)) => {
                iter.next_kind()?;
                break;
            }
            Some(_) => return Err(ParseError::UnexpectedToken),
        }
    }

    // Body — `None` for an empty body (next token is a terminator or `esac`).
    skip_newlines(iter)?;
    if iter.peek_kind()?.is_none() {
        return Err(ParseError::UnterminatedCase);
    }
    let is_term = matches!(
        iter.peek_kind()?,
        Some(TokenKind::Op(
            Operator::DoubleSemi | Operator::SemiAmp | Operator::DoubleSemiAmp,
        ))
    );
    let is_esac = peek_leading_keyword(iter)? == Some(Keyword::Esac);
    let body = if is_term || is_esac {
        None
    } else {
        Some(parse_and_or(iter, &[Keyword::Esac])?)
    };

    // Terminator — an absent one (next token is `esac` or end) is an implicit `Break`.
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
        iter.next_kind()?;
    }
    let terminator = match iter.peek_kind()? {
        Some(TokenKind::Op(Operator::DoubleSemi)) => {
            iter.next_kind()?;
            CaseTerminator::Break
        }
        Some(TokenKind::Op(Operator::SemiAmp)) => {
            iter.next_kind()?;
            CaseTerminator::FallThrough
        }
        Some(TokenKind::Op(Operator::DoubleSemiAmp)) => {
            iter.next_kind()?;
            CaseTerminator::ContinueMatch
        }
        _ => CaseTerminator::Break,
    };

    Ok(CaseItem { patterns, body, terminator })
}

/// Parses `while LIST; do LIST; done` or `until LIST; do LIST; done`.
/// Mirrors `parse_while` (~1886) in `command.rs`: consume the opener keyword
/// (setting `until`), then condition stops at `do`, then body stops at `done`.
/// Trailing redirects are handled by `maybe_wrap_redirects`.
fn parse_while(iter: &mut Lexer) -> Result<Command, ParseError> {
    let until = match peek_leading_keyword(iter)? {
        Some(Keyword::While) => false,
        Some(Keyword::Until) => true,
        _ => unreachable!("parse_command guarantees a while/until keyword here"),
    };
    consume_command_word(iter)?; // consume the `while`/`until` opener keyword
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
    // Skip an inter-token `Blank` after `(` (`( a )`); the oracle's Word lexer
    // never emits one.
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
        iter.next_kind()?;
    }

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
        // Skip inter-token blanks (atom path) so the connector/terminator
        // dispatch below sees the real token.
        while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
            iter.next_kind()?;
        }
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

/// Shared tail of both funcdef forms (mirrors `command.rs`'s
/// `finish_function_body`): skip newlines, require a body, parse it via the
/// atom-path `parse_command`, and validate its shape. A body that is itself a
/// still-deferred construct makes `parse_command` return `UnsupportedCommand`,
/// which propagates — the funcdef defers cleanly (pinned case).
fn finish_function_body(name: String, iter: &mut Lexer) -> Result<Command, ParseError> {
    skip_newlines(iter)?;
    if iter.peek_kind()?.is_none() {
        return Err(ParseError::UnterminatedFunction);
    }
    let body = parse_command(iter)?;
    if !crate::command::is_function_body_shape(&body) {
        return Err(ParseError::FunctionBody);
    }
    Ok(Command::FunctionDef { name, body: Box::new(body) })
}

/// `function NAME [()] compound` (mirrors `command.rs`'s
/// `parse_function_keyword_def`). Caller confirmed the leading keyword is
/// `function` via `peek_leading_keyword`. Skips the atom-stream `Blank`s the
/// Word-lexer never emitted.
fn parse_function_keyword_def(iter: &mut Lexer) -> Result<Command, ParseError> {
    consume_command_word(iter)?; // consume the `function` keyword word
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) { iter.next_kind()?; }
    // Name: a single valid identifier word.
    let name_word = consume_command_word(iter)?;
    let name = crate::command::valid_function_name_text(&name_word)
        .ok_or(ParseError::FunctionName)?;
    // Optional `()` (blanks may sit between name/`(`/`)` in the atom stream).
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) { iter.next_kind()?; }
    if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen))) {
        iter.next_kind()?; // `(`
        while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) { iter.next_kind()?; }
        match iter.next_kind()? {
            Some(TokenKind::Op(Operator::RParen)) => {}
            _ => return Err(ParseError::FunctionBody),
        }
    }
    finish_function_body(name, iter)
}

/// `name() compound` (mirrors `command.rs`'s `parse_function_def`). The caller
/// consumed the name (`name_word`) and confirmed the next non-`Blank` token is
/// `Op(LParen)`. Skips atom-stream `Blank`s inside `( )`.
fn parse_function_def(name_word: Word, iter: &mut Lexer) -> Result<Command, ParseError> {
    let name = crate::command::valid_function_name_text(&name_word)
        .ok_or(ParseError::FunctionName)?;
    iter.next_kind()?; // `(`
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) { iter.next_kind()?; }
    match iter.next_kind()? {
        Some(TokenKind::Op(Operator::RParen)) => {}
        _ => return Err(ParseError::FunctionBody),
    }
    finish_function_body(name, iter)
}

/// Skips inter-atom whitespace inside `[[ … ]]`: BOTH `Blank` (atom-stream
/// word-boundary tokens, which the oracle's Word-lexer never surfaces as a
/// token at all — it folds them silently into token boundaries) AND
/// `Newline` (delegated to the oracle's own `skip_test_newlines`, which the
/// atom stream also emits explicitly). Loops to a fixpoint so any
/// Blank/Newline interleaving (`[[\n  -f a ]]`) fully drains. Every peek
/// decision in the cascade below that follows a word/operator/`(`/`)` needs
/// this first, since (unlike the oracle) a `Blank` atom can sit at exactly
/// that position without being consumed by anything else.
fn skip_test_ws(iter: &mut Lexer) -> Result<(), ParseError> {
    loop {
        while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) { iter.next_kind()?; }
        if matches!(iter.peek_kind()?, Some(TokenKind::Newline)) {
            skip_test_newlines(iter)?;
            continue;
        }
        break;
    }
    Ok(())
}

/// Skips ONLY inter-atom `Blank` tokens (never `Newline`) inside `[[ … ]]`.
/// Used at the operand/operator BOUNDARY positions where the oracle skips
/// NOTHING (a `Blank` is invisible to the oracle's Word-lexer, but a `Newline`
/// is significant there — it must reach the operand check and trip
/// `TestExprMissingOperand`/`UnterminatedDoubleBracket`, matching the oracle).
/// Contrast `skip_test_ws` (Blank+Newline), used ONLY at the four positions
/// where the oracle calls `skip_test_newlines` (after `[[`, the `||`/`&&`
/// loops, before `]]`).
fn skip_test_blanks(iter: &mut Lexer) -> Result<(), ParseError> {
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) { iter.next_kind()?; }
    Ok(())
}

/// v253: atom-native `[[ … ]]`. Mirrors command.rs's
/// `parse_double_bracket_with_assigns`, but reads operands via
/// `parse_word_command` (the atom stream has `Lit` atoms, not pre-lexed `Word`
/// tokens). `=~` is DEFERRED to a later iteration (returns `UnsupportedCommand`
/// without pulling the regex RHS — see `parse_test_atom` below).
fn parse_double_bracket(iter: &mut Lexer, inline_assignments: Vec<Assignment>) -> Result<Command, ParseError> {
    iter.next_kind()?; // consume `[[`
    skip_test_ws(iter)?;
    if iter.peek_kind()?.and_then(keyword_of_consumed) == Some(Keyword::DoubleBracketClose) {
        return Err(ParseError::EmptyDoubleBracket);
    }
    if iter.peek_kind()?.is_none() {
        return Err(ParseError::UnterminatedDoubleBracket);
    }
    let expr = parse_test_or(iter)?;
    skip_test_ws(iter)?;
    match iter.next_kind()? {
        Some(tok) if keyword_of_consumed(&tok) == Some(Keyword::DoubleBracketClose) => {}
        _ => return Err(ParseError::UnterminatedDoubleBracket),
    }
    Ok(Command::DoubleBracket { expr: Box::new(expr), inline_assignments })
}

/// Lowest precedence: `||`.
fn parse_test_or(iter: &mut Lexer) -> Result<TestExpr, ParseError> {
    let mut lhs = parse_test_and(iter)?;
    skip_test_ws(iter)?;
    while matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::Or))) {
        iter.next_kind()?;
        skip_test_ws(iter)?;
        let rhs = parse_test_and(iter)?;
        lhs = TestExpr::Or(Box::new(lhs), Box::new(rhs));
        skip_test_ws(iter)?;
    }
    Ok(lhs)
}

/// Next precedence: `&&`.
fn parse_test_and(iter: &mut Lexer) -> Result<TestExpr, ParseError> {
    let mut lhs = parse_test_not(iter)?;
    skip_test_ws(iter)?;
    while matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::And))) {
        iter.next_kind()?;
        skip_test_ws(iter)?;
        let rhs = parse_test_not(iter)?;
        lhs = TestExpr::And(Box::new(lhs), Box::new(rhs));
        skip_test_ws(iter)?;
    }
    Ok(lhs)
}

/// Next precedence: `!` (right-associative). Reuses parser.rs's own
/// atom-aware `is_bang_word` (defined above for pipeline negation) rather than
/// the oracle's `Word`-only version, since the atom stream's `!` arrives as a
/// bare unquoted `Lit` atom. Leads with `skip_test_blanks` (NOT `skip_test_ws`):
/// this is the "start of an operand expression" position, where the oracle
/// skips NOTHING. A pending `Blank` must be dropped (it's invisible to the
/// oracle), but a pending `Newline` must NOT be — for the grouping first
/// operand (`[[ (\na ) ]]`) and the post-`!` operand (`[[ !\nx ]]`), the
/// oracle leaves the newline in place so it reaches `parse_test_atom` and
/// errors `TestExprMissingOperand`. Skipping it here would wrongly accept
/// those inputs (the T1-review CRITICAL bug).
fn parse_test_not(iter: &mut Lexer) -> Result<TestExpr, ParseError> {
    skip_test_blanks(iter)?;
    if iter.peek_kind()?.map(is_bang_word).unwrap_or(false) {
        iter.next_kind()?;
        let inner = parse_test_not(iter)?;
        return Ok(TestExpr::Not(Box::new(inner)));
    }
    parse_test_primary(iter)
}

/// Highest precedence: `( expr )` grouping or a single test atom.
fn parse_test_primary(iter: &mut Lexer) -> Result<TestExpr, ParseError> {
    if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen))) {
        iter.next_kind()?;
        let inner = parse_test_or(iter)?;
        // The oracle does NOT skip newlines before `)` in parse_test_primary —
        // any newline before `)` was already consumed by `parse_test_or`'s
        // trailing `skip_test_newlines` (mirrored by our `parse_test_or`'s
        // trailing `skip_test_ws`), which always runs before returning here. So
        // only a stray `Blank` could remain; drop it with `skip_test_blanks`
        // (Blank-only), NOT `skip_test_ws` — this is not an oracle
        // `skip_test_newlines` site.
        skip_test_blanks(iter)?;
        match iter.next_kind()? {
            Some(TokenKind::Op(Operator::RParen)) => {}
            None => return Err(ParseError::UnterminatedDoubleBracket),
            _ => return Err(ParseError::TestExprMissingOperand),
        }
        return Ok(inner);
    }
    parse_test_atom(iter)
}

/// Reads one operand `Word` inside `[[ ]]`. EOF → `UnterminatedDoubleBracket`;
/// a `]]`/`)`/operator/`Newline` where an operand was expected →
/// `TestExprMissingOperand`. Leading `skip_test_blanks` (NOT `skip_test_ws`):
/// this is called right after a unary or binary operator, exactly where the
/// oracle's `next_test_word` (command.rs) skips NOTHING. A pending `Blank`
/// (invisible to the oracle) is dropped, but a pending `Newline` is
/// significant — it means "no operand on this line", so it must fall through
/// to the guard below and yield `TestExprMissingOperand` (e.g. `[[ -f\nx ]]`,
/// `[[ a ==\nb ]]`), matching the oracle. `RedirFd`/`Heredoc` (non-word-start
/// atoms that would otherwise be handed to `parse_word_command` → the wrong
/// `UnexpectedToken` variant) are likewise rejected here as missing operands.
fn next_test_word_atom(iter: &mut Lexer) -> Result<Word, ParseError> {
    skip_test_blanks(iter)?;
    match iter.peek_kind()? {
        None => return Err(ParseError::UnterminatedDoubleBracket),
        Some(tok) => {
            if keyword_of_consumed(tok) == Some(Keyword::DoubleBracketClose)
                || matches!(
                    tok,
                    TokenKind::Op(_)
                        | TokenKind::Newline
                        | TokenKind::RedirFd(_)
                        | TokenKind::Heredoc { .. }
                )
            {
                return Err(ParseError::TestExprMissingOperand);
            }
        }
    }
    parse_word_command(iter, false)
}

/// True if the next atom is a `[[ ]]` binary operator. `<`/`>` arrive as
/// `Op(RedirIn)`/`Op(RedirOut)`; every other operator is a single unquoted
/// `Lit` atom (the lexer has no dedicated token for it). KEEP IN SYNC with
/// the operator match arms in `parse_test_atom`.
///
/// The `Lit` arm requires BOTH (a) the text is in the operator set AND (b) the
/// operator word ENDS right there — the atom AFTER it is a WORD BOUNDARY
/// (`Blank`/`Newline`/`Op`/EOF), NOT a glued word-continuation atom. This
/// mirrors the oracle's `next_is_test_binary_operator`, which peeks the
/// FULLY-ASSEMBLED `Word` token: `==$x` (operator glued to an expansion with no
/// intervening space) is assembled by the oracle as ONE `Word([Literal("=="),
/// Var("x")])`, which is NOT in its operator set → the oracle takes the
/// "not a binary operator" branch (lone-word `-n a`, leaving `==$x`
/// unconsumed → the `]]`-consume then trips → `UnterminatedDoubleBracket`).
/// The atom stream splits `==$x` into `Lit("==")` + `DollarName{...}` (no
/// `Blank`), so without the peek2 boundary check the `Lit("==")` alone would
/// look like an operator and mis-classify the glued form. See
/// `atoms_double_bracket_glued_operator`.
fn next_is_test_binary_operator_atom(iter: &mut Lexer) -> Result<bool, ParseError> {
    Ok(match iter.peek_kind()? {
        Some(TokenKind::Op(Operator::RedirIn)) | Some(TokenKind::Op(Operator::RedirOut)) => true,
        Some(TokenKind::Lit { text, quoted: false }) => {
            let in_set = matches!(
                text.as_str(),
                "==" | "=" | "!=" | "=~" | "-eq" | "-ne" | "-lt" | "-gt"
                    | "-le" | "-ge" | "-nt" | "-ot" | "-ef"
            );
            // (b) the operator word must END here: the next atom is a word
            // boundary, not a glued continuation (`Lit`/`DollarName`/`Var`/
            // `ParamOpen`/`QuoteRun`/… with no intervening `Blank`).
            in_set
                && matches!(
                    iter.peek2_kind()?,
                    None | Some(TokenKind::Blank)
                        | Some(TokenKind::Newline)
                        | Some(TokenKind::Op(_))
                )
        }
        _ => false,
    })
}

/// Parses a single test — either a unary test (`-f path`) or a binary/lone-word
/// test (`lhs op rhs` / bare `word` ≡ `-n word`). Mirrors command.rs's
/// `parse_test_atom`, reading operands via `parse_word_command` since the atom
/// stream has no pre-lexed `Word` tokens.
fn parse_test_atom(iter: &mut Lexer) -> Result<TestExpr, ParseError> {
    if iter.peek_kind()?.is_none() {
        return Err(ParseError::UnterminatedDoubleBracket);
    }
    // Present terminator with nothing before it → empty body.
    match iter.peek_kind()? {
        Some(tok) if keyword_of_consumed(tok) == Some(Keyword::DoubleBracketClose)
            || matches!(tok, TokenKind::Op(Operator::RParen)) => {
            return Err(ParseError::EmptyDoubleBracket);
        }
        _ => {}
    }
    // A leading non-operand atom where an operand was expected: an operator
    // (not `(`, already handled by parse_test_primary), a `Newline` (the oracle
    // leaves a newline at the first-operand position — e.g. `[[ (\na ) ]]`,
    // `[[ !\nx ]]` — where its `parse_test_atom` matches only a `Word` and
    // returns `TestExprMissingOperand` for anything else), or a non-word-start
    // `RedirFd`/`Heredoc`. All → `TestExprMissingOperand`, matching the oracle's
    // `_ => Err(TestExprMissingOperand)` first-word match.
    if matches!(
        iter.peek_kind()?,
        Some(
            TokenKind::Op(_)
                | TokenKind::Newline
                | TokenKind::RedirFd(_)
                | TokenKind::Heredoc { .. }
        )
    ) {
        return Err(ParseError::TestExprMissingOperand);
    }

    let first = parse_word_command(iter, false)?;

    if let Some(op) = try_unary_op(&first) {
        let operand = next_test_word_atom(iter)?;
        return Ok(TestExpr::Unary { op, operand });
    }

    let lhs = first;
    // A Blank sits between the operand and the operator/terminator that follows
    // it (`parse_word_command` stops at `Blank` without consuming it). Use
    // `skip_test_blanks` (NOT `skip_test_ws`): the oracle checks the operator
    // IMMEDIATELY after the lhs with no newline-skip, so a `Newline` here must
    // stay to make `next_is_test_binary_operator_atom` see it (→ lone-word,
    // leaving whatever follows for the `]]`-consume to trip on). Skipping it
    // would wrongly glue an operator on the next line (`[[ a\n== b ]]`).
    skip_test_blanks(iter)?;
    if !next_is_test_binary_operator_atom(iter)? {
        return Ok(TestExpr::Unary { op: TestUnaryOp::StringNonEmpty, operand: lhs });
    }

    // Consume the operator.
    match iter.peek_kind()? {
        Some(TokenKind::Op(Operator::RedirIn)) => { iter.next_kind()?; let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::StringLt, lhs, rhs }) }
        Some(TokenKind::Op(Operator::RedirOut)) => { iter.next_kind()?; let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::StringGt, lhs, rhs }) }
        _ => {
            let op_word = parse_word_command(iter, false)?;
            let op_text = match word_literal_text(&op_word) {
                Some(t) => t.to_string(),
                None => return Err(ParseError::TestExprBadOperator(format!("{op_word:?}"))),
            };
            match op_text.as_str() {
                "==" | "=" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::StringEq, lhs, rhs }) }
                "!=" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::StringNe, lhs, rhs }) }
                "=~" => {
                    let pattern = parse_regex_operand(iter)?;
                    // The oracle lexes the regex operand EAGERLY as one Word, then
                    // `next_test_word` rejects it if that Word is the `]]` close
                    // keyword: an empty `""` operand swallows the trailing `]]` into
                    // the pattern text (the leading-ws skip in `scan_regex_operand`),
                    // so `next_test_word` sees a `]]`-keyword Word →
                    // `TestExprMissingOperand`. Mirror that guard here (the atom path
                    // assembles the same `]]` pattern via `Mode::Regex`).
                    if word_literal_text(&pattern) == Some("]]") {
                        return Err(ParseError::TestExprMissingOperand);
                    }
                    Ok(TestExpr::Regex { lhs, pattern })
                }
                "<" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::StringLt, lhs, rhs }) }
                ">" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::StringGt, lhs, rhs }) }
                "-eq" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::IntEq, lhs, rhs }) }
                "-ne" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::IntNe, lhs, rhs }) }
                "-lt" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::IntLt, lhs, rhs }) }
                "-gt" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::IntGt, lhs, rhs }) }
                "-le" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::IntLe, lhs, rhs }) }
                "-ge" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::IntGe, lhs, rhs }) }
                "-nt" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::NewerThan, lhs, rhs }) }
                "-ot" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::OlderThan, lhs, rhs }) }
                "-ef" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::SameFile, lhs, rhs }) }
                other => Err(ParseError::TestExprBadOperator(other.to_string())),
            }
        }
    }
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
        // `$(…)` inside `"…"` in an operand is deferred (only the unquoted-operand
        // `$(` path is wired — for CmdSubOpen in v244 T4 and for ArithOpen in v246
        // T6; both in-dquote sites remain deferred for `$(cmd)` and `$((`).
        // `${x:-$(cmd)}` (unquoted operand) is now in-scope — moved to cs_in_param_operand.
        // `${x:-`cmd`}` (unquoted-operand backtick) is now in-scope — moved to bt_in_param_operand (v245 T6).
        // `${x:-$((1+1))}` (unquoted operand) is now in-scope — moved to arith_in_param_operand (v246 T6).
        for s in [
            "${x:-\"$(cmd)\"}",
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
        let mut lx = Lexer::new_live_atoms(s, &Default::default(), LexerOptions::default());
        parse_sequence(&mut lx)
    }
    #[test]
    fn atoms_scaffolding_exists() {
        // The atom lexer + repointed harness wire up. Empty input parses to None
        // on both paths (EOF handled by the skeleton).
        assert_eq!(new_seq("").unwrap(), old_seq("").unwrap());
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

    // v247 T2 tests

    #[test]
    fn atoms_plain_words() {
        diff_cmd("echo");
        diff_cmd("echo hi");
        diff_cmd("echo   hi    there");     // multiple blanks collapse
        diff_cmd("  echo hi  ");            // leading/trailing blanks
        diff_cmd("echo 'a b' \"c d\" e");   // quoted runs stay one word
        diff_cmd("echo a'b'c\"d\"");        // glued quotes = one word
        diff_cmd("echo a\\ b");             // escaped space = one word
        diff_cmd("echo $'x\\ty'");          // $'…' ANSI-C
    }

    #[test]
    fn atoms_trailing_backslash() {
        diff_cmd("echo a\\");
        diff_cmd("echo \\");
        diff_cmd("echo ab\\");
        diff_cmd("echo a\\ b");   // escaped space mid-word stays Quoted{Backslash} — must still match
        diff_cmd("echo a b\\");
    }

    // v247 T3 tests

    #[test]
    fn atoms_expansions() {
        diff_cmd("echo $x");
        diff_cmd("echo ${x:-d}");
        diff_cmd("echo $(echo hi)");
        diff_cmd("echo `echo hi`");
        diff_cmd("echo $((1+2))");
        diff_cmd("echo $x$y \"$a ${b}\" pre$(c)post");
        diff_cmd("echo ~ ~root ~/x");
        diff_cmd("echo $? $@ $1");
    }

    #[test]
    fn atoms_lone_dollar() {
        // lone `$` is a standalone Literal, never merged (top level)
        diff_cmd("echo a$");
        diff_cmd("echo a$.");
        diff_cmd("echo a$ b");
        diff_cmd("echo $");
        diff_cmd("echo $x$");
        // lone `$` inside double quotes
        diff_cmd("echo \"$ x\"");
        diff_cmd("echo \"foo $ bar\"");
        diff_cmd("echo \"$.\"");
        diff_cmd("echo \"a$\"");
        diff_cmd("echo \"$'x'\"");
        // merges that MUST still work (regression guard for the accumulator)
        diff_cmd("echo a\\");        // trailing backslash folds into preceding literal
        diff_cmd("echo ab\\");
        diff_cmd("echo a b\\");
    }

    // v247 T4 tests

    #[test]
    fn atoms_assignments() {
        diff_cmd("x=1");
        diff_cmd("x=1 y=2 cmd");
        diff_cmd("x+=abc");
        diff_cmd("a[0]=v");
        diff_cmd("a[$i]=v");
        diff_cmd("x=$y\"z\"");
        diff_cmd("PATH=/bin:/usr/bin cmd arg");
        diff_cmd("x=");                  // empty value
        diff_cmd("a[i]+=v");             // subscript append
        diff_cmd("x=~/foo");             // assignment-value tilde
        diff_cmd("x=a:~/b:~/c");         // tilde after unquoted ':'
        diff_cmd("PATH=~/bin:/usr/bin"); // value-start tilde + literal
        diff_cmd("cmd x=1 arg");         // prefix assignment before argv
    }

    #[test]
    fn atoms_bracket_not_assignment() {
        // name[...] NOT followed by =/+= : whole bracket region is literal (oracle parity)
        diff_cmd("arr[$i]");
        diff_cmd("a[$x]");
        diff_cmd("a['x']");
        diff_cmd("a[\"x\"]");
        diff_cmd("a[`c`]");
        diff_cmd("a[a\\b]");
        diff_cmd("a[${y}]");
        diff_cmd("a[$x]y");
        diff_cmd("pre a[$i] post");
        diff_cmd("a[$x");            // unclosed
        diff_cmd("ls [abc]*");        // standalone glob (no identifier) — still literal
        diff_cmd("echo a[b]");
        // real assignments must STILL work
        diff_cmd("a[0]=v");
        diff_cmd("a[$i]=v");
        diff_cmd("a[i]+=v");
        diff_cmd("a[b[c]]=v");
    }

    // v247 T5 tests: redirects / operators / separators / comments / continuations
    #[test]
    fn atoms_structure() {
        for s in [
            // pipelines / and-or / separators
            "a | b | c", "a && b || c", "a; b; c", "a &", "a&&b", "a||b", "a|b",
            // redirects
            "echo hi > out", "echo hi >> out 2>&1", "cat < in", "cat <> f",
            "3< in 4> out cmd", "{fd}> out cmd", "cmd 2>&1 >file", "cmd >&2",
            "echo a >| f", "echo a &> f", "echo a &>> f", "ls</dev/null",
            // comments
            "echo a  # trailing comment", "# whole line comment", "a#b",
            // line continuations
            "echo a \\\n  b", "a\\\n&&\\\nb",
            // separators glued / spaced
            "echo a;", "echo a ;b", "a| b |c",
            // adversarial extensions
            "3>out", "{fd}>out cmd", "2>&1", "1>&2", "cmd 3>&1 4>&2",
            "a>b", "x=2>out", "a3>out", "echo>out",           // fd-prefix boundaries
            "cmd</in>out", "echo hi>>log 2>>err",             // glued redirects
            "a  ;  b  &&  c", "a|&b",                          // spaced ops + |& desugar
            "echo '|' \"&&\" \\;",                             // quoted/escaped metachars stay literal
            "x=1 y=2 cmd >o", ">o", "<i >o",                   // assignments + bare redirects
            "echo a# still one word", "a#b>c",                 // mid-word # then redirect
            "echo\ta\tb", "a\r b",                             // tab / CR whitespace
        ] { diff_cmd(s); }
    }

    // v247 T6 tests: in-scope compounds on the atom path
    #[test]
    fn atoms_compounds() {
        for s in [
            "if true; then echo a; fi",
            "if a; then b; elif c; then d; else e; fi",
            "while read x; do echo $x; done",
            "until false; do echo a; done",
            "for i in a b c; do echo $i; done",
            "for i in $list; do :; done",
            "for i; do echo $i; done",                     // no `in` list
            "case $x in a) echo a;; b|c) echo bc;; *) echo d;; esac",
            "case $x in (a) echo a;; esac",                // parenthesized pattern
            "select x in a b; do echo $x; break; done",
            "( cd /tmp && ls )",
            "{ echo a; echo b; }",
            "if true; then echo a; fi | wc -l",            // compound in a pipeline
            "for i in a b; do if $i; then echo y; fi; done", // nested compounds
            "{echo",                                        // NOT a brace group — literal word
            "iffy --opt",                                   // NOT a keyword
            "echo if then fi",                              // keywords as args (mid-command) stay literal
        ] { diff_cmd(s); }
    }
    #[test]
    fn atoms_compounds_deferred() {
        // still deferred on the atom path (T7 will also assert these).
        // `f() { :; }` is NO LONGER deferred — v248 T2 implements the POSIX
        // `name()` funcdef form (see `atoms_function_paren_form`).
        // `[[ a == b ]]` is NO LONGER deferred — v253 T1 implements `[[ … ]]`
        // (see `atoms_double_bracket_core`).
        // `(( 1+2 ))` (standalone arith command) is NO LONGER deferred —
        // v255 T1 implements it (see `atoms_arith_command`).
        // `for ((i=0;i<2;i++)); do :; done` is NO LONGER deferred — v256 T2
        // implements C-style `for (( … ))` (see `atoms_arith_for`).
        diff_cmd("for ((i=0;i<2;i++)); do :; done");
        for s in ["coproc c { :; }"] {
            assert!(matches!(new_seq(s), Err(ParseError::UnsupportedCommand)),
                "expected UnsupportedCommand for {s:?}, got {:?}", new_seq(s));
        }
    }

    #[test]
    fn atoms_blank_boundaries() {
        // C1: bang after connectors
        for s in ["foo && ! bar", "a && ! b", "a || ! b", "a; ! b", "foo &&   ! bar", "! a", "!a", "a && b"] { diff_cmd(s); }
        // C2: leading/trailing/only blanks + blank/comment lines at boundaries
        for s in ["   ", "\t", " ", "  \n  ", "   # indented", " #c", "a; ", "echo hi;  ", "a; #c", "", "\n", "  \n\n  "] {
            assert_eq!(new_seq(s).map_err(|e| format!("{e:?}")), old_seq(s).map_err(|e| format!("{e:?}")), "boundary case {s:?}");
        }
    }
    #[test]
    fn atoms_procsub_core() {
        diff_cmd("cat <(echo hi)");
        diff_cmd("tee >(cat)");
        diff_cmd("echo <(a) >(b)");        // multiple, both dirs
        diff_cmd("diff <(sort x) <(sort y)");
        diff_cmd("x<(y)");                  // glued to leading literal
        diff_cmd("wc < <(sort f)");         // procsub as a redirect TARGET
        diff_cmd("sort > >(uniq)");
    }

    #[test]
    fn atoms_procsub_supported() {
        // process substitution now parses on the atom path, byte-identical to the oracle.
        for s in ["cat <(echo hi)", "tee >(cat)", "echo <(a) >(b)"] {
            diff_cmd(s);
        }
    }

    // ── v251 T3: full process-substitution corpus ──────────────────────────
    #[test]
    fn atoms_procsub_corpus() {
        // nested
        diff_cmd("cat <( cat <(echo x) )");
        diff_cmd("echo >( tee >(cat) )");
        // bodies: pipelines / expansions / compounds inside
        diff_cmd("cat <(echo $x | sort)");
        diff_cmd("cat <(echo ${y:-d})");
        diff_cmd("cat <(if true; then echo a; fi)");
        diff_cmd("cat <(a && b || c)");
        // funcdef body inside a procsub — v248 funcdef support extends into
        // `parse_subshell_sequence`, so this is NOT deferred (observed: byte-
        // identical to the oracle, unlike `[[ … ]]`/`(( … ))` bodies below).
        diff_cmd("cat <( f() { :; } )");
        // adjacency with other word parts
        diff_cmd("echo pre$(c)<(d)post");
        diff_cmd("cat <(a)<(b)");            // two procsubs glued into one word
        // empty body
        diff_cmd("cat <()");
        diff_cmd("tee >()");
    }

    #[test]
    fn atoms_procsub_quoted_literal() {
        // inside quotes `<(`/`>(` are LITERAL — no procsub (matches the oracle).
        diff_cmd("echo \"<(x)\"");
        diff_cmd("echo '<(x)'");
        // Escaped `<` (`\<`): the backslash strips `<`'s special meaning, so
        // `(x)` never becomes a `ProcSubOpen` target — a bare `(` then
        // surfaces in argument position. Observed: the ORACLE itself does not
        // treat this as a literal glued word either — it errors
        // `UnexpectedToken` (command.rs's generic "stray `(` where a word was
        // expected" outcome; see e.g. the `w_tok("hi"), Op(LParen)` case at
        // command.rs ~4556). So this is error-parity, not a `diff_cmd` case.
        assert_eq!(
            new_seq("echo \\<(x)").map(|_| ()).map_err(|e| format!("{e:?}")),
            old_seq("echo \\<(x)").map(|_| ()).map_err(|e| format!("{e:?}")),
            "escaped `<(` error parity",
        );
    }

    #[test]
    fn atoms_procsub_for_case_positions() {
        // for-list / case-pattern positions: the `<(` atom is `ProcSubOpen`,
        // never an `Op`, so these loops' `Some(TokenKind::Op(_)) =>
        // UnexpectedToken` guards don't intercept it — it falls to the
        // `parse_word_command` dispatch same as any other word-start atom,
        // which already special-cases a fresh `ProcSubOpen`. Observed: the
        // oracle parses all three identically, no dispatch-set extension
        // needed.
        diff_cmd("for x in <(a); do :; done");
        diff_cmd("case <(a) in x) echo y;; esac");
        diff_cmd("case x in <(a)) echo y;; esac");
    }

    #[test]
    fn atoms_procsub_errors() {
        // Malformed at EOF (`cat <(` with no closer): the oracle's BATCH lexer
        // rejects this at LEX time (`UnterminatedSubstitution`, before parsing
        // even starts), so `old_seq` cannot yield a `Result` to compare — the
        // atom path (incremental live lexer) rejects the same input at PARSE
        // time. Both REJECT; assert parity of rejection (mirrors
        // `atoms_error_parity`'s `echo $(`/`echo ${` treatment).
        assert!(new_seq("cat <(").is_err(), "atom path must reject unterminated `cat <(`");

        // `[[ … ]]` inside a procsub body is NO LONGER deferred — v253 T1
        // implements `[[ … ]]` everywhere `parse_command` is reached, including
        // inside `parse_process_sub`'s body sequence.
        diff_cmd("cat <( [[ x ]] )");
    }

    // ── v252 T1: positional array literals ───────────────────────────────────

    #[test]
    fn atoms_array_literal_rich_values() {
        diff_cmd("a=(\"x y\" 'z' bare)");            // double/single/bare
        diff_cmd("a=($x ${y} ${z:-d})");             // param expansions
        diff_cmd("a=($(echo hi) `echo bye`)");       // command subs
        diff_cmd("a=($((1 + 2)) end)");              // arith
        diff_cmd("a=(~ ~/x a=~)");                   // tilde eligibility
        // NOTE: the brief's literal case was `[ab]c` (a bracket AT the START
        // of a value) — that collides with the ORACLE's pre-existing (T1-era,
        // out-of-scope-for-T2) array-literal subscript sniff: ANY value
        // beginning with `[` unconditionally attempts `[expr]=` subscript
        // parsing (`scan_array_literal`), and `[ab]c` has no `=` after `]`, so
        // the oracle itself errors (`ArrayLiteralMissingEquals`) — even the
        // LEX stage fails (`old_seq`'s `tokenize_with_opts(...).expect("lex")`
        // panics), so no diff/err helper can express it. That leading-`[`
        // sniff is Task 3's territory ("do NOT touch subscripts"), so this
        // uses a mid-value bracket instead — still proves globs/brackets stay
        // literal in a value, without hitting the deferred subscript sniff.
        diff_cmd("a=(*.txt foo?bar pre[ab]c)");      // globs (patterns kept literal in AST)
        diff_cmd("a=(pre$xpost \"$mix\"tail)");      // adjacency/glue within a value
        diff_cmd("a=(\n  one\n  two\n)");            // newline separators
        diff_cmd("a=(one # comment\n two)");         // comment separator
        diff_cmd("arr=\\\n(1 2)");                   // `\<NL>` between prefix and `(`
        diff_cmd("a=(foo\\ bar)");                   // backslash-escaped space stays one element
    }

    // ── v252 merge-gate fix: `\<NL>` line continuation is GLUE, not a
    // separator, when it abuts element text with no surrounding whitespace.
    #[test]
    fn atoms_array_literal_line_continuation() {
        diff_cmd("a=(1\\\n2)");           // glued: one element `12`
        diff_cmd("a=(1 \\\n2)");          // space then continuation
        diff_cmd("a=(1\\\n 2)");          // continuation then space
        diff_cmd("a=(\\\n1)");            // leading continuation
        diff_cmd("a=(1\\\n2\\\n3)");      // multiple glued continuations
        diff_cmd("a=([0]=a\\\nb)");       // glued continuation inside a subscripted value
        diff_cmd("a=(a\\\nb c)");         // glued then a real (space) separator
    }

    #[test]
    fn atoms_array_literal_positional() {
        diff_cmd("a=(1 2 3)");
        diff_cmd("a=()");                 // empty
        diff_cmd("a=(x)");                // single
        diff_cmd("a=(  1   2  )");        // extra spaces
        diff_cmd("arr+=(4 5)");           // append form
        diff_cmd("a=(a|b c;d e<f)");      // |;&<> literal inside values
        diff_cmd("a=({1..3})");           // brace-expanded bare element -> 1 2 3
        diff_cmd("a=(x{a,b}y)");          // brace expansion with prefix/suffix
        diff_cmd("pre a=(1 2) post");     // assignment mid-command still one word
    }

    #[test]
    fn atoms_array_literal_subscripts() {
        diff_cmd("a=([0]=x [1]=y)");             // explicit subscripts
        diff_cmd("a=([2]=two 1 [0]=zero)");      // mixed positional + subscripted
        diff_cmd("a=([i+1]=v)");                 // arithmetic subscript expr
        diff_cmd("a=([k]={a,b})");               // subscripted: brace stays LITERAL (no expansion)
        diff_cmd("m[k]=(1 2)");                  // name[sub]=(…) prefix form
        diff_cmd("m[k]+=(3)");                   // name[sub]+=(…) prefix form
        diff_cmd("a=([0]= [1]=y)");              // empty subscripted value
        diff_cmd("a=([$i]=v [x]=$y)");           // expansion in subscript and value
        diff_cmd("a=([0]=x[1]y)");               // `[` MID-value stays literal
    }

    #[test]
    fn atoms_array_literal_subscript_regressions() {
        // BUG 1 (regression of T1/T2): a `[` MID-value, AFTER a non-literal atom
        // that ended its own scan_step, must stay LITERAL — not open a subscript.
        diff_cmd("a=($x[0])");                   // positional value $x[0]
        diff_cmd("a=(pre$x[0]post)");            // positional, glued around `$x`
        diff_cmd("a=([0]=$y[1]z)");              // subscripted value $y[1]z
        diff_cmd("a=([0]=\"x\"[1]z)");           // subscripted value "x"[1]z (after a quote)
        // BUG 2: a subscripted EMPTY value immediately before `)` → empty element,
        // not an UnexpectedToken error.
        diff_cmd("a=([0]=)");                    // sole empty subscripted value
        diff_cmd("a=([2]=two [0]=)");            // empty value as the LAST element
    }

    #[test]
    fn atoms_array_literal_error_parity() {
        // `[i]` without `=` → ArrayLiteralMissingEquals on BOTH paths (lexer-level).
        // The oracle's `old_seq` panics on a lex error (`tokenize_with_opts(..).expect`),
        // so compare against the fallible `tokenize_with_opts` directly.
        assert!(new_seq("a=([0])").is_err());
        assert!(tokenize_with_opts("a=([0])", LexerOptions::default()).is_err());
        // Leading `[…]` with no `=` after `]` (the T2-deferred `[ab]c` case).
        assert!(new_seq("a=([ab]c)").is_err());
        assert!(tokenize_with_opts("a=([ab]c)", LexerOptions::default()).is_err());
        // EOF before `)` → UnterminatedArrayLiteral on both.
        assert!(new_seq("a=(1 2").is_err());
        assert!(tokenize_with_opts("a=(1 2", LexerOptions::default()).is_err());
        assert!(new_seq("a=(").is_err());
        assert!(tokenize_with_opts("a=(", LexerOptions::default()).is_err());
    }

    #[test]
    fn atoms_array_literal_append_subscript_funcdef_parity() {
        // v252 T4 review follow-up: `AssignPrefix`-prefixed leading words
        // (`a+=…`/`a[i]=…`/`a[i]+=…`) are lexed with a leading zero-width
        // `AssignPrefix` atom (unlike plain `a=…`, a `Lit("a=")`), so before
        // this fix they SKIPPED the funcdef-lookahead dispatch entirely and
        // fell to `parse_simple` → the trailing-`Op(LParen)` arm →
        // `UnsupportedCommand`, diverging from the oracle's `FunctionName`.
        // The outer dispatch gate now admits a leading `AssignPrefix`, so a
        // CLOSED array literal glued before a second `(` reaches the same
        // funcdef attempt and gets `FunctionName` parity (a multi-part /
        // non-Literal word is not a valid function name). Error-parity (both
        // sides `Err(FunctionName)`), not `diff_cmd` (which requires `Ok`).
        assert_eq!(new_seq("a+=(1)(2)"), old_seq("a+=(1)(2)"), "error parity for \"a+=(1)(2)\"");
        assert_eq!(new_seq("a[0]=(1)(2)"), old_seq("a[0]=(1)(2)"), "error parity for \"a[0]=(1)(2)\"");
        assert!(matches!(new_seq("a+=(1)(2)"), Err(ParseError::FunctionName)),
            "a+=(1)(2) → FunctionName, got {:?}", new_seq("a+=(1)(2)"));
        assert!(matches!(new_seq("a[0]=(1)(2)"), Err(ParseError::FunctionName)),
            "a[0]=(1)(2) → FunctionName, got {:?}", new_seq("a[0]=(1)(2)"));
        // Bonus reconciliation (same root cause): a single-part `AssignPrefix`
        // word (empty value) followed by `(` is NOT a valid function name on
        // the oracle either (`AssignPrefix` is not a `Literal`), so both give
        // `FunctionName` — unlike the v248-pinned single-`Literal` `a=b ()`
        // shape, which the oracle accepts as `FunctionDef` (still deferred).
        assert_eq!(new_seq("a+= (echo hi)"), old_seq("a+= (echo hi)"), "error parity for \"a+= (echo hi)\"");

        // REGRESSION GUARD: ordinary append/subscript assignments (no following
        // second `(`) must still parse as normal assignment simple-commands —
        // the funcdef attempt must never be entered for these.
        diff_cmd("a+=(1 2)");     // append array literal
        diff_cmd("a+=(1)");       // append single-element array literal
        diff_cmd("a[0]=(1 2)");   // subscripted array literal
        diff_cmd("a[i]=x");       // subscripted SCALAR assignment
        diff_cmd("a=(1 2)");      // plain (Literal-prefixed) array literal, unchanged
    }

    #[test]
    fn atoms_array_literal_declare_routing() {
        // If declare/local args route through the command-word path, these are
        // diff_cmd. If they route through DeclArg (different path), replace each
        // with the observed-actual behavior and leave a NOTE comment documenting
        // the deferral (per spec: declare is deferred ONLY if it routes differently).
        diff_cmd("declare -a x=(1 2)");
        diff_cmd("local a=(1 2)");
        diff_cmd("export e=(1)");
        diff_cmd("readonly r=(1)");
    }

    #[test]
    fn atoms_array_literal_corpus() {
        diff_cmd("a=(${arr[@]} ${arr[*]})");        // array expansions as values
        diff_cmd("a=(x=y z=w)");                     // `=`-containing values (NOT subscripts)
        diff_cmd("a=(=leading)");                    // value starting with `=`
        // `)(` — a `(` glued right after a CLOSED array literal is NOT array
        // glue (that only happens for the FIRST `(` immediately after `name=`/
        // `name+=`, captured as the zero-width `ArrayOpen` atom). The oracle
        // attempts a function-definition parse for ANY leading word followed
        // by `(`; `valid_function_name_text` then rejects the multi-part
        // assignment word (`[Literal("a="), ArrayLiteral(..)]`, not a single
        // Literal) with `FunctionName`. v252 T4 tightened the
        // `parse_command_or_pipeline` guard (was: any `is_assignment_word`
        // word never attempts funcdef) to skip the funcdef attempt ONLY when
        // the oracle would accept the word as a function name
        // (`is_assignment_word(&w) && valid_function_name_text(&w).is_some()` —
        // exactly the single unquoted-`Literal` shape like `a=b`), preserving
        // the v248-pinned `a=b () {…}` divergence (see
        // `atoms_function_assignment_name_divergence`), so a closed array
        // literal (and the `AssignPrefix`-led `a+=(..)`/`a[i]=(..)` shapes) now
        // falls through to the SAME `FunctionName` error as the oracle.
        assert_eq!(new_seq("a=(one)(two)"), old_seq("a=(one)(two)"), "error parity for \"a=(one)(two)\"");
        diff_cmd("a=(a)b");                          // text glued after the close paren
        diff_cmd("cmd a=(1 2) b=(3 4)");             // two array assignments in one command
        diff_cmd("a=(   )");                         // whitespace-only body == empty
        diff_cmd("a=(\n)");                          // newline-only body == empty
        // "nots a =(1 2)" — space before `=` means `a` and `=(1` are SEPARATE
        // words (`=(1` is not assignment-shaped, so no array literal is even
        // attempted); both paths reject the same way, but neither actually
        // returns `Ok` (bash: `a` is a bare word, `=(1` and `2)` are literal
        // args to `nots`, which IS valid... but on both paths here the `(`/`)`
        // inside `=(1` and `2)` are lexed as bare `Op(LParen)`/`Op(RParen)`
        // operator tokens at word-start, which is unexpected in argument
        // position) — use error-PARITY, not `diff_cmd` (which requires `Ok`
        // on both sides).
        assert_eq!(new_seq("nots a =(1 2)"), old_seq("nots a =(1 2)"), "error parity for \"nots a =(1 2)\"");

        // T2 carry-forward: `a=(x=(1 2) y)` — a NESTED array literal AS AN
        // ELEMENT VALUE. NOT a `diff_cmd`: the oracle's `scan_array_element_word`
        // scans an element's raw text up to the first unescaped whitespace/`)`
        // WITHOUT tracking `(` nesting at all, then re-tokenizes that (possibly
        // truncated) substring. For this input the raw text is cut at the space
        // inside "x=(1 2)" (giving "x=(1"), and re-tokenizing "x=(1" recurses into
        // `scan_array_literal` for the embedded `=(` with no closing `)` in the
        // truncated slice — so the ORACLE ITSELF fails to lex this input
        // (`UnterminatedArrayLiteral`), confirmed against real bash 5.2 (`bash -c
        // 'a=(x=(1 2) y)'` → "syntax error near unexpected token `('" — bash
        // doesn't support this construct either). The atom path's `ArrayOpen`
        // recursion in `parse_word_command` is more general than the oracle's
        // element scanner: it happily recurses into a nested `Mode::ArrayLiteral`
        // and parses the inner array correctly, so it currently ACCEPTS input the
        // oracle (and bash) reject. Bit-for-bit reproducing the oracle's
        // incidental truncate-then-retokenize failure is disproportionate for a
        // construct with no real bash meaning — documented here as a narrow,
        // low-severity, atom-more-permissive gap (candidate follow-on `[deferred]`
        // divergence for the whole-branch review), not pinned or forced to pass.
        assert!(tokenize_with_opts("a=(x=(1 2) y)", LexerOptions::default()).is_err());
        assert!(new_seq("a=(x=(1 2) y)").is_ok());

        // `a=($(cat <<X\nhi\nX\n))` (heredoc-in-cmdsub as an array element) is
        // DELIBERATELY OMITTED here: it hits the documented v250 gap where a
        // heredoc's body is dropped when the heredoc sits inside a `$(…)`/`` `…` ``
        // body (the parser's redirect-attach walk doesn't recurse into
        // `WordPart::CommandSub`/`Backtick`/`Arith` sequences) — a known,
        // pre-existing carry-forward, not an array-literal bug. Confirmed by
        // direct comparison: `old_seq` fills the heredoc `body` with
        // `[Literal("hi"), Literal("\n", quoted:true)]`; `new_seq` gives
        // `body: Word([])`. Per the brief: drop the line, rely on the existing
        // carry-forward note, do not pin new.
    }

    #[test]
    fn atoms_dquote_nested() {
        diff_cmd("echo \"$(echo hi)\"");
        diff_cmd("echo \"$(echo $x)\"");
        diff_cmd("echo \"a${b}c\"");
        diff_cmd("echo \"$a $b\"");
        diff_cmd("echo \"pre$(c)$((1+2))post\"");
        diff_cmd("echo \"\\$lit \\\" \\\\ end\"");
    }

    // ── v247 T7: broadened differential corpus + deferred/error parity ──────────

    #[test]
    fn atoms_adversarial() {
        // Adversarial word-splitting / gluing across quotes, expansions, and
        // operators — the atom-assembled Word must match the oracle byte-for-byte.
        for s in [
            "a\"b\"$c", "a\\ b", "x=$y\"z\"", "$a$b$c", "'a'\"b\"c$d",
            "  a   b  ", "a>b", "a>>b<c", "echo \"$(echo $x)\"", "echo ${a[$i]}",
        ] { diff_cmd(s); }
    }

    #[test]
    fn atoms_error_parity() {
        // Parser-level malformed input (the oracle LEXES successfully): the atom
        // path must return the SAME error as the oracle. (Normalize Ok/Err to
        // unit + error-debug so a divergent error PAYLOAD — not just variant —
        // is still caught.)
        for s in ["if true", "for", "case x in", "( a"] {
            assert_eq!(
                new_seq(s).map(|_| ()).map_err(|e| format!("{e:?}")),
                old_seq(s).map(|_| ()).map_err(|e| format!("{e:?}")),
                "error parity for {s:?}",
            );
        }
        // `echo $(` / `echo ${` are LEXER-level rejects on the oracle: the
        // production batch `tokenize_with_opts` errors on the unterminated opener
        // (`UnterminatedSubstitution`) BEFORE parsing, so `old_seq` cannot yield a
        // Result to compare. The atom path (incremental live lexer) rejects the
        // same inputs at PARSE time. Both REJECT — assert parity of rejection, not
        // of the error stage.
        for s in ["echo $(", "echo ${"] {
            assert!(new_seq(s).is_err(),
                "atom path must reject {s:?}, got {:?}", new_seq(s));
        }
    }

    #[test]
    fn atoms_deferred_unsupported() {
        // Every deferred construct defers CLEANLY on the atom path (proving the
        // deferral is deliberate, not an accidental parse). The oracle may parse
        // some of these — the point is only that the atom path returns
        // UnsupportedCommand rather than a wrong AST.
        // `f() { :; }` is NO LONGER deferred — v248 T2 implements the POSIX
        // `name()` funcdef form (see `atoms_function_paren_form`).
        // NOTE: `cat <<EOF\nx\nEOF` (expanding heredoc) is NO LONGER deferred —
        // v250 T4 implements expanding-heredoc bodies (see `atoms_heredoc_expanding`).
        // NOTE: `a=(1 2 3)` (positional array literal) is NO LONGER deferred —
        // v252 T1 implements `name=(…)`/`name+=(…)` via `Mode::ArrayLiteral`
        // (see `atoms_array_literal_positional`).
        // NOTE: `[[ a == b ]]` is NO LONGER deferred — v253 T1 implements
        // `[[ … ]]` (see `atoms_double_bracket_core`).
        // NOTE: `(( 1+2 ))` (standalone arith command) is NO LONGER deferred —
        // v255 T1 implements it (see `atoms_arith_command`).
        // NOTE: `for ((i=0;i<3;i++)); do :; done` (C-style ArithFor) is NO LONGER
        // deferred — v256 T2 implements it (see `atoms_arith_for`).
        diff_cmd("for ((i=0;i<3;i++)); do :; done");
        for s in [
            "coproc x { :; }",
            // `$[expr]` legacy arith (deferred to Stage 2): defers cleanly rather
            // than mis-lexing `$` + `[expr]` as two literals. Word-start and glued.
            "echo $[1+2]", "echo pre$[1+2]post",
        ] {
            assert!(matches!(new_seq(s), Err(ParseError::UnsupportedCommand)),
                "expected UnsupportedCommand on atom path for {s:?}, got {:?}", new_seq(s));
        }
        // `$[expr]` inside `"…"` defers via `parse_dquote` → UnsupportedExpansion.
        for s in ["echo \"$[1+2]\"", "echo \"pre$[1+2]\""] {
            assert!(matches!(new_seq(s), Err(ParseError::UnsupportedExpansion)),
                "expected UnsupportedExpansion on atom path for {s:?}, got {:?}", new_seq(s));
        }
    }

    #[test]
    fn atoms_no_hang_on_redirect_in_word_list() {
        // Regression: a `RedirFd`/`Heredoc` atom where a word is expected (for/
        // select `in`-list, case pattern) must ERROR, not spin. The oracle hits an
        // `unreachable!()`/UnexpectedToken on the same malformed input (it consumes
        // first, so it panics rather than hangs); the atom path must terminate with
        // an Err. (No `diff_*` here — `old_seq` would panic.)
        for s in [
            "for i in <<a; do :; done",
            "for i in 3>f; do :; done",
            "select i in <<a; do :; done",
            "case x in <<a) ;; esac",
        ] {
            assert!(new_seq(s).is_err(),
                "atom path must reject (not hang on) {s:?}, got {:?}", new_seq(s));
        }
    }

    // ── v248: function definitions on the atom path ──────────────────────────
    #[test]
    fn atoms_function_keyword_form() {
        diff_cmd("function f { :; }");
        diff_cmd("function f() { :; }");
        diff_cmd("function f ()  { :; }");        // spaced ()
        diff_cmd("function greet { echo hi; }");
        diff_cmd("function f\n{ :; }");           // newline before body
        diff_cmd("function 1 { :; }");            // numeric name is valid (AST parity, not just Ok/Ok)
    }

    #[test]
    fn atoms_function_paren_form() {
        diff_cmd("f(){ :; }");
        diff_cmd("f() { :; }");
        diff_cmd("f ()  { :; }");                 // spaced name/()
        diff_cmd("f() ( a; b )");                  // subshell body
        diff_cmd("f() if x; then y; fi");          // if body
        diff_cmd("f() while x; do y; done");       // while body
        diff_cmd("f() for i in a b; do echo $i; done");
        diff_cmd("f() case $x in a) echo a;; esac");
        diff_cmd("f() select x in a b; do echo $x; break; done");
        diff_cmd("f() until x; do y; done");        // until body
        diff_cmd("f() { :; } >log");               // redirected body
        diff_cmd("f() { :; } 2>&1");
        diff_cmd("f() { g() { :; }; }");           // nested funcdef
        diff_cmd("if true; then f() { :; }; fi");  // funcdef inside a compound
        diff_cmd("f() { :; } | cat");               // funcdef as a pipeline stage
        diff_cmd("f() { :; }; g() { :; }");          // two funcdefs, ; separated
        diff_cmd("true && f() { :; }");              // funcdef after a connector
    }
    #[test]
    fn atoms_function_not_a_def() {
        diff_cmd("f");                             // bare word = plain command
        diff_cmd("echo function");                 // `function` mid-command = arg
        diff_cmd("func --opt");                    // prefix of `function` = plain command (mark/rewind restores)
    }

    #[test]
    fn atoms_function_defs_errors() {
        for s in [
            "f() echo",          // non-compound body → FunctionBody
            "function",          // no name → FunctionName
            "function if { :; }", // reserved word as name → FunctionName
            "f(",                // unterminated
            "f()",               // `()` then EOF → UnterminatedFunction/FunctionBody
            "f ( a )",           // `(` not followed by `)` → FunctionBody (NOT a command)
        ] {
            assert_eq!(
                new_seq(s).map(|_| ()).map_err(|e| format!("{e:?}")),
                old_seq(s).map(|_| ()).map_err(|e| format!("{e:?}")),
                "funcdef error parity for {s:?}",
            );
        }
    }

    #[test]
    fn atoms_function_defs_deferred() {
        // Body is itself deferred → whole funcdef defers (lifts when [[ ]]/arith land).
        // `f() [[ x ]]` is NO LONGER deferred — v253 T1 implements `[[ … ]]`
        // (see `atoms_function_double_bracket_body`).
        // `f() (( 1 ))` is NO LONGER deferred — v255 T1 implements the standalone
        // arith command, and funcdef bodies dispatch through `parse_command`.
        diff_cmd("f() (( 1 ))");
        // `f() for ((i=0;i<2;i++)); do :; done` is NO LONGER deferred — v256 T2
        // implements C-style `for (( … ))`, and `Command::ArithFor` is in the
        // oracle's `is_function_body_shape` allow-list (command.rs:1168).
        diff_cmd("f() for ((i=0;i<2;i++)); do :; done");
    }

    #[test]
    fn atoms_function_double_bracket_body() {
        diff_cmd("f() [[ x ]]");
        diff_cmd("f() [[ -f a && -f b ]]");
    }

    #[test]
    fn atoms_function_assignment_name_divergence() {
        // KNOWN divergence (v248): the oracle accepts `a=b () {...}` as
        // FunctionDef{name:"a=b"} because command.rs checks `(` before the
        // assignment check; the atom path's is_assignment_word guard defers it.
        // bash itself rejects this as a syntax error, so the atom path (defer) is
        // arguably more correct. Pinned so the Stage-2 live-flip differential gate
        // knows about it. (If a future iteration reconciles this, update here.)
        assert!(matches!(new_seq("a=b () { :; }"), Err(ParseError::UnsupportedCommand)),
            "atom path defers `a=b () {{...}}`, got {:?}", new_seq("a=b () { :; }"));
        assert!(old_seq("a=b () { :; }").is_ok(),
            "oracle accepts `a=b () {{...}}` (documents the divergence)");
    }

    // ── v249: here-strings (`<<<`) on the atom path ──────────────────────────
    #[test]
    fn atoms_here_string_redirect() {
        diff_cmd("cat <<< hello");
        diff_cmd("wc -l <<<foo");                 // glued, no space
        diff_cmd("cat <<< \"$x\"");                // quoted expansion target
        diff_cmd("cat <<< 'lit'");
        diff_cmd("cat <<< $'a\\tb'");              // ANSI-C target
        diff_cmd("cat <<< $var");
        diff_cmd("cat <<< a b");                    // target is `a`; `b` is an arg
        diff_cmd("cmd <<< x > out");                // here-string + file redirect, source order
        diff_cmd("cmd 2>&1 <<< x");                 // fd-dup + here-string
        diff_cmd("cmd <<< a <<< b");                // two here-strings, ordered list
        diff_cmd("{ cat; } <<< x");                 // brace-group trailing here-string
        diff_cmd("if true; then :; fi <<< x");       // if-compound trailing here-string
    }

    #[test]
    fn atoms_here_string_leading() {
        diff_cmd("<<< word");
        diff_cmd("<<<foo");                         // glued
        diff_cmd("<<< \"$x\"");
        diff_cmd("<<< word > out");                 // leading here-string + file redirect
        diff_cmd("<<< x | cat");                    // here-string stage in a pipeline
        // Determined by observation: the oracle accepts a leading `<<<` as the
        // first pipeline stage (falls through to parse_pipeline → parse_simple_stage
        // exactly like the atom path), so this is `diff_cmd` parity, not a divergence.
    }

    #[test]
    fn atoms_here_string_fd_prefix() {
        // Determined by observation: `3<<<` lexes fine on the oracle's batch
        // tokenizer (no lexer-level panic) and both paths produce the identical
        // AST, so this is ordinary `diff_cmd` parity.
        diff_cmd("3<<< word");                      // fd-prefixed here-string
    }

    #[test]
    fn atoms_here_string_errors() {
        // Determined by observation: none of these inputs panic `old_seq` at the
        // lexer level (the oracle lexes all of them successfully and rejects at
        // parse time), so every one is a plain error-parity comparison — no
        // atom-path-only bucket needed (contrast `atoms_error_parity`'s
        // `echo $(`/`echo ${` split, which DOES need one).
        for s in ["cat <<<", "<<<", "cat <<< |", "cat <<< <", "cat <<< ;"] {
            assert_eq!(
                new_seq(s).map(|_| ()).map_err(|e| format!("{e:?}")),
                old_seq(s).map(|_| ()).map_err(|e| format!("{e:?}")),
                "here-string error parity for {s:?}",
            );
        }
    }

    #[test]
    fn atoms_heredoc_expanding_no_trailing_newline() {
        // v250 T4: EXPANDING heredocs are now supported end-to-end (the T3 defer
        // gate is gone). These are the exact cases the old deferral test used,
        // now asserted for oracle parity — including a delimiter line at EOF with
        // no trailing newline.
        diff_cmd("cat <<EOF\nx\nEOF");
        diff_cmd("<<EOF\nx\nEOF");
    }

    // v250 T4 tests: expanding heredocs (bare/unquoted delimiter) end-to-end

    #[test]
    fn atoms_heredoc_expanding() {
        diff_cmd("cat <<EOF\nhello $x\nEOF\n");
        diff_cmd("cat <<EOF\n${y:-d} and $(echo hi)\nEOF\n");
        diff_cmd("cat <<EOF\n`echo bt` $((1+2))\nEOF\n");
        diff_cmd("cat <<EOF\nlit \\$notvar \\` \\\\ end\nEOF\n");   // heredoc backslash rules
        diff_cmd("cat <<EOF\na \\\nb\nEOF\n");                        // \<NL> line continuation
        diff_cmd("cat <<EOF\n\"quotes\" 'stay' literal\nEOF\n");     // quotes literal in body
    }

    #[test]
    fn atoms_heredoc_expanding_more() {
        diff_cmd("cat <<EOF\nplain text\nEOF\n");                    // plain, quoted:false content
        diff_cmd("cat <<EOF\nEOF\n");                                 // empty expanding body
        diff_cmd("cat <<EOF\n\nEOF\n");                               // single blank line
        diff_cmd("cat <<EOF\n$x$y${z}\nEOF\n");                       // adjacent expansions
        diff_cmd("cat <<EOF\n$1 $@ $? $#\nEOF\n");                    // specials
        diff_cmd("cat <<-EOF\n\tindented $x\n\tEOF\n");               // <<- tab strip + expand
        diff_cmd("cat <<EOF\nline one\nline two $x\nEOF\n");          // multi-line
        diff_cmd("cat <<EOF\ntrailing $\nEOF\n");                     // lone $ at line end
        diff_cmd("cat <<EOF && echo ok\nhi $x\nEOF\n");               // sequence continues
        diff_cmd("cat <<EOF | wc -l\nhi $x\nEOF\n");                  // pipeline stage
        diff_cmd("<<EOF\nx $y\nEOF\n");                                // leading expanding heredoc
    }

    #[test]
    fn atoms_heredoc_expanding_edges() {
        diff_cmd("cat <<EOF\nend \\$\nEOF\n");                        // escaped $ right before newline sep
        diff_cmd("cat <<EOF\n\\$\\`\\\\\nEOF\n");                     // all three escapes, adjacent
        diff_cmd("cat <<EOF\nx\\\nEOF\nEOF\n");                       // `x\` continues onto `EOF`, NOT delim
        diff_cmd("cat <<EOF\n`echo $x`\nEOF\n");                      // var inside backtick in body
        diff_cmd("cat <<EOF\n${x:-`echo hi`}\nEOF\n");                // backtick inside ${…} in body
        diff_cmd("cat <<EOF\nouter $(echo $inner) tail\nEOF\n");      // nested $() with var
        diff_cmd("cat <<'A' <<B\nlit $x\nA\nexp $y\nB\n");            // literal + expanding, ordered
        diff_cmd("cat <<B <<'A'\nexp $y\nB\nlit $x\nA\n");            // expanding + literal, ordered
        diff_cmd("cat <<EOF\na\\zb\nEOF\n");                          // lone backslash (ordinary) stays literal
    }

    #[test]
    fn atoms_heredoc_expanding_continuation_delimiter() {
        // v250 T4 fix (F1): a close delimiter FORMED across a `\<NL>` continuation
        // spans multiple physical lines. `heredoc_at_delim_line` reads the whole
        // joined logical line to match, so the consumption must advance the real
        // cursor by that whole span — consuming only one physical line would leak
        // the remainder as a spurious command. bash: `EO\<NL>F` joins to `EOF` =
        // the delimiter, body empty, then runs `echo after`.
        diff_cmd("cat <<EOF\nEO\\\nF\necho after\n");        // `EO\<NL>F` == EOF (empty body)
        diff_cmd("cat <<-EOF\n\tEO\\\nF\necho after\n");     // <<- variant: `\tEO\<NL>F` strips to EOF
        // Guard the other direction (no over-consumption): a continuation-joined
        // BODY line that is NOT the delimiter must stay a body line, with the real
        // `EOF` line still closing it and `echo after` following.
        diff_cmd("cat <<EOF\nab\\\ncd\nEOF\necho after\n");  // `ab\<NL>cd` == abcd (body, not delim)
    }

    #[test]
    fn atoms_heredoc_multiline_cmdsub_divergence() {
        // v250 T4 KNOWN divergence (F2, INTENTIONAL — atom path is the target/bash
        // behavior): a multi-line `$(…)` inside an expanding heredoc body whose `)`
        // is on a LATER line than its `$(`. bash ALLOWS this (verified:
        //   cat <<EOF
        //   $(echo hi
        //   echo bye)
        //   EOF
        // prints hi then bye). The atom path pushes a CommandSub sub-mode that scans
        // the nested command across newlines from the cursor, so it parses fine. The
        // command.rs ORACLE scans each heredoc body line with a LINE-LOCAL cursor, so
        // an unclosed `$(` on its own line is an error there. This is an accepted
        // atom-vs-oracle divergence; the atom path is correct. Do NOT use `diff_cmd`.
        let s = "cat <<EOF\n$(echo hi\necho bye)\nEOF\n";
        assert!(new_seq(s).is_ok(),
            "atom path must parse multi-line $() in heredoc (matches bash): {:?}", new_seq(s));
        // The oracle diverges at the LEXER level: its line-local heredoc-body scan
        // hits an unclosed `$(` on the first body line and errors before parsing
        // (observed `Err(LexError::UnterminatedSubstitution)`). `old_seq` would
        // `.expect("lex")`-panic on it, so probe the fallible lexer directly.
        assert!(
            tokenize_with_opts(s, LexerOptions::default()).is_err(),
            "oracle lexer is expected to diverge (line-bounded heredoc scan errors on the split $())");
    }

    #[test]
    fn atoms_heredoc_in_cmdsub_body_drop_divergence() {
        // v250 KNOWN DEFERRED divergence (NOT intentional-correct like
        // `atoms_heredoc_multiline_cmdsub_divergence` above — this one is a real gap,
        // tracked as a `[deferred]` entry in docs/bash-divergences.md): a heredoc
        // opened INSIDE a command substitution has its body DROPPED on the atom
        // path. Root cause: `fill_command`/`fill_sequence` in this file only walk
        // the top-level command tree to attach collected heredoc bodies to their
        // `Heredoc` redirection atoms — they don't descend into the nested
        // `Sequence` carried by `WordPart::CommandSub` (backticks lower to the
        // same variant) or `WordPart::Arith` word parts, so a heredoc whose
        // `<<X` appears inside a `$(…)` or `` `…` `` never gets its body
        // attached and is left as `Word([])`. The
        // oracle (command.rs) is correct here. This is OUT of v250's declared scope
        // (§4 covers cmdsub-INSIDE-a-heredoc-body, the reverse nesting, which
        // works) and is dormant (`command_atoms` stays false).
        //
        // This test PINS the current (wrong) atom-path behavior. It must be
        // updated (and likely replaced with a `diff_cmd`) when the gap is fixed.
        fn inner_heredoc_body(seq: &Sequence) -> &Word {
            // Unwrap: Sequence -> Pipeline -> Simple(Exec) "echo" -> args[0] Word
            // -> CommandSub part (backticks lower to the same WordPart::CommandSub)
            // -> inner Sequence -> Pipeline -> Simple(Exec) "cat" -> redirects[0]
            // -> Heredoc.body
            let Command::Pipeline(p) = &seq.first else { panic!("expected pipeline") };
            let Command::Simple(SimpleCommand::Exec(outer)) = &p.commands[0] else {
                panic!("expected exec")
            };
            let arg = &outer.args[0];
            let WordPart::CommandSub { sequence: inner_seq, .. } = &arg.0[0] else {
                panic!("expected CommandSub part, got {:?}", arg.0[0])
            };
            let Command::Pipeline(ip) = &inner_seq.first else { panic!("expected inner pipeline") };
            let Command::Simple(SimpleCommand::Exec(inner)) = &ip.commands[0] else {
                panic!("expected inner exec")
            };
            let RedirOp::Heredoc { body, .. } = &inner.redirects[0].op else {
                panic!("expected heredoc redirect")
            };
            body
        }

        for s in [
            "echo $(cat <<X\nhi\nX\n)\n",   // $(...) command substitution
            "echo `cat <<X\nhi\nX\n`\n",    // backtick command substitution
        ] {
            let new = new_seq(s).unwrap().expect("new_seq parses");
            let old = old_seq(s).unwrap().expect("old_seq parses");
            // The two ASTs differ overall (this is the divergence) ...
            assert_ne!(new, old, "expected atom path to diverge from oracle for {s:?}");
            // ... specifically at the inner heredoc body: atom path drops it, the
            // oracle keeps it. If this ever becomes equal, the gap has been fixed —
            // update/replace this test (e.g. with diff_cmd).
            assert_eq!(inner_heredoc_body(&new), &Word(vec![]),
                "atom path expected to have DROPPED the inner heredoc body for {s:?}: {new:#?}");
            assert_ne!(inner_heredoc_body(&old), &Word(vec![]),
                "oracle expected to have a NON-EMPTY inner heredoc body for {s:?}: {old:#?}");
        }
    }

    // v250 T3 tests: literal heredocs (quoted/escaped delimiter) end-to-end

    #[test]
    fn atoms_heredoc_literal() {
        diff_cmd("cat <<'EOF'\nhello $x\nEOF\n");
        diff_cmd("cat <<'EOF'\nEOF\n");                 // empty body
        diff_cmd("cat <<-'EOF'\n\ttabbed\n\tEOF\n");     // <<- strip
        diff_cmd("cat <<\"EOF\"\nline1\nline2\nEOF\n");  // double-quoted delim = literal
        diff_cmd("<<'EOF'\nx\nEOF\n");                    // leading heredoc (empty-words cmd)
    }

    #[test]
    fn atoms_heredoc_literal_sequence_continuation() {
        // A newline-consumption site that fails to drain the heredoc-body atom
        // group after the delimiter line would make the parser choke on (or
        // hang trying to parse) whatever follows — guard every shape that
        // keeps parsing PAST a literal heredoc's body.
        diff_cmd("cat <<'EOF'\nx\nEOF\necho done\n");         // ; -like newline connector
        diff_cmd("cat <<'EOF'\nx\nEOF\necho a; echo b\n");    // more of the sequence after
        diff_cmd("cat <<'EOF' && echo ok\nx\nEOF\n");         // && after a heredoc-bearing stage
        diff_cmd("cat <<'EOF' | wc -l\nx\nEOF\n");            // heredoc stage in a pipeline
        diff_cmd("cat <<'A' <<'B'\nfirst\nA\nsecond\nB\n");   // two heredocs, ordered bodies
        diff_cmd("if cat <<'EOF'; then echo y; fi\nx\nEOF\n"); // heredoc in a compound's condition
        diff_cmd("for i in 1; do cat <<'EOF'; done\nx\nEOF\n"); // heredoc inside a loop body
    }

    // v250 T5 tests: systematic positional coverage (every command position)

    #[test]
    fn atoms_heredoc_positions() {
        diff_cmd("cat <<A <<B\nbodyA\nA\nbodyB\nB\n");            // stacked, order A then B
        diff_cmd("a <<X | b <<Y\nx\nX\ny\nY\n");                   // across a pipeline
        diff_cmd("{ cat <<EOF\nx\nEOF\n}\n");                      // heredoc in a brace group
        diff_cmd("if true; then cat <<EOF\nx\nEOF\nfi\n");         // heredoc in an if body
        diff_cmd("cat <<EOF >out arg\nx\nEOF\n");                  // interleaved with redirect + word
        diff_cmd("cat <<A; echo hi\nbodyA\nA\n");                  // heredoc then `;` then command
    }

    #[test]
    fn atoms_heredoc_positions_compound_bodies() {
        diff_cmd("while false; do cat <<EOF; done\nx\nEOF\n");     // heredoc in a while body
        diff_cmd("for i in 1; do cat <<EOF; done\nx\nEOF\n");      // heredoc in a for body (expanding)
        diff_cmd("case x in a) cat <<EOF;; esac\nx\nEOF\n");       // heredoc in a case body
        diff_cmd("( cat <<EOF\nx\nEOF\n)\n");                      // heredoc in a subshell
    }

    #[test]
    fn atoms_heredoc_positions_trailing_compound_redirect() {
        // Redirected{inner, redirects}: the wrapped command's own heredoc body
        // must be collected BEFORE the compound's own trailing heredoc body.
        diff_cmd("{ cat; } <<EOF\nx\nEOF\n");
        diff_cmd("if true; then cat; fi <<EOF\nx\nEOF\n");
    }

    #[test]
    fn atoms_heredoc_positions_misc() {
        diff_cmd("cat 2>&1 <<EOF\nx\nEOF\n");                      // heredoc after another redirect
        // Mixed literal + expanding, stacked: proves the per-heredoc expand
        // flag routes through the attach walk to the RIGHT redirect.
        diff_cmd("cat <<'A' <<B\n$lit\nA\n$exp\nB\n");
        // FD-prefixed heredoc: the `3` is a `RedirFd` atom emitted ahead of
        // the `<<` opener by the word-run arm.
        diff_cmd("cat 3<<EOF\nx\nEOF\n");
    }

    // v250 T6: mark/rewind heredoc-state generation guard + error parity + adversarial corpus

    #[test]
    fn atoms_heredoc_marks_dont_span_bodies() {
        // NOTE (fix pass, corrects a prior comment error): funcdef detection on
        // the atom path does NOT use `mark`/`rewind` — it seeds the already-
        // consumed leading word instead (v248's seed-not-rewind approach), so it
        // never calls `Lexer::rewind` at all. The ONLY live `mark`/`rewind` pair
        // reachable on the atom command path is the arith `$((`-bail wrinkle in
        // `parse_arith_expansion` (see `arith_wrinkle_falls_back_to_cmdsub`): a
        // depth-0 `)` not followed by `)` bails the arith scan, and the parser
        // rewinds to the `$((` start to re-drive it as `$(` + a subshell `(`.
        //
        // These two plain cases (no `$((` anywhere) never call `rewind` at all,
        // so `heredoc_gen`'s `debug_assert_eq!` is not exercised by them — they
        // only prove the heredoc plumbing itself is fine.
        diff_cmd("cat <<EOF\nx\nEOF\n");
        diff_cmd("f() { cat <<EOF\nx\nEOF\n}\n");

        // This case DOES drive `rewind` while a heredoc body is actively being
        // emitted (`emitting_heredoc.is_some()`): the expanding body line
        // `$((cat) )` opens an arith expansion whose body bails (mirrors
        // `arith_wrinkle_falls_back_to_cmdsub`'s `"$((cat) )"`), so
        // `parse_arith_expansion` rewinds back to the `$((` start and re-drives
        // it as a command substitution containing a subshell — all while the
        // heredoc body atom stream is mid-emission. If a future change widened
        // the mark/rewind window to cross a heredoc-state mutation (the `began`
        // flip, an `at_line_start` toggle, the newline trigger, or body-end),
        // `Lexer::rewind`'s `debug_assert_eq!(self.heredoc_gen, m.heredoc_gen,
        // ...)` would panic under a debug test build. Verified (fix pass) via a
        // temporary `eprintln!` in `rewind` that this input actually reaches
        // `rewind` with `emitting_heredoc` still `Some`.
        diff_cmd("cat <<EOF\n$((cat) )\nEOF\n");
    }

    #[test]
    fn atoms_heredoc_errors() {
        // Determined by observation (see v250 T6 report): all three inputs are
        // LEXER-level rejects on the oracle — `tokenize_with_opts` returns
        // `Err(LexError::UnterminatedHeredoc)` before parsing even starts, so
        // `old_seq`'s `.expect("lex")` would panic on them (mirrors
        // `atoms_error_parity`'s `echo $(`/`echo ${` split). Assert only that the
        // atom path rejects too, not error-payload equality.
        for s in ["cat <<EOF\nno close\n", "cat <<", "cat <<-\n"] {
            assert!(new_seq(s).is_err(),
                "atom path must reject unterminated/malformed heredoc {s:?}, got {:?}", new_seq(s));
        }
    }

    #[test]
    fn atoms_heredoc_adversarial() {
        for s in [
            "cat <<EOF\nEOFX not a close\nEOF\n",         // delimiter as substring
            "cat <<EOF\n  EOF\nEOF\n",                     // indented non-close (no <<-)
            "cat <<-EOF\n\t\tEOF\n",                       // <<- tabbed close
            "cat <<'E'\n$x `no` ${expand}\nE\n",           // literal: nothing expands
            "cat <<EOF\n$1 $@ $? $#\nEOF\n",               // special params in expanding body
            "cat <<EOF\n\\EOF\nEOF\n",                     // escaped-looking body line
        ] { diff_cmd(s); }
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

    // v243 T5 tests

    #[test]
    fn cmd_for_select() {
        diff_cmd("for i in a b c; do echo $i; done");
        diff_cmd("for i; do x; done");               // no-`in`
        diff_cmd("for i in; do x; done");            // empty in-list
        diff_cmd("for i in a; do for j in b; do x; done; done");   // nested
        diff_cmd("for i in a b; do if x; then y; fi; done");
        diff_cmd("for i in a; do x; done | cat");    // as pipeline stage
        diff_cmd("for i in a; do x; done 2>&1");     // trailing redirect
        diff_cmd("select x in a b; do y; done");
        diff_cmd("select x; do y; done");            // no-`in`
        diff_cmd("select x in a b c; do echo $x; break; done");
        // `for ((…)) …` (ArithFor) is NO LONGER deferred — v256 T2 implements it
        // (see `atoms_arith_for`).
        diff_cmd("for ((i=0;i<3;i++)); do x; done");
        diff_err("for i in a; do x");                // unterminated parity
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
        // `for i in a; do x; done` removed: for/select are now in-scope (Task 5).
        // `case x in …; esac` removed: case is now in-scope (Task 6).
        // `f() { x; }` removed: function-def (`name()`) is now in-scope (v248 T2).
        // `[[ -n x ]]` removed: `[[ … ]]` is now in-scope (v253 T1).
        // `(( 1+2 ))` removed: standalone arith command is now in-scope (v255 T1).
        for s in ["coproc x"] {
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
    fn cmd_heredoc_supported() {
        // Here-string (`<<<`, v249 T1), LITERAL heredocs (`<<'EOF'`/`<<"EOF"`,
        // v250 T3 — `atoms_heredoc_literal`), and EXPANDING heredocs (bare/unquoted
        // delimiter, v250 T4 — `atoms_heredoc_expanding`) are ALL supported now.
        diff_cmd("cat <<EOF\nx\nEOF");
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

    // v243 T6 tests

    #[test]
    fn cmd_case() {
        diff_cmd("case $x in a) 1;; esac");
        diff_cmd("case $x in a) 1;; b) 2;; esac");
        diff_cmd("case $x in a|b|c) 1;; esac");       // pattern list
        diff_cmd("case $x in (a) 1;; esac");          // leading paren
        diff_cmd("case x in a) ;; esac");             // empty body
        diff_cmd("case x in a) 1;; *) 2;; esac");     // default
        diff_cmd("case $x in a) 1;& b) 2;; esac");    // ;& fallthrough
        diff_cmd("case $x in a) 1;;& b) 2;; esac");   // ;;& continue-match
        diff_cmd("case $x in a) if y; then z; fi;; esac");  // compound in body
        diff_cmd("case $x in a) case $y in b) c;; esac;; esac");  // nested case
        diff_cmd("case $x in a) 1;; esac | cat");    // case as pipeline stage
        diff_cmd("case $x in a) 1;; esac >f");        // trailing redirect
        diff_cmd("for i in a; do case $i in q) x;; esac; done");  // case in for body
        diff_err("case x in");                         // unterminated parity
    }

    // v243 T7 tests

    #[test]
    fn cmd_compound_deferred_still() {
        // `[[ -n x ]]` (test grammar) removed: now in-scope, v253 T1.
        // `f() { x; }` (function def, `name()`) removed: now in-scope, v248 T2.
        // `(( 1+2 ))` / `(( x + $y ))` (standalone arith command) removed: now
        // in-scope, v255 T1 (see `atoms_arith_command`).
        // `for ((…)) …` (ArithFor) removed: now in-scope, v256 T2 (see
        // `atoms_arith_for`).
        diff_unsupported("coproc x");
        // `cat <<<w` (here-string) removed: now in-scope, v249 T1.
    }

    // v255: standalone arith command `(( … ))`
    #[test]
    fn atoms_arith_command() {
        // Glued `((` that closes on the matching `))` → Command::Arith (byte-identical).
        diff_cmd("(( 1 + 2 ))");
        diff_cmd("((1+2))");
        diff_cmd("(( x = 5 ))");
        diff_cmd("(( x++ ))");
        diff_cmd("(( $x + 1 ))");        // embedded expansion — wires parse_arith_body
        // Primary bail → nested subshell backoff (depth-0 `)` not followed by `)`).
        diff_cmd("((cmd); c2)");
        // Spaced `( (` is NEVER arith — regression guard for the existing subshell path.
        diff_cmd("( ( 3 * 4 ) )");
        // Unterminated glued `((` (no matching `))`): both paths bail → subshell → same
        // parse error (oracle falls back to `( (1+2)` → UnterminatedSubshell; no lex panic).
        diff_err("((1+2)");
    }

    #[test]
    fn atoms_arith_command_disambiguation() {
        // ── Close cleanly → Command::Arith ──────────────────────────────────
        diff_cmd("(())");                // empty body → Arith(Word([]))
        diff_cmd("(( ))");               // single-space body → Arith([Literal " "])
        diff_cmd("(( (1+2) * 3 ))");     // inner grouping parens: depth-tracked, NOT a bail
        diff_cmd("(( a[0] + 1 ))");      // subscript brackets are plain body text
        diff_cmd("(( a + $b + ${c} ))"); // multiple embedded expansions
        diff_cmd("(( 1+2 ))");           // exact string freed from the old deferral tests
        diff_cmd("(( x + $y ))");        // exact string freed from the old deferral tests
        // ── Bail → nested subshell (depth-0 `)` not followed by `)`) ─────────
        diff_cmd("((echo hi) )");        // glued open, inner closes with a single `)`
        diff_cmd("(( 3*4 ) )");          // glued open, SPACED close
        diff_cmd("((a) && (b))");        // `)` after `a` at depth 0 not followed by `)`
        diff_cmd("((a); (b))");
        // ── Spaced `( (` → subshell (existing path; regression guards) ───────
        diff_cmd("( (echo hi) )");
        diff_cmd("( ( a ) )");
    }

    #[test]
    fn atoms_arith_command_composition() {
        diff_cmd("(( 1 )) && echo hi");   // in an && list
        diff_cmd("(( 1 )) || echo no");   // in an || list
        diff_cmd("(( 1 )); echo done");   // in a `;` list
        diff_cmd("(( 1+2 )) >out");       // trailing redirect → Redirected{ inner: Arith }
        diff_cmd("(( 1 )) | cat");        // pipeline stage
        diff_cmd("if (( x > 0 )); then y; fi");        // arith as an if-condition
        diff_cmd("while (( i < 3 )); do x; done");     // arith as a while-condition
        diff_cmd("for i in a; do (( n++ )); done");    // arith in a for body
    }

    // v256: C-style for (( … ))
    #[test]
    fn atoms_arith_for() {
        // Well-formed → Command::ArithFor (byte-identical to the oracle).
        diff_cmd("for ((i=0;i<3;i++)); do echo $i; done");
        diff_cmd("for ((;;)) do :; done");                 // all sections empty → None
        diff_cmd("for (( i = 0 ; i < n ; i++ )); do x; done"); // sections trimmed
        diff_cmd("for ((i=0,j=0; i<3; i++,j++)); do :; done");  // comma is literal
        diff_cmd("for ((i=$x; i<${n}; i++)); do :; done");  // embedded expansions
        diff_cmd("for ((i=(1+2); i<9; i++)); do :; done");  // inner grouping parens
        diff_cmd("for ((;;)); do break; done");
        // Section-count errors (both paths ArithForHeader with identical message).
        diff_err("for ((a;b;c;d)); do :; done");            // got 4
        diff_err("for ((a)); do :; done");                  // got 1
        diff_err("for ((a; b)); do :; done");               // got 2
    }

    #[test]
    fn atoms_arith_for_composition() {
        diff_cmd("for ((;;)); do break; done | cat");         // pipeline stage
        diff_cmd("for ((;;)); do :; done && echo hi");        // && list
        diff_cmd("for ((;;)); do :; done >out");              // trailing redirect (Redirected{inner:ArithFor})
        diff_cmd("for\n((;;)); do :; done");                  // newline before header
        diff_cmd("for (($x;;)); do :; done");                 // expansion in init only
        diff_cmd("if x; then for ((i=0;i<2;i++)); do y; done; fi"); // nested in a compound body
        diff_cmd("for ((i=0;i<2;i++)); do for ((j=0;j<2;j++)); do :; done; done"); // nested arith-for
    }

    #[test]
    fn atoms_arith_for_edges() {
        // Unterminated / malformed headers → UnterminatedLoop on both paths.
        diff_err("for ((i=0;i<3;i++)");            // single close, EOF
        diff_err("for ((i=0;i<3;i++); do x; done"); // single close before `;` → bail
        diff_err("for ((;;)) done");                // missing `do`
        diff_err("for ((;;)); do :");               // missing `done`
        // Suspected divergence (per the plan): a `;` inside a quote in the header.
        // The oracle's `split_top_level_semi` ignores quotes and splits inside
        // the quoted run. Observed atom-path behavior (this test) shows the
        // Mode::Arith for-header scanner (`scan_step_arith`, lexer.rs) has NO
        // dquote sub-mode at all in arith bodies — `"` is just accumulated as a
        // literal char, and the `;` classification only checks `for_header &&
        // depth == 0` (paren depth, not quote state). So the inner `;` in
        // `"a;b"` is NOT protected on the atom path either: it also splits into
        // 4 sections. Both paths therefore agree — this is NOT a live divergence,
        // and diff_err (not a manual is_ok/is_err pin) is the right assertion.
        diff_err("for (( \"a;b\"; ; )); do :; done");
    }

    #[test]
    fn cmd_deep_nesting() {
        diff_cmd("if x; then while y; do case $z in a) ( b );; esac; done; fi");
        diff_cmd("{ for i in a b; do if $i; then echo $i; fi; done; }");
        diff_cmd("while x; do { a; ( b ); }; done");
        diff_cmd("case $x in a) for i in 1 2; do echo $i; done;; b) { y; };; esac");
        diff_cmd("( if x; then y; else z; fi ) | { cat; }");
    }

    #[test]
    fn cmd_for_arith_unterminated_edge() {
        // T5 Minor: unterminated `for ((` (two consecutive LParen not forming an ArithBlock)
        // — the oracle guards it as UnterminatedLoop; parse_for may fall through to the
        // var-name read. Verify against the oracle. If tokenize itself errors (so neither
        // parser is reached), note that instead.
        for s in ["for (( ", "for ((", "for (()"] {
            // Only compare if the input LEXES (both sides use the same tokens). If
            // tokenize_with_opts errors, skip (document in your report) — a lex error
            // means the parser is never reached and there is no divergence to fix.
            if tokenize_with_opts(s, LexerOptions::default()).is_ok() {
                assert_eq!(new_seq(s), old_seq(s), "for-arith-unterminated mismatch for {s:?}");
            }
        }
    }

    // ── v244 T1: command-substitution differential harness ───────────────────
    //
    // THE PRODUCTION LEXER IS THE ORACLE.  When `new_cs` ≠ `old_cs`, fix
    // the new path to match — never weaken or skip the comparison.

    /// Recursively find the first `CommandSub` `WordPart` in a slice.
    /// Descends into `Quoted` wrappers (the production lexer wraps `$(…)` inside
    /// `"…"` in a `Quoted { style: Double, parts: [CommandSub{…}] }` node).
    fn find_command_sub(parts: &[WordPart]) -> Option<WordPart> {
        for p in parts {
            match p {
                WordPart::CommandSub { .. } => return Some(p.clone()),
                WordPart::Quoted { parts, .. } => {
                    if let Some(cs) = find_command_sub(parts) {
                        return Some(cs);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Build the expected `WordPart::CommandSub` using the PRODUCTION lexer (oracle).
    /// Wraps `s` in `"…"` when `quoted=true` to simulate a double-quoted context.
    fn old_cs(s: &str, quoted: bool) -> WordPart {
        let src = if quoted { format!("\"{s}\"") } else { s.to_string() };
        let toks = tokenize_with_opts(&src, LexerOptions::default()).expect("old lex");
        match &toks[0].kind {
            TokenKind::Word(w) => find_command_sub(&w.0)
                .expect("no comsub part in production token"),
            _ => panic!("production token is not a Word for {src:?}"),
        }
    }

    /// Build the expected `WordPart::CommandSub` using the NEW parser-driven path.
    fn new_cs(s: &str, quoted: bool) -> Result<WordPart, ParseError> {
        let mut lx = Lexer::new_live(s, &Default::default(), LexerOptions::default());
        parse_command_sub(&mut lx, quoted)
    }

    /// Assert that the new and old paths produce identical results for both
    /// unquoted and quoted contexts.
    fn diff_cs(s: &str) {
        assert_eq!(new_cs(s, false).unwrap(), old_cs(s, false), "unquoted {s:?}");
        assert_eq!(new_cs(s, true).unwrap(),  old_cs(s, true),  "quoted   {s:?}");
    }

    fn diff_cs_deferred(s: &str) {
        assert!(matches!(new_cs(s, false), Err(ParseError::UnsupportedExpansion)),
                "expected deferred for {s:?}, got {:?}", new_cs(s, false));
    }

    #[test]
    fn cs_simple() {
        diff_cs("$(echo hi)");
        diff_cs("$(echo hi there)");
        diff_cs("$(true)");
        diff_cs("$()");            // empty -> empty Sequence (NOT EmptySubshell)
    }

    #[test]
    fn cs_body_grammar() {
        diff_cs("$(a; b)");
        diff_cs("$(a; b; c)");
        diff_cs("$(a | b)");
        diff_cs("$(a | b | c)");
        diff_cs("$(a && b || c)");
        diff_cs("$(a; b;)");                       // trailing ;
        diff_cs("$(a &)");                          // background in body
        diff_cs("$(if x; then y; fi)");             // compound body (v243)
        diff_cs("$(for i in a b; do echo $i; done)");
        diff_cs("$(while x; do y; done)");
        diff_cs("$(case $z in a) b;; esac)");
        diff_cs("$( (echo x) )");                   // comsub of a subshell (SPACED)
        diff_cs("$({ echo x; })");                  // comsub of a brace group
        diff_cs("$(f() { x; })");                   // function-def body (v248 T2)
        diff_cs("$([[ -n x ]])");                   // `[[ ]]` body (v253 T1)
    }

    // v244 T3 tests

    #[test]
    fn cs_nesting_quoting() {
        diff_cs("$(echo $(date))");               // nested: inner fat-built, outer new-path
        diff_cs("$(echo ${x})");                  // ${…} in a body word (fat-built, passes through)
        diff_cs("$(a $(b) $(c))");                // two nested
        diff_cs("$(echo \"$(date)\")");           // nested inside dquotes in the body
        diff_cs("$(<file)");                       // body is a bare redirect
        diff_cs("$(cat < in > out)");
        diff_cs("$(echo hi\n)");                   // trailing newline in body
    }

    // ── v244 T4 tests ────────────────────────────────────────────────────────

    #[test]
    fn cs_in_param_operand() {
        diff_ok("${x:-$(echo d)}");
        diff_ok("${x:+$(cmd)}");
        diff_ok("${x=$(a b)}");
        diff_ok("${x:-a$(b)c}");                    // comsub between literals in an operand
        diff_ok("${x/$(a)/$(b)}");                  // pattern + replacement operands
        diff_ok("${x:-$(echo $(date))}");            // nested comsub inside an operand
    }

    // ── v244 T5 tests ────────────────────────────────────────────────────────

    #[test]
    fn cs_deferred_boundary() {
        diff_cs_deferred("$((1+2))");               // arith expansion (WordPart::Arith, not comsub)
        diff_cs_deferred("$(( a + b ))");
        diff_cs_deferred("`echo hi`");              // backtick (own iteration)
        // `$(f() { x; })` removed: function-def body now parses (v248 T2);
        // see `cs_body_grammar`'s `diff_cs("$(f() { x; })")`.
        // `$([[ -n x ]])` removed: `[[ ]]` body now parses (v253 T1); see
        // `cs_body_grammar`'s `diff_cs("$([[ -n x ]])")`.
        diff_cs_deferred("$(coproc x)");            // body defers (coproc)
    }

    #[test]
    fn cs_error_parity() {
        let new = new_cs("$(echo", false);
        assert!(new.is_err(), "unterminated comsub must Err, got {new:?}");
    }

    // ── v245 T1: backtick command-substitution differential harness ──────────
    //
    // THE PRODUCTION LEXER IS THE ORACLE.  When `new_bt` ≠ `old_bt`, fix the
    // new path to match — never weaken or skip the comparison.

    /// Build the expected `WordPart::CommandSub` (from a backtick substitution)
    /// using the PRODUCTION lexer (oracle).  Wraps `s` in `"…"` when
    /// `quoted=true` to simulate a double-quoted context.
    fn old_bt(s: &str, quoted: bool) -> WordPart {
        let src = if quoted { format!("\"{s}\"") } else { s.to_string() };
        let toks = tokenize_with_opts(&src, LexerOptions::default()).expect("old lex");
        match &toks[0].kind {
            TokenKind::Word(w) => find_command_sub(&w.0).expect("no comsub part in production token"),
            _ => panic!("production token is not a Word for {src:?}"),
        }
    }

    /// Build the expected `WordPart::CommandSub` using the NEW parser-driven
    /// backtick path (skeleton in Task 1; full body in Task 2+).
    fn new_bt(s: &str, quoted: bool) -> Result<WordPart, ParseError> {
        let mut lx = Lexer::new_live(s, &Default::default(), LexerOptions::default());
        parse_backtick_sub(&mut lx, quoted)
    }

    /// Assert that the new and old paths produce identical results for both
    /// unquoted and quoted contexts.
    fn diff_bt(s: &str) {
        assert_eq!(new_bt(s, false).unwrap(), old_bt(s, false), "unquoted {s:?}");
        assert_eq!(new_bt(s, true).unwrap(),  old_bt(s, true),  "quoted   {s:?}");
    }

    fn diff_bt_deferred(s: &str) {
        assert!(matches!(new_bt(s, false), Err(ParseError::UnsupportedExpansion)),
                "expected deferred for {s:?}, got {:?}", new_bt(s, false));
    }

    // ── v245 T1 scaffolding test ─────────────────────────────────────────────

    #[test]
    fn bt_scaffolding_exists() {
        // Verify that the new Mode variant and atom kinds compile.
        let _ = Mode::Backtick { depth: 0 };
        let _ = TokenKind::BeginBacktick;
        let _ = TokenKind::EndBacktick;
        // The production oracle must be callable for a simple backtick substitution.
        let _ = old_bt("`echo hi`", false);
    }

    // ── v245 T2: depth-0 backtick core ──────────────────────────────────────

    #[test]
    fn bt_depth0() {
        diff_bt("`echo hi`");
        diff_bt("`echo hi there`");
        diff_bt("`a | b`");
        diff_bt("`a && b || c`");
        diff_bt("`a; b`");
        diff_bt("`if x; then y; fi`");
        diff_bt("``");                 // empty -> empty Sequence
    }

    // ── v245 T3: body content — \$/\\ unescape, $()/${} in body, quoted ─────

    #[test]
    fn bt_body_content() {
        diff_bt("`echo \\$x`");        // \$ -> variable $x
        diff_bt("`echo \\\\`");        // \\ -> literal backslash
        diff_bt("`echo \\n`");         // \n -> preserved (backslash + n)
        diff_bt("`echo $(date)`");     // $() in body -> fat-built, passes through
        diff_bt("`echo ${x}`");        // ${} in body -> fat-built
        diff_bt("`echo $HOME`");       // bare $ expands
        diff_bt("`echo \"quoted\"`");  // dquotes in body
        diff_bt("`echo \\\\x`");       // \\x -> Quoted{Backslash,[Literal("x")]}
        diff_bt("`echo \\\\ x`");      // \\ <space> -> quoted space (no word-split)
        diff_bt("`echo \\\\$HOME`");   // \\$ -> Quoted{Backslash,[Literal("$")]}, no expand
    }

    // ── v245 T4: depth-1 nesting — `\`` opens/closes a child backtick ─────────

    #[test]
    fn bt_depth1_nesting() {
        diff_bt("`echo \\`date\\``");            // `echo `date`` (nested once)
        diff_bt("`a \\`b\\` c`");                // outer body: a `b` c
        diff_bt("`\\`inner\\``");                // nested at the start
        diff_bt("`echo \\`echo hi\\``");
        diff_bt("`x \\`y | z\\` w`");            // pipeline in the nested body
    }

    // ── v245 T5: depth-2 nesting — `\\\`` opens/closes a level-2 child ────────
    //
    // Proves the unified depth-aware `\`-run decode GENERALIZES to arbitrary
    // depth: at D=2 the child-open delimiter is `\\\`` (3 backslashes, B=2^2−1=3)
    // and the close is `\`` (1 backslash, B=2^1−1=1); at D=3 the open is again
    // `\\\`` (B=2^3−1... no — the formula is B=f(run,depth), pinned to the oracle
    // below).  (Rust `\\\\\\`` == the shell's `\\\`` — three backslashes + `.)
    #[test]
    fn bt_depth2_nesting() {
        diff_bt("`a \\`b \\\\\\`c\\\\\\` d\\` e`");   // depth-2: \\\` around c
        diff_bt("`\\`\\\\\\`x\\\\\\`\\``");             // depth-2 at the start
        diff_bt("`echo \\`echo \\\\\\`echo hi\\\\\\`\\``");
    }

    // ── v245 T5 (addendum): pin the bare-backtick-at-D≥2 malformed-input divergence ──
    //
    // KNOWN DIVERGENCE [deferred, v245]: the single-pass scan_step_backtick
    // leniently accepts some malformed inputs that the recursive production oracle
    // REJECTS at the lex stage.  Well-formed nesting is byte-identical (see
    // bt_depth2_nesting).  Pinned here so the future Stage-2 live-wiring
    // reconciles it (make the new path reject too) rather than silently shipping a
    // parser that accepts what bash rejects.
    #[test]
    fn bt_malformed_divergence_deferred() {
        // KNOWN DIVERGENCE [deferred, v245]: at backtick depth >= 2, a bare ` is
        // not a valid delimiter (well-formed nesting always escapes deeper
        // delimiters).  scan_step_backtick leniently consumes it as literal body
        // content, so the NEW path accepts these MALFORMED inputs while the
        // recursive production oracle rejects them at the lex stage with
        // LexError::Substitution(UnterminatedSubstitution).  See the comment at
        // the bare-`-at-D≥2 branch in scan_step_backtick (lexer.rs) and
        // bt_depth2_nesting for the byte-identical well-formed proof.
        for s in [
            "`\\`x` y\\` z`",   // shell: `\`x` y\` z`  — bare ` inside D=2 body
            "`\\`a`b\\``",      // shell: `\`a`b\``      — bare ` inside D=2 body
        ] {
            // Production oracle rejects at the lex stage:
            assert!(
                tokenize_with_opts(s, LexerOptions::default()).is_err(),
                "expected production lex to reject malformed {s:?}",
            );
            // New (parser-driven) path currently accepts (DIVERGENCE):
            assert!(
                new_bt(s, false).is_ok(),
                "new path currently accepts malformed {s:?} — update this test if reconciled",
            );
        }
    }

    #[test]
    fn bt_backslash_run_divergence_deferred() {
        // KNOWN DIVERGENCE [deferred, v245 — reconcile at Stage-2 live-wiring]:
        // the body `\`-run decode in scan_step_backtick consumes backslashes two
        // at a time incrementally, but the production oracle collapses the WHOLE
        // contiguous run first (backtick unescape: `\\`→`\`, `\$`→`$`, `` \` ``→`` ` ``)
        // and THEN re-lexes the survivors as a command.  The two passes agree for
        // runs of 1–3 backslashes (the corpus in bt_body_content), but diverge for
        // runs >= 4 and for an ODD run immediately before `$`/`` ` ``.  Worst case:
        // `` `echo \\\$x` `` — the new path decodes to Var{x} (EXPANDS $x) while the
        // oracle keeps `$x` literal.  These are WELL-FORMED inputs, unlike the
        // malformed class in bt_malformed_divergence_deferred.  All are dormant
        // (parser-driven path is not live), so there is no production impact today.
        // Deferred to a dedicated follow-on iteration with a full parity matrix.
        for s in [
            "`echo \\\\\\\\x`",     // shell: `echo \\\\x`   (4 backslashes + x)
            "`echo \\\\\\\\ x`",    // shell: `echo \\\\ x`  (4 backslashes + space)
            "`echo \\\\\\\\\\\\x`", // shell: `echo \\\\\\x` (6 backslashes + x)
            "`echo \\\\\\$x`",      // shell: `echo \\\$x`   (3 backslashes + $x — spurious expand)
        ] {
            // Both paths succeed, but they DISAGREE (the divergence).  If a future
            // fix makes them agree, this assertion fires — delete the pin then.
            let new = new_bt(s, false).expect("new path should parse");
            let old = old_bt(s, false);
            assert_ne!(
                new, old,
                "new path now MATCHES oracle for {s:?} — divergence reconciled, remove this pin",
            );
        }
    }

    // ── v245 T6 tests ────────────────────────────────────────────────────────

    #[test]
    fn bt_in_param_operand() {
        diff_ok("${x:-`echo d`}");
        diff_ok("${x:+`cmd`}");
        diff_ok("${x:-a`b`c}");
    }

    #[test]
    fn bt_error_parity() {
        let new = new_bt("`echo", false);
        assert!(new.is_err(), "unterminated backtick must Err, got {new:?}");
    }

    // ── v246 T1: arithmetic-expansion differential harness ───────────────────
    //
    // THE PRODUCTION LEXER IS THE ORACLE.  When `new_arith` ≠ `old_arith`, fix
    // the new path to match — never weaken or skip the comparison.

    fn find_arith(parts: &[WordPart]) -> Option<WordPart> {
        for p in parts {
            match p {
                WordPart::Arith { .. } => return Some(p.clone()),
                WordPart::Quoted { parts, .. } => {
                    if let Some(f) = find_arith(parts) { return Some(f); }
                }
                _ => {}
            }
        }
        None
    }

    /// Production oracle: the `WordPart::Arith` the batch lexer builds for `s`.
    fn old_arith(s: &str, quoted: bool) -> WordPart {
        let src = if quoted { format!("\"{s}\"") } else { s.to_string() };
        let toks = tokenize_with_opts(&src, LexerOptions::default()).expect("old lex");
        match &toks[0].kind {
            TokenKind::Word(w) => find_arith(&w.0).expect("no arith part in production token"),
            _ => panic!("production token is not a Word for {src:?}"),
        }
    }

    /// New parser-driven path.
    fn new_arith(s: &str, quoted: bool) -> Result<WordPart, ParseError> {
        let mut lx = Lexer::new_live(s, &Default::default(), LexerOptions::default());
        parse_arith_expansion(&mut lx, quoted)
    }

    /// Assert new == old for both unquoted and quoted contexts.
    fn diff_arith(s: &str) {
        assert_eq!(new_arith(s, false).unwrap(), old_arith(s, false), "unquoted {s:?}");
        assert_eq!(new_arith(s, true).unwrap(),  old_arith(s, true),  "quoted   {s:?}");
    }

    // ── v246 T1 scaffolding test ──────────────────────────────────────────────

    #[test]
    fn arith_scaffolding_exists() {
        let _ = TokenKind::ArithOpen;
        let _ = TokenKind::ArithClose;
        let _ = TokenKind::ArithBail;
        // Empty arith `$(( ))` round-trips through the skeleton (body filled in Task 2+).
        // Production `$(( ))` yields Arith { body: Word([...]) }; the skeleton only
        // guarantees the harness wires up, so just assert new_arith succeeds here.
        assert!(new_arith("$(())", false).is_ok(), "skeleton must parse $(())");
    }

    // ── v246 T2 tests ────────────────────────────────────────────────────────

    #[test]
    fn arith_depth0_plain() {
        diff_arith("$((1+2))");
        diff_arith("$(( 1 + 2 ))");
        diff_arith("$((0))");
        diff_arith("$((a+1))");   // bare identifier is literal body text
        diff_arith("$(( x * y ))");
    }

    #[test]
    fn arith_unterminated_errs() {
        assert!(new_arith("$((1+2", false).is_err(), "unterminated must Err");
        assert!(new_arith("$(( ", false).is_err(), "unterminated must Err");
    }

    // ── v246 T3 tests ────────────────────────────────────────────────────────

    #[test]
    fn arith_grouping_parens() {
        diff_arith("$(( (1+2)*3 ))");
        diff_arith("$(( ((1+2)) ))");
        diff_arith("$(( a*(b+c) ))");
        // Paren-BALANCED body that merely looks command-shaped: `(echo hi)` closes
        // at depth 0 as `))`, so production keeps it as Arith (not the wrinkle).
        diff_arith("$(( (echo hi) ))");
    }

    #[test]
    fn arith_embedded_expansions() {
        diff_arith("$(( $x + 1 ))");
        diff_arith("$(( ${y} ))");
        diff_arith("$(( $(echo 1) ))");
        diff_arith("$(( `echo 1` ))");
        diff_arith("$(( $x + ${y} + 2 ))");
    }

    // ── v246 T3 fix tests (special/positional params) ──────────────────────────

    #[test]
    fn arith_special_params() {
        diff_arith("$(( $? ))");
        diff_arith("$(( $1 ))");
        diff_arith("$(( $1 + $2 ))");
        diff_arith("$(( $# ))");
        diff_arith("$(( $@ ))");
        diff_arith("$(( $* ))");
    }

    // ── v246 T5 tests (the `$( (…) )` wrinkle) ────────────────────────────────

    #[test]
    fn arith_wrinkle_falls_back_to_cmdsub() {
        // `$((cat) )` / `$((echo hi) )` are really `$( (cat) )` / `$( (echo hi) )` —
        // a command-sub whose body starts with a subshell.  A depth-0 `)` not
        // followed by `)` makes the arith scan Bail; the parser rewinds to the
        // `$((` start and re-drives as a command substitution.  Both paths agree.
        for s in ["$((cat) )", "$((echo hi) )"] {
            assert_eq!(new_arith(s, false).unwrap(), old_cs(s, false), "wrinkle {s:?}");
            assert_eq!(new_arith(s, true).unwrap(),  old_cs(s, true),  "wrinkle quoted {s:?}");
        }
    }

    #[test]
    fn arith_wrinkle_cmdsub_body_error_matches() {
        // `$((a)b)` is really `$( (a)b )`, whose subshell body `(a)b` is itself a
        // syntax error (a bare word immediately after `)`).  Production errors on
        // it; the new path must ALSO error — reaching that error via the ArithBail
        // → cmdsub retry, not by spuriously succeeding as arith.
        assert!(new_arith("$((a)b)", false).is_err(), "new path must error on $((a)b)");
        assert!(
            tokenize_with_opts("$((a)b)", LexerOptions::default()).is_err(),
            "production errors on $((a)b) too"
        );
    }

    // ── v246 T4 tests ────────────────────────────────────────────────────────

    #[test]
    fn arith_nested() {
        diff_arith("$(( 3 * $((5*10)) ))");
        diff_arith("$(( $((1+1)) + $((2+2)) ))");
        diff_arith("$(( $(( $((1)) )) ))");
    }

    // ── v246 T6 tests: operand wiring + error parity ────────────────────────

    #[test]
    fn arith_in_param_operand() {
        diff_ok("${x:-$((1+1))}");
        diff_ok("${x:+$((n))}");
        diff_ok("${x:-a$((i))b}");
    }

    #[test]
    fn arith_error_parity() {
        assert!(new_arith("$((1+1", false).is_err(), "unterminated arith must Err");
    }

    // ── v246 follow-up: nested + operand wrinkle-bail tests ────────────────────
    //
    // T5 proved the top-level wrinkle (`$((cat) )` bailing to a cmdsub-of-
    // subshell) matches the oracle; these tests prove the bail ALSO matches when
    // it happens (a) embedded inside an OUTER arith body that itself closes
    // legitimately, and (b) embedded inside a `${…}` operand.  All four/three
    // inputs below were verified against the oracle (`old_arith`/`old_part` via
    // `diff_arith`/`diff_ok`) before writing this test — no divergence found, so
    // no `*_divergence_deferred` pin is needed here.

    #[test]
    fn arith_wrinkle_nested_in_outer_arith() {
        // The inner `$((cat) )` bails to a `$( (cat) )` cmdsub-of-subshell; the
        // outer `$((...))` still closes legitimately as arith, so the WHOLE
        // expression is genuinely arith at the top level (diff_arith applies).
        diff_arith("$(( $((cat) ) ))");             // bail alone in the outer body
        diff_arith("$(( 1 + $((echo hi) ) ))");      // bail alongside other arith text
        diff_arith("$(( $((cat) ) + 1 ))");           // bail followed by more arith text
        diff_arith("$(( $(( $((cat) ) )) ))");        // bail nested two arith levels deep
    }

    #[test]
    fn arith_wrinkle_bail_in_operand() {
        // The bail happening inside a `${…}` operand (rather than at the
        // top level or nested in an outer arith) — routes through
        // parse_param_expansion, so diff_ok (not diff_arith) is the right harness.
        diff_ok("${x:-$((cat) )}");
        diff_ok("${x:+$((cat) )}");
        diff_ok("${x:-a$((cat) )b}");                 // bail between literals in an operand
    }

    #[test]
    fn arith_wrinkle_nested_error_parity() {
        // `$(( $((a)b) ))`: the inner `$((a)b)` bails to a cmdsub-of-subshell
        // whose body `(a)b` is itself a syntax error (bare word after `)`,
        // same shape as arith_wrinkle_cmdsub_body_error_matches but nested one
        // arith level deeper).  Both paths must error.
        assert!(new_arith("$(( $((a)b) ))", false).is_err(), "new path must error");
        assert!(
            tokenize_with_opts("$(( $((a)b) ))", LexerOptions::default()).is_err(),
            "production errors on $(( $((a)b) )) too"
        );
    }

    // v253 T1 tests: `[[ … ]]` grammar core

    #[test]
    fn atoms_double_bracket_core() {
        diff_cmd("[[ -f /etc/passwd ]]");     // unary file test
        diff_cmd("[[ -z $x ]]");              // unary string test w/ expansion
        diff_cmd("[[ hello ]]");              // lone word ≡ -n hello
        diff_cmd("[[ $x ]]");                 // lone word w/ expansion
        diff_cmd("[[ a == b ]]");             // string eq
        diff_cmd("[[ a = b ]]");              // string eq (single =)
        diff_cmd("[[ a != b ]]");             // string ne
        diff_cmd("[[ $x == a* ]]");           // glob RHS stays a pattern word
        diff_cmd("[[ 3 -eq 3 ]]");            // int eq
        diff_cmd("[[ 3 -lt 5 ]]");            // int lt
        diff_cmd("[[ a < b ]]");              // string lt via Op(RedirIn)
        diff_cmd("[[ a > b ]]");              // string gt via Op(RedirOut)
        diff_cmd("[[ f1 -nt f2 ]]");          // file newer-than
        diff_cmd("[[ -f a && -f b ]]");       // logical and
        diff_cmd("[[ -f a || -f b ]]");       // logical or
        diff_cmd("[[ ! -d c ]]");             // negation
        diff_cmd("[[ ( a == b ) ]]");         // grouping
        diff_cmd("[[ -f a && -f b || ! -d c ]]"); // precedence
    }

    #[test]
    fn atoms_double_bracket_extra() {
        diff_err("[[ ]]");                              // EmptyDoubleBracket
        diff_err("[[");                                 // UnterminatedDoubleBracket
        diff_err("[[ -f");                               // unary op, no operand → UnterminatedDoubleBracket
        diff_err("[[ a == ]]");                          // binary op, no rhs → TestExprMissingOperand
        // `~~` is NOT in the operator set, so BOTH paths take the lone-word
        // branch on `a` (≡ `-n a`), leave `~~` unconsumed, and trip the
        // `]]`-consume → `UnterminatedDoubleBracket` (not `TestExprBadOperator`).
        // The test passes because both agree. `TestExprBadOperator` is in fact
        // defensively UNREACHABLE on both the atom AND oracle paths — the
        // `next_is_binary` recognition set and the operator match-arm set are
        // identical, so any word that reaches the match is already known to be
        // in the set. There is therefore no genuine `TestExprBadOperator`-parity
        // case to test; that's expected and matches the oracle.
        diff_err("[[ a ~~ b ]]");                        // unrecognized op → both lone-word → UnterminatedDoubleBracket
        diff_err("[[ && a ]]");                          // leading operator → TestExprMissingOperand
        diff_cmd("if [[ -f a ]]; then echo y; fi");      // `[[ ]]` as an if-condition
        diff_cmd("while [[ -n $x ]]; do echo y; done");  // `[[ ]]` as a while-condition
        diff_cmd("[[\n  -f a\n  && -f b\n]]");            // Blank/Newline interleaving (skip_test_ws)
        diff_cmd("[[ ! ( -f a && -f b ) ]]");             // negated grouping
    }

    /// Operator GLUED to an expansion with no intervening space (`==$x`,
    /// `-eq$n`, `=~$x`): the atom stream splits this into `Lit("==")` +
    /// `DollarName{...}` (no `Blank`), while the oracle assembles the whole
    /// thing as ONE `Word([Literal("=="), Var("x")])`. The oracle's
    /// `next_is_test_binary_operator` peeks that assembled multi-part word,
    /// which is NOT in its operator set → lone-word `-n a`, leaving `==$x`
    /// unconsumed → the `]]`-consume trips → `UnterminatedDoubleBracket`. The
    /// atom path's `next_is_test_binary_operator_atom` matches this ONLY by
    /// requiring the operator `Lit` to END at a word boundary (peek2 is
    /// `Blank`/`Newline`/`Op`/EOF), so a glued continuation classifies as
    /// NOT-an-operator too. Regression for the T1-review bug.
    #[test]
    fn atoms_double_bracket_glued_operator() {
        diff_err("[[ a ==$x ]]");   // string-eq glued → UnterminatedDoubleBracket on both
        diff_err("[[ a -eq$n ]]");  // int-eq glued → same
        diff_err("[[ a =~$x ]]");   // regex glued → same (NOT the =~ deferral: not recognized as an op)
        diff_cmd("[[ a == $x ]]");  // SPACED == is a NORMAL StringEq binary (rhs $x) — must NOT regress
        diff_cmd("[[ a =~ $x ]]"); // SPACED =~ IS recognized → v254 ports the regex RHS (TestExpr::Regex)
    }

    /// v253 T3: an inline-assignment PREFIX immediately followed by `[[`
    /// (`FOO=1 [[ … ]]`) routes to `Command::DoubleBracket` with the peeled
    /// assignments attached as `inline_assignments`, byte-identical to the
    /// oracle (command.rs's `parse_command_inner`, the assignment-peeling loop
    /// that dispatches to `parse_double_bracket_with_assigns` when `[[` follows).
    /// This closes the T1-state `atoms_double_bracket_assign_prefix_divergence`
    /// pin. The atom-path interception lives in
    /// `parse_simple_with_leading_word`'s word-assembly loop: BEFORE assembling
    /// the next word, if every word collected so far is an assignment word AND
    /// `[[` follows, the collected words are peeled into `Vec<Assignment>` and
    /// dispatched to `parse_double_bracket(iter, assigns)` — forward-only
    /// (peek the keyword, then dispatch), no `mark`/`rewind`.
    #[test]
    fn atoms_double_bracket_inline_assignments() {
        diff_cmd("FOO=hi [[ -n $FOO ]]");
        diff_cmd("A=1 B=2 [[ $A == 1 ]]");        // multiple leading assignments
        diff_cmd("x=y [[ $x == y && -n $x ]]");
        diff_cmd("x=1 [[ -f a ]]");                // the former T1 pin case
    }

    /// v253 T3 (OBSERVATION): `[[ ]]` used as a pipeline / logical / negated /
    /// sequence stage or as an `if` condition. The T1 `[[` dispatch fires
    /// wherever a command is parsed, so the compound-stage wiring already
    /// covers these (as it does for `if`/`while`).
    #[test]
    fn atoms_double_bracket_as_stage() {
        diff_cmd("[[ -f a ]] && echo yes");        // as && stage
        diff_cmd("[[ -f a ]] || echo no");         // as || stage
        diff_cmd("! [[ -f a ]]");                  // negated command
        diff_cmd("[[ -f a ]]; echo done");         // in a sequence
        diff_cmd("if [[ -n $x ]]; then echo y; fi"); // as an if condition
    }

    /// v253 T3-fix: a trailing redirect on a command-position `[[ … ]]` wraps
    /// in `Redirected` (the dispatch site now calls `maybe_wrap_redirects`, like
    /// every other atom-path compound + the oracle command.rs:1050-1053).
    /// The inline-assignment site stays UNWRAPPED — `FOO=hi [[ … ]] >out` leaves
    /// `>out` pending on BOTH the atom path and the oracle
    /// (command.rs:1111 returns `parse_double_bracket_with_assigns` unwrapped),
    /// so both error identically (`diff_err` proves the site is left unwrapped —
    /// wrapping it would make the atom path return `Ok(Redirected)` and diverge).
    #[test]
    fn atoms_double_bracket_trailing_redirect() {
        diff_cmd("[[ -f a ]] >out");               // fixed case → Redirected
        diff_cmd("[[ -f a ]] 2>&1");               // fd-dup redirect
        diff_cmd("[[ -n $x ]] >f 2>&1");           // multi-redirect
        diff_err("FOO=hi [[ -f a ]] >out");        // inline-assign site UNWRAPPED → both Err(UnexpectedToken)
    }

    /// `=~` is PORTED (v254): the atom path assembles the regex-pattern RHS via
    /// `Mode::Regex`/`parse_regex_operand` and produces `TestExpr::Regex`
    /// byte-identically to the oracle (previously deferred with
    /// `UnsupportedCommand`).
    #[test]
    fn atoms_double_bracket_regex_ported() {
        diff_cmd("[[ a =~ b ]]");
        diff_cmd("[[ a =~ b* ]]");
        diff_cmd("[[ $x =~ ^[0-9]+$ ]]");
        diff_cmd("[[ -f a && $x =~ y ]]"); // regex inside a logical expr
        diff_cmd("[[ x =~ ^a.*b$ ]]");
        diff_cmd("[[ $s =~ [0-9]+ ]]");
    }

    #[test]
    fn atoms_regex_core() {
        diff_cmd("[[ $x =~ abc ]]");            // plain literal
        diff_cmd("[[ $x =~ ^a.c$ ]]");          // anchors + metachar `.`
        diff_cmd("[[ $x =~ [0-9]+ ]]");         // bracket class + quantifier
        diff_cmd("[[ $x =~ a|b ]]");            // `|` literal
        diff_cmd("[[ $x =~ a<b>c ]]");          // `<` `>` literal
        diff_cmd("[[ $x =~ a;b ]]");            // `;` literal
        diff_cmd("[[ $x =~ a&b ]]");            // `&` literal
        diff_cmd("[[ $x =~ (a b) ]]");          // paren-depth: space kept inside ( )
        diff_cmd("[[ $x =~ ((a) (b))+ ]]");     // nested groups
        diff_cmd("[[ $x =~ (a b)c ]]");         // group then trailing literal
        diff_cmd("[[ $x =~ $p ]]");             // Var
        diff_cmd("[[ $x =~ ${p}x ]]");          // ${…} then literal
        diff_cmd("[[ $x =~ ${a[0]} ]]");        // subscript expansion
        diff_cmd("[[ $x =~ $(cmd) ]]");         // command-sub
        diff_cmd("[[ $x =~ $((1+1)) ]]");       // arith
        diff_cmd("[[ $x =~ a$b|c$(d) ]]");      // mixed literal + expansions
    }

    /// v254 T1 hardening: the `\x`/quote/expansion traps of `scan_regex_operand`.
    /// The oracle keeps `\x` as an UNQUOTED literal `\x` (backslash kept), inlines
    /// single-quoted + double-quoted bodies FLAT, and wraps only `$'…'` ANSI-C.
    #[test]
    fn atoms_regex_traps() {
        diff_cmd("[[ $x =~ a\\.b ]]");    // THE #1 TRAP: \x kept as unquoted literal `a\.b`
        diff_cmd("[[ $x =~ a\\ b ]]");    // escaped space kept literal
        diff_cmd("[[ $x =~ \\) ]]");      // escaped paren
        diff_cmd("[[ $x =~ a\\\\b ]]");   // escaped backslash
        diff_cmd("[[ $x =~ 'a b' ]]");    // single-quoted run
        diff_cmd("[[ $x =~ \"a b\" ]]");  // double-quoted span
        diff_cmd("[[ $x =~ $'x\\ny' ]]"); // ansi-c
        diff_cmd("[[ $x =~ ${p:-d} ]]");  // param op
        diff_cmd("[[ $x =~ `echo a` ]]"); // backtick
        diff_cmd("[[ $x =~ pre$(c)post ]]"); // glued cmdsub
        diff_cmd("[[ $x =~ (a\\ b)c ]]"); // escaped space inside group + trailing
        diff_cmd("[[ $x =~ $@ ]]");       // $@
        diff_cmd("[[ $x =~ $* ]]");       // $*
        diff_cmd("[[ $x =~ $? ]]");       // $?
        diff_cmd("[[ $x =~ a$ ]]");       // trailing lone $
        diff_cmd("[[ $x =~ .* ]]");       // metachars
    }

    /// v254 T1 review fix: the oracle treats empty `''` and `""` DIFFERENTLY in a
    /// regex operand. `''` pushes a real `Literal{"",true}` (content → the space
    /// terminates → `Ok`), but `""` pushes NOTHING (operand stays "unstarted" → the
    /// space is skipped as still-leading → `]]` swallowed → `Err`). The atom path
    /// makes `body_started` parser-managed + drops the injected empty-`""` marker.
    #[test]
    fn atoms_regex_empty_quotes() {
        diff_err("[[ $x =~ \"\" ]]");   // empty dquote → both Err(TestExprMissingOperand) (space skipped, pattern becomes `]]`, rejected as operand)
        diff_cmd("[[ $x =~ '' ]]");     // empty squote → both Ok, pattern [Literal "" q:true], space terminates
        diff_cmd("[[ $x =~ a\"\"b ]]"); // → both [Lit "a", Lit "b"] (empty dquote adds nothing)
        diff_cmd("[[ $x =~ a''b ]]");   // → both [Lit "a", Lit "" q:true, Lit "b"] (empty squote kept)
        diff_cmd("[[ $x =~ \"x\" ]]");  // non-empty dquote unaffected: pattern [Lit "x" q:true]
        diff_cmd("[[ $x =~ abc ]]");    // no regression to the started/terminator logic
        diff_cmd("[[ $x =~ (a b) ]]");  // paren-depth space still kept, terminator still fires
    }

    /// v254 T1 review MINOR — glued `=~<b`/`=~>b` live-flip carry-forward. With no
    /// space, `<`/`>` is lexed as `Op(RedirIn/RedirOut)` in command mode and
    /// buffered by `next_is_test_binary_operator_atom`'s peek2, so
    /// `parse_regex_operand`'s first atom is that `Op` → `TestExprBadOperator`,
    /// whereas the oracle scans `<b` INTO the regex operand. This is a known
    /// divergence to reconcile BEFORE flipping `command_atoms` live; pin the
    /// CURRENT behavior of both paths (they disagree: atom Err, oracle Ok).
    #[test]
    fn atoms_regex_glued_redir_carryforward() {
        // Both paths ERROR (different kinds: atom `TestExprBadOperator` because the
        // buffered `Op(RedirIn/RedirOut)` is the first regex atom; oracle
        // `TestExprMissingOperand`). Pin the AGREEMENT that both reject; the exact
        // error kind is a v254 live-flip carry-forward to reconcile before the
        // `command_atoms` flip.
        assert_eq!(new_seq("[[ a =~<b ]]").is_err(), old_seq("[[ a =~<b ]]").is_err());
        assert_eq!(new_seq("[[ a =~>b ]]").is_err(), old_seq("[[ a =~>b ]]").is_err());
        // SPACED forms (the supported v254 shape) fully agree on the AST:
        diff_cmd("[[ a =~ <b ]]");  // spaced: `<b` is the operand on both
        diff_cmd("[[ a =~ >b ]]");
    }

    /// v254 live-flip carry-forward (PRE-EXISTING, inherited): `$"…"` locale
    /// quoting. The oracle's `scan_dollar_expansion` drops the `$` for `$"`
    /// (locale-translation = identity), yielding pattern `[Literal "abc"
    /// quoted:true]`, but the shared `emit_unquoted_dollar_atom` classifier has
    /// no `$"` arm, so the atom path emits `DollarLit` + `BeginDquote` →
    /// pattern `[Literal "$", Literal "abc" quoted:true]`. This gap is NOT
    /// introduced by v254 — it affects command position too (`echo $"hi"`
    /// diverges the same way) — so it is pinned here, not fixed: reconcile in
    /// the shared `$`-classifier before flipping `command_atoms` live.
    #[test]
    fn atoms_regex_dollar_dquote_carryforward() {
        let s = "[[ $x =~ $\"abc\" ]]";
        let n = new_seq(s);
        let o = old_seq(s);
        assert!(n.is_ok(), "expected atom path Ok for {s:?}, got {n:?}");
        assert!(o.is_ok(), "expected oracle Ok for {s:?}, got {o:?}");
        assert_ne!(n.unwrap(), o.unwrap(), "expected a KNOWN AST divergence for {s:?}");
    }

    /// v254 T2: systematic quoting/escapes/continuations/terminator-edges
    /// corpus for the `=~` regex operand.
    #[test]
    fn atoms_regex_quoting_escapes_terminators() {
        // Quoting
        diff_cmd("[[ $x =~ \"a b\" ]]");         // dquote keeps the space, quoted part
        diff_cmd("[[ $x =~ 'a.b' ]]");           // squote literal
        diff_cmd("[[ $x =~ x\"$y\"z ]]");        // literal + dquoted expansion + literal
        diff_cmd("[[ $x =~ \"$p\"* ]]");         // dquoted var then literal `*`
        // Escapes — backslash KEPT in the plain literal (NOT a Backslash QuoteRun)
        diff_cmd("[[ $x =~ \\. ]]");             // `\.` → literal `\.`
        diff_cmd("[[ $x =~ a\\.b ]]");           // `a\.b` one literal
        diff_cmd("[[ $x =~ a\\ b ]]");           // escaped space does NOT terminate
        diff_cmd("[[ $x =~ \\$lit ]]");          // escaped `$` → literal `$lit`
        // Line continuations
        diff_cmd("[[ $x =~ a\\\nb ]]");          // mid-pattern `\<NL>` → `ab`
        diff_cmd("[[ $x =~ \\\n  foo ]]");       // leading `\<NL>` + indent skipped → `foo`
        // Terminator edges
        diff_err("[[ $x =~ ]]");                 // pattern `]]`, then no `]]` → Unterminated
        diff_err("[[ $x =~ foo");                // EOF → Unterminated
        diff_cmd("[[ $x =~ a]] ]]");             // pattern `a]]`, then space, then `]]` closes
    }

    /// v254 T3: regex as a PRIMARY in the `[[ ]]` cascade — composed with
    /// `&&`/`||`, grouping `( … )`, negation `!`, a following normal binary,
    /// and under a leading inline assignment. Exercises `parse_test_and`/
    /// `parse_test_or`/`parse_test_not`/`parse_test_primary` around
    /// `TestExpr::Regex` byte-identically to the oracle.
    #[test]
    fn atoms_regex_composition() {
        diff_cmd("[[ -f a && $x =~ b|c ]]");     // regex after &&
        diff_cmd("[[ $x =~ a || $y =~ b ]]");    // regex on both sides of ||
        diff_cmd("[[ ( $x =~ b ) ]]");           // grouped regex
        diff_cmd("[[ ! $x =~ b ]]");             // negated regex
        diff_cmd("[[ $x =~ a && $y == b ]]");    // regex then a normal binary
        diff_cmd("FOO=hi [[ $FOO =~ h.* ]]");    // regex under an inline assignment
    }

    /// v254 T3: adversarial corpus — POSIX classes, alternation groups, escaped
    /// metachars, param-default/backtick/cmdsub expansions, mixed quoting, and
    /// the two UNBALANCED-PAREN cases that keep `paren_depth > 0` so the
    /// trailing ` ]]` is swallowed as literal whitespace/text inside the still-
    /// open group, running the operand to EOF → `Err(UnterminatedDoubleBracket)`
    /// on both paths.
    #[test]
    fn atoms_regex_corpus() {
        diff_cmd("[[ $x =~ a*b? ]]");                 // glob-like quantifiers (literal in ERE)
        diff_cmd("[[ $x =~ [[:alpha:]]+ ]]");         // POSIX class (nested [] and :)
        diff_cmd("[[ $x =~ (foo|bar)+baz ]]");        // alternation inside a group
        diff_cmd("[[ $x =~ a\\|b ]]");                // escaped pipe → literal `\|`
        diff_cmd("[[ $x =~ ${a:-def} ]]");            // param default inside pattern
        diff_cmd("[[ $x =~ `echo re` ]]");            // backtick command-sub
        diff_cmd("[[ $x =~ \"a b\"c'd e' ]]");        // dquote + literal + squote (spaces via quotes)
        diff_cmd("[[ $x =~ pre$(cmd)post ]]");        // cmdsub glued between literals
        diff_err("[[ $x =~ a(b ]]");                  // unbalanced `(` → depth stays >0, ` ]]` swallowed literal → EOF → Unterminated
        diff_err("[[ $x =~ (a b ]]");                 // unbalanced open group swallows ` ]]` (depth>0 ws literal) → Unterminated
    }

    // v253 T2: adversarial precedence/grouping/newlines corpus (hardens the
    // T1 cascade — see `parse_test_or`/`parse_test_and`/`parse_test_not`).
    #[test]
    fn atoms_double_bracket_precedence() {
        diff_cmd("[[ a && b && c ]]");            // left-assoc &&
        diff_cmd("[[ a || b || c ]]");             // left-assoc ||
        diff_cmd("[[ a && b || c && d ]]");        // && binds tighter than ||
        diff_cmd("[[ ! a && b ]]");                // ! binds tighter than &&
        diff_cmd("[[ ! ! a ]]");                   // right-assoc double negation
        diff_cmd("[[ ( a || b ) && c ]]");         // grouping overrides precedence
        diff_cmd("[[ ( ( a ) ) ]]");                // nested grouping
        diff_cmd("[[ -n a && ( -z b || -f c ) ]]");
        diff_cmd("[[ a\n&&\nb ]]");                // newlines around &&
        diff_cmd("[[\n  a == b\n]]");               // newlines after [[ and before ]]
        diff_cmd("[[ a ||\n b ]]");                 // newline after ||
    }

    /// T1-review CRITICAL regression: a `Newline` at an operand/operator
    /// BOUNDARY (where the oracle skips nothing) must NOT be skipped — the atom
    /// path previously used `skip_test_ws` (Blank+Newline) at three such sites
    /// and wrongly ACCEPTED multi-line inputs the oracle REJECTS. `skip_test_ws`
    /// is now confined to the four oracle `skip_test_newlines` sites; the
    /// boundary sites use `skip_test_blanks` (Blank-only), and the newline
    /// reaches the operand check → the same error the oracle raises.
    #[test]
    fn atoms_double_bracket_newline_boundaries() {
        diff_err("[[ -f\nx ]]");    // unary operand on next line → TestExprMissingOperand (both)
        diff_err("[[ a ==\nb ]]");  // binary rhs on next line → TestExprMissingOperand (both)
        diff_err("[[ a\n== b ]]");  // operator on next line → lone-word then leftover → UnterminatedDoubleBracket (both)
        diff_err("[[ (\na ) ]]");   // newline after `(` (grouping first operand) → TestExprMissingOperand (both)
        diff_err("[[ !\nx ]]");     // newline after `!` (post-negation operand) → TestExprMissingOperand (both)
        // The LEGIT multi-line cases (newline breaks only at the four oracle
        // skip sites) still parse identically — see also
        // `atoms_double_bracket_precedence`.
        diff_cmd("[[\n  a == b\n]]");
        diff_cmd("[[ a\n&&\nb ]]");
        diff_cmd("[[ a ||\n b ]]");
    }

    // v253 T4: error-parity hardening (Empty/Unterminated/MissingOperand plus
    // unary/binary operand-missing variants). `TestExprBadOperator` is
    // defensively unreachable on both paths (see `atoms_double_bracket_extra`'s
    // `[[ a ~~ b ]]` note) so it is not exercised here as a distinct variant.
    #[test]
    fn atoms_double_bracket_errors() {
        // Same ERROR VARIANT on both paths (full `Result` equality, not just
        // is_err()==is_err()).
        diff_err("[[ ]]");          // EmptyDoubleBracket
        diff_err("[[ a == b");      // UnterminatedDoubleBracket (EOF)
        diff_err("[[ < b ]]");      // MissingOperand (leading Op)
        diff_err("[[ == b ]]");     // `==` is a Word → lone-word then leftover `b` → Unterminated
        diff_err("[[ -f ]]");       // unary missing operand
        diff_err("[[ a == ]]");     // binary missing rhs
    }

    // v253 T4: adversarial corpus (expansions/quotes/globs/tokenization edges).
    #[test]
    fn atoms_double_bracket_corpus() {
        diff_cmd("[[ \"$x\" == \"$y\" ]]");          // quoted operands
        diff_cmd("[[ ${a[0]} -gt 0 ]]");             // subscript expansion operand
        diff_cmd("[[ $(cmd) == out ]]");             // command-sub operand
        diff_cmd("[[ a=b ]]");                       // `a=b` is ONE word (lone-word -n), NOT an assignment
        diff_cmd("[[ -n a=b ]]");
        diff_cmd("[[ x != y* ]]");                   // glob pattern RHS of !=
        diff_cmd("[[ -f 'a b' ]]");                  // quoted operand w/ space
        diff_cmd("[[ a\\ b == c ]]");                // escaped space in operand
        diff_cmd("[[ ! ( a == b ) || c ]]");         // ! before a group
        diff_cmd("[[ -e / ]]");
        diff_cmd("[[ -o errexit ]]");                // -o shell-option unary
    }
}
