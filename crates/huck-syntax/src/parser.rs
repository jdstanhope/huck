//! Parser-driven front-end. Consumes the stack-mode lexer's atoms and builds the
//! AST (`WordPart`/`Word` + `command.rs` command types). Live in production since
//! the v264 flip and the sole front-end since v266 (the old Word-lexer/oracle
//! parser is deleted): `parse_sequence` is the entry point.

use crate::command::{
    ArithForClause, AssignTarget, Assignment, CaseClause, CaseItem, CaseTerminator, Command,
    Connector, Delim, ElifBranch, ExecCommand, ExpectFailure, ForClause, Found, IfClause,
    ParseError, Pipeline, RedirOp, Redirection, SelectClause, Sequence, SimpleCommand,
    TestBinaryOp, TestExpr, TestUnaryOp, WhileClause, is_compound_opener, skip_test_newlines,
    try_unary_op, word_literal_text,
};
use crate::lexer::{
    ArithDelim, ArrayLiteralElement, Lexer, Mode, Operator, ParamModifier, ParamOpKind, ProcDir,
    QuoteStyle, SubscriptKind, SubstAnchor, SubstKind, TokenKind, Word, WordPart,
    brace_expand_parts,
};

/// Parse shell source into a command AST using the default lexer configuration
/// (no aliases, default `LexerOptions`). Returns `Ok(None)` for empty or
/// comment-only input. For alias expansion or custom options, build a
/// [`Lexer`](crate::lexer::Lexer) explicitly and call [`parse_sequence`].
pub fn parse(src: &str) -> Result<Option<Sequence>, ParseError> {
    let mut lx = crate::lexer::Lexer::new(
        src,
        &Default::default(),
        crate::lexer::LexerOptions::default(),
    );
    parse_sequence(&mut lx)
}

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
            None | Some(TokenKind::ParamClose | TokenKind::RBracket | TokenKind::ParamSep,)
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
            TokenKind::Lit {
                text,
                quoted: atom_q,
            } => {
                parts.push(WordPart::Literal {
                    text,
                    quoted: atom_q || quoted,
                });
            }
            TokenKind::DollarName {
                name: n,
                quoted: atom_q,
            } => {
                // The atom carries its own `quoted` context (set by the lexer
                // from its per-frame `in_dquote` flag); OR with enclosing.
                let eff = quoted || atom_q;
                let part = match n.as_str() {
                    "@" => WordPart::AllArgs {
                        quoted: eff,
                        joined: false,
                    },
                    "*" => WordPart::AllArgs {
                        quoted: eff,
                        joined: true,
                    },
                    "?" => WordPart::LastStatus { quoted: eff },
                    _ => WordPart::Var {
                        name: n,
                        quoted: eff,
                    },
                };
                parts.push(part);
            }
            TokenKind::CmdSubOpen => {
                // v244 T4: `$(cmd)` signal from scan_step_param_operand.
                // The cursor is at `$(` — parse_command_sub pushes Mode::CommandSub
                // and scan_step_command_sub(false) owns consuming `$(`.
                // G1/v270: a `$(…)` inside a `"…"` span within the operand is
                // quoted (must not word-split) — OR the enclosing `quoted` with the
                // operand frame's live `in_dquote` (see `Lexer::operand_in_dquote`).
                let q = quoted || iter.operand_in_dquote();
                let cs = parse_command_sub(iter, q)?;
                parts.push(cs);
                // v264 flip-fix (Finding 2): clear the `)`-boundary_reset leak on
                // the continuing operand word, mirroring `parse_word_command`.
                iter.clear_cmd_at_word_start();
            }
            TokenKind::BeginBacktick => {
                // v245 T6: `` `cmd` `` signal from scan_step_param_operand.
                // The cursor is at `` ` `` — parse_backtick_sub (v274: capture the
                // raw body under Mode::BacktickRaw, unescape, then re-parse with a
                // fresh Lexer) owns consuming the opening `` ` ``.
                // G1/v270: quoted when inside a `"…"` operand span.
                let q = quoted || iter.operand_in_dquote();
                let bt = parse_backtick_sub(iter, q)?;
                parts.push(bt);
            }
            TokenKind::ArithOpen => {
                // v246 T6: `$((…))` signal from scan_step_param_operand.
                // Cursor is at `$((` — parse_arith_expansion pushes Mode::Arith
                // whose first scan consumes `$((`.
                // G1/v270: quoted when inside a `"…"` operand span.
                let q = quoted || iter.operand_in_dquote();
                let a = parse_arith_expansion(iter, q)?;
                parts.push(a);
            }
            TokenKind::LegacyArithOpen => {
                let a = parse_legacy_arith_expansion(iter, quoted)?;
                parts.push(a);
            }
            TokenKind::DeferredExpansion => {
                // `$(cmd)` / `$((…))` inside a nested `"…"` operand span — still
                // deferred (see the `DeferredExpansion` doc comment).
                return Err(ParseError::UnsupportedExpansion);
            }
            // v259 F3: `$"…"` locale quoting in a param-expansion operand context
            // (`scan_step_param_operand` drops the `$` and emits a zero-width
            // `BeginDquote`, leaving the `"` for the normal double-quote
            // assembler). A bare `"…"` never reaches here — this operand
            // scanner's OWN "outside dquote" arm inlines it flat directly, with
            // no `BeginDquote` signal — so this arm only ever fires for `$"…"`.
            //
            // The oracle's representation for `$"…"` differs by which operand
            // this is: VALUE-family operands (`scan_braced_param_expansion`'s
            // `${x:-…}`/`${x/…/…}`/`${x:o:l}` bodies) inline the span FLAT (no
            // `Quoted` wrapper — mirrors the flat inlining `parse_regex_operand`
            // already does for the same oracle scanner shape). Subscript
            // operands (`${a[i]}` / array-literal `[i]=`) instead re-tokenize
            // via `scan_subscript`/`parse_subscript_body` — the general
            // tokenizer — which DOES keep the `Quoted{Double,…}` wrapper.
            // Distinguish via the enclosing mode (captured before `parse_dquote`
            // pushes its own `Mode::DoubleQuote` frame).
            TokenKind::BeginDquote => {
                let in_subscript =
                    matches!(iter.current_mode(), Mode::ParamSubscriptOperand { .. });
                let dq = parse_dquote(iter, quoted)?;
                if in_subscript {
                    parts.push(dq);
                } else {
                    match dq {
                        WordPart::Quoted { parts: inner, .. } => parts.extend(inner),
                        other => parts.push(other),
                    }
                }
            }
            // v263: a bare `'…'` inside a SUBSCRIPT operand (scan_step_param_operand
            // emits QuoteRun{Single} only when end==']') wraps in Quoted{Single} to
            // match the oracle's scan_subscript. QuoteRun reaches parse_word solely
            // from Mode::ParamSubscriptOperand; value families keep emitting flat Lit.
            TokenKind::QuoteRun { style, text } => {
                parts.push(WordPart::Quoted {
                    style,
                    parts: vec![WordPart::Literal { text, quoted: true }],
                });
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
        // v318 (#218): read before the `peek_kind` borrow below (the ProcSubOpen
        // guard needs it, but a guard clause can't call back into `iter` while
        // `peek_kind`'s borrow is live).
        let in_assign = iter.in_assignment_value();
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
                // v264 flip-fix (Finding 2): the `)` closing the cmdsub ran
                // `boundary_reset()` (→ `cmd_at_word_start = true`), which leaks
                // into this CONTINUING word and mis-classifies a glued `#`/`~`
                // (`echo a$(true)#b`, `echo $(echo X)~root`). The word continues,
                // so force mid-word. A following `Blank`/`Newline` re-arms
                // word-start, so the spaced `$(…) #c` comment case stays correct.
                iter.clear_cmd_at_word_start();
            }
            // v252: compound array RHS. The prefix part (Literal "name=" or
            // AssignPrefix) is already accumulated; glue the ArrayLiteral after it.
            Some(TokenKind::ArrayOpen) => {
                iter.next_kind()?;            // discard the signal (cursor on `(`)
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_array_literal(iter)?);
            }
            // v264: extglob group (`+(a|b)`, gated by `LexerOptions::extglob`).
            // The zero-width `ExtglobOpen` signal is discarded here (mirrors
            // `ArrayOpen`/`CmdSubOpen`) so `parse_extglob_group`'s own
            // `push_mode` + first pull re-scans the real `<prefix>(` under the
            // NEW mode frame. The group is a WORD PART glued mid-word — extend
            // (not push a single part) and CONTINUE the word so trailing
            // literals (`+(a|b)*`) glue after it.
            Some(TokenKind::ExtglobOpen { .. }) => {
                iter.next_kind()?;            // discard the signal
                flush_lit(&mut acc, &mut parts);
                let group = parse_extglob_group(iter)?;
                parts.extend(group);
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
            //
            // v318 (#218) EXCEPTION: an assignment RHS in progress (`f=<(cmd)`)
            // is the SAME word as its `name=` prefix in bash — `f=<(cmd)` sets
            // `f` to `/dev/fd/N`, it does not end the word at `<(`. The lexer's
            // `in_assignment_value` (set by `try_scan_assign_prefix` for the
            // `name=` atom, not reset across `<(`/`>(` — see the ProcSubOpen
            // scan arm) distinguishes this from an ordinary already-accumulated
            // word (`x<(y)`), which keeps the break above.
            Some(TokenKind::ProcSubOpen { .. })
                if !(parts.is_empty() && acc.is_none()) && !in_assign =>
            {
                break;
            }
            Some(TokenKind::ProcSubOpen { dir }) => {
                let dir = dir.clone();
                iter.next_kind()?;            // discard the signal (cursor stays on `(`)
                // v318 (#218): flush any pending literal run FIRST (e.g. the
                // `name=` prefix of an in-progress assignment value) so it lands
                // before the ProcessSub part instead of after it. Previously this
                // arm only ever ran with an empty `acc` (fresh word start), so no
                // flush was needed; the assignment-RHS exception above now also
                // reaches here with `acc` non-empty.
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_process_sub(iter, dir)?);
                // v264 flip-fix (Finding 2): same `)`-boundary_reset leak as the
                // cmdsub arm — force mid-word for the continuing word (`<(y)z`).
                iter.clear_cmd_at_word_start();
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
            Some(TokenKind::LegacyArithOpen) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_legacy_arith_expansion(iter, quoted)?);
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
            Some(TokenKind::Tilde { .. }) => {
                if let Some(TokenKind::Tilde { spec, assign_ctx }) = iter.next_kind()? {
                    flush_lit(&mut acc, &mut parts);
                    parts.push(WordPart::Tilde { spec, assign_ctx });
                }
            }
            // v247 T4: a bare scalar-append assignment-prefix atom (`name+=`).
            // Carried into the Word unchanged as the leading `WordPart::AssignPrefix`;
            // `try_split_assignment` consumes it later. (The `name[sub]=` /
            // `name[sub]+=` indexed form is no longer lexer-assembled — see the
            // `LBracket` arm below, v268.)
            Some(TokenKind::AssignPrefix { .. }) => {
                if let Some(TokenKind::AssignPrefix { target, append }) = iter.next_kind()? {
                    flush_lit(&mut acc, &mut parts);
                    parts.push(WordPart::AssignPrefix { target, append });
                }
            }
            // v268: `name[` at command-word start — the lexer emitted `Lit name`
            // then this zero-width `LBracket` and set `pending_lvalue_subscript`
            // (see `try_scan_assign_prefix`'s `Some('[')` arm). Assemble the
            // subscript Word under `Mode::ParamSubscriptOperand` — the same
            // machinery `${a[i]}`/array-literal `[i]=` use — then decide by the
            // token that follows the closing `]`: an `AssignEq` (the lexer's
            // `pending_lvalue_subscript` hook only emits it when `=`/`+=`
            // immediately follows, no space) confirms an indexed assignment;
            // anything else means this was an ordinary glob word (`a[bc]`,
            // `a[$x]` with no `=`) and the whole `name[sub]` folds back into
            // this word's parts.
            Some(TokenKind::LBracket) => {
                iter.next_kind()?; // consume LBracket
                // The name is whatever literal was accumulated immediately before
                // `[` (the lexer emits `Lit name` then `LBracket`, and nothing else
                // can sit between them at true word-start). If for some reason
                // there is none, fall back to treating `[` as a literal char.
                let name = match acc.take() {
                    Some((n, false)) => n,
                    other => {
                        acc = other;
                        push_lit(&mut acc, &mut parts, "[".to_string(), quoted);
                        continue;
                    }
                };
                // Mark BEFORE pushing the subscript mode so an unclosed/malformed
                // bracket (`a[$x` with no `]`, or a lex error partway through, e.g.
                // an unterminated quote) can rewind cleanly to just after `[` and
                // fall through to ORDINARY command-word scanning over the rest —
                // mirrors the old lexer bridge's forgiving fallback ("NOTHING is
                // consumed... falls back to ordinary word scanning"). Unlike that
                // old fallback, the rewound content is no longer literal-swallowed:
                // it re-scans as normal word atoms, so `$x` EXPANDS — confirmed
                // against real bash (`a=(1 2); echo a[$x` prints `a5`, not `a$x`),
                // so this is a correctness improvement, not a regression.
                let mark = iter.mark();
                iter.push_mode(Mode::ParamSubscriptOperand { in_dquote: false, enclosing_dquote: false });
                let closed: Result<Word, ParseError> = (|| {
                    let sub_word = parse_word(iter, false)?;
                    match iter.next_kind()? {
                        Some(TokenKind::RBracket) => Ok(sub_word),
                        _ => Err(ParseError::UnsupportedExpansion),
                    }
                })();
                iter.pop_mode(); // ParamSubscriptOperand
                match closed {
                    Ok(sub_word) => match iter.peek_kind()? {
                        Some(TokenKind::AssignEq { append }) => {
                            let append = *append;
                            iter.next_kind()?;                    // consume AssignEq
                            iter.begin_assignment_value(append);  // value-mode BEFORE value pull
                            parts.push(WordPart::AssignPrefix {
                                target: AssignTarget::Indexed { name, subscript: sub_word },
                                append,
                            });
                            // The value flows into this SAME word next; the caller's
                            // `try_split_assignment` splits on the leading AssignPrefix.
                        }
                        _ => {
                            // Glob fold-back: name + `[` + subscript parts + `]` → one
                            // word. Literal subscript parts (`a[bc]`) merge into the
                            // surrounding literal run through the SAME `acc` buffer
                            // (byte-identical to the old lexer's literal-swallow
                            // fallback); a non-literal part (`a[$x]`, D1 fix) simply
                            // ends the run and is pushed as its own part, exactly like
                            // any other glued expansion (`a$x]`).
                            push_lit(&mut acc, &mut parts, name, quoted);
                            push_lit(&mut acc, &mut parts, "[".to_string(), quoted);
                            for part in sub_word.0 {
                                match part {
                                    WordPart::Literal { text, quoted: q } => push_lit(&mut acc, &mut parts, text, q || quoted),
                                    other => { flush_lit(&mut acc, &mut parts); parts.push(other); }
                                }
                            }
                            push_lit(&mut acc, &mut parts, "]".to_string(), quoted);
                        }
                    },
                    Err(_) => {
                        // Never validly closed: rewind to right after `[` and let
                        // the outer loop's ordinary dispatch re-scan from there.
                        iter.rewind(&mark);
                        // v268 fix: `mark` was taken AFTER the lexer set
                        // `pending_lvalue_subscript = true` for this `name[`, so the
                        // rewind above just restored the flag to `true`. We've
                        // decided this is NOT an assignment (never validly closed),
                        // so clear it — otherwise the very next command-scan step
                        // would misread a bare `=`/`+=` right after `[` as a
                        // spurious AssignEq the parser can't place (e.g. `a[=x`).
                        iter.clear_pending_lvalue_subscript();
                        push_lit(&mut acc, &mut parts, name, quoted);
                        push_lit(&mut acc, &mut parts, "[".to_string(), quoted);
                    }
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
            // on the Word-lexer path — command-sub bodies (`new`), for/select
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
                    // Capture the offending token BEFORE consuming it —
                    // `unexpected_here` peeks the CURRENT token, so it must run
                    // before the `next_kind()` consume below or it would report
                    // whatever token comes AFTER this one instead.
                    let f = iter.unexpected_here(None)?;
                    iter.next_kind()?;
                    return Err(ParseError::Unexpected(f));
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
    iter.push_mode(Mode::Regex {
        paren_depth: 0,
        body_started: false,
    });
    // Drop any already-buffered leading Blank/Newline (the `next_is_test_binary_
    // operator_atom` peek2 buffered exactly one boundary atom after `=~`). The
    // lexer's own leading-skip handles the rest (spaces after a newline, `\<NL>`).
    while matches!(
        iter.peek_kind()?,
        Some(TokenKind::Blank) | Some(TokenKind::Newline)
    ) {
        iter.next_kind()?;
    }
    let mut parts: Vec<WordPart> = Vec::new();
    let mut acc: Option<(String, bool)> = None;
    loop {
        match iter.peek_kind()? {
            Some(TokenKind::RegexEnd) => {
                iter.next_kind()?;
                break;
            }
            Some(TokenKind::Lit { .. }) => {
                if let Some(TokenKind::Lit { text, quoted: q }) = iter.next_kind()? {
                    push_lit(&mut acc, &mut parts, text, q);
                }
            }
            Some(TokenKind::DollarLit { .. }) => {
                if let Some(TokenKind::DollarLit { quoted: q }) = iter.next_kind()? {
                    flush_lit(&mut acc, &mut parts);
                    parts.push(WordPart::Literal {
                        text: "$".into(),
                        quoted: q,
                    });
                }
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
                        _ => parts.push(WordPart::Quoted {
                            style,
                            parts: vec![WordPart::Literal { text, quoted: true }],
                        }),
                    }
                }
            }
            Some(TokenKind::ParamOpen { .. }) => {
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_param_expansion(iter, false)?);
            }
            Some(TokenKind::CmdSubOpen) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_command_sub(iter, false)?);
            }
            Some(TokenKind::ArithOpen) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_arith_expansion(iter, false)?);
            }
            Some(TokenKind::LegacyArithOpen) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_legacy_arith_expansion(iter, false)?);
            }
            Some(TokenKind::BeginBacktick) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_backtick_sub(iter, false)?);
            }
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
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                match parse_dquote(iter, false)? {
                    WordPart::Quoted { parts: inner, .. } => {
                        let is_empty_marker = inner.len() == 1
                            && matches!(&inner[0], WordPart::Literal { text, quoted: true } if text.is_empty());
                        if !is_empty_marker {
                            parts.extend(inner);
                        }
                    }
                    other => parts.push(other),
                }
            }
            Some(TokenKind::DollarName { .. }) => {
                if let Some(TokenKind::DollarName { name, quoted: q }) = iter.next_kind()? {
                    flush_lit(&mut acc, &mut parts);
                    parts.push(match name.as_str() {
                        "@" => WordPart::AllArgs {
                            quoted: q,
                            joined: false,
                        },
                        "*" => WordPart::AllArgs {
                            quoted: q,
                            joined: true,
                        },
                        "?" => WordPart::LastStatus { quoted: q },
                        _ => WordPart::Var { name, quoted: q },
                    });
                }
            }
            // v258: `$[expr]` inside a regex is handled by the `LegacyArithOpen`
            // arm above, not this catch-all. Other still-deferred constructs
            // inside a regex operand fall through here (v247).
            Some(TokenKind::DeferredExpansion) => return Err(ParseError::UnsupportedCommand),
            other => {
                return Err(ParseError::TestExprBadOperator(format!(
                    "regex operand: {other:?}"
                )));
            }
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

/// v264: assemble one extglob group `<prefix>( … )` as a flat list of word
/// parts (NOT wrapped in a `Word` — the caller glues them into the
/// surrounding word). The parser has just consumed the zero-width
/// `ExtglobOpen{prefix}` signal (cursor still sits on `<prefix>(`); push
/// `Mode::Extglob` and pull atoms until the zero-width `ExtglobEnd` (the lexer
/// emits it, and pops the mode, in the same call that closes the group — see
/// `scan_step_extglob`). Mirrors `parse_regex_operand`'s part-assembly arms:
/// nested `$(…)`/`` `…` ``/`${…}`/`"…"`/`'…'` recurse via the existing
/// sub-parsers (parser-owned recursion — THE RULE: the lexer never
/// forward-scans a nested `$(…)`, only tracks paren depth incrementally; the
/// parser owns delimiter-matching by recursing here).
fn parse_extglob_group(iter: &mut Lexer) -> Result<Vec<WordPart>, ParseError> {
    iter.push_mode(Mode::Extglob { paren_depth: 0 });
    let mut parts: Vec<WordPart> = Vec::new();
    let mut acc: Option<(String, bool)> = None;
    loop {
        match iter.peek_kind()? {
            Some(TokenKind::ExtglobEnd) => {
                iter.next_kind()?;
                break;
            }
            Some(TokenKind::Lit { .. }) => {
                if let Some(TokenKind::Lit { text, quoted: q }) = iter.next_kind()? {
                    push_lit(&mut acc, &mut parts, text, q);
                }
            }
            Some(TokenKind::DollarLit { .. }) => {
                if let Some(TokenKind::DollarLit { quoted: q }) = iter.next_kind()? {
                    flush_lit(&mut acc, &mut parts);
                    parts.push(WordPart::Literal {
                        text: "$".into(),
                        quoted: q,
                    });
                }
            }
            Some(TokenKind::QuoteRun { .. }) => {
                if let Some(TokenKind::QuoteRun { style, text }) = iter.next_kind()? {
                    flush_lit(&mut acc, &mut parts);
                    // Mirrors the oracle's `scan_extglob_group` `'` arm: a
                    // SINGLE-quoted run inlines FLAT as `Literal{quoted:true}`
                    // (no `Quoted` wrapper); a `$'…'` ANSI-C run keeps its
                    // `Quoted{AnsiC,…}` wrapper (same split `parse_regex_operand`
                    // makes — Backslash/Double never reach here).
                    match style {
                        QuoteStyle::Single => parts.push(WordPart::Literal { text, quoted: true }),
                        _ => parts.push(WordPart::Quoted {
                            style,
                            parts: vec![WordPart::Literal { text, quoted: true }],
                        }),
                    }
                }
            }
            Some(TokenKind::ParamOpen { .. }) => {
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_param_expansion(iter, false)?);
            }
            Some(TokenKind::CmdSubOpen) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_command_sub(iter, false)?);
            }
            Some(TokenKind::ArithOpen) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_arith_expansion(iter, false)?);
            }
            Some(TokenKind::LegacyArithOpen) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_legacy_arith_expansion(iter, false)?);
            }
            Some(TokenKind::BeginBacktick) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_backtick_sub(iter, false)?);
            }
            // Mirrors the oracle's `scan_extglob_group` `"` arm (delegates to
            // `scan_dquote_expansion_body`, which inlines FLAT, not a `Quoted`
            // wrapper — same as `parse_regex_operand`'s non-subscript case).
            // Drop the atom-native `parse_dquote`'s injected empty-`""` marker
            // (`[Literal{"",true}]`) so an empty `""` contributes no part,
            // matching `scan_dquote_expansion_body`'s empty-body no-op.
            Some(TokenKind::BeginDquote) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                match parse_dquote(iter, false)? {
                    WordPart::Quoted { parts: inner, .. } => {
                        let is_empty_marker = inner.len() == 1
                            && matches!(&inner[0], WordPart::Literal { text, quoted: true } if text.is_empty());
                        if !is_empty_marker {
                            parts.extend(inner);
                        }
                    }
                    other => parts.push(other),
                }
            }
            Some(TokenKind::DollarName { .. }) => {
                if let Some(TokenKind::DollarName { name, quoted: q }) = iter.next_kind()? {
                    flush_lit(&mut acc, &mut parts);
                    parts.push(match name.as_str() {
                        "@" => WordPart::AllArgs {
                            quoted: q,
                            joined: false,
                        },
                        "*" => WordPart::AllArgs {
                            quoted: q,
                            joined: true,
                        },
                        "?" => WordPart::LastStatus { quoted: q },
                        _ => WordPart::Var { name, quoted: q },
                    });
                }
            }
            Some(TokenKind::DeferredExpansion) => return Err(ParseError::UnsupportedCommand),
            other => {
                return Err(ParseError::TestExprBadOperator(format!(
                    "extglob group: {other:?}"
                )));
            }
        }
    }
    flush_lit(&mut acc, &mut parts);
    Ok(parts)
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
    iter.push_mode(Mode::DoubleQuote {
        body_started: false,
    });
    let result = (|| -> Result<Vec<WordPart>, ParseError> {
        let mut parts: Vec<WordPart> = Vec::new();
        // Pending coalescible literal chunk (see `push_lit`/`flush_lit`); a
        // `DollarLit` is a barrier that flushes it and pushes `$` standalone.
        let mut acc: Option<(String, bool)> = None;
        loop {
            match iter.peek_kind()? {
                // Closing `"` — consume and finish.
                Some(TokenKind::EndDquote) => {
                    iter.next_kind()?;
                    break;
                }
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
                Some(TokenKind::LegacyArithOpen) => {
                    iter.next_kind()?;
                    flush_lit(&mut acc, &mut parts);
                    parts.push(parse_legacy_arith_expansion(iter, true)?);
                }
                Some(TokenKind::DollarName { .. }) => {
                    if let Some(TokenKind::DollarName { name, quoted: _ }) = iter.next_kind()? {
                        flush_lit(&mut acc, &mut parts);
                        parts.push(match name.as_str() {
                            "@" => WordPart::AllArgs {
                                quoted: true,
                                joined: false,
                            },
                            "*" => WordPart::AllArgs {
                                quoted: true,
                                joined: true,
                            },
                            "?" => WordPart::LastStatus { quoted: true },
                            _ => WordPart::Var { name, quoted: true },
                        });
                    }
                }
                Some(TokenKind::DollarLit { .. }) => {
                    if let Some(TokenKind::DollarLit { quoted: _ }) = iter.next_kind()? {
                        flush_lit(&mut acc, &mut parts);
                        parts.push(WordPart::Literal {
                            text: "$".into(),
                            quoted: true,
                        });
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
        parts.push(WordPart::Literal {
            text: String::new(),
            quoted: true,
        });
    }
    Ok(WordPart::Quoted {
        style: crate::lexer::QuoteStyle::Double,
        parts,
    })
}

/// Convert the subscript word assembled by `parse_word` into a `SubscriptKind`.
/// A bare unquoted `@` or `*` literal maps to `All` / `Star` respectively;
/// anything else becomes `Index(word)`.  Mirrors `scan_param_subscript` in the
/// production lexer.
fn subscript_kind_from(w: Word) -> SubscriptKind {
    match w.0.as_slice() {
        [
            WordPart::Literal {
                text,
                quoted: false,
            },
        ] if text == "@" => SubscriptKind::All,
        [
            WordPart::Literal {
                text,
                quoted: false,
            },
        ] if text == "*" => SubscriptKind::Star,
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
pub(crate) fn parse_param_expansion(
    iter: &mut Lexer,
    quoted: bool,
) -> Result<WordPart, ParseError> {
    // 1. Push the mode and consume the `ParamOpen` (`${`) token.
    iter.push_mode(Mode::ParamExpansion {
        seen_name: false,
        indirect: false,
        start_off: 0,
    });

    // v264 (M-156): seed the head scanner's extquote double-quote gate. The
    // oracle gates a `$'…'`-decoded NAME on `quoted || opts.in_dquote`; fold our
    // `quoted` arg into `opts.in_dquote` for the duration of this expansion so the
    // lexer resolves the gate. `saved_dq` restores the inherited value on exit;
    // the pattern-family operands (`#`/`%`/`/`/`^`/`,`) keep the elevated value so
    // a NESTED `${…}` inside them sees the enclosing dquote (mirrors the oracle's
    // `opts.with_in_dquote(quoted || opts.in_dquote)`), while value/error/substring
    // operands are lowered back to `saved_dq` (oracle passes `opts` unchanged).
    let saved_dq = iter.in_dquote();
    let m156_dq = quoted || saved_dq;
    iter.set_in_dquote(m156_dq);

    // Restore-on-exit helper for the M-156 gate flag.
    macro_rules! restore_dq {
        () => {
            iter.set_in_dquote(saved_dq);
        };
    }

    match iter.next_kind()? {
        Some(TokenKind::ParamOpen { .. }) => {
            // Record the `${`'s `$` offset for bad-subst raw reconstruction. In
            // production the enclosing scanner consumes `${` (2 ASCII bytes) and
            // the cursor now sits just past it, so `$` is at `cursor - 2`.
            iter.set_param_start_off_from_cursor();
        }
        _ => {
            restore_dq!();
            iter.pop_mode();
            return Err(ParseError::UnsupportedExpansion);
        }
    }

    // Byte offset of the leading `$` of this `${…}`, captured once now that the
    // frame's `start_off` is set. A bad-substitution arm (name-position or
    // post-name) uses it with `source_span` to assemble the verbatim `${…}` raw
    // AFTER the parser has driven the body's tail to the matching `}`.
    let start_off = iter.param_start_off();

    // Operand macros, hoisted above the name-dispatch match so BOTH the
    // name-position and post-name `ParamBadSubst` arms (and the operator arm)
    // can use them.
    //
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

    // 3. The parameter name (always present; may be "" for bad-subst). A
    // `$'…'`-decoded name (`ParamNameDecoded`) sets `name_decoded` so the bare
    // form is promoted to `ParamExpansion{None}` (matching the oracle's `declare
    // -f` round-trip). A `ParamBadSubst` here is a name-position bad substitution.
    let mut name_decoded = false;
    let name = match iter.next_kind()? {
        Some(TokenKind::ParamName(n)) => n,
        Some(TokenKind::ParamNameDecoded(n)) => {
            name_decoded = true;
            n
        }
        Some(TokenKind::ParamBadSubst) => {
            // Name-position bad substitution. The lexer emitted only a marker
            // (cursor left on the offending char); drive the rest of the body to
            // the matching `}` via the operand machinery (correct nesting/quote
            // matching), discard the resulting word, then assemble the verbatim
            // `${…}` raw from `source_span`. If the body is unterminated,
            // `word_in_mode!` itself returns the lexer's Unterminated* error
            // (propagates as rc=2 — the intended behavior).
            let _ = word_in_mode!(
                Mode::ParamWordOperand {
                    in_dquote: false,
                    enclosing_dquote: quoted
                },
                quoted
            );
            let close_off = iter.peek_span()?.map(|s| s.offset).unwrap_or(start_off);
            expect_close!();
            let raw = iter.source_span(start_off, close_off).to_string();
            restore_dq!();
            iter.pop_mode();
            return Ok(WordPart::ParamExpansion {
                name: String::new(),
                modifier: ParamModifier::BadSubst { raw },
                quoted,
                subscript: None,
                indirect: false,
            });
        }
        _ => {
            restore_dq!();
            iter.pop_mode();
            return Err(ParseError::UnsupportedExpansion);
        }
    };

    // The NAME's M-156 gate has now fired. Lower the gate to the inherited value
    // for the subscript and value/error/substring operands (the oracle scans them
    // with `opts` unchanged); pattern-family operands re-elevate below.
    iter.set_in_dquote(saved_dq);

    // 4. Optional subscript `[…]`.
    let mut subscript: Option<SubscriptKind> = None;
    if matches!(iter.peek_kind()?, Some(TokenKind::LBracket)) {
        iter.next_kind()?; // consume LBracket
        iter.push_mode(Mode::ParamSubscriptOperand {
            in_dquote: false,
            enclosing_dquote: false,
        });
        let sub_word = match parse_word(iter, false) {
            Ok(w) => w,
            Err(e) => {
                iter.pop_mode(); // ParamSubscriptOperand
                restore_dq!();
                iter.pop_mode(); // ParamExpansion
                return Err(e);
            }
        };
        match iter.next_kind()? {
            Some(TokenKind::RBracket) => {}
            _ => {
                iter.pop_mode(); // ParamSubscriptOperand
                restore_dq!();
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
            if indirect
                && matches!(
                    subscript,
                    Some(SubscriptKind::All) | Some(SubscriptKind::Star)
                )
            {
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
                // `${a[i]}` / `${a[@]}` / `${a[*]}` — bare subscripted reference,
                // OR `${#a[i]}` — length OF a subscripted element/array. The
                // oracle keeps the `Length` modifier alongside the subscript
                // (`${#a[0]}` = Length{subscript:Index(0)}); honor `length_form`
                // here so a `#`+subscript is not dropped to `None` (v263).
                WordPart::ParamExpansion {
                    name,
                    modifier: if length_form {
                        ParamModifier::Length
                    } else {
                        ParamModifier::None
                    },
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
                WordPart::AllArgs {
                    quoted,
                    joined: false,
                }
            } else if name == "*" {
                // `${*}` — all positional args (joined=true).
                WordPart::AllArgs {
                    quoted,
                    joined: true,
                }
            } else if name_decoded {
                // `${$'x1'}` / `${a$'b'}` — a `$'…'`-decoded name in bare form.
                // The oracle promotes the plain `Var` to `ParamExpansion{None}`
                // so `declare -f` reconstructs the normalised `${x1}` form.
                WordPart::ParamExpansion {
                    name,
                    modifier: ParamModifier::None,
                    quoted,
                    subscript: None,
                    indirect: false,
                }
            } else {
                // `${name}` — plain variable reference.
                WordPart::Var { name, quoted }
            }
        }

        // ── Prefix-name expansion: `${!pfx*}` / `${!pfx@}` (indirect:false).
        Some(TokenKind::ParamPrefixClose { at }) => WordPart::ParamExpansion {
            name,
            modifier: ParamModifier::PrefixNames { at },
            quoted,
            subscript: None,
            indirect: false,
        },

        // ── Post-name bad substitution (`${x!}`, `${V@}`, `${-3}`, `${x@Z}`).
        // The lexer emitted only a marker; drive the body's tail to the matching
        // `}` via the operand machinery, discard the word, then assemble the
        // verbatim `${…}` raw from `source_span`. The common cleanup at the end
        // of the function runs `restore_dq!()` + `pop_mode()`.
        Some(TokenKind::ParamBadSubst) => {
            let _ = word_in_mode!(
                Mode::ParamWordOperand {
                    in_dquote: false,
                    enclosing_dquote: quoted
                },
                quoted
            );
            let close_off = iter.peek_span()?.map(|s| s.offset).unwrap_or(start_off);
            expect_close!();
            let raw = iter.source_span(start_off, close_off).to_string();
            WordPart::ParamExpansion {
                name: String::new(),
                modifier: ParamModifier::BadSubst { raw },
                quoted,
                subscript: None,
                indirect: false,
            }
        }

        // ── Operator: pattern removal, substitute, case, transform, substring
        Some(TokenKind::ParamOp(op_kind)) => {
            // Pattern-family operands (`#`/`%`/`/`/`^`/`,`) re-elevate the M-156
            // gate so a NESTED `${…}` in the pattern sees the enclosing dquote
            // (mirrors the oracle's `opts.with_in_dquote(quoted || opts.in_dquote)`).
            // Value/error/substring operands keep the inherited value (`saved_dq`,
            // already restored above).
            if matches!(
                op_kind,
                ParamOpKind::RemovePrefix(_)
                    | ParamOpKind::RemoveSuffix(_)
                    | ParamOpKind::Substitute(_)
                    | ParamOpKind::Case(_, _)
            ) {
                iter.set_in_dquote(m156_dq);
            }

            match op_kind {
                // ── Value family: UseDefault / AssignDefault / ErrorIfUnset / UseAlternate
                // Production: `modifier_with_operand(chars, quoted/false, ...)`.
                // `ErrorIfUnset` uses `enclosing_dquote=false`; others use `quoted`.
                ParamOpKind::UseDefault(colon) => {
                    let word = word_in_mode!(
                        Mode::ParamWordOperand {
                            in_dquote: false,
                            enclosing_dquote: quoted
                        },
                        quoted
                    );
                    expect_close!();
                    WordPart::ParamExpansion {
                        name,
                        modifier: ParamModifier::UseDefault { word, colon },
                        quoted,
                        subscript,
                        indirect,
                    }
                }
                ParamOpKind::AssignDefault(colon) => {
                    let word = word_in_mode!(
                        Mode::ParamWordOperand {
                            in_dquote: false,
                            enclosing_dquote: quoted
                        },
                        quoted
                    );
                    expect_close!();
                    WordPart::ParamExpansion {
                        name,
                        modifier: ParamModifier::AssignDefault { word, colon },
                        quoted,
                        subscript,
                        indirect,
                    }
                }
                ParamOpKind::ErrorIfUnset(colon) => {
                    // Production: `modifier_with_operand(chars, false, ...)` — NOT `quoted`.
                    let word = word_in_mode!(
                        Mode::ParamWordOperand {
                            in_dquote: false,
                            enclosing_dquote: false
                        },
                        false
                    );
                    expect_close!();
                    WordPart::ParamExpansion {
                        name,
                        modifier: ParamModifier::ErrorIfUnset { word, colon },
                        quoted,
                        subscript,
                        indirect,
                    }
                }
                ParamOpKind::UseAlternate(colon) => {
                    let word = word_in_mode!(
                        Mode::ParamWordOperand {
                            in_dquote: false,
                            enclosing_dquote: quoted
                        },
                        quoted
                    );
                    expect_close!();
                    WordPart::ParamExpansion {
                        name,
                        modifier: ParamModifier::UseAlternate { word, colon },
                        quoted,
                        subscript,
                        indirect,
                    }
                }

                // ── Pattern removal: RemovePrefix / RemoveSuffix
                // Production: `modifier_with_operand(chars, false, ...)` — enclosing_dquote=false.
                ParamOpKind::RemovePrefix(longest) => {
                    let pattern = word_in_mode!(
                        Mode::ParamWordOperand {
                            in_dquote: false,
                            enclosing_dquote: false
                        },
                        false
                    );
                    expect_close!();
                    WordPart::ParamExpansion {
                        name,
                        modifier: ParamModifier::RemovePrefix { pattern, longest },
                        quoted,
                        subscript,
                        indirect,
                    }
                }
                ParamOpKind::RemoveSuffix(longest) => {
                    let pattern = word_in_mode!(
                        Mode::ParamWordOperand {
                            in_dquote: false,
                            enclosing_dquote: false
                        },
                        false
                    );
                    expect_close!();
                    WordPart::ParamExpansion {
                        name,
                        modifier: ParamModifier::RemoveSuffix { pattern, longest },
                        quoted,
                        subscript,
                        indirect,
                    }
                }

                // ── Substitute: ${var/pat/repl} / ${var//…} / ${var/#…} / ${var/%…}
                // Pattern in ParamSubstPatternOperand (sep=/); replacement in ParamWordOperand.
                // Both operands: enclosing_dquote=false (mirrors scan_substitution_operand).
                // Absent replacement (no ParamSep) → empty Word, matching bash ${var/pat}.
                ParamOpKind::Substitute(subst_kind) => {
                    let (anchor, all) = match subst_kind {
                        SubstKind::First => (SubstAnchor::None, false),
                        SubstKind::All => (SubstAnchor::None, true),
                        SubstKind::Prefix => (SubstAnchor::Prefix, false),
                        SubstKind::Suffix => (SubstAnchor::Suffix, false),
                    };

                    // Pattern in subst-pattern mode (sep = `/`).
                    iter.push_mode(Mode::ParamSubstPatternOperand {
                        in_dquote: false,
                        enclosing_dquote: false,
                    });
                    let pattern = match parse_word(iter, false) {
                        Ok(w) => {
                            iter.pop_mode();
                            w
                        }
                        Err(e) => {
                            iter.pop_mode();
                            iter.pop_mode();
                            return Err(e);
                        }
                    };

                    // Optional `/replacement`.
                    let replacement = if matches!(iter.peek_kind()?, Some(TokenKind::ParamSep)) {
                        iter.next_kind()?; // consume `/`
                        word_in_mode!(
                            Mode::ParamWordOperand {
                                in_dquote: false,
                                enclosing_dquote: false
                            },
                            false
                        )
                    } else {
                        Word(vec![])
                    };

                    expect_close!();
                    WordPart::ParamExpansion {
                        name,
                        modifier: ParamModifier::Substitute {
                            pattern,
                            replacement,
                            anchor,
                            all,
                        },
                        quoted,
                        subscript,
                        indirect,
                    }
                }

                // ── Case conversion: ${var^pat} / ${var^^} / ${var,pat} / ${var,,}
                // Production: `scan_optional_braced_operand` — empty body → None.
                ParamOpKind::Case(direction, all) => {
                    let word = word_in_mode!(
                        Mode::ParamWordOperand {
                            in_dquote: false,
                            enclosing_dquote: false
                        },
                        false
                    );
                    expect_close!();
                    let pattern = if word.0.is_empty() { None } else { Some(word) };
                    WordPart::ParamExpansion {
                        name,
                        modifier: ParamModifier::Case {
                            direction,
                            all,
                            pattern,
                        },
                        quoted,
                        subscript,
                        indirect,
                    }
                }

                // ── Transform: ${var@Q} / ${var@U} / etc.
                // No operand: the operator letter was already consumed by the head mode.
                // Only a ParamClose follows.
                ParamOpKind::Transform(op) => {
                    expect_close!();
                    WordPart::ParamExpansion {
                        name,
                        modifier: ParamModifier::Transform { op },
                        quoted,
                        subscript,
                        indirect,
                    }
                }

                // ── Substring: ${var:offset} / ${var:offset:length}
                // Offset in ParamSubstringOffsetOperand (sep = `:`); length in ParamWordOperand.
                // Empty offset (${var:}) → BadSubst, matching dispatch_braced_modifier's
                // `Some(':') / Some('}') → recover_bad_subst` branch.
                ParamOpKind::Substring => {
                    // Offset in substring-offset mode (sep = `:`).
                    iter.push_mode(Mode::ParamSubstringOffsetOperand {
                        in_dquote: false,
                        enclosing_dquote: false,
                    });
                    let offset = match parse_word(iter, false) {
                        Ok(w) => {
                            iter.pop_mode();
                            w
                        }
                        Err(e) => {
                            iter.pop_mode();
                            iter.pop_mode();
                            return Err(e);
                        }
                    };

                    // Optional `:length`.
                    let length = if matches!(iter.peek_kind()?, Some(TokenKind::ParamSep)) {
                        iter.next_kind()?; // consume `:`
                        Some(word_in_mode!(
                            Mode::ParamWordOperand {
                                in_dquote: false,
                                enclosing_dquote: false
                            },
                            false
                        ))
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
                            quoted,
                            subscript,
                            indirect,
                        }
                    }
                }
            }
        }

        _ => {
            restore_dq!();
            iter.pop_mode(); // ParamExpansion
            return Err(ParseError::UnsupportedExpansion);
        }
    };

    // 6. Restore the M-156 gate flag and pop the ParamExpansion frame.
    restore_dq!();
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
            for c in &mut p.commands {
                zero_lines_in_command(c);
            }
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
            if let Some(b) = &mut clause.else_body {
                zero_lines_in_sequence(b);
            }
        }
        Command::While(clause) => {
            zero_lines_in_sequence(&mut clause.condition);
            zero_lines_in_sequence(&mut clause.body);
        }
        Command::For(clause) => zero_lines_in_sequence(&mut clause.body),
        Command::Select(clause) => zero_lines_in_sequence(&mut clause.body),
        Command::Case(clause) => {
            for item in &mut clause.items {
                if let Some(b) = &mut item.body {
                    zero_lines_in_sequence(b);
                }
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
    iter.push_mode(Mode::CommandSub {
        body_started: false,
    });
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

    // 2. Dispatch: empty body or non-empty body. Skip a leading run of
    // Blank/Newline atoms first — bash treats a command-substitution body
    // that is only whitespace/newlines (`$( )`, `$(\n\t)`) as empty, unlike
    // an explicit subshell (`( )`/`(\n)` are syntax errors there; see the
    // `parse_subshell` caller above, which only skips `Blank`, not
    // `Newline`, before its own empty check).
    while matches!(
        iter.peek_kind()?,
        Some(TokenKind::Blank) | Some(TokenKind::Newline)
    ) {
        iter.next_kind()?;
    }
    // #109: a body that is only whitespace/comments reaching EOF before `)` is
    // an UNTERMINATED substitution, not a missing command — mirror
    // parse_subshell's guard (~4664) so the REPL/stdin reader keeps reading.
    if iter.peek_kind()?.is_none() {
        let pos = iter.cursor_pos();
        iter.pop_mode();
        return Err(unterminated_cmdsub(pos));
    }
    let sequence = if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::RParen))) {
        // Empty (or whitespace/newline-only) body `$()`/`$( )`/`$(\n)` —
        // consume `)` and construct the same Sequence the production oracle
        // yields via `parse_substitution_body("")` → `unwrap_or_else(empty_sequence)`.
        iter.next_kind()?; // consume `)`
        Sequence {
            first: Command::Pipeline(Pipeline {
                negate: false,
                commands: Vec::new(),
            }),
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
                // (body-deferred constructs still remaining, e.g. C-style
                // arith-for headers with unsupported shapes) to UnsupportedExpansion
                // so parse_command_sub has a consistent return type for all
                // deferrals. (`[[`, function-def, coproc bodies, and legacy `$[expr]`
                // arith are no longer deferred — v253/v248/v257/v258.)
                //
                // v314 (#211): `parse_subshell_sequence` is BESPOKE-shared by
                // both a real subshell `( … ` and a command substitution
                // `$( … ` — its own `UnterminatedSubshell` return can't tell
                // them apart. bash reports the two differently at EOF: a bare
                // `(` is Shape 2 (`unexpected end of file`), but `$(` is
                // Shape 3 (`unexpected EOF while looking for matching `)'`) —
                // verified against real bash 5.2.21. Re-map here, in the ONE
                // caller that knows it's the `$(` context.
                let pos = iter.cursor_pos();
                iter.pop_mode();
                let mapped = match e {
                    ParseError::UnsupportedCommand => ParseError::UnsupportedExpansion,
                    ParseError::UnterminatedSubshell => unterminated_cmdsub(pos),
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

/// Builds the Shape-3 `ParseError` for an unterminated `$( … ` command
/// substitution reaching real EOF before its closing `)` — bash's
/// `unexpected EOF while looking for matching `)'` (see the v314 (#211) note
/// at `parse_command_sub`'s two call sites, which both hit real EOF via the
/// shared `parse_subshell_sequence`/whitespace-only-body checks that can't
/// tell a `$(` apart from a bare `(`).
fn unterminated_cmdsub(pos: usize) -> ParseError {
    ParseError::Unexpected(ExpectFailure {
        found: Found::Eof,
        matching: Some(Delim::DollarParen),
        pos,
    })
}

/// Builds the Shape-3 `ParseError` for an unterminated `` `…` `` backtick
/// substitution reaching real EOF before its closing `` ` `` — bash's
/// `unexpected EOF while looking for matching ```'` (v314 Task 5, #211; see
/// the note at `parse_backtick_sub`'s raw-capture loop, the only raise site).
fn unterminated_backtick(pos: usize) -> ParseError {
    ParseError::Unexpected(ExpectFailure {
        found: Found::Eof,
        matching: Some(Delim::Backtick),
        pos,
    })
}

/// v251: assemble a `WordPart::ProcessSub` for a `<(…)`/`>(…)` process
/// substitution. Mirrors `parse_command_sub`: the body is a paren-delimited
/// command sequence lexed under `Mode::CommandSub` (the lexer's bare-`(` opener
/// path; the word-mode `ProcSubOpen` signal was already consumed by the caller).
/// `dir` comes from that signal.
pub(crate) fn parse_process_sub(iter: &mut Lexer, dir: ProcDir) -> Result<WordPart, ParseError> {
    iter.push_mode(Mode::CommandSub {
        body_started: false,
    });
    match iter.next_kind()? {
        Some(TokenKind::CmdSubOpen) => {} // the real opener, scanned under CommandSub mode
        _ => {
            iter.pop_mode();
            return Err(ParseError::UnsupportedExpansion);
        }
    }
    // Skip a leading run of Blank/Newline atoms before checking for an empty
    // body — same whitespace/newline-only-body rule as `parse_command_sub`
    // (`<( )`/`<(\n)` are empty process substitutions in bash, unlike an
    // explicit subshell).
    while matches!(
        iter.peek_kind()?,
        Some(TokenKind::Blank) | Some(TokenKind::Newline)
    ) {
        iter.next_kind()?;
    }
    // #109: same guard as parse_command_sub — a comment-only/empty body at EOF
    // is an unterminated process substitution.
    //
    // v314 Task 5 (#211): bash reports `<(`/`>(` at real EOF the same way it
    // reports `$(` — Shape 3 (`unexpected EOF while looking for matching
    // `)'`), not the bare-`(` subshell's Shape 2 (`unexpected end of file`) —
    // verified against real bash 5.2.21 (`bash -c 'cat <(foo'`). Reuse
    // `unterminated_cmdsub` (the same `$(`-style mapping `parse_command_sub`
    // uses) rather than the raw `UnterminatedSubshell`.
    if iter.peek_kind()?.is_none() {
        let pos = iter.cursor_pos();
        iter.pop_mode();
        return Err(unterminated_cmdsub(pos));
    }
    let sequence = if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::RParen))) {
        iter.next_kind()?; // consume `)`
        Sequence {
            first: Command::Pipeline(Pipeline {
                negate: false,
                commands: Vec::new(),
            }),
            rest: Vec::new(),
            background: false,
        }
    } else {
        match parse_subshell_sequence(iter) {
            Ok(mut seq) => {
                zero_lines_in_sequence(&mut seq);
                seq
            }
            Err(e) => {
                // v314 Task 5 (#211): same `$(`-vs-bare-`(` remap as
                // `parse_command_sub` — `parse_subshell_sequence` can't tell a
                // process-sub `<( … `/`>( … ` from a real subshell `( … `, but
                // bash reports the two differently at EOF.
                let pos = iter.cursor_pos();
                iter.pop_mode();
                let mapped = match e {
                    ParseError::UnsupportedCommand => ParseError::UnsupportedExpansion,
                    ParseError::UnterminatedSubshell => unterminated_cmdsub(pos),
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
/// True when a command word (its leading `name=(…)` compound assignment) may
/// appear in ARGUMENT position — bash's `ASSIGNMENT_BUILTIN` set plus the
/// hard-coded `eval`/`let` (see parse.y `read_token_word`, `PST_ASSIGNOK`).
fn is_compound_assignment_builtin(name: &str) -> bool {
    matches!(
        name,
        "declare" | "typeset" | "local" | "export" | "readonly" | "alias" | "eval" | "let"
    )
}

/// True when a word carries a `name=(…)` compound array-literal assignment (an
/// `ArrayLiteral` part). Used to gate its position: valid only as a leading
/// assignment or as an argument to a declaration builtin.
fn word_has_array_literal(w: &Word) -> bool {
    w.0.iter().any(|p| matches!(p, WordPart::ArrayLiteral(_)))
}

pub(crate) fn parse_array_literal(iter: &mut Lexer) -> Result<WordPart, ParseError> {
    iter.push_mode(Mode::ArrayLiteral {
        body_started: false,
        expect_subscript_eq: false,
        at_element_start: true,
        subscript_append: false,
    });
    let mut elements: Vec<ArrayLiteralElement> = Vec::new();
    loop {
        match iter.peek_kind()? {
            Some(TokenKind::Blank) | Some(TokenKind::Newline) => {
                iter.next_kind()?;
            }
            Some(TokenKind::ArrayClose) => {
                iter.next_kind()?;
                break;
            }
            // v252 T3: an explicit `[expr]=value` element. The lexer emitted a
            // zero-width `LBracket` at element start; assemble the subscript Word
            // under `Mode::ParamSubscriptOperand` (identical to the `${a[i]}`
            // reader at parse_param_expansion), then the lexer consumes the
            // required `=` (or errors `ArrayLiteralMissingEquals`) as we scan the
            // value. Subscripted values keep single-value semantics — NO brace
            // expansion (matches `scan_array_literal`).
            Some(TokenKind::LBracket) => {
                iter.next_kind()?; // consume LBracket
                iter.push_mode(Mode::ParamSubscriptOperand {
                    in_dquote: false,
                    enclosing_dquote: false,
                });
                let sub_word = match parse_word(iter, false) {
                    Ok(w) => w,
                    Err(e) => {
                        iter.pop_mode();
                        iter.pop_mode();
                        return Err(e);
                    }
                };
                match iter.next_kind() {
                    Ok(Some(TokenKind::RBracket)) => {}
                    Ok(_) => {
                        iter.pop_mode();
                        iter.pop_mode();
                        return Err(ParseError::UnsupportedExpansion);
                    }
                    Err(e) => {
                        iter.pop_mode();
                        iter.pop_mode();
                        return Err(ParseError::Lex(Box::new(e)));
                    }
                }
                iter.pop_mode(); // ParamSubscriptOperand
                // The lexer consumes the required `=` / `+=` (or errors
                // ArrayLiteralMissingEquals) as `parse_word_command` scans on.
                let value = match parse_word_command(iter, false) {
                    Ok(v) => v,
                    Err(e) => {
                        iter.pop_mode();
                        return Err(e);
                    }
                };
                // The lexer recorded whether the operator was `+=` (append) on
                // the ArrayLiteral mode; read it now, before the next element's
                // scan can overwrite it. Nothing between the operator scan and
                // here touches this flag.
                let append = matches!(
                    iter.current_mode(),
                    Mode::ArrayLiteral {
                        subscript_append: true,
                        ..
                    }
                );
                // An empty value (`[i]= ` / `[i]=)`) re-tokenizes to a single
                // empty literal in the oracle (`scan_array_element_word`'s
                // `words.is_empty()` fallback), NOT an empty Word.
                let value = if value.0.is_empty() {
                    Word(vec![WordPart::Literal {
                        text: String::new(),
                        quoted: false,
                    }])
                } else {
                    value
                };
                elements.push(ArrayLiteralElement {
                    subscript: Some(sub_word),
                    value,
                    append,
                });
            }
            Some(_) => {
                // A positional value: parse_word_command stops at the next
                // Blank/Newline/ArrayClose (its catch-all arm breaks WITHOUT
                // consuming once something has been accumulated — see its doc
                // comment). Then brace-expand (bare elements only).
                let value = match parse_word_command(iter, false) {
                    Ok(v) => v,
                    Err(e) => {
                        iter.pop_mode();
                        return Err(e);
                    }
                };
                match brace_expand_parts(value.0) {
                    Ok(expansions) => {
                        for p in expansions {
                            elements.push(ArrayLiteralElement {
                                subscript: None,
                                value: Word(p),
                                append: false,
                            });
                        }
                    }
                    Err(e) => {
                        iter.pop_mode();
                        return Err(ParseError::Lex(Box::new(e)));
                    }
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

/// The empty `Sequence` an empty command substitution (`` `` `` / `$()`)
/// yields — a `Pipeline` with no commands.
fn empty_sequence() -> Sequence {
    Sequence {
        first: Command::Pipeline(Pipeline {
            negate: false,
            commands: Vec::new(),
        }),
        rest: Vec::new(),
        background: false,
    }
}

/// Assemble a `WordPart::CommandSub` for a `` `…` `` backtick substitution using
/// bash's three-phase model (v274):
///
/// 1. **Raw capture** — push the dumb `Mode::BacktickRaw` and stream the body
///    verbatim (`BacktickRawText` chunks between `BeginBacktick`/`EndBacktick`),
///    concatenating into `raw`.  Quote-blind and `$()`-blind: the close is a bare
///    backtick under one-char `\` lookahead.
/// 2. **One-level unescape** — `unescape_backtick_body` removes the backslash for
///    exactly `\\`, `\$`, `` \` ``; every other `\c` is kept verbatim.
/// 3. **Recursive re-parse** — a FRESH sub-lexer over the cooked body (inheriting
///    the parent's aliases + opts, but with `in_dquote` cleared) is parsed by
///    `parse_sequence`.  Nesting and `$()` fall out of this recursion.
///
/// Always owns the push/pop of its own `BacktickRaw` frame (nesting is handled by
/// phase 3, not by a shared depth counter); pops on ALL exit paths.
pub(crate) fn parse_backtick_sub(iter: &mut Lexer, quoted: bool) -> Result<WordPart, ParseError> {
    // Phase 1 — capture the raw body under the dumb BacktickRaw mode.  The
    // fallible body runs in an immediately-invoked closure so EVERY exit path
    // (including a `?`-propagated LexError) flows through the single `pop_mode`.
    iter.push_mode(Mode::BacktickRaw);
    let raw = (|| -> Result<String, ParseError> {
        match iter.next_kind()? {
            Some(TokenKind::BeginBacktick) => {}
            _ => return Err(ParseError::UnsupportedExpansion),
        }
        let mut raw = String::new();
        loop {
            match iter.next_kind()? {
                Some(TokenKind::BacktickRawText(s)) => raw.push_str(&s),
                Some(TokenKind::EndBacktick) => return Ok(raw),
                // v314 Task 5 (#211): bash reports an unterminated `` `...` ``
                // at real EOF as Shape 3 (`unexpected EOF while looking for
                // matching ```'`) — verified against real bash 5.2.21
                // (`bash -c 'echo `foo'`). Mirror `unterminated_cmdsub`'s
                // `$(`-style mapping with `Delim::Backtick`.
                None => return Err(unterminated_backtick(iter.cursor_pos())),
                _ => return Err(ParseError::UnsupportedExpansion),
            }
        }
    })();
    iter.pop_mode();
    let raw = raw?;

    // Phase 2 — one-level unescape.
    let cooked = crate::lexer::unescape_backtick_body(&raw);

    // Phase 3 — re-parse the cooked body as a command Sequence with a FRESH
    // lexer.  `in_dquote` is cleared: the body is its own context even inside
    // `"`...`"`.
    let mut sub_opts = iter.opts();
    sub_opts.in_dquote = false;
    let mut sub = Lexer::new(&cooked, iter.aliases(), sub_opts);
    let sequence = match parse_sequence(&mut sub) {
        Ok(Some(mut seq)) => {
            zero_lines_in_sequence(&mut seq);
            seq
        }
        Ok(None) => empty_sequence(), // `` `` `` — same empty Sequence as before.
        // v316 (#213): wrap the body-relative error so the engine renderer
        // can report bash's `command substitution:` marker + echo the
        // backtick body (not the outer line) instead of falling through to
        // the generic `-c:` rendering.
        Err(inner) => {
            let err_pos = sub.cursor_pos();
            return Err(ParseError::InCommandSub {
                inner: Box::new(inner),
                body: cooked,
                err_pos,
            });
        }
    };
    Ok(WordPart::CommandSub { sequence, quoted })
}

/// Assemble a `WordPart::Arith` for a `$(( … ))` arithmetic expansion.
///
/// Pushes `Mode::Arith { paren_depth: 0, in_squote: false, in_dquote: false, body_started: false, for_header: false, delim: ArithDelim::Paren }`;
/// the mode's first scan consumes the opening `$((` and emits `ArithOpen`.  The
/// parser assembles the body `Word` (literal runs + embedded expansions), stops on
/// `ArithClose`, and on `ArithBail` rewinds to the `$((` start and re-drives as a
/// command substitution of a subshell (`$( (…) )`).  Owns the push/pop lifecycle;
/// pops the `Arith` frame on ALL exit paths.
enum ArithBodyOutcome {
    Closed(Word),
    Bail,
}

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
            Some(TokenKind::ArithClose) => {
                iter.next_kind()?;
                return Ok(ArithBodyOutcome::Closed(Word(parts)));
            }
            Some(TokenKind::ArithBail) => {
                return Ok(ArithBodyOutcome::Bail);
            } // Task 5 consumes/rewinds
            Some(TokenKind::ParamOpen { .. }) => {
                parts.push(parse_param_expansion(iter, true)?);
            }
            Some(TokenKind::CmdSubOpen) => {
                iter.next_kind()?;
                parts.push(parse_command_sub(iter, true)?);
            }
            Some(TokenKind::BeginBacktick) => {
                iter.next_kind()?;
                parts.push(parse_backtick_sub(iter, true)?);
            }
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
            Some(TokenKind::ArithOpen) => {
                iter.next_kind()?;
                parts.push(parse_arith_expansion(iter, true)?);
            }
            Some(TokenKind::LegacyArithOpen) => {
                iter.next_kind()?;
                parts.push(parse_legacy_arith_expansion(iter, true)?);
            }
            Some(TokenKind::Lit { .. }) => {
                if let Some(TokenKind::Lit { text, quoted }) = iter.next_kind()? {
                    parts.push(WordPart::Literal { text, quoted });
                }
            }
            Some(TokenKind::DollarName { .. }) => {
                if let Some(TokenKind::DollarName { name, quoted }) = iter.next_kind()? {
                    let part = match name.as_str() {
                        "@" => WordPart::AllArgs {
                            quoted,
                            joined: false,
                        },
                        "*" => WordPart::AllArgs {
                            quoted,
                            joined: true,
                        },
                        "?" => WordPart::LastStatus { quoted },
                        _ => WordPart::Var { name, quoted },
                    };
                    parts.push(part);
                }
            }
            _ => return Err(ParseError::UnsupportedExpansion),
        }
    }
}

pub(crate) fn parse_arith_expansion(
    iter: &mut Lexer,
    quoted: bool,
) -> Result<WordPart, ParseError> {
    // Mark BEFORE pushing the Arith mode / consuming the `$((` opener, so an
    // `ArithBail` rewind returns to the `$((` start with the pre-push mode stack
    // (mark captures `self.modes`). `parse_arith_expansion` is always called at a
    // pull boundary (the parser dispatches on a peeked opener), so mark/rewind's
    // pull-boundary assert holds.
    let mark = iter.mark();
    iter.push_mode(Mode::Arith {
        paren_depth: 0,
        in_squote: false,
        in_dquote: false,
        body_started: false,
        for_header: false,
        delim: ArithDelim::Paren,
    });
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

/// v258: assemble a `WordPart::Arith` for a `$[ … ]` legacy arithmetic expansion
/// (bash treats `$[ expr ]` as exactly `$(( expr ))`). Mirrors
/// `parse_arith_expansion` but with `delim: Bracket` and WITHOUT the bail path:
/// `$[` closes on a single depth-0 `]` (`ArithClose`) — there is no `$( (` wrinkle,
/// so no `mark`/`rewind`. The mode's first scan consumes `$[` and emits
/// `LegacyArithOpen`; `parse_arith_body` assembles the body and returns `Closed` on
/// `ArithClose`.
pub(crate) fn parse_legacy_arith_expansion(
    iter: &mut Lexer,
    quoted: bool,
) -> Result<WordPart, ParseError> {
    iter.push_mode(Mode::Arith {
        paren_depth: 0,
        in_squote: false,
        in_dquote: false,
        body_started: false,
        for_header: false,
        delim: ArithDelim::Bracket,
    });
    let result = (|| -> Result<ArithBodyOutcome, ParseError> {
        match iter.next_kind()? {
            Some(TokenKind::LegacyArithOpen) => {}
            _ => return Err(ParseError::UnsupportedExpansion),
        }
        parse_arith_body(iter, quoted)
    })();
    iter.pop_mode();
    match result? {
        ArithBodyOutcome::Closed(body) => Ok(WordPart::Arith { body, quoted }),
        // `$[` has no bail path (single-`]` close, no `$( (` wrinkle); a Bail here
        // would mean the lexer emitted an ArithBail in Bracket mode, which it never
        // does. Treat defensively as an unsupported expansion.
        ArithBodyOutcome::Bail => Err(ParseError::UnsupportedExpansion),
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
    iter.push_mode(Mode::Arith {
        paren_depth: 0,
        in_squote: false,
        in_dquote: false,
        body_started: true,
        for_header: false,
        delim: ArithDelim::Paren,
    });
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
            Some(TokenKind::Newline | TokenKind::Blank) => {
                iter.next_kind()?;
            }
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
    // #86: only peek for a heredoc body when one is actually pending. `&&`
    // short-circuits, so with no pending heredoc `peek_kind()` is never called
    // and the next unit's first token is not scanned — a unit-terminating
    // newline ends the unit cleanly instead of over-scanning into (and failing
    // on) a following unit that begins with a lex error.
    while iter.has_pending_heredoc_body()
        && matches!(iter.peek_kind()?, Some(TokenKind::HeredocBodyBegin { .. }))
    {
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
        _ => unreachable!(
            "lexer emits a complete heredoc body group beginning with HeredocBodyBegin"
        ),
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
            Some(TokenKind::HeredocBodyEnd) => {
                iter.next_kind()?;
                break;
            }
            Some(TokenKind::Lit { .. }) => {
                if let Some(TokenKind::Lit { text, quoted }) = iter.next_kind()? {
                    push_heredoc_literal_lines(&mut parts, &text, quoted);
                }
            }
            _ => {
                unreachable!("lexer emits a complete literal heredoc body group (one Lit then End)")
            }
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
            Some(TokenKind::HeredocBodyEnd) => {
                iter.next_kind()?;
                break;
            }
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
            Some(TokenKind::LegacyArithOpen) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_legacy_arith_expansion(iter, true)?);
            }
            Some(TokenKind::DollarName { .. }) => {
                if let Some(TokenKind::DollarName { name, quoted: _ }) = iter.next_kind()? {
                    flush_lit(&mut acc, &mut parts);
                    parts.push(match name.as_str() {
                        "@" => WordPart::AllArgs {
                            quoted: true,
                            joined: false,
                        },
                        "*" => WordPart::AllArgs {
                            quoted: true,
                            joined: true,
                        },
                        "?" => WordPart::LastStatus { quoted: true },
                        _ => WordPart::Var { name, quoted: true },
                    });
                }
            }
            Some(TokenKind::DollarLit { .. }) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                parts.push(WordPart::Literal {
                    text: "$".into(),
                    quoted: true,
                });
            }
            // v258: `$[expr]` legacy arith inside the body is handled by the
            // `LegacyArithOpen` arm above, not this catch-all. Other still-deferred
            // constructs inside an expanding heredoc body fall through here.
            Some(TokenKind::DeferredExpansion) => return Err(ParseError::UnsupportedExpansion),
            // Defense-in-depth: the lexer is expected to emit only body-part atoms
            // between `HeredocBodyBegin` and `HeredocBodyEnd`, but a malformed
            // heredoc-in-comsub/backtick construct must NEVER panic. Surface a clean
            // parse error instead of `unreachable!`.
            _ => {
                return Err(ParseError::Lex(Box::new(
                    crate::lexer::LexError::UnterminatedHeredoc,
                )));
            }
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
        parts.push(WordPart::Literal {
            text: line.to_string(),
            quoted,
        });
        parts.push(WordPart::Literal {
            text: "\n".to_string(),
            quoted,
        });
        rest = &tail[1..];
    }
    // A trailing fragment with no newline shouldn't occur for a literal
    // heredoc body (the lexer always appends '\n' after every content line),
    // but guard defensively rather than silently drop trailing text.
    if !rest.is_empty() {
        parts.push(WordPart::Literal {
            text: rest.to_string(),
            quoted,
        });
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
        TokenKind::Lit {
            text,
            quoted: false,
        } => text == "!",
        _ => false,
    }
}

/// Reserved-word kinds.  Mirrors `command.rs`'s `Keyword` exactly so that
/// Tasks 2–7 can share the same stop-at sets and function signatures.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum Keyword {
    If,
    Then,
    Elif,
    Else,
    Fi,
    While,
    Until,
    Do,
    Done,
    For,
    In,
    Case,
    Esac,
    LBrace,
    RBrace,
    DoubleBracketOpen,  // `[[`
    DoubleBracketClose, // `]]`
    Function,
    Select,
    Coproc,
}

/// Returns the keyword a `TokenKind` represents, or `None`.  A token is a
/// keyword only when it is a `Word` of exactly one part — an *unquoted*
/// `Literal` whose text equals the keyword.  Mirrors `keyword_of` in
/// `command.rs`.
pub(crate) fn keyword_kind(token: &TokenKind) -> Option<Keyword> {
    let TokenKind::Word(Word(parts)) = token else {
        return None;
    };
    if parts.len() != 1 {
        return None;
    }
    let WordPart::Literal {
        text,
        quoted: false,
    } = &parts[0]
    else {
        return None;
    };
    keyword_from_str(text)
}

/// The SINGLE keyword table: maps a reserved-word text to its `Keyword`.  Both
/// the Word-token recognizer (`keyword_kind`) and the atom-stream recognizer
/// (`peek_leading_keyword`) delegate here so there is exactly one source of
/// truth.  Mirrors `command.rs`'s `keyword_of` text match.
pub(crate) fn keyword_from_str(text: &str) -> Option<Keyword> {
    match text {
        "if" => Some(Keyword::If),
        "then" => Some(Keyword::Then),
        "elif" => Some(Keyword::Elif),
        "else" => Some(Keyword::Else),
        "fi" => Some(Keyword::Fi),
        "while" => Some(Keyword::While),
        "until" => Some(Keyword::Until),
        "do" => Some(Keyword::Do),
        "done" => Some(Keyword::Done),
        "for" => Some(Keyword::For),
        "in" => Some(Keyword::In),
        "case" => Some(Keyword::Case),
        "esac" => Some(Keyword::Esac),
        "{" => Some(Keyword::LBrace),
        "}" => Some(Keyword::RBrace),
        "[[" => Some(Keyword::DoubleBracketOpen),
        "]]" => Some(Keyword::DoubleBracketClose),
        "function" => Some(Keyword::Function),
        "select" => Some(Keyword::Select),
        "coproc" => Some(Keyword::Coproc),
        _ => None,
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
        Some(TokenKind::Lit {
            text,
            quoted: false,
        }) => keyword_from_str(text),
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
            // v264: a closing keyword (`}`/`done`/`fi`/`esac`) immediately before
            // the backtick-body terminator (`` `…done` ``) is at a boundary —
            // EndBacktick is the backtick analogue of `Op(RParen)` for `$( … )`.
            | Some(TokenKind::EndBacktick)
    );
    Ok(if boundary { Some(kw) } else { None })
}

/// Consume the next command word, assembling it from atoms via
/// `parse_word_command` (atom path) or taking a legacy `TokenKind::Word` token
/// whole (non-atom path).  Callers that expect a keyword here must have already
/// verified it via `peek_leading_keyword` (which also skips leading blanks).
fn consume_command_word(iter: &mut Lexer) -> Result<Word, ParseError> {
    // Recovery cursor context (Task 4): a word assembled here is command-position
    // (the leading command word, an `if`/`while` condition command, a for/case
    // subject, or a keyword). No-op off the recovery path.
    iter.set_recovery_cmd_word(true);
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
        TokenKind::Lit {
            text,
            quoted: false,
        } => keyword_from_str(text),
        _ => None,
    }
}

/// Extract a `for`/`select` loop-variable name from an assembled `Word`: it must
/// be a single unquoted `Literal`.
fn for_variable_name_word(w: &Word) -> Option<String> {
    if w.0.len() != 1 {
        return None;
    }
    let WordPart::Literal {
        text,
        quoted: false,
    } = &w.0[0]
    else {
        return None;
    };
    if text.is_empty() {
        return None;
    }
    Some(text.clone())
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
                Some(TokenKind::Heredoc {
                    expand, strip_tabs, ..
                }) => (expand, strip_tabs),
                _ => unreachable!("peek confirmed Heredoc"),
            };
            Ok(vec![Redirection {
                fd: fd_prefix.unwrap_or(crate::command::RedirFd::Number(0)),
                op: crate::command::RedirOp::Heredoc {
                    body: Word(vec![]),
                    expand,
                    strip_tabs,
                },
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
            // Recovery cursor context (iteration 2): the word assembled here is a
            // bare redirect operand. A redirect target is never command-position,
            // so clear the command-word flag (an operator-glued redirect like
            // `echo >whi` leaves it `true` from the preceding command word); the
            // `RedirectTarget` position then replaces the `Argument` fallback. If
            // the cursor (EOF) falls inside an inner expansion (`> $HOM`, `> $(whi`)
            // that inner-mode position wins instead. Reset once the redirect is
            // built so a following argument word is not misread. No-op off the
            // recovery path.
            iter.set_recovery_cmd_word(false);
            iter.set_recovery_redirect_target(true);
            let target = match iter.peek_kind()? {
                Some(TokenKind::Op(_)) => return Err(ParseError::RedirectTargetIsOperator),
                Some(TokenKind::Newline) => {
                    return Err(ParseError::Unexpected(iter.unexpected_here(None)?));
                }
                // v314 (#211): bash spells a MISSING redirect target at real
                // EOF (no more input at all, not even a newline — the common
                // `-c`-string shape) the SAME as an explicit trailing newline:
                // `near unexpected token 'newline'` (verified against real
                // bash 5.2.21: `bash -c 'echo <>'` — Shape 1, not the generic
                // "unexpected end of file" every OTHER trailing-operator-at-
                // EOF site reports). Synthesize a `Newline` token rather than
                // deferring to `unexpected_here`'s `Found::Eof`, which the
                // renderer would classify as Shape 2 instead.
                None => {
                    return Err(ParseError::Unexpected(ExpectFailure {
                        found: Found::Token(TokenKind::Newline),
                        matching: None,
                        pos: iter.cursor_pos(),
                    }));
                }
                Some(TokenKind::Heredoc { .. }) => {
                    return Err(ParseError::RedirectTargetIsOperator);
                }
                Some(TokenKind::RedirFd(_)) => return Err(ParseError::RedirectTargetIsOperator),
                Some(TokenKind::ArithBlock(..)) => {
                    return Err(ParseError::RedirectTargetIsOperator);
                }
                Some(TokenKind::Word(_)) => match iter.next_kind()? {
                    Some(TokenKind::Word(word)) => word,
                    _ => unreachable!("peek confirmed Word"),
                },
                Some(_) => parse_word_command(iter, false)?,
            };
            iter.set_recovery_redirect_target(false);
            Ok(crate::command::build_redirections(op, target, fd_prefix))
        }
        _ => {
            // A bare fd-prefix with no following redirect operator: defensively
            // guard (the lexer only emits RedirFd glued to an op, but be safe).
            if fd_prefix.is_some() {
                return Err(ParseError::Unexpected(iter.unexpected_here(None)?));
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

/// v264 flip-fix (Finding 1): brace-expand an assembled COMMAND word and push
/// the 1→N products onto `dest`, gated on the lexer's brace-expand flag
/// (`set +B` disables it). Mirrors the oracle's `emit_word_with_braces`
/// (lexer.rs), which brace-expands command words (program + args) and for/select
/// in-list words at lex time — BEFORE the parser splits program/args or peels
/// assignments — emitting N `Word` tokens. Quoted/escaped braces (`"{a,b}"`,
/// `a\{b\}`) and braces inside expansions (`${x:-{a,b}}`) stay literal because
/// `brace_expand_parts` sentinel-protects non-literal / quoted parts, so this
/// reuse gets that for free. Applied ONLY at the command-assembly level (here and
/// the for/select in-list collection), NEVER inside `parse_word_command` — that
/// helper is SHARED by `[[ … ]]` operands and `case` patterns, which the oracle
/// scans through paths that do NOT call `emit_word_with_braces`.
fn push_command_word_brace_expanded(
    dest: &mut Vec<Word>,
    word: Word,
    iter: &Lexer,
) -> Result<(), ParseError> {
    if !iter.brace_expand_enabled() {
        dest.push(word);
        return Ok(());
    }
    for parts in brace_expand_parts(word.0)? {
        dest.push(Word(parts));
    }
    Ok(())
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
        // v264 flip-fix (Finding 1): the program word brace-expands too, matching
        // the oracle (which emits N Words at lex time BEFORE program/arg split).
        // The FIRST product becomes the program, the rest leading args — the
        // program/arg split below reads `all_words[0]`=program, remainder=args.
        push_command_word_brace_expanded(&mut all_words, w, iter)?;
    }

    loop {
        // Trailing-blank alias chain: mirrors the oracle's arg-loop hook
        // (`command.rs:2173-2175`). If the last command-position alias
        // expansion ended with a blank, the next argument word is eligible
        // for alias expansion too. `take_trailing_eligible()` returns `true`
        // at most once per expansion (it resets the flag), so this is a
        // no-op in the overwhelmingly common non-alias/non-trailing-blank
        // case.
        //
        // The oracle's Word-token stream has no atom for inter-word
        // whitespace (it's silently absorbed between tokens), so its hook
        // always lands with the cursor ON the next real word. The atom
        // stream DOES have an explicit `Blank` atom for a 2nd+ argument (the
        // 1st argument's leading blank is already consumed by the caller —
        // `parse_command`'s `consume_command_word` + blank-skip, and by
        // `peek_leading_keyword`'s own leading-blank skip). Consulting
        // `take_trailing_eligible()` while sitting ON that `Blank` would
        // consume (and waste — `maybe_expand_command_alias` unconditionally
        // resets the flag) the one-shot flag before reaching the real word,
        // so defer to the iteration where the `Blank` has already been
        // skipped (below) and the cursor sits on the real next atom.
        let at_blank = matches!(iter.peek_kind()?, Some(TokenKind::Blank));
        if !at_blank && iter.take_trailing_eligible() {
            iter.expand_command_alias()?;
        }
        let Some(token) = iter.peek_kind()? else {
            break;
        };
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
        // v274: the OLD single-frame `Mode::Backtick` depth-tracking scanner used
        // to leave the lexer mid-body with a REAL `BeginBacktick` child-open token
        // reachable here (nested `` \` `` inside a backtick BODY, recursed into
        // directly by this generic word loop). That mode is gone: `parse_backtick_sub`
        // now captures the raw body under `Mode::BacktickRaw` (never delegating to
        // this loop) and any nesting falls out of the phase-3 re-parse hitting the
        // ordinary top-level `BeginBacktick` ZERO-WIDTH signal instead (handled by
        // `parse_word_command`'s BeginBacktick arm). So there is no longer a
        // reachable `BeginBacktick` token at this point in the token stream.
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
                    | TokenKind::LegacyArithOpen
                    | TokenKind::Tilde { .. }
                    | TokenKind::BeginDquote
                    | TokenKind::AssignPrefix { .. }
                    | TokenKind::ExtglobOpen { .. }
            )
        ) {
            // v264 flip-fix (Finding 1): argument command words brace-expand
            // (1→N Words), matching the oracle's lex-time `emit_word_with_braces`.
            // Recovery cursor context (Task 4): this is an argument-position word.
            iter.set_recovery_cmd_word(false);
            let w = parse_word_command(iter, false)?;
            push_command_word_brace_expanded(&mut all_words, w, iter)?;
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
                if all_words
                    .last()
                    .is_some_and(crate::command::is_assignment_word)
                {
                    return Err(ParseError::UnsupportedCommand);
                }
                // `kind` (the `(` itself) was already consumed above (line
                // 2951) before this match ran, so `iter.unexpected_here` would
                // report the token AFTER the `(` instead — build the
                // `ExpectFailure` directly from the already-owned `kind`.
                return Err(ParseError::Unexpected(ExpectFailure {
                    found: Found::Token(kind.clone()),
                    matching: None,
                    pos: iter.cursor_pos(),
                }));
            }
            _ => return Err(ParseError::UnsupportedCommand),
        }
    }

    if all_words.is_empty() && redirects.is_empty() {
        // A real token (operator) sitting where a command was expected — the
        // word loop above breaks WITHOUT consuming at any stage/list
        // terminator (`|`/`;`/`&&`/`||`/`&`/`)`/`;;`/`;&`/`;;&`/newline) or
        // EOF, so the cursor is still ON that token here. If it's a real
        // token, bash names it (Shape 1); if it's EOF, this falls through to
        // the plain `MissingCommand` (handled by the unterminated-construct
        // fallback elsewhere).
        if iter.peek_kind()?.is_some() {
            return Err(ParseError::Unexpected(iter.unexpected_here(None)?));
        }
        return Err(ParseError::MissingCommand);
    }

    // A `name=(…)` array-literal (compound assignment) is only valid in an
    // assignment-acceptable position, matching bash's parser: as a LEADING
    // assignment (every preceding word is assignment-shaped, i.e. still in the
    // command-word prefix), or as an argument to a declaration builtin — bash's
    // `ASSIGNMENT_BUILTIN` set (declare/typeset/local/export/readonly/alias)
    // plus the hard-coded `eval`/`let` (parse.y `read_token_word`,
    // `PST_ASSIGNOK`). Anywhere else bash rejects the unexpected `(` with a
    // syntax error (`printf … a=(a b)` → rc 2). The lexer emits `ArrayOpen`
    // wherever it sees `name=(` at a word start regardless of position, so this
    // is the parser's job (delimiter/position ownership stays with the parser).
    let command_word_is_decl_builtin = all_words
        .iter()
        .find(|w| !crate::command::is_assignment_word(w))
        .and_then(|w| crate::command::word_literal_text(w))
        .is_some_and(is_compound_assignment_builtin);
    for (i, w) in all_words.iter().enumerate() {
        if !word_has_array_literal(w) {
            continue;
        }
        let in_leading_prefix = all_words[..i]
            .iter()
            .all(crate::command::is_assignment_word);
        if !in_leading_prefix && !command_word_is_decl_builtin {
            // v314 (#211) NOT migrated: bash names the array literal's OWN
            // `(` here (`echo x=(1 2)` -> `near unexpected token '('`), but
            // that token was consumed well before this point — inside the
            // `parse_word_command`/`parse_array_literal` call that produced
            // `w` (this loop runs AFTER the whole word list is assembled).
            // `iter.unexpected_here` at this point would report whatever
            // token comes NEXT in the stream, not the `(` — capturing the
            // right token needs threading its position out of the array-
            // literal parse, which is out of scope for this task (no harness
            // case exercises this site). Left as the untyped `UnexpectedToken`
            // (handled by the renderer's descriptive fallback).
            return Err(ParseError::UnexpectedToken);
        }
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
        return Ok(Command::Simple(SimpleCommand::Assign(
            inline_assignments,
            line,
        )));
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
/// All other keywords are terminators/closers that can never start a
/// command → `UnexpectedKeyword` (v257 T3 fix; see the match arm below).
fn parse_command(iter: &mut Lexer) -> Result<Command, ParseError> {
    // Skip leading newlines (mirrors `parse_command_inner` command.rs:1019).
    skip_newlines(iter)?;
    // Read-time alias expansion at command position (mirrors the oracle's
    // `command.rs:1020` + `:2302` sites — this ONE choke point covers both,
    // since `parse_pipeline` calls `parse_command` for the first stage and
    // `finish_pipeline` calls it for every subsequent stage). Must run BEFORE
    // the ArithBlock/LParen/keyword dispatch below so `alias x=if` expands to
    // the reserved word before the reserved-word check runs.
    iter.expand_command_alias()?;
    // EOF with no token. v314 (#211): NOT a near-token guard candidate — this
    // branch's own condition IS the EOF check (`peek_kind().is_none()`), so a
    // "real token present" guard here would never fire; stays `MissingCommand`
    // (handled by the Shape 2/3 unterminated-construct fallback upstream).
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
        Some(Keyword::If) => return parse_if(iter),
        Some(Keyword::While) | Some(Keyword::Until) => return parse_while(iter),
        Some(Keyword::For) => return parse_for(iter),
        Some(Keyword::Select) => return parse_select(iter),
        Some(Keyword::Case) => return parse_case(iter),
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
        Some(Keyword::Coproc) => {
            let cmd = parse_coproc(iter)?;
            return maybe_wrap_redirects(cmd, iter);
        }
        // Every `Keyword` variant is either dispatched above or is a bare
        // terminator/closer that can never legally start a command (`then`,
        // `elif`, `else`, `fi`, `do`, `done`, `in`, `esac`, `}`, `]]`) — the
        // oracle's fallthrough arm (`command.rs` `parse_command_inner`,
        // `Some(other) => Err(UnexpectedKeyword(other.name()))`) raises the
        // SAME error for all of them, not a generic deferral. v257 T3 found
        // this returning `UnsupportedCommand` instead (originally discovered via
        // `coproc 123 { :; }` when a non-identifier coproc name forced the
        // anonymous body path and `parse_command` read the stray `123 {` as a
        // simple command up to `;`, then hit the unmatched `}`; the coproc name
        // rule now accepts any single non-keyword word, so `coproc 123 { :; }`
        // parses as named and no longer reaches here — but the arm's behaviour
        // is unchanged and still correct for genuine terminator keywords).
        Some(_) => return Err(ParseError::Unexpected(iter.unexpected_here(None)?)),
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
        // Recovery cursor context (Gap A, iteration 2): the command word is now
        // fully consumed — the parser next expects an ARGUMENT. `consume_command_word`
        // left the flag `true` (command position); flip it so an EMPTY trailing
        // word at the whitespace boundary (`echo `, `ls `) reports `Argument`, not
        // `Command`. When the cursor is still ON the command word (`whi`, `echo $(whi`)
        // the EOF capture was already taken INSIDE `consume_command_word`'s
        // terminating peek with the flag `true`, so this is moot there. No-op off
        // the recovery path.
        iter.set_recovery_cmd_word(false);
        while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
            iter.next_kind()?;
        }
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

/// A `Word` consisting of a single unquoted `Literal` — used to run
/// `valid_identifier_text` against a peeked bare `Lit` atom without consuming it.
fn single_lit_word(text: &str) -> Word {
    Word(vec![WordPart::Literal {
        text: text.to_string(),
        quoted: false,
    }])
}

/// True if `k` is a compound-command opener keyword (the keyword half of the
/// oracle's `is_compound_opener`: `{`, if/while/until/for/case/select, `[[`).
/// `(`/`((` are `Op(LParen)` and are checked separately by the caller.
fn is_compound_opener_kw(k: Keyword) -> bool {
    matches!(
        k,
        Keyword::LBrace
            | Keyword::If
            | Keyword::While
            | Keyword::Until
            | Keyword::For
            | Keyword::Case
            | Keyword::Select
            | Keyword::DoubleBracketOpen
    )
}

/// Word-boundary set used by `peek_leading_keyword` (a following non-boundary
/// atom means the previous `Lit` has more parts and is not a bare keyword).
fn is_word_boundary_tok(t: Option<&TokenKind>) -> bool {
    matches!(
        t,
        None | Some(TokenKind::Blank)
            | Some(TokenKind::Newline)
            | Some(TokenKind::Op(_))
            | Some(TokenKind::RedirFd(_))
            | Some(TokenKind::Heredoc { .. })
            // v264: backtick-body terminator, the analogue of `Op(RParen)`.
            | Some(TokenKind::EndBacktick)
    )
}

/// Non-consuming decision for `coproc NAME <compound>` (the oracle's
/// `peek(Word valid_ident) && is_compound_opener(peek2)`): the leading word is a
/// bare valid-identifier `Lit` AND the token immediately after it starts a
/// compound command. Bounded lookahead (peek 0..3); consumes nothing. The caller
/// has already consumed `coproc` and skipped blanks.
fn peek_coproc_named(iter: &mut Lexer) -> Result<bool, ParseError> {
    // word1 must be a bare valid-identifier Lit (atom path) or a legacy Word
    // token (fat-lexer path, e.g. inside $(...)/backticks).
    let text = match iter.peek_kind()? {
        // Legacy fat-lexer path: the NAME arrives as a single `Word` token
        // with no `Blank` before the opener. Mirror the oracle's
        // `parse_coproc_command` exactly.
        Some(TokenKind::Word(w)) => {
            // bash parses `coproc WORD compound-command` for ANY word as the
            // name and defers the valid-identifier check to RUNTIME. The NAME is
            // any single, non-keyword word (`valid_function_name_text`, NOT the
            // stricter `valid_identifier_text`) — a keyword like `{`/`if`/`while`
            // is instead the anonymous compound command (`coproc <compound>`).
            let named = crate::command::valid_function_name_text(w).is_some();
            if !named {
                return Ok(false);
            }
            return Ok(is_compound_opener(iter.peek2_kind()?));
        }
        Some(TokenKind::Lit {
            text,
            quoted: false,
        }) => text.clone(),
        _ => return Ok(false),
    };
    // Any single non-keyword word is a NAME candidate (identifier validity is a
    // RUNTIME check in bash, e.g. `coproc @ { :; }` parses); a keyword word is the
    // anonymous form's compound opener and must fall through to `Ok(false)`.
    if crate::command::valid_function_name_text(&single_lit_word(&text)).is_none() {
        return Ok(false);
    }
    // Examine the token AFTER word1.
    match iter.peek2_kind()? {
        // Glued compound opener: `MYP(...)`.
        Some(TokenKind::Op(Operator::LParen)) => Ok(true),
        // Space, then a compound opener at peek(2).
        Some(TokenKind::Blank) => {
            // peek_nth_kind(2): `(`/`((` or a compound keyword.
            let two = match iter.peek_nth_kind(2)? {
                Some(TokenKind::Op(Operator::LParen)) => return Ok(true),
                Some(TokenKind::Lit {
                    text,
                    quoted: false,
                }) => text.clone(),
                _ => return Ok(false),
            };
            match keyword_from_str(&two) {
                Some(k) if is_compound_opener_kw(k) => {
                    // Boundary after the keyword (mirrors peek_leading_keyword).
                    Ok(is_word_boundary_tok(iter.peek_nth_kind(3)?))
                }
                _ => Ok(false),
            }
        }
        // A word-continuation (multi-part word) or a non-opener boundary → anonymous.
        _ => Ok(false),
    }
}

/// Parse a coproc body, reproducing the oracle's `parse_command_inner`: a simple
/// command is extended into a pipeline (`|` consumed, wrapped in `Pipeline`); a
/// compound command is returned alone (a trailing `|` is left to the OUTER
/// pipeline, so `coproc { a; } | cat` becomes `Pipeline[Coproc{..}, cat]`); a
/// leading `!` is the program name, not negation (`parse_command` does not strip
/// it).
fn parse_coproc_body(iter: &mut Lexer) -> Result<Command, ParseError> {
    skip_newlines(iter)?;
    let first = parse_command(iter)?;
    if matches!(first, Command::Simple(_)) {
        finish_pipeline(iter, first, false, false)
    } else {
        Ok(first)
    }
}

/// Parse a `coproc [NAME] command`. The caller (the compound-keyword dispatch)
/// has NOT consumed `coproc`. Returns the bare `Command::Coproc`; the dispatch
/// wraps trailing redirects. Named form = a valid-identifier word followed by a
/// compound opener (non-consuming `peek_coproc_named`); otherwise anonymous
/// (name = "COPROC"), body parsed from the untouched stream.
fn parse_coproc(iter: &mut Lexer) -> Result<Command, ParseError> {
    consume_command_word(iter)?; // `coproc`
    skip_test_blanks(iter)?; // blanks only — a NEWLINE after `coproc` makes it anonymous
    if peek_coproc_named(iter)? {
        let name_word = consume_command_word(iter)?;
        // `valid_function_name_text` (not `valid_identifier_text`): the NAME may
        // be any single non-keyword word (`@`, `1x`, …); bash validates it as an
        // identifier at RUNTIME, not at parse time. `run_coproc` performs that
        // check and mirrors bash's `` `NAME': not a valid identifier `` error.
        let name = crate::command::valid_function_name_text(&name_word)
            .expect("peek_coproc_named verified a single non-keyword word");
        skip_test_blanks(iter)?;
        let body = parse_coproc_body(iter)?;
        Ok(Command::Coproc {
            name,
            body: Box::new(body),
        })
    } else {
        let body = parse_coproc_body(iter)?;
        Ok(Command::Coproc {
            name: "COPROC".to_string(),
            body: Box::new(body),
        })
    }
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
    // v262 F2: skip any leading inter-token Blank the atom scanner emits after a
    // compound opener / keyword / connector (`{ ! a; }`, `while ! a`, `then ! a`),
    // so the bang-count loop below sees the `!` rather than the Blank in front of
    // it. (The loop already skips blanks BETWEEN successive bangs; this covers the
    // one before the FIRST bang.) A command never begins with a meaningful Blank,
    // so this is a no-op for the paths that already arrive blank-free.
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
        iter.next_kind()?;
    }
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
    finish_pipeline(iter, first, negate, bangs > 0)
}

/// Given an already-parsed first stage, finish a pipeline: consume any `|`-joined
/// stages and apply the oracle's wrapping rule. Split out of `parse_pipeline` so
/// the coproc body parser can reuse the `|`-loop for a simple first stage (v257).
/// `had_bangs` (v259 CF3) tracks whether ANY leading `!` preceded the first
/// stage, regardless of parity: the oracle wraps a compound first-stage in
/// `Pipeline{negate,[cmd]}` whenever there was at least one leading `!`, even
/// an EVEN count (`negate` false) — `negate` alone under-wraps `! ! { a; }`.
fn finish_pipeline(
    iter: &mut Lexer,
    first: Command,
    negate: bool,
    had_bangs: bool,
) -> Result<Command, ParseError> {
    // A trailing inter-token `Blank` may sit between a compound command's
    // terminator (e.g. `fi`, `}`, `)`) and a following `|` (the atom scanner
    // emits it; simple commands already swallow it in `parse_simple`).
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
        iter.next_kind()?;
    }

    // No `|` follows — wrapping decision mirrors the oracle:
    //   simple                    → always wrap in Pipeline (oracle: parse_pipeline_with_first)
    //   compound, no leading `!`  → return as-is (oracle: parse_command_then_pipeline)
    //   compound, any leading `!` → wrap (negate may be false on an even count, v259 CF3)
    if !matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::Pipe))) {
        return Ok(match first {
            Command::Simple(_) => Command::Pipeline(Pipeline {
                negate,
                commands: vec![first],
            }),
            cmd if negate || had_bangs => Command::Pipeline(Pipeline {
                negate,
                commands: vec![cmd],
            }),
            cmd => cmd,
        });
    }

    // A `|` follows — collect all stages into a Pipeline.
    // v259 F1: mirror the oracle's parse_command_then_pipeline hoist
    // (command.rs:833). An even-bang (>=2) compound first stage is wrapped
    // Pipeline{negate:false,[cmd]} by the oracle and does NOT hoist (guard is
    // p.negate && len==1), so the inner 1-elem pipeline survives nested as the
    // first stage. Odd-bang hoists (negate is the outer flag here, `first` is
    // raw) → stays flat; zero-bang / simple → unchanged.
    let first_stage = match first {
        Command::Simple(_) => first,
        cmd if had_bangs && !negate => Command::Pipeline(Pipeline {
            negate: false,
            commands: vec![cmd],
        }),
        other => other,
    };
    let mut stages = vec![first_stage];
    iter.next_kind()?; // consume `|`
    // Recovery cursor context (Gap A, iteration 2): a `|` starts a new pipeline
    // stage — a command word is expected next, so an empty trailing word (`echo hi | `)
    // reports `Command`, not the previous stage's argument position. No-op off recovery.
    iter.set_recovery_cmd_word(true);
    skip_newlines(iter)?;

    loop {
        // A `coproc` is invalid as a non-first pipeline stage (mirrors the
        // oracle's `parse_next_stage`, command.rs:2344).
        if peek_leading_keyword(iter)? == Some(Keyword::Coproc) {
            return Err(ParseError::UnexpectedKeyword("coproc".to_string()));
        }
        stages.push(parse_command(iter)?);
        while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
            iter.next_kind()?;
        }
        if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::Pipe))) {
            iter.next_kind()?; // consume `|`
            // Recovery (Gap A): next pipeline stage expects a command word.
            iter.set_recovery_cmd_word(true);
            skip_newlines(iter)?;
        } else {
            break;
        }
    }

    Ok(Command::Pipeline(Pipeline {
        negate,
        commands: stages,
    }))
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
    parse_and_or_opts(iter, stop_at, false)
}

/// The shared body of [`parse_and_or`]. When `stop_at_top_newline` is set, a
/// top-level `TokenKind::Newline` terminates the command UNIT (used by
/// [`parse_one_unit`] for the non-interactive script reader); otherwise a
/// top-level newline is a Semi-like continue connector.
fn parse_and_or_opts(
    iter: &mut Lexer,
    stop_at: &[Keyword],
    stop_at_top_newline: bool,
) -> Result<Sequence, ParseError> {
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
        // Bound history within a long sequence. `maybe_prune_history` only acts at
        // genuine top level (`modes.len() == 1`); a live arith Mark CAN straddle
        // this call in a nested `$((… $({compound})…))`, but only ever at
        // `modes.len() >= 2`, so the depth guard suppresses the prune there. See
        // the SAFETY note on `maybe_prune_history`.
        iter.maybe_prune_history();
        // ── Stop check 1: before consuming any connector (mirrors ~890) ──────
        // Atom-aware keyword recognition (a bare `Lit` keyword, not a `Word`).
        if peek_leading_keyword(iter)?
            .map(|k| stop_at.contains(&k))
            .unwrap_or(false)
        {
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
        // Recovery cursor context (Gap A, iteration 2): a connector (`;`, newline,
        // `&`, `&&`, `||`) starts a NEW command — the parser next expects a command
        // word. Reset the flag (the preceding command's last argument left it
        // `false`) so an EMPTY trailing word right after the separator (`echo hi; `,
        // `echo hi && `) reports `Command`, not `Argument`. No-op off recovery.
        iter.set_recovery_cmd_word(true);
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
                let stop_kw = peek_leading_keyword(iter)?
                    .map(|k| stop_at.contains(&k))
                    .unwrap_or(false);
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
                        return Err(ParseError::Unexpected(iter.unexpected_here(None)?));
                    }
                    // A command follows → `&` is a separator.
                    Some(_) => {
                        rest.push((Connector::Amp, parse_command_then_pipeline(iter)?));
                    }
                }
            }

            // ── `;` or newline — semi-like connector ─────────────────────────
            TokenKind::Op(Operator::Semi) | TokenKind::Newline => {
                // v264 unit mode: a top-level NEWLINE ends the command unit
                // (already consumed as `token`). Drain any heredoc-body atom
                // groups the lexer emitted for THIS unit's line — the atom path
                // emits them after the newline, unlike the oracle which
                // pre-collects during tokenization — so `fill_sequence` can
                // attach them; then end the unit WITHOUT skipping inter-unit
                // newlines or parsing the next command. `;` still separates
                // within a unit.
                if stop_at_top_newline && matches!(token, TokenKind::Newline) {
                    collect_heredoc_bodies_after_newline(iter)?;
                    break;
                }
                skip_newlines(iter)?;
                // ── Stop check 3: stop_at keyword after `;`/newline (~958) ───
                if peek_leading_keyword(iter)?
                    .map(|k| stop_at.contains(&k))
                    .unwrap_or(false)
                {
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
                // `other` (== `token`) was already consumed above (line 3652)
                // before this match ran, so `iter.unexpected_here` would
                // report the token AFTER it instead — build the
                // `ExpectFailure` directly from the already-owned `other`.
                // `spell_token` names a bare-keyword `Lit`/`Word` the same
                // way whether or not `keyword_of_consumed` recognizes it, so
                // one `Unexpected` arm covers both the former
                // `UnexpectedKeyword`/`UnexpectedToken` split.
                return Err(ParseError::Unexpected(ExpectFailure {
                    found: Found::Token(other),
                    matching: None,
                    pos: iter.cursor_pos(),
                }));
            }
        }
    }

    Ok(Sequence {
        first,
        rest,
        background,
    })
}

/// v250 T3 + v260 CF1: walk every redirect in `redirects` (in source order),
/// filling (a) any still-empty `RedirOp::Heredoc { body }` placeholder from
/// `bodies`, and (b) any heredoc nested INSIDE another redirect's own `Word`
/// (a here-string, a file/dup target — `cat <<<$(a <<X)`, `echo >$(f <<X)`,
/// `echo >&$(f <<X)`) via `fill_word`. A heredoc body is "still empty"
/// (`Word(vec![])`) exactly when `parse_one_redirect` built it as a
/// provisional placeholder; an ALREADY-filled one (can't happen on the atom
/// path today, but keeps this idempotent) is left alone. EXHAUSTIVE over
/// `RedirOp` — no `_ =>` wildcard, so a future variant can't silently drop a
/// nested body.
fn fill_redirects(redirects: &mut [Redirection], bodies: &mut impl Iterator<Item = Word>) {
    for r in redirects.iter_mut() {
        match &mut r.op {
            RedirOp::File { target, .. } => fill_word(target, bodies),
            RedirOp::Dup { source, .. } => fill_word(source, bodies),
            RedirOp::Move { source, .. } => fill_word(source, bodies),
            RedirOp::Close => {}
            RedirOp::Heredoc { body, .. } => {
                if body.0.is_empty() {
                    if let Some(next) = bodies.next() {
                        *body = next;
                    }
                }
            }
            RedirOp::HereString(word) => fill_word(word, bodies),
        }
    }
}

/// v260 CF1: fill heredoc bodies whose openers sit inside a `Word`. Recurses
/// into every nested `Sequence`/`Word` a `WordPart` can carry, in source order,
/// so the shared FIFO body queue attaches each body to its placeholder.
/// EXHAUSTIVE over `WordPart` — no `_ =>` wildcard.
fn fill_word(word: &mut Word, bodies: &mut impl Iterator<Item = Word>) {
    fill_word_parts(&mut word.0, bodies);
}

fn fill_word_parts(parts: &mut [WordPart], bodies: &mut impl Iterator<Item = Word>) {
    for part in parts.iter_mut() {
        match part {
            WordPart::CommandSub { sequence, .. } => fill_sequence(sequence, bodies),
            WordPart::ProcessSub { sequence, .. } => fill_sequence(sequence, bodies),
            WordPart::Arith { body, .. } => fill_word(body, bodies),
            WordPart::Quoted { parts, .. } => fill_word_parts(parts, bodies),
            WordPart::ParamExpansion {
                subscript,
                modifier,
                ..
            } => {
                // Source order: `${a[i]:-word}` — subscript before the modifier.
                if let Some(SubscriptKind::Index(w)) = subscript {
                    fill_word(w, bodies);
                }
                fill_param_modifier(modifier, bodies);
            }
            WordPart::ArrayLiteral(elems) => {
                for el in elems.iter_mut() {
                    if let Some(sub) = el.subscript.as_mut() {
                        fill_word(sub, bodies); // `[idx]=val` — subscript before value
                    }
                    fill_word(&mut el.value, bodies);
                }
            }
            // No nested Word — nothing to fill.
            WordPart::Literal { .. }
            | WordPart::Tilde { .. }
            | WordPart::Var { .. }
            | WordPart::LastStatus { .. }
            | WordPart::AllArgs { .. }
            | WordPart::AssignPrefix { .. } => {}
        }
    }
}

/// EXHAUSTIVE over `ParamModifier`; recurses into each variant's Word(s) in
/// source order.
fn fill_param_modifier(modifier: &mut ParamModifier, bodies: &mut impl Iterator<Item = Word>) {
    match modifier {
        ParamModifier::UseDefault { word, .. }
        | ParamModifier::AssignDefault { word, .. }
        | ParamModifier::ErrorIfUnset { word, .. }
        | ParamModifier::UseAlternate { word, .. } => fill_word(word, bodies),
        ParamModifier::RemovePrefix { pattern, .. }
        | ParamModifier::RemoveSuffix { pattern, .. } => fill_word(pattern, bodies),
        ParamModifier::Substitute {
            pattern,
            replacement,
            ..
        } => {
            fill_word(pattern, bodies); // `${x/pat/rep}` — pattern before replacement
            fill_word(replacement, bodies);
        }
        ParamModifier::Substring { offset, length } => {
            fill_word(offset, bodies); // `${x:off:len}` — offset before length
            if let Some(l) = length.as_mut() {
                fill_word(l, bodies);
            }
        }
        ParamModifier::Case {
            pattern: Some(p), ..
        } => fill_word(p, bodies),
        // No Word to fill.
        ParamModifier::Case { pattern: None, .. }
        | ParamModifier::None
        | ParamModifier::Length
        | ParamModifier::IndirectKeys
        | ParamModifier::PrefixNames { .. }
        | ParamModifier::Transform { .. }
        | ParamModifier::BadSubst { .. } => {}
    }
}

/// v260 CF1: fill heredoc bodies nested in a `[[ … ]]` test expression's
/// operand `Word`s, in source order. EXHAUSTIVE over `TestExpr` — no `_ =>`
/// wildcard (there is no parenthesized-grouping variant: `(…)` inside a test
/// expression is resolved at parse time, not represented in the AST).
fn fill_test_expr(expr: &mut TestExpr, bodies: &mut impl Iterator<Item = Word>) {
    match expr {
        TestExpr::Unary { operand, .. } => fill_word(operand, bodies),
        TestExpr::Binary { lhs, rhs, .. } => {
            fill_word(lhs, bodies);
            fill_word(rhs, bodies);
        }
        TestExpr::Regex { lhs, pattern } => {
            fill_word(lhs, bodies);
            fill_word(pattern, bodies);
        }
        TestExpr::Not(inner) => fill_test_expr(inner, bodies),
        TestExpr::And(lhs, rhs) | TestExpr::Or(lhs, rhs) => {
            fill_test_expr(lhs, bodies);
            fill_test_expr(rhs, bodies);
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
        Command::Simple(SimpleCommand::Assign(items, _)) => {
            // v260 CF1: a bare assignment carries no redirects, but its value
            // Words can nest heredocs (`x=$(cat <<X)`, `a=($(cat <<X))`).
            for a in items.iter_mut() {
                if let AssignTarget::Indexed { subscript, .. } = &mut a.target {
                    fill_word(subscript, bodies);
                }
                fill_word(&mut a.value, bodies);
            }
        }
        Command::Simple(SimpleCommand::Exec(exec)) => {
            // v260 CF1: walk the command's own Words, then its redirects, in
            // source order (words-then-redirects). Inline assignments precede the
            // program, which precedes the args, which precede trailing redirects.
            for a in exec.inline_assignments.iter_mut() {
                if let AssignTarget::Indexed { subscript, .. } = &mut a.target {
                    fill_word(subscript, bodies);
                }
                fill_word(&mut a.value, bodies);
            }
            fill_word(&mut exec.program, bodies);
            for arg in exec.args.iter_mut() {
                fill_word(arg, bodies);
            }
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
            // v260 CF1: the `in WORDS` list precedes the body in source order.
            for w in clause.words.iter_mut() {
                fill_word(w, bodies);
            }
            fill_sequence(&mut clause.body, bodies);
        }
        Command::Case(clause) => {
            // v260 CF1: subject, then each item's `|`-separated patterns
            // (before its body), in source order.
            fill_word(&mut clause.subject, bodies);
            for item in clause.items.iter_mut() {
                for pat in item.patterns.iter_mut() {
                    fill_word(pat, bodies);
                }
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
        // wrapper, handled there. v260 CF1: any inline assignment prefixes
        // (`FOO=hi [[ … ]]`) precede the test expression itself.
        Command::DoubleBracket {
            expr,
            inline_assignments,
        } => {
            for a in inline_assignments.iter_mut() {
                if let AssignTarget::Indexed { subscript, .. } = &mut a.target {
                    fill_word(subscript, bodies);
                }
                fill_word(&mut a.value, bodies);
            }
            fill_test_expr(expr, bodies);
        }
        // `((expr))` is a bare arithmetic `Word`, no redirect list of its own.
        Command::Arith(body) => fill_word(body, bodies),
        // C-style `for ((init;cond;step))`: the header sections are bare
        // `Word`s (no redirect list of their own); v260 CF1 fills them, in
        // source order, before the `Sequence` body.
        Command::ArithFor(clause) => {
            if let Some(init) = clause.init.as_mut() {
                fill_word(init, bodies);
            }
            if let Some(cond) = clause.cond.as_mut() {
                fill_word(cond, bodies);
            }
            if let Some(step) = clause.step.as_mut() {
                fill_word(step, bodies);
            }
            fill_sequence(&mut clause.body, bodies);
        }
        Command::Select(clause) => {
            // v260 CF1: the `in WORDS` list (when present) precedes the body.
            if let Some(words) = clause.words.as_mut() {
                for w in words.iter_mut() {
                    fill_word(w, bodies);
                }
            }
            fill_sequence(&mut clause.body, bodies);
        }
        Command::Redirected { inner, redirects } => {
            // Source order: the wrapped command's own (possibly nested)
            // heredocs appear before the compound's OWN trailing redirects
            // (`{ …<<A…; } <<B` — A's body precedes B's in the source).
            fill_command(inner, bodies);
            fill_redirects(redirects, bodies);
        }
        // `coproc [NAME] command`: recurse into the wrapped command. Reachable
        // since v257 T2 (coproc is no longer deferred on the atom path).
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

/// The atom-path entry point; assembles a `Sequence` from the lexer's atom
/// stream.
///
/// Returns `Ok(None)` on empty input (newlines only or EOF).
pub fn parse_sequence(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError> {
    // v259 CF2: discard any heredoc bodies leaked by a prior parse that errored
    // after pushing them (take_heredoc_bodies drains only on this fn's success
    // path). Safe: the atom parse_sequence is the single non-reentrant top-level
    // entry, so nothing legitimately carries a body into a fresh call.
    let _ = iter.take_heredoc_bodies();
    // Skip leading newlines AND inter-token blanks (the atom scanner emits a
    // `Blank` for folded whitespace), so a blank-only / blank+comment line
    // reduces to `Ok(None)`.
    skip_newlines(iter)?;
    if iter.peek_kind()?.is_none() {
        return Ok(None);
    }
    let mut seq = parse_and_or(iter, &[])?;
    // A leftover trailing `Blank` (atom path only — e.g. `"a; "`) is NOT
    // content; skip it so the stray-terminator check below sees the real
    // next token.
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
        iter.next_kind()?;
    }
    // A stray terminator (`;;`/`;&`/`;;&`) left after the top-level
    // sequence → `UnexpectedToken`.
    if iter.peek_kind()?.is_some() {
        return Err(ParseError::Unexpected(iter.unexpected_here(None)?));
    }
    // v250 T3: attach every heredoc body collected along the way (in source
    // order == emission order) to its still-empty placeholder.
    let mut bodies = iter.take_heredoc_bodies().into_iter();
    fill_sequence(&mut seq, &mut bodies);
    Ok(Some(seq))
}

/// v264: parse ONE top-level command unit from the atom stream, stopping at
/// (and consuming) the next top-level newline or EOF. Skips leading blank
/// lines. Returns `Ok(None)` when only newlines/blanks/EOF remain. Used by
/// the non-interactive script reader (`run_sourced_contents_in_sinks`).
pub fn parse_one_unit(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError> {
    iter.maybe_prune_history(); // bound history across units on the shared lexer
    // Discard any heredoc bodies leaked by a prior unit that errored after
    // pushing them (mirrors parse_sequence's CF2 hygiene). take_heredoc_bodies
    // drains only on the success path below, so on a clean loop this is a no-op.
    let _ = iter.take_heredoc_bodies();
    // Skip leading Newline/Blank atoms (and any heredoc-body groups) — mirrors
    // parse_sequence's leading skip and the oracle's leading-newline skip.
    skip_newlines(iter)?;
    if iter.peek_kind()?.is_none() {
        return Ok(None);
    }
    let mut seq = parse_and_or_opts(iter, &[], true)?;
    // Attach heredoc bodies collected for this unit (no stray-terminator check —
    // more units may follow; the caller loops).
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
        // An empty body at genuine EOF: under `recover_at_eof` synthesize a
        // minimal `:` body so a truncated compound command still yields a tree;
        // otherwise it is unterminated. The strict path (recover_at_eof ==
        // false) is unchanged — it still returns `Err(unterminated)`.
        Err(ParseError::MissingCommand) if iter.peek_kind()?.is_none() => {
            if iter.recover_at_eof() {
                Ok(synthetic_colon_sequence())
            } else {
                Err(unterminated)
            }
        }
        // Recovery: the section body ran into a synthetic EOF-recovery closer.
        // This happens when the truncation sits inside an enclosing lexer-mode:
        // in `echo $(if whi` the inner `if` condition meets the `$(`'s synthetic
        // `)` (an `Unexpected` error), never a bare EOF. Best-effort a `:` body so
        // the enclosing compound still recovers and records its frame. Guarded by
        // `recover_at_eof`, so the strict path is byte-for-byte unaffected.
        Err(e) => {
            if iter.recover_at_eof() && iter.peek_is_recovery_close()? {
                Ok(synthetic_colon_sequence())
            } else {
                Err(e)
            }
        }
        other => other,
    }
}

/// A synthetic no-op `:` command wrapped as a one-command `Sequence`. Used
/// under `recover_at_eof` to fill a compound command's missing body (an `if`
/// then-branch, a loop body, …) so the recovery tree is walkable. Only ever
/// built on the recovery path.
fn synthetic_colon_sequence() -> Sequence {
    Sequence {
        first: Command::Simple(SimpleCommand::Exec(ExecCommand {
            inline_assignments: Vec::new(),
            program: Word(vec![WordPart::Literal {
                text: ":".to_string(),
                quoted: false,
            }]),
            args: Vec::new(),
            redirects: Vec::new(),
            line: 0,
        })),
        rest: Vec::new(),
        background: false,
    }
}

/// Consume a compound command's continuation/closing keyword (`then`, `do`,
/// `done`, `fi`, `}`, `in`, `esac`). Returns `Ok(true)` when the keyword was
/// present and consumed, `Ok(false)` when it is ABSENT AT EOF under
/// `recover_at_eof` (the caller then synthesizes the remainder), or
/// `Err(on_missing)` otherwise.
///
/// The strict path (`recover_at_eof == false`) is byte-for-byte identical to
/// `expect_keyword`: it consumes on a match and returns `Err(on_missing)` on a
/// mismatch — it can never return `Ok(false)`.
fn expect_or_recover(
    iter: &mut Lexer,
    expected: Keyword,
    on_missing: ParseError,
) -> Result<bool, ParseError> {
    if peek_leading_keyword(iter)? == Some(expected) {
        consume_command_word(iter)?;
        Ok(true)
    } else if iter.recover_at_eof() && iter.peek_is_recovery_close()? {
        // Recover not only at bare EOF but also when the delimiter would sit past
        // a truncated inner lexer-mode: `echo $(if whi` reaches EOF inside the
        // `$(`, so the next token is the synthetic `)` closer, not `None`. Keying
        // off "the next token is a synthetic EOF closer" lets the inner compound
        // still recover (and record its frame) instead of erroring.
        Ok(false)
    } else {
        Err(on_missing)
    }
}

/// Wraps a freshly-parsed compound command in `Command::Redirected` when one
/// or more redirects immediately follow its terminator; otherwise returns the
/// command unchanged.  Mirrors `maybe_wrap_redirects` in `command.rs`.
pub(crate) fn maybe_wrap_redirects(cmd: Command, iter: &mut Lexer) -> Result<Command, ParseError> {
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
        Ok(Command::Redirected {
            inner: Box::new(cmd),
            redirects,
        })
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
    let body = parse_compound_section(iter, &[Keyword::RBrace], ParseError::UnterminatedBrace)?;
    // Under recovery, a missing `}` at EOF closes the group with what was parsed.
    if !expect_or_recover(iter, Keyword::RBrace, ParseError::UnterminatedBrace)? {
        // Recovery (Task 4): `{ …` truncated before `}` — cursor is in the group.
        iter.push_recovery_frame(crate::recover::Frame::BraceGroup);
    }
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
    // Under recovery, `if COND` with no `then` at EOF synthesizes a `:`
    // then-branch (and skips the elif/else/fi tail, which is all at EOF).
    let then_body = if expect_or_recover(iter, Keyword::Then, ParseError::UnterminatedIf)? {
        parse_compound_section(
            iter,
            &[Keyword::Elif, Keyword::Else, Keyword::Fi],
            ParseError::UnterminatedIf,
        )?
    } else {
        // Recovery (Task 4): `if COND` truncated with no `then` — the cursor sits
        // in the if-condition.
        iter.push_recovery_frame(crate::recover::Frame::IfCondition);
        synthetic_colon_sequence()
    };

    let mut elif_branches = Vec::new();
    while peek_leading_keyword(iter)? == Some(Keyword::Elif) {
        consume_command_word(iter)?; // consume `elif`
        let condition = parse_compound_section(iter, &[Keyword::Then], ParseError::UnterminatedIf)?;
        if !expect_or_recover(iter, Keyword::Then, ParseError::UnterminatedIf)? {
            // Recovery: `elif COND` with no `then` at EOF — synthesize its body
            // and stop; the else/fi tail is all at EOF below.
            elif_branches.push(ElifBranch {
                condition,
                body: synthetic_colon_sequence(),
            });
            break;
        }
        let body = parse_compound_section(
            iter,
            &[Keyword::Elif, Keyword::Else, Keyword::Fi],
            ParseError::UnterminatedIf,
        )?;
        elif_branches.push(ElifBranch { condition, body });
    }

    let else_body = if peek_leading_keyword(iter)? == Some(Keyword::Else) {
        consume_command_word(iter)?; // consume `else`
        Some(parse_compound_section(
            iter,
            &[Keyword::Fi],
            ParseError::UnterminatedIf,
        )?)
    } else {
        None
    };

    // Under recovery, a missing `fi` at EOF closes the `if` with what was parsed.
    expect_or_recover(iter, Keyword::Fi, ParseError::UnterminatedIf)?;
    let clause = IfClause {
        condition,
        then_body,
        elif_branches,
        else_body,
    };
    maybe_wrap_redirects(Command::If(Box::new(clause)), iter)
}

/// Skips `;`/newline separators before `do`, then consumes `do`, the loop body,
/// and `done`.  Returns the parsed body `Sequence`.  Shared by `parse_for` and
/// `parse_select`.  Mirrors `parse_do_body_done` (~1522) in `command.rs`.
fn parse_do_body_done(iter: &mut Lexer) -> Result<Sequence, ParseError> {
    // bash accepts at most ONE `;` before `do`/`{` (an optional list terminator);
    // a second `;` (`for … ; ; do`) is a syntax error. Track it so the Blank-skip
    // below can't bridge across to a second `;` and wrongly accept `; ;`.
    let mut saw_semi = false;
    loop {
        match iter.peek_kind()? {
            // Skip inter-token blanks so a spaced separator before `do`/`{`
            // (`)) ; do`, `for ((…)) ; do …`) reaches the keyword — the atom
            // scanner emits a `Blank` between the `))`/word and the `;`, and a
            // bare `Blank` here would otherwise stop the skip early.
            Some(TokenKind::Blank) => {
                iter.next_kind()?;
            }
            Some(TokenKind::Op(Operator::Semi)) if !saw_semi => {
                iter.next_kind()?;
                saw_semi = true;
            }
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
    // bash (ksh-derived) accepts a `{ list; }` brace group in place of
    // `do … done` for `for`/`select` loops (but NOT `while`/`until`, which
    // inline their own do/done and never reach here). The AST is unchanged:
    // the loop body is the compound-list between `{` and `}`. Reuse the exact
    // `{`-reserved-word rules of `parse_brace_group` so recognition stays
    // identical (leading blanks/newlines already skipped above; a redirect on
    // the loop — `for … { … } > f` — is left for the caller's
    // `maybe_wrap_redirects`, exactly as with the `done` form).
    if peek_leading_keyword(iter)? == Some(Keyword::LBrace) {
        expect_keyword(iter, Keyword::LBrace, ParseError::UnterminatedBrace)?;
        let body = parse_compound_section(iter, &[Keyword::RBrace], ParseError::UnterminatedBrace)?;
        expect_or_recover(iter, Keyword::RBrace, ParseError::UnterminatedBrace)?;
        return Ok(body);
    }
    // Under recovery, a missing `do` at EOF synthesizes a `:` loop body (and
    // the trailing `done`, also at EOF, is skipped).
    if !expect_or_recover(iter, Keyword::Do, ParseError::UnterminatedLoop)? {
        return Ok(synthetic_colon_sequence());
    }
    let body = parse_compound_section(iter, &[Keyword::Done], ParseError::UnterminatedLoop)?;
    expect_or_recover(iter, Keyword::Done, ParseError::UnterminatedLoop)?;
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
        if trimmed.is_empty() {
            parts.remove(0);
        } else {
            parts[0] = WordPart::Literal {
                text: trimmed,
                quoted,
            };
        }
    }
    // Trim the trailing Literal.
    if let Some(WordPart::Literal { text, quoted }) = parts.last().cloned() {
        let trimmed = text.trim_end().to_string();
        let last = parts.len() - 1;
        if trimmed.is_empty() {
            parts.pop();
        } else {
            parts[last] = WordPart::Literal {
                text: trimmed,
                quoted,
            };
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(Word(parts))
    }
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
            Some(TokenKind::ArithClose) => {
                iter.next_kind()?;
                sections.push(Word(cur));
                return Ok(sections);
            }
            Some(TokenKind::ArithSemi) => {
                iter.next_kind()?;
                sections.push(Word(std::mem::take(&mut cur)));
            }
            Some(TokenKind::ArithBail) => {
                return Err(ParseError::UnterminatedLoop);
            }
            Some(TokenKind::ParamOpen { .. }) => {
                cur.push(parse_param_expansion(iter, true)?);
            }
            Some(TokenKind::CmdSubOpen) => {
                iter.next_kind()?;
                cur.push(parse_command_sub(iter, true)?);
            }
            Some(TokenKind::BeginBacktick) => {
                iter.next_kind()?;
                cur.push(parse_backtick_sub(iter, true)?);
            }
            Some(TokenKind::ArithOpen) => {
                iter.next_kind()?;
                cur.push(parse_arith_expansion(iter, true)?);
            }
            Some(TokenKind::LegacyArithOpen) => {
                iter.next_kind()?;
                cur.push(parse_legacy_arith_expansion(iter, true)?);
            }
            Some(TokenKind::Lit { .. }) => {
                if let Some(TokenKind::Lit { text, quoted }) = iter.next_kind()? {
                    cur.push(WordPart::Literal { text, quoted });
                }
            }
            Some(TokenKind::DollarName { .. }) => {
                if let Some(TokenKind::DollarName { name, quoted }) = iter.next_kind()? {
                    let part = match name.as_str() {
                        "@" => WordPart::AllArgs {
                            quoted,
                            joined: false,
                        },
                        "*" => WordPart::AllArgs {
                            quoted,
                            joined: true,
                        },
                        "?" => WordPart::LastStatus { quoted },
                        _ => WordPart::Var { name, quoted },
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
    iter.push_mode(Mode::Arith {
        paren_depth: 0,
        in_squote: false,
        in_dquote: false,
        body_started: true,
        for_header: true,
        delim: ArithDelim::Paren,
    });
    let result = parse_arith_for_body(iter);
    iter.pop_mode();
    let sections = match result {
        Ok(s) => s,
        Err(ParseError::Lex(_)) => return Err(ParseError::UnterminatedLoop),
        Err(e) => return Err(e),
    };
    if sections.len() != 3 {
        return Err(ParseError::ArithForHeader(format!(
            "expected 3 sections separated by `;`, got {}",
            sections.len()
        )));
    }
    let init = trim_section(&sections[0]);
    let cond = trim_section(&sections[1]);
    let step = trim_section(&sections[2]);
    let body = parse_do_body_done(iter)?;
    maybe_wrap_redirects(
        Command::ArithFor(Box::new(ArithForClause {
            init,
            cond,
            step,
            body,
        })),
        iter,
    )
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
        // Recovery (Gap A, iteration 2): after `in`, the parser expects word-LIST
        // words (argument position), not a command. `consume_command_word` left the
        // flag `true`; flip it so an empty trailing word (`for x in `) reports
        // `Argument`. No-op off recovery.
        iter.set_recovery_cmd_word(false);
        loop {
            while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
                iter.next_kind()?;
            }
            let is_do = peek_leading_keyword(iter)? == Some(Keyword::Do);
            let stop = is_do
                || matches!(
                    iter.peek_kind()?,
                    None | Some(TokenKind::Newline) | Some(TokenKind::Op(Operator::Semi))
                )
                // Recovery (Task 4): a for/select word-list truncated inside an
                // enclosing lexer mode (`echo $(for x in y`) reaches EOF with the
                // next token being the synthetic recovery-close (an `Op` stamped
                // at the EOF offset). Treat it as end-of-list so control falls
                // through to the `ForList` push guard instead of erroring on the
                // `Op(_)` arm below. Gated on `recover_at_eof` → strict path dead.
                || (iter.recover_at_eof() && iter.peek_is_recovery_close()?);
            if stop {
                break;
            }
            match iter.peek_kind()? {
                Some(TokenKind::Op(_)) => {
                    return Err(ParseError::Unexpected(iter.unexpected_here(None)?));
                }
                // v264 flip-fix (Finding 1): for-loop in-list words brace-expand
                // (oracle: `emit_word_with_braces` is called for for/select lists).
                // Recovery cursor context (Task 4): a for-list word is argument-position.
                _ => {
                    iter.set_recovery_cmd_word(false);
                    let w = parse_word_command(iter, false)?;
                    push_command_word_brace_expanded(&mut words, w, iter)?;
                }
            }
        }
        true
    } else {
        false
    };

    // Recovery (Task 4): `for x [in …]` truncated before `do` — the cursor sits
    // in the loop's word list (or right after the variable). `peek_is_recovery_close`
    // (not bare `is_none`) so a `for` truncated inside an enclosing lexer-mode
    // (`echo $(for x in y`) still records its frame past the synthetic closer.
    if iter.recover_at_eof() && iter.peek_is_recovery_close()? {
        iter.push_recovery_frame(crate::recover::Frame::ForList);
    }
    let body = parse_do_body_done(iter)?;
    maybe_wrap_redirects(
        Command::For(Box::new(ForClause {
            var,
            words,
            has_in,
            body,
        })),
        iter,
    )
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
        // Recovery (Gap A, iteration 2): after `in`, select expects word-LIST words
        // (argument position). Flip the flag so `select x in ` reports `Argument`.
        iter.set_recovery_cmd_word(false);
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
                )
                // Recovery (Task 4): a for/select word-list truncated inside an
                // enclosing lexer mode (`echo $(for x in y`) reaches EOF with the
                // next token being the synthetic recovery-close (an `Op` stamped
                // at the EOF offset). Treat it as end-of-list so control falls
                // through to the `ForList` push guard instead of erroring on the
                // `Op(_)` arm below. Gated on `recover_at_eof` → strict path dead.
                || (iter.recover_at_eof() && iter.peek_is_recovery_close()?);
            if stop {
                break;
            }
            match iter.peek_kind()? {
                Some(TokenKind::Op(_)) => {
                    return Err(ParseError::Unexpected(iter.unexpected_here(None)?));
                }
                // v264 flip-fix (Finding 1): select in-list words brace-expand too.
                // Recovery cursor context (Task 4): a select-list word is argument-position.
                _ => {
                    iter.set_recovery_cmd_word(false);
                    let w = parse_word_command(iter, false)?;
                    push_command_word_brace_expanded(&mut list, w, iter)?;
                }
            }
        }
        Some(list)
    } else {
        None
    };

    // Recovery (Task 4): `select x [in …]` truncated before `do` — mirror the
    // `parse_for` push guard (~4690) so the `ForList` frame is recorded even when
    // the select is nested inside a truncated lexer-mode (`echo $(select y in z`).
    // `peek_is_recovery_close` (not bare `is_none`) so the synthetic closer past
    // the enclosing `$(` still records the frame.
    if iter.recover_at_eof() && iter.peek_is_recovery_close()? {
        iter.push_recovery_frame(crate::recover::Frame::ForList);
    }
    let body = parse_do_body_done(iter)?;
    maybe_wrap_redirects(
        Command::Select(Box::new(SelectClause { var, words, body })),
        iter,
    )
}

/// Parses `case WORD in [clause]... esac`.  Mirrors `parse_case` (~1673) in
/// `command.rs`.  Returns `Command::Case(Box::new(CaseClause{subject, items}))`.
fn parse_case(iter: &mut Lexer) -> Result<Command, ParseError> {
    expect_keyword(iter, Keyword::Case, ParseError::UnterminatedCase)?;
    skip_newlines(iter)?;

    // Subject word (assembled from atoms — e.g. `$x`, `x`).
    let recover = iter.recover_at_eof();
    let subject = match iter.peek_kind()? {
        // Recovery: `case ` with no subject at EOF — an empty subject Word.
        None if recover => Word(Vec::new()),
        None => return Err(ParseError::UnterminatedCase),
        Some(TokenKind::Op(_)) => return Err(ParseError::Unexpected(iter.unexpected_here(None)?)),
        _ => consume_command_word(iter)?,
    };

    skip_newlines(iter)?;
    let mut items: Vec<CaseItem> = Vec::new();
    // Under recovery, a missing `in` at EOF (`case whi`) closes the `case` with
    // no clauses; otherwise parse the clause list and `esac` as usual.
    let saw_in = expect_or_recover(iter, Keyword::In, ParseError::UnterminatedCase)?;
    if !saw_in {
        // Recovery (Task 4): `case SUBJ` truncated before `in` — cursor is in the
        // case subject.
        iter.push_recovery_frame(crate::recover::Frame::CaseSubject);
    }
    if saw_in {
        skip_newlines(iter)?;
        loop {
            skip_newlines(iter)?;
            if peek_leading_keyword(iter)? == Some(Keyword::Esac) {
                break;
            }
            if iter.peek_kind()?.is_none() {
                // Recovery: EOF before `esac` — close with the clauses so far.
                if iter.recover_at_eof() {
                    break;
                }
                return Err(ParseError::UnterminatedCase);
            }
            items.push(parse_case_item(iter)?);
        }
        expect_or_recover(iter, Keyword::Esac, ParseError::UnterminatedCase)?;
    }
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
            Some(TokenKind::Op(_)) => {
                return Err(ParseError::Unexpected(iter.unexpected_here(None)?));
            }
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
            Some(_) => return Err(ParseError::Unexpected(iter.unexpected_here(None)?)),
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

    Ok(CaseItem {
        patterns,
        body,
        terminator,
    })
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
    // Under recovery, `while COND` with no `do` at EOF synthesizes a `:` body
    // (and the trailing `done`, also at EOF, is skipped).
    let body = if expect_or_recover(iter, Keyword::Do, ParseError::UnterminatedLoop)? {
        let b = parse_compound_section(iter, &[Keyword::Done], ParseError::UnterminatedLoop)?;
        expect_or_recover(iter, Keyword::Done, ParseError::UnterminatedLoop)?;
        b
    } else {
        // Recovery (Task 4): `while COND` truncated with no `do` — cursor is in
        // the loop condition.
        iter.push_recovery_frame(crate::recover::Frame::WhileCondition);
        synthetic_colon_sequence()
    };
    maybe_wrap_redirects(
        Command::While(Box::new(WhileClause {
            condition,
            body,
            until,
        })),
        iter,
    )
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

    // No tokens at all → unterminated (or, under recovery, an empty `:` body).
    if iter.peek_kind()?.is_none() {
        if iter.recover_at_eof() {
            return maybe_wrap_redirects(
                Command::Subshell {
                    body: Box::new(synthetic_colon_sequence()),
                },
                iter,
            );
        }
        return Err(ParseError::UnterminatedSubshell);
    }

    let body = parse_subshell_sequence(iter)?;
    maybe_wrap_redirects(
        Command::Subshell {
            body: Box::new(body),
        },
        iter,
    )
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
        let recover = iter.recover_at_eof();
        match iter.peek_kind()? {
            // End of tokens before `)` → unterminated (or, under recovery,
            // close the subshell body with what was parsed).
            None if recover => break,
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
                    if iter.recover_at_eof() {
                        break;
                    }
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
                    return Ok(Sequence {
                        first,
                        rest,
                        background: true,
                    });
                }
                if iter.peek_kind()?.is_none() {
                    if iter.recover_at_eof() {
                        // Recovery: `( cmd &` at EOF — a backgrounded body.
                        return Ok(Sequence {
                            first,
                            rest,
                            background: true,
                        });
                    }
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

    Ok(Sequence {
        first,
        rest,
        background: false,
    })
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
    Ok(Command::FunctionDef {
        name,
        body: Box::new(body),
    })
}

/// `function NAME [()] compound` (mirrors `command.rs`'s
/// `parse_function_keyword_def`). Caller confirmed the leading keyword is
/// `function` via `peek_leading_keyword`. Skips the atom-stream `Blank`s the
/// Word-lexer never emitted.
fn parse_function_keyword_def(iter: &mut Lexer) -> Result<Command, ParseError> {
    consume_command_word(iter)?; // consume the `function` keyword word
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
        iter.next_kind()?;
    }
    // Name: a single valid identifier word.
    let name_word = consume_command_word(iter)?;
    let name =
        crate::command::valid_function_name_text(&name_word).ok_or(ParseError::FunctionName)?;
    // Optional `()` (blanks may sit between name/`(`/`)` in the atom stream).
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
        iter.next_kind()?;
    }
    if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen))) {
        iter.next_kind()?; // `(`
        while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
            iter.next_kind()?;
        }
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
    let name =
        crate::command::valid_function_name_text(&name_word).ok_or(ParseError::FunctionName)?;
    iter.next_kind()?; // `(`
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
        iter.next_kind()?;
    }
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
        while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
            iter.next_kind()?;
        }
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
    while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
        iter.next_kind()?;
    }
    Ok(())
}

/// v253: atom-native `[[ … ]]`. Mirrors command.rs's
/// `parse_double_bracket_with_assigns`, but reads operands via
/// `parse_word_command` (the atom stream has `Lit` atoms, not pre-lexed `Word`
/// tokens). `=~` is DEFERRED to a later iteration (returns `UnsupportedCommand`
/// without pulling the regex RHS — see `parse_test_atom` below).
fn parse_double_bracket(
    iter: &mut Lexer,
    inline_assignments: Vec<Assignment>,
) -> Result<Command, ParseError> {
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
    Ok(Command::DoubleBracket {
        expr: Box::new(expr),
        inline_assignments,
    })
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

/// G3: parse the `[[ … ]]` `==`/`!=`/`=` pattern RHS operand with force-extglob
/// armed on the lexer, so an extglob-shaped group (`@(a|b)`, `!(x)`, …) is
/// recognized even when `shopt extglob` is OFF — bash ALWAYS treats that RHS as
/// an extended pattern. The lexer emits the same zero-width `ExtglobOpen` signal
/// the `shopt`-on path uses; `parse_word_command` assembles the `Mode::Extglob`
/// group. The flag is disarmed afterward (INCLUDING on error) so it never leaks
/// into later scanning; the lexer's depth guard already confines it to this
/// operand's own mode level (nested `$(…)` etc. do not inherit the force).
fn parse_pattern_operand(iter: &mut Lexer) -> Result<Word, ParseError> {
    iter.set_force_extglob(true);
    let r = next_test_word_atom(iter);
    iter.set_force_extglob(false);
    r
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
        Some(TokenKind::Lit {
            text,
            quoted: false,
        }) => {
            let in_set = matches!(
                text.as_str(),
                "==" | "="
                    | "!="
                    | "=~"
                    | "-eq"
                    | "-ne"
                    | "-lt"
                    | "-gt"
                    | "-le"
                    | "-ge"
                    | "-nt"
                    | "-ot"
                    | "-ef"
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
        Some(tok)
            if keyword_of_consumed(tok) == Some(Keyword::DoubleBracketClose)
                || matches!(tok, TokenKind::Op(Operator::RParen)) =>
        {
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
        return Ok(TestExpr::Unary {
            op: TestUnaryOp::StringNonEmpty,
            operand: lhs,
        });
    }

    // Consume the operator.
    match iter.peek_kind()? {
        Some(TokenKind::Op(Operator::RedirIn)) => {
            iter.next_kind()?;
            let rhs = next_test_word_atom(iter)?;
            Ok(TestExpr::Binary {
                op: TestBinaryOp::StringLt,
                lhs,
                rhs,
            })
        }
        Some(TokenKind::Op(Operator::RedirOut)) => {
            iter.next_kind()?;
            let rhs = next_test_word_atom(iter)?;
            Ok(TestExpr::Binary {
                op: TestBinaryOp::StringGt,
                lhs,
                rhs,
            })
        }
        _ => {
            let op_word = parse_word_command(iter, false)?;
            let op_text = match word_literal_text(&op_word) {
                Some(t) => t.to_string(),
                None => return Err(ParseError::TestExprBadOperator(format!("{op_word:?}"))),
            };
            match op_text.as_str() {
                "==" | "=" => {
                    let rhs = parse_pattern_operand(iter)?;
                    Ok(TestExpr::Binary {
                        op: TestBinaryOp::StringEq,
                        lhs,
                        rhs,
                    })
                }
                "!=" => {
                    let rhs = parse_pattern_operand(iter)?;
                    Ok(TestExpr::Binary {
                        op: TestBinaryOp::StringNe,
                        lhs,
                        rhs,
                    })
                }
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
                "<" => {
                    let rhs = next_test_word_atom(iter)?;
                    Ok(TestExpr::Binary {
                        op: TestBinaryOp::StringLt,
                        lhs,
                        rhs,
                    })
                }
                ">" => {
                    let rhs = next_test_word_atom(iter)?;
                    Ok(TestExpr::Binary {
                        op: TestBinaryOp::StringGt,
                        lhs,
                        rhs,
                    })
                }
                "-eq" => {
                    let rhs = next_test_word_atom(iter)?;
                    Ok(TestExpr::Binary {
                        op: TestBinaryOp::IntEq,
                        lhs,
                        rhs,
                    })
                }
                "-ne" => {
                    let rhs = next_test_word_atom(iter)?;
                    Ok(TestExpr::Binary {
                        op: TestBinaryOp::IntNe,
                        lhs,
                        rhs,
                    })
                }
                "-lt" => {
                    let rhs = next_test_word_atom(iter)?;
                    Ok(TestExpr::Binary {
                        op: TestBinaryOp::IntLt,
                        lhs,
                        rhs,
                    })
                }
                "-gt" => {
                    let rhs = next_test_word_atom(iter)?;
                    Ok(TestExpr::Binary {
                        op: TestBinaryOp::IntGt,
                        lhs,
                        rhs,
                    })
                }
                "-le" => {
                    let rhs = next_test_word_atom(iter)?;
                    Ok(TestExpr::Binary {
                        op: TestBinaryOp::IntLe,
                        lhs,
                        rhs,
                    })
                }
                "-ge" => {
                    let rhs = next_test_word_atom(iter)?;
                    Ok(TestExpr::Binary {
                        op: TestBinaryOp::IntGe,
                        lhs,
                        rhs,
                    })
                }
                "-nt" => {
                    let rhs = next_test_word_atom(iter)?;
                    Ok(TestExpr::Binary {
                        op: TestBinaryOp::NewerThan,
                        lhs,
                        rhs,
                    })
                }
                "-ot" => {
                    let rhs = next_test_word_atom(iter)?;
                    Ok(TestExpr::Binary {
                        op: TestBinaryOp::OlderThan,
                        lhs,
                        rhs,
                    })
                }
                "-ef" => {
                    let rhs = next_test_word_atom(iter)?;
                    Ok(TestExpr::Binary {
                        op: TestBinaryOp::SameFile,
                        lhs,
                        rhs,
                    })
                }
                other => Err(ParseError::TestExprBadOperator(other.to_string())),
            }
        }
    }
}

#[cfg(test)]
mod tests;
